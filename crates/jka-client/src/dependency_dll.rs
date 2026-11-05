//! Client-owned native dependencies are outside JK2/JKA game roots.
//! Preload linked phonon.dll after entry (MSVC /DELAYLOAD) and before the
//! first audionimbus call; also usable by the NGX feature on demand.

#[cfg(windows)]
pub(crate) fn preload(name: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };
    // Callers supply compile-time DLL basenames, never user/server input.
    if !matches!(name, "phonon.dll" | "nvngx_dlss.dll") {
        return Err("unrecognized client dependency".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = exe.parent().ok_or("no executable directory")?
        .join("DinurdoJK").join("dependencies").join(name);
    if !path.is_file() {
        return Err(format!("missing dependency: {}", path.display()));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(), std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    };
    if module.is_null() {
        return Err(format!("could not load {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    // Deliberately retain one reference for the lifetime of the process:
    // the delay-import helper / NVIDIA NGX may acquire the already loaded DLL.
    Ok(())
}
