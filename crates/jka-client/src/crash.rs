//! Native crash and hang diagnostics.
//!
//! The panic hook in `logging` only sees Rust panics. A fault inside a GPU
//! driver (access violation, heap corruption, stack overflow, ...) kills the
//! process without a trace, which leaves `latest.log` ending mid-sentence. This
//! module records what the panic hook cannot:
//!
//! * an unhandled-exception filter that appends a report (exception code,
//!   faulting module + offset, stack, memory state) to `latest.log` and writes
//!   a minidump beside the exe;
//! * `write_hang_dump`, which writes a minidump of a *live* process so a wedged
//!   driver call can be inspected after the fact;
//! * a one-shot description of the machine (OS, CPU, RAM) for the log header.
//!
//! Frames are logged as `module+0xRVA`; the shipped exe is stripped, so they
//! only resolve against the `.pdb` of the exact build. The minidump carries
//! every thread's stack for the same purpose.
//!
//! `__fastfail` aborts (Rust double-panic abort, some CRT checks) bypass the
//! unhandled-exception filter by design; those leave only Windows Error
//! Reporting evidence.

use std::path::Path;

#[cfg(windows)]
pub fn install(log_path: &Path) {
    win::install(log_path);
}

#[cfg(windows)]
pub fn log_environment() {
    win::log_environment();
}

/// Write a minidump of the running process (all thread stacks) and log where it
/// went. For wedged calls that never return, not for crashes.
#[cfg(windows)]
pub fn write_hang_dump(prefix: &str) {
    match win::write_hang_minidump(prefix) {
        Ok(path) => eprintln!("[CRASH] wrote diagnostic minidump {}", path.display()),
        Err(error) => eprintln!("[CRASH] could not write diagnostic minidump: {error}"),
    }
}

#[cfg(not(windows))]
pub fn install(_log_path: &Path) {}

#[cfg(not(windows))]
pub fn log_environment() {}

#[cfg(not(windows))]
pub fn write_hang_dump(_prefix: &str) {}

#[cfg(windows)]
mod win {
    use std::{
        ffi::c_void,
        fmt::Write as _,
        fs::{self, File, OpenOptions},
        io::Write as _,
        os::windows::io::AsRawHandle,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, OnceLock,
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    type Handle = *mut c_void;
    type Filter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;

    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_STACK_OVERFLOW: u32 = 0xC000_00FD;
    const MAX_FRAMES: usize = 48;
    /// Dumps kept per prefix (`crash-`, `hang-...`); older ones are deleted so a
    /// crash loop cannot fill the player's disk.
    const KEEP_DUMPS: usize = 3;
    /// MiniDumpWithHandleData | WithUnloadedModules | WithIndirectlyReferencedMemory
    /// | WithThreadInfo. No data segments: those add every loaded driver's globals.
    const MINIDUMP_TYPE: u32 = 0x4 | 0x20 | 0x40 | 0x1000;

    static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

    /// A dump with no exception attached, for wedged calls rather than crashes.
    pub fn write_hang_minidump(prefix: &str) -> Result<PathBuf, String> {
        write_minidump(prefix, std::ptr::null(), 0)
    }

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *mut ExceptionRecord,
        address: *mut c_void,
        number_parameters: u32,
        information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        exception_record: *mut ExceptionRecord,
        context_record: *mut c_void,
    }

    /// `MINIDUMP_EXCEPTION_INFORMATION` is declared under `pshpack4.h`.
    #[repr(C, packed(4))]
    struct MinidumpExceptionInformation {
        thread_id: u32,
        exception_pointers: *const ExceptionPointers,
        client_pointers: i32,
    }

    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }

    type MiniDumpWriteDump = unsafe extern "system" fn(
        Handle,
        u32,
        Handle,
        u32,
        *const MinidumpExceptionInformation,
        *const c_void,
        *const c_void,
    ) -> i32;

    #[link(name = "Kernel32")]
    extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetCurrentProcess() -> Handle;
        fn GetCurrentProcessId() -> u32;
        fn GetCurrentThreadId() -> u32;
        fn RtlCaptureStackBackTrace(
            frames_to_skip: u32,
            frames_to_capture: u32,
            frames: *mut *mut c_void,
            hash: *mut u32,
        ) -> u16;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut Handle) -> i32;
        fn GetModuleFileNameW(module: Handle, buffer: *mut u16, size: u32) -> u32;
        fn LoadLibraryW(name: *const u16) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }

    pub fn install(log_path: &Path) {
        let _ = LOG_PATH.set(log_path.to_path_buf());
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }

    pub fn log_environment() {
        let mut version = OsVersionInfo {
            size: std::mem::size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        };
        let os = if unsafe { RtlGetVersion(&mut version) } == 0 {
            // Windows 11 still reports 10.0; the build number tells them apart.
            let name = if version.major == 10 && version.build >= 22_000 {
                "Windows 11"
            } else {
                "Windows"
            };
            format!(
                "{name} {}.{} build {}",
                version.major, version.minor, version.build
            )
        } else {
            "Windows (version unavailable)".to_owned()
        };
        let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown CPU".into());
        let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
        println!(
            "[SYSTEM] {os}; {cpu}; {threads} logical cores; {}",
            memory_summary()
        );
        println!(
            "[SYSTEM] DinurdoJK {} ({})",
            env!("CARGO_PKG_VERSION"),
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "unknown exe path".into())
        );
    }

    fn memory_summary() -> String {
        let mut status = MemoryStatus {
            length: std::mem::size_of::<MemoryStatus>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return "memory status unavailable".into();
        }
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        format!(
            "RAM {:.1}/{:.1} GiB free ({}% load); commit {:.1}/{:.1} GiB free; address space {:.1} GiB free",
            status.avail_phys as f64 / GIB,
            status.total_phys as f64 / GIB,
            status.memory_load,
            status.avail_page_file as f64 / GIB,
            status.total_page_file as f64 / GIB,
            status.avail_virtual as f64 / GIB,
        )
    }

    fn exception_name(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "ACCESS_VIOLATION",
            0xC000_0006 => "IN_PAGE_ERROR",
            0xC000_001D => "ILLEGAL_INSTRUCTION",
            0xC000_0025 => "NONCONTINUABLE_EXCEPTION",
            0xC000_008C => "ARRAY_BOUNDS_EXCEEDED",
            0xC000_0094 => "INT_DIVIDE_BY_ZERO",
            0xC000_0096 => "PRIVILEGED_INSTRUCTION",
            0xC000_00FD => "STACK_OVERFLOW",
            0xC000_0374 => "HEAP_CORRUPTION",
            0xC000_0409 => "STACK_BUFFER_OVERRUN (fast-fail)",
            0xE06D_7363 => "C++ exception",
            0x8000_0003 => "BREAKPOINT",
            _ => "unrecognized",
        }
    }

    /// `module.dll+0xRVA` for an address, or the raw address when it belongs to
    /// no loaded module (JIT/driver-generated code).
    fn describe_address(address: usize) -> String {
        const FROM_ADDRESS: u32 = 0x4;
        const UNCHANGED_REFCOUNT: u32 = 0x2;
        let mut module: Handle = std::ptr::null_mut();
        let found = unsafe {
            GetModuleHandleExW(
                FROM_ADDRESS | UNCHANGED_REFCOUNT,
                address as *const c_void,
                &mut module,
            )
        } != 0;
        if !found || module.is_null() {
            return format!("0x{address:016x} (no module)");
        }
        let mut buffer = [0_u16; 260];
        let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        let path = String::from_utf16_lossy(&buffer[..(length as usize).min(buffer.len())]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        format!("{name}+0x{:x}", address - module as usize)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn write_minidump(
        prefix: &str,
        exception: *const ExceptionPointers,
        thread_id: u32,
    ) -> Result<PathBuf, String> {
        let directory = LOG_PATH
            .get()
            .and_then(|path| path.parent())
            .ok_or("diagnostics are not installed")?;
        prune_dumps(directory, prefix);
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = directory.join(format!("{prefix}-{seconds}.dmp"));
        let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;

        unsafe {
            let library = LoadLibraryW(wide("dbghelp.dll").as_ptr());
            if library.is_null() {
                return Err("dbghelp.dll is unavailable".into());
            }
            let symbol = GetProcAddress(library, b"MiniDumpWriteDump\0".as_ptr());
            if symbol.is_null() {
                return Err("MiniDumpWriteDump is unavailable".into());
            }
            let write: MiniDumpWriteDump = std::mem::transmute(symbol);
            let info = MinidumpExceptionInformation {
                thread_id,
                exception_pointers: exception,
                // The pointers live in this process, not a debugger's.
                client_pointers: 0,
            };
            let ok = write(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file.as_raw_handle() as Handle,
                MINIDUMP_TYPE,
                if exception.is_null() { std::ptr::null() } else { &info },
                std::ptr::null(),
                std::ptr::null(),
            );
            if ok == 0 {
                return Err(format!(
                    "MiniDumpWriteDump failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(path)
    }

    /// Keep only the newest `KEEP_DUMPS - 1` dumps of this prefix so the new one
    /// makes `KEEP_DUMPS`.
    fn prune_dumps(directory: &Path, prefix: &str) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let mut dumps: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(prefix) && name.ends_with(".dmp")
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        dumps.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in dumps.into_iter().skip(KEEP_DUMPS - 1) {
            let _ = fs::remove_file(path);
        }
    }

    /// What the faulting thread captured. Plain data so it can be gathered
    /// without allocating on a possibly exhausted stack.
    struct FaultSnapshot {
        code: u32,
        address: usize,
        parameters: [usize; 15],
        parameter_count: usize,
        thread_id: u32,
        thread_name: Option<String>,
        frames: [usize; MAX_FRAMES],
        frame_count: usize,
        pointers: usize,
    }

    unsafe extern "system" fn filter(info: *const ExceptionPointers) -> i32 {
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if info.is_null() || REPORTED.swap(true, Ordering::SeqCst) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let record = unsafe { (*info).exception_record };
        if record.is_null() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let record = unsafe { &*record };
        let overflow = record.code == STATUS_STACK_OVERFLOW;

        let mut snapshot = FaultSnapshot {
            code: record.code,
            address: record.address as usize,
            parameters: record.information,
            parameter_count: (record.number_parameters as usize).min(15),
            thread_id: unsafe { GetCurrentThreadId() },
            thread_name: None,
            frames: [0; MAX_FRAMES],
            frame_count: 0,
            pointers: info as usize,
        };
        // After a stack overflow there is no stack to spare for either.
        if !overflow {
            snapshot.thread_name = std::thread::current().name().map(str::to_owned);
            let mut frames = [std::ptr::null_mut::<c_void>(); MAX_FRAMES];
            let count = unsafe {
                RtlCaptureStackBackTrace(0, MAX_FRAMES as u32, frames.as_mut_ptr(), std::ptr::null_mut())
            } as usize;
            for (slot, frame) in snapshot.frames.iter_mut().zip(&frames[..count]) {
                *slot = *frame as usize;
            }
            snapshot.frame_count = count;
        }

        // The report runs on a fresh thread with its own stack, and this thread
        // waits for it: the exception pointers must stay alive for the dump, and
        // a stack-overflowed thread cannot do the work itself. The wait is bounded
        // so a corrupted heap that deadlocks the reporter cannot hang the exit.
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("crash-reporter".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                report(&snapshot);
                let _ = done_tx.send(());
            });
        if spawned.is_ok() {
            let _ = done_rx.recv_timeout(Duration::from_secs(30));
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    fn report(fault: &FaultSnapshot) {
        let mut text = String::new();
        let name = fault.thread_name.as_deref().unwrap_or("<unnamed/unknown>");
        let _ = writeln!(text);
        let _ = writeln!(
            text,
            "================ FATAL: UNHANDLED NATIVE EXCEPTION ================"
        );
        let _ = writeln!(
            text,
            "exception 0x{:08X} ({}) on thread '{name}' (tid {})",
            fault.code,
            exception_name(fault.code),
            fault.thread_id
        );
        let _ = writeln!(text, "faulting instruction: {}", describe_address(fault.address));
        if fault.code == 0xC000_0005 || fault.code == 0xC000_0006 {
            if fault.parameter_count >= 2 {
                let kind = match fault.parameters[0] {
                    0 => "reading",
                    1 => "writing",
                    8 => "executing (DEP)",
                    _ => "accessing",
                };
                let _ = writeln!(
                    text,
                    "access violation {kind} address 0x{:016x}",
                    fault.parameters[1]
                );
            }
        }
        let _ = writeln!(text, "{}", memory_summary());
        if fault.frame_count > 0 {
            let _ = writeln!(text, "stack (innermost first):");
            for (index, frame) in fault.frames[..fault.frame_count].iter().enumerate() {
                let _ = writeln!(text, "  #{index:<2} {}", describe_address(*frame));
            }
        } else {
            let _ = writeln!(text, "stack: not captured (stack overflow); see minidump");
        }
        match write_minidump("crash", fault.pointers as *const ExceptionPointers, fault.thread_id) {
            Ok(path) => {
                let _ = writeln!(text, "minidump: {}", path.display());
            }
            Err(error) => {
                let _ = writeln!(text, "minidump failed: {error}");
            }
        }
        let _ = writeln!(
            text,
            "===================================================================="
        );

        // Straight to the file through a fresh handle: the logger's mutex may be
        // held by the very thread that crashed.
        if let Some(path) = LOG_PATH.get() {
            if let Ok(mut file) = OpenOptions::new().append(true).open(path) {
                let _ = file.write_all(text.as_bytes());
                let _ = file.flush();
            }
        }
        std::eprintln!("{text}");
    }
}
