//! Fatal startup problems must be visible: the client is a GUI-subsystem
//! process with its own console, so an `eprintln!` before the window exists is
//! invisible and the exit looks like a silent crash.
use std::path::{Path, PathBuf};

/// Everything the client needs beside the executable before anything else can
/// run. Game assets are NOT checked here: missing game files are recoverable
/// through the Asset Mapping screen.
// Windows and Linux have different native Steam Audio dependencies.
// Linux executables also link libphonon.so before main(); the release runner
// configures an $ORIGIN RUNPATH so this bundled file can be loaded at startup.
#[cfg(windows)]
const REQUIRED_DEPENDENCIES: &[&str] = &["phonon.dll"];
#[cfg(target_os = "linux")]
const REQUIRED_DEPENDENCIES: &[&str] = &["libphonon.so"];
#[cfg(target_os = "macos")]
const REQUIRED_DEPENDENCIES: &[&str] = &["libphonon.dylib"];
#[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
const REQUIRED_DEPENDENCIES: &[&str] = &[];

/// Show `message` in a native dialog (and on stderr for console launches).
pub(crate) fn show(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_TOPMOST};
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(message), wide("DinurdoJK cannot start"));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR | MB_TOPMOST) };
    }
}

/// Print the message, show the dialog, and give `main` its failure exit code.
pub(crate) fn fatal(message: impl std::fmt::Display) -> std::process::ExitCode {
    show(&message.to_string());
    std::process::ExitCode::FAILURE
}

/// The client-owned `DinurdoJK/` folder must sit beside the executable and hold
/// its dependencies. Returns the explanation to show, or None when it is intact.
pub(crate) fn verify_client_folder(install: &Path) -> Option<String> {
    let folder = install.join(jka_assets::pk3::DINURDOJK_DIRECTORY);
    let dependencies = folder.join("dependencies");
    let executable_name = std::env::current_exe().ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| if cfg!(windows) { "DinurdoJK.exe" } else { "the DinurdoJK executable" }.to_owned());
    let expect = |detail: String| Some(format!(
        "{detail}\n\nThe '{name}' folder must be in the same folder as {executable_name} ({install}). \
         Keep the executable together with its '{name}' folder, or reinstall/update.",
        name = jka_assets::pk3::DINURDOJK_DIRECTORY, install = install.display(),
    ));
    if !folder.is_dir() {
        return expect(format!("Cannot find the required folder:\n{}", folder.display()));
    }
    let missing: Vec<PathBuf> = REQUIRED_DEPENDENCIES.iter()
        .map(|name| dependencies.join(name)).filter(|path| !path.is_file()).collect();
    if missing.is_empty() { return None; }
    let list = missing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
    expect(format!("Required file(s) missing:\n{list}"))
}
