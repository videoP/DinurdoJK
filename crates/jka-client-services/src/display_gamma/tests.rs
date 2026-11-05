//! File-backed display simulation: no test changes the user's display gamma.
use super::{guardian::*, *};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn baseline() -> Ramp {
    Ramp(std::array::from_fn(|i| {
        let value = (i % 256) as f32 / 255.0;
        let exponent = [1.0, 1.05, 0.95][i / 256];
        (value.powf(exponent) * 65535.0).round() as u16
    }))
}
fn directory(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("gamma-audit")
        .join(format!("{name}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
}
#[derive(Default)]
struct Behavior {
    ignore_writes: AtomicBool,
    lie_on_reads: AtomicBool,
    fail_writes: AtomicBool,
    writes: Mutex<usize>,
}
struct Mock {
    root: PathBuf,
    behavior: Arc<Behavior>,
}
impl Mock {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            behavior: Arc::new(Behavior::default()),
        }
    }
    fn device(&self, index: usize) -> Device {
        Device {
            name: format!("MOCK DISPLAY {index}"),
            identity: format!("{}-{index}", self.root.display()),
        }
    }
    fn physical(&self, index: usize) -> Ramp {
        let bytes = fs::read(self.root.join(format!("monitor-{index}.ramp"))).unwrap();
        Ramp(std::array::from_fn(|i| {
            u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]])
        }))
    }
    fn seed(&self, index: usize, ramp: &Ramp) {
        let bytes: Vec<_> = ramp
            .0
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        atomic_write(&self.root.join(format!("monitor-{index}.ramp")), &bytes).unwrap();
    }
    fn index(device: &Device) -> usize {
        device.name.rsplit(' ').next().unwrap().parse().unwrap()
    }
}
impl Backend for Mock {
    #[cfg(windows)]
    type Lease = super::windows::Lease;
    #[cfg(not(windows))]
    type Lease = ();
    fn owner(&self) -> Owner {
        #[cfg(windows)]
        {
            super::windows::own_identity().unwrap()
        }
        #[cfg(not(windows))]
        {
            Owner {
                pid: std::process::id(),
                born: 1,
            }
        }
    }
    fn lock(&self, device: &Device) -> Result<Self::Lease, String> {
        #[cfg(windows)]
        {
            super::windows::monitor_lock(device)
        }
        #[cfg(not(windows))]
        {
            let _ = device;
            Ok(())
        }
    }
    fn window_device(&self, hwnd: usize) -> Result<Device, String> {
        if !(1..=2).contains(&hwnd) {
            return Err("test monitor was disconnected".into());
        }
        Ok(self.device(hwnd))
    }
    fn read(&self, device: &Device) -> Result<Ramp, String> {
        if device != &self.device(Self::index(device)) {
            return Err("test monitor was replaced".into());
        }
        if self.behavior.lie_on_reads.load(Ordering::Relaxed) {
            Ok(baseline())
        } else {
            Ok(self.physical(Self::index(device)))
        }
    }
    fn write(&self, device: &Device, ramp: &Ramp) -> Result<(), String> {
        assert!(
            recovery_path(&self.root, device).exists(),
            "a durable backup must precede every gamma write"
        );
        *self.behavior.writes.lock().unwrap() += 1;
        if self.behavior.fail_writes.load(Ordering::Relaxed) {
            return Err("mock driver write failed".into());
        }
        if !self.behavior.ignore_writes.load(Ordering::Relaxed) {
            self.seed(Self::index(device), ramp);
        }
        Ok(())
    }
}
fn keeper(name: &str) -> Keeper<Mock> {
    let root = directory(name);
    let mock = Mock::new(root.clone());
    mock.seed(1, &baseline());
    mock.seed(2, &baseline());
    Keeper::new(mock, root)
}
#[test]
fn hardware_curve_preserves_calibration_and_neutral_exactly() {
    let original = baseline();
    assert_eq!(original.with_gamma(1.0), original);
    for gamma in [0.5, 1.5, 2.0, 3.0] {
        let ramp = original.with_gamma(gamma);
        for c in 0..3 {
            let channel = &ramp.0[c * 256..(c + 1) * 256];
            assert_eq!(channel[0], original.0[c * 256]);
            assert_eq!(channel[255], original.0[c * 256 + 255]);
            assert!(channel.windows(2).all(|v| v[0] <= v[1]));
        }
    }
}
#[test]
fn durable_snapshots_validate_all_channels_checksum_and_lengths() {
    let snapshot = Snapshot {
        device: Device {
            name: "DISPLAY 1".into(),
            identity: "monitor-id".into(),
        },
        owner: Owner {
            pid: 123,
            born: 456,
        },
        original: baseline(),
        applied: baseline().with_gamma(2.0),
        previous: baseline(),
    };
    let bytes = snapshot.encode();
    assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
    for len in [0, 7, 20, 120, bytes.len() - 1] {
        assert!(Snapshot::decode(&bytes[..len]).is_err());
    }
    let mut corrupt = bytes.clone();
    corrupt[100] ^= 1;
    assert!(Snapshot::decode(&corrupt).is_err());
    let mut trailing = bytes;
    trailing.push(1);
    assert!(Snapshot::decode(&trailing).is_err());
}
#[test]
fn changes_start_from_original_and_focus_restore_releases_backup() {
    let mut keeper = keeper("original");
    let device = keeper.backend.device(1);
    keeper.apply(1, 2.0).unwrap();
    keeper.apply(1, 3.0).unwrap();
    assert_eq!(keeper.backend.physical(1), baseline().with_gamma(3.0));
    keeper.restore_all().unwrap();
    assert_eq!(keeper.backend.physical(1), baseline());
    assert!(!recovery_path(&keeper.backend.root, &device).exists());
    keeper.apply(1, 0.5).unwrap();
    assert_eq!(keeper.backend.physical(1), baseline().with_gamma(0.5));
    keeper.restore_all().unwrap();
}
#[test]
fn moving_monitor_restores_previous_display_before_changing_next() {
    let mut keeper = keeper("monitors");
    keeper.apply(1, 2.0).unwrap();
    keeper.apply(2, 3.0).unwrap();
    assert_eq!(keeper.backend.physical(1), baseline());
    assert_eq!(keeper.backend.physical(2), baseline().with_gamma(3.0));
    keeper.restore_all().unwrap();
    assert_eq!(keeper.backend.physical(2), baseline());
}
#[test]
fn driver_that_reports_old_values_still_gets_an_explicit_restore() {
    let mut keeper = keeper("lying-driver");
    keeper
        .backend
        .behavior
        .lie_on_reads
        .store(true, Ordering::Relaxed);
    assert!(keeper.apply(1, 2.0).is_err());
    assert_ne!(keeper.backend.physical(1), baseline());
    keeper.restore_all().unwrap();
    assert_eq!(keeper.backend.physical(1), baseline());
    assert_eq!(*keeper.backend.behavior.writes.lock().unwrap(), 2);
}
#[test]
fn ignored_writes_are_rejected_and_restoration_failure_keeps_backup() {
    let mut keeper = keeper("ignored-driver");
    keeper
        .backend
        .behavior
        .ignore_writes
        .store(true, Ordering::Relaxed);
    assert!(keeper.apply(1, 2.0).is_err());
    keeper
        .backend
        .behavior
        .ignore_writes
        .store(false, Ordering::Relaxed);
    keeper.restore_all().unwrap();
    keeper.apply(1, 2.0).unwrap();
    keeper
        .backend
        .behavior
        .fail_writes
        .store(true, Ordering::Relaxed);
    assert!(keeper.restore_all().is_err());
    let path = recovery_path(&keeper.backend.root, &keeper.backend.device(1));
    assert!(path.exists());
    keeper
        .backend
        .behavior
        .fail_writes
        .store(false, Ordering::Relaxed);
    keeper.restore_all().unwrap();
    assert!(!path.exists());
}
#[test]
fn external_calibration_is_not_overwritten() {
    let mut keeper = keeper("external");
    keeper.apply(1, 2.0).unwrap();
    let external = baseline().with_gamma(0.5);
    keeper.backend.seed(1, &external);
    assert!(keeper.restore_all().is_err());
    assert_eq!(keeper.backend.physical(1), external);
    assert!(recovery_path(&keeper.backend.root, &keeper.backend.device(1)).exists());
}
#[test]
fn corrupt_backup_or_unwritable_directory_prevents_hardware_changes() {
    let mut keeper = keeper("corrupt");
    let path = recovery_path(&keeper.backend.root, &keeper.backend.device(1));
    fs::write(&path, b"bad backup").unwrap();
    assert!(keeper.apply(1, 2.0).is_err());
    assert_eq!(*keeper.backend.behavior.writes.lock().unwrap(), 0);
    fs::remove_file(path).unwrap();
    let blocker = keeper.backend.root.join("not-a-directory");
    fs::write(&blocker, b"x").unwrap();
    let mock = Mock::new(keeper.backend.root.clone());
    let mut blocked = Keeper::new(mock, blocker);
    assert!(blocked.apply(1, 2.0).is_err());
    assert!(
        !blocked.recover().is_empty(),
        "inaccessible recovery storage cannot report success"
    );
    assert_eq!(blocked.backend.physical(1), baseline());
}
#[cfg(windows)]
#[test]
fn active_guardian_ownership_prevents_another_game_from_restoring_it() {
    let mut first = keeper("ownership");
    first.apply(1, 2.0).unwrap();
    // Windows named mutexes are recursive on the same thread; use another thread
    // to model another guardian process, which cannot acquire this monitor lease.
    let root = first.backend.root.clone();
    let errors = std::thread::spawn(move || {
        let mut second = Keeper::new(Mock::new(root.clone()), root);
        second.recover()
    })
    .join()
    .unwrap();
    assert_eq!(errors.len(), 1);
    assert_ne!(first.backend.physical(1), baseline());
    first.restore_all().unwrap();
}

/// Standalone --cfg test executable entry, used for real process-death tests.
/// It writes only a mock ramp file; the Windows gamma APIs are never called.
#[cfg(windows)]
pub(super) fn process_entry() -> Option<i32> {
    use std::os::windows::process::CommandExt;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--mock-guardian") => {
            let pid = args.next()?.parse().ok()?;
            let born = args.next()?.parse().ok()?;
            let root = PathBuf::from(args.next()?);
            let owner = Owner { pid, born };
            let backend = Mock::new(root.clone());
            Some(run_guardian(backend, root, move |quit| {
                let _ = super::windows::wait_parent(owner);
                let _ = quit.send(None);
            }))
        }
        Some("--mock-parent") => {
            let root = PathBuf::from(args.next()?);
            let exit_kind = args.next()?;
            let owner = super::windows::own_identity().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--mock-guardian",
                    &owner.pid.to_string(),
                    &owner.born.to_string(),
                    root.to_str().unwrap(),
                ])
                .creation_flags(0x08000000)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["ready"],
                true
            );
            writeln!(
                input,
                "{}",
                serde_json::json!({"id":1,"hwnd":1,"gamma":2.0})
            )
            .unwrap();
            input.flush().unwrap();
            line.clear();
            output.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["active"],
                true
            );
            fs::write(root.join("ready"), child.id().to_string()).unwrap();
            if exit_kind == "graceful" {
                writeln!(input, "{}", serde_json::json!({"id":2})).unwrap();
                input.flush().unwrap();
                line.clear();
                output.read_line(&mut line).unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&line).unwrap()["restored"],
                    true
                );
                drop(input);
                child.wait().unwrap();
                Some(0)
            } else {
                // The driver terminates this process abruptly, skipping Drop.
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        Some("--read-only-display") => {
            let window =
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            let backend = super::windows::WindowsBackend { parent: None };
            let result = backend.window_device(window as usize).and_then(|device| {
                let ramp = backend.read(&device)?;
                std::println!(
                    "Read-only display probe: {} (valid SDR gamma ramp: {})",
                    device.name,
                    ramp.valid()
                );
                Ok(())
            });
            if let Err(error) = result {
                std::println!("Read-only display probe: {error}");
            }
            Some(0)
        }
        Some("--process-driver") => {
            for scenario in ["graceful", "crash", "guardian-crash"] {
                let root = directory(scenario);
                let mock = Mock::new(root.clone());
                mock.seed(1, &baseline());
                let mut parent = Command::new(std::env::current_exe().unwrap())
                    .args(["--mock-parent", root.to_str().unwrap(), scenario])
                    .creation_flags(0x08000000)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !root.join("ready").exists() {
                    assert!(Instant::now() < deadline, "mock parent did not start");
                    if let Some(status) = parent.try_wait().unwrap() {
                        assert!(
                            root.join("ready").exists(),
                            "mock parent exited unexpectedly: {status}"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                if scenario != "graceful" {
                    assert_ne!(mock.physical(1), baseline());
                    if scenario == "guardian-crash" {
                        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
                        let pid = fs::read_to_string(root.join("ready"))
                            .unwrap()
                            .parse()
                            .unwrap();
                        let handle =
                            unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
                        assert!(!handle.is_null());
                        assert_ne!(unsafe { TerminateProcess(handle, 99) }, 0);
                        assert_eq!(unsafe { WaitForSingleObject(handle, 3000) }, 0);
                        unsafe {
                            CloseHandle(handle);
                        }
                    }
                    parent.kill().unwrap();
                }
                parent.wait().unwrap();
                if scenario == "guardian-crash" {
                    assert_ne!(mock.physical(1), baseline());
                    assert!(recovery_path(&root, &mock.device(1)).exists());
                    let mut restarted = Keeper::new(Mock::new(root.clone()), root.clone());
                    assert!(restarted.recover().is_empty());
                    // Restart, use hardware gamma again, then quit normally.
                    restarted.apply(1, 3.0).unwrap();
                    restarted.restore_all().unwrap();
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                while mock.physical(1) != baseline()
                    || recovery_path(&root, &mock.device(1)).exists()
                {
                    assert!(
                        Instant::now() < deadline,
                        "guardian did not restore after {scenario}"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::println!(
                    "PASS: {scenario} restored exact mock calibration and removed backup"
                );
            }
            Some(0)
        }
        _ => None,
    }
}
