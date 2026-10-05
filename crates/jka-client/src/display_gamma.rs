//! Optional hardware brightness, isolated in a hidden crash-restoration process.
//! The game never changes the desktop ramp directly. All work is event driven.
#[path = "display_gamma/guardian.rs"]
mod guardian;
#[cfg(test)]
#[path = "display_gamma/tests.rs"]
mod tests;
#[cfg(windows)]
#[path = "display_gamma/windows.rs"]
mod windows;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc},
    time::Duration,
};

#[derive(Debug)]
pub struct Status {
    pub generation: u64,
    pub active: bool,
    pub restored: bool,
    pub result: Result<(), String>,
}
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub hwnd: usize,
    pub gamma: f32,
}
struct Request {
    generation: u64,
    target: Option<Target>,
    finished: Option<mpsc::Sender<Result<(), String>>>,
}
#[derive(Default)]
pub struct Controller {
    tx: Option<mpsc::Sender<Request>>,
}
impl Controller {
    pub fn request(
        &mut self,
        generation: u64,
        target: Option<Target>,
        notify: Arc<dyn Fn(Status) + Send + Sync>,
    ) -> Result<(), String> {
        if self.tx.is_none() {
            if target.is_none() {
                notify(Status {
                    generation,
                    active: false,
                    restored: true,
                    result: Ok(()),
                });
                return Ok(());
            }
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("gamma-controller".into())
                .spawn(move || controller_worker(rx, notify))
                .map_err(|e| e.to_string())?;
            self.tx = Some(tx);
        }
        self.tx
            .as_ref()
            .unwrap()
            .send(Request {
                generation,
                target,
                finished: None,
            })
            .map_err(|e| e.to_string())
    }
    /// Explicit because the game's normal fast exit skips destructors.
    pub fn restore_before_exit(&mut self) -> Result<(), String> {
        let Some(tx) = &self.tx else { return Ok(()) };
        let (finished, wait) = mpsc::channel();
        tx.send(Request {
            generation: u64::MAX,
            target: None,
            finished: Some(finished),
        })
        .map_err(|e| e.to_string())?;
        wait.recv_timeout(Duration::from_secs(3))
            .map_err(|e| e.to_string())?
    }
    pub fn was_started(&self) -> bool {
        self.tx.is_some()
    }
}
struct Helper {
    _child: Child,
    input: ChildStdin,
    replies: mpsc::Receiver<serde_json::Value>,
}
impl Helper {
    fn spawn() -> Result<Self, String> {
        #[cfg(not(windows))]
        {
            return Err("Hardware brightness is available on Windows only".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let owner = windows::own_identity()?;
            let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
            command
                .args([
                    "--gamma-guardian",
                    &owner.pid.to_string(),
                    &owner.born.to_string(),
                ])
                .creation_flags(0x08000000)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = command
                .spawn()
                .map_err(|e| format!("start gamma guardian: {e}"))?;
            let input = child
                .stdin
                .take()
                .ok_or("gamma guardian has no input pipe")?;
            let output = child
                .stdout
                .take()
                .ok_or("gamma guardian has no status pipe")?;
            let (tx, replies) = mpsc::channel();
            std::thread::Builder::new()
                .name("gamma-status".into())
                .spawn(move || {
                    for line in BufReader::new(output).lines().map_while(Result::ok) {
                        let Ok(value) = serde_json::from_str(&line) else {
                            break;
                        };
                        if tx.send(value).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            let helper = Self {
                _child: child,
                input,
                replies,
            };
            let ready = helper
                .replies
                .recv_timeout(Duration::from_secs(5))
                .map_err(|e| format!("gamma guardian startup: {e}"))?;
            if ready.get("ready").and_then(|v| v.as_bool()) != Some(true) {
                return Err("gamma guardian failed its startup handshake".into());
            }
            Ok(helper)
        }
    }
    fn request(&mut self, request: &Request) -> Result<(bool, bool, Result<(), String>), String> {
        let value = match request.target {
            Some(target) => {
                serde_json::json!({"id":request.generation,"hwnd":target.hwnd,"gamma":target.gamma})
            }
            None => serde_json::json!({"id":request.generation}),
        };
        writeln!(self.input, "{value}")
            .and_then(|_| self.input.flush())
            .map_err(|e| e.to_string())?;
        let value = self
            .replies
            .recv_timeout(Duration::from_secs(8))
            .map_err(|e| format!("gamma guardian response: {e}"))?;
        if value.get("id").and_then(|v| v.as_u64()) != Some(request.generation) {
            return Err("gamma guardian response generation mismatch".into());
        }
        let active = value
            .get("active")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let restored = value
            .get("restored")
            .and_then(|v| v.as_bool())
            .unwrap_or(!active);
        let result = value
            .get("error")
            .and_then(|v| v.as_str())
            .map_or(Ok(()), |error| Err(error.to_owned()));
        Ok((active, restored, result))
    }
}
fn controller_worker(rx: mpsc::Receiver<Request>, notify: Arc<dyn Fn(Status) + Send + Sync>) {
    let mut helper: Option<Helper> = None;
    while let Ok(mut request) = rx.recv() {
        // Coalesce slider/focus changes before issuing the next slow driver call.
        while request.finished.is_none() {
            let Ok(next) = rx.try_recv() else { break };
            request = next;
        }
        let result = (|| {
            if helper.is_none() {
                let errors = recover_before_launch();
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
                if request.target.is_none() {
                    return Ok((false, true, Ok(())));
                }
                helper = Some(Helper::spawn()?);
            }
            helper.as_mut().unwrap().request(&request)
        })();
        let (active, restored, result) = match result {
            Ok(status) => status,
            Err(error) => {
                // EOF requests restoration independently of the parent-exit wait.
                // Keep the helper alive: forcibly killing it would defeat recovery.
                helper = None;
                let errors = recover_before_launch();
                let restored = errors.is_empty();
                let error = if restored {
                    error
                } else {
                    format!("{error}; {}", errors.join("; "))
                };
                (false, restored, Err(error))
            }
        };
        if let Some(finished) = request.finished {
            let _ = finished.send(result);
        } else {
            notify(Status {
                generation: request.generation,
                active,
                restored,
                result,
            });
        }
    }
    // Dropping input delivers EOF. The child serializes restoration itself.
}
pub fn backup_directory() -> Result<PathBuf, String> {
    let root =
        std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable for gamma recovery")?;
    Ok(PathBuf::from(root).join("DinurdoJK").join("gamma-recovery"))
}
pub fn recover_before_launch() -> Vec<String> {
    #[cfg(windows)]
    {
        let directory = match backup_directory() {
            Ok(path) => path,
            Err(error) => return vec![error],
        };
        let mut keeper = guardian::Keeper::new(windows::WindowsBackend { parent: None }, directory);
        keeper.recover()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
/// Handle the private helper entry before logging, asset/audio loading, or windows.
pub fn helper_entry() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--gamma-guardian") {
        return None;
    }
    #[cfg(windows)]
    {
        let pid = args.next().and_then(|v| v.parse().ok());
        let born = args.next().and_then(|v| v.parse().ok());
        let (Some(pid), Some(born)) = (pid, born) else {
            return Some(1);
        };
        let owner = guardian::Owner { pid, born };
        if !windows::parent_alive(owner) {
            return Some(1);
        }
        let directory = match backup_directory() {
            Ok(path) => path,
            Err(_) => return Some(1),
        };
        Some(run_guardian(
            windows::WindowsBackend {
                parent: Some(owner),
            },
            directory,
            move |quit| {
                let _ = windows::wait_parent(owner);
                let _ = quit.send(None);
            },
        ))
    }
    #[cfg(not(windows))]
    {
        Some(1)
    }
}
/// Shared with process tests; both parent exit and stdin EOF terminate the helper.
fn run_guardian<B: guardian::Backend>(
    backend: B,
    directory: PathBuf,
    watch: impl FnOnce(mpsc::Sender<Option<serde_json::Value>>) + Send + 'static,
) -> i32 {
    let mut keeper = guardian::Keeper::new(backend, directory);
    let (tx, rx) = mpsc::channel();
    let quit = tx.clone();
    if std::thread::Builder::new()
        .name("gamma-parent-wait".into())
        .spawn(move || watch(quit))
        .is_err()
    {
        return 1;
    }
    if std::thread::Builder::new()
        .name("gamma-input".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(Some(value)).is_err() {
                    return;
                }
            }
            let _ = tx.send(None);
        })
        .is_err()
    {
        return 1;
    }
    std::println!("{}", serde_json::json!({"ready":true}));
    let _ = std::io::stdout().flush();
    while let Ok(Some(value)) = rx.recv() {
        let Some(id) = value.get("id").and_then(|v| v.as_u64()) else {
            break;
        };
        let target = value.get("hwnd").and_then(|v| v.as_u64());
        let gamma = value.get("gamma").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
        let active = target.is_some() && gamma != 1.0;
        let result = match target.filter(|_| active) {
            Some(hwnd) => keeper.apply(hwnd as usize, gamma),
            None => keeper.restore_all(),
        };
        let reply = match result {
            Ok(()) => serde_json::json!({"id":id,"active":active,"restored":!active}),
            Err(error) => {
                let restore = keeper.restore_all();
                let restored = restore.is_ok();
                let error = match restore {
                    Ok(()) => error,
                    Err(restore) => format!("{error}; restore: {restore}"),
                };
                serde_json::json!({"id":id,"active":false,"restored":restored,"error":error})
            }
        };
        std::println!("{reply}");
        let _ = std::io::stdout().flush();
    }
    match keeper.restore_all() {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = path.with_extension(format!("{}.{}.tmp", std::process::id(), stamp));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        #[cfg(windows)]
        {
            windows::replace_file(&temporary, path)?;
        }
        #[cfg(not(windows))]
        {
            fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn monitor_key(hwnd: usize) -> Result<String, String> {
    #[cfg(windows)]
    {
        use guardian::Backend;
        let backend = windows::WindowsBackend { parent: None };
        Ok(guardian::device_key(&backend.window_device(hwnd)?))
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        Err("hardware brightness is available on Windows only".into())
    }
}

#[cfg(all(test, windows))]
pub fn test_process_entry() -> Option<i32> {
    tests::process_entry()
}
