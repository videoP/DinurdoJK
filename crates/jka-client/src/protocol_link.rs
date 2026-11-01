//! `jka://host[:port]` launch links. Never feed the browser-supplied URL
//! directly into the console command buffer: `;`, newlines and comments are
//! meaningful there. Only a validated socket target is passed to `connect`.

pub(crate) fn parse_jka_url(url: &str) -> Result<String, String> {
    let Some(scheme) = url.get(..6) else {
        return Err("Expected jka://host[:port]".into());
    };
    if !scheme.eq_ignore_ascii_case("jka://") {
        return Err("Expected jka://host[:port]".into());
    }
    let authority = url[6..].strip_suffix('/').unwrap_or(&url[6..]);
    if authority.is_empty() || authority.len() > 255 {
        return Err("jka:// needs a host (up to 255 characters)".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    // JKA's current NET_StringToAdr resolves IPv4 and DNS hostnames; IPv6
    // brackets, URI userinfo, paths, query strings and URL escapes aren't
    // supported. Refuse them rather than silently connecting elsewhere.
    if host.is_empty()
        || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err("Invalid jka:// hostname (IPv4 or DNS name required)".into());
    }
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
            || !port.parse::<u16>().is_ok_and(|port| port != 0)
        {
            return Err("Invalid jka:// port (expected 1..65535)".into());
        }
    }
    Ok(authority.to_owned())
}

/// Current Windows handler state, used only when the user opens the settings page.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegistrationStatus {
    Unregistered,
    OwnedByUs,
    OtherHandler,
}

#[cfg(windows)]
mod windows_registration {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE,
        REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW,
    };

    const SCHEME: &str = "jka";
    const ROOT: &str = "Software\\Classes\\jka";
    const OPEN_COMMAND: &str = "Software\\Classes\\jka\\shell\\open\\command";
    const ICON: &str = "Software\\Classes\\jka\\DefaultIcon";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn error(operation: &str, status: u32) -> String {
        format!("{operation}: {}", std::io::Error::from_raw_os_error(status as i32))
    }

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) { unsafe { RegCloseKey(self.0); } }
    }

    fn open(root: HKEY, path: &str) -> Result<Option<Key>, String> {
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut key) };
        match status {
            0 => Ok(Some(Key(key))),
            2 | 3 => Ok(None), // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND
            other => Err(error("Reading protocol association", other)),
        }
    }

    fn set_value(path: &str, name: Option<&str>, value: &[u16]) -> Result<(), String> {
        let mut key = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER, wide(path).as_ptr(), 0, std::ptr::null(),
                0, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut(),
            )
        };
        if status != 0 { return Err(error("Creating protocol association", status)); }
        let key = Key(key);
        let name = name.map(wide);
        let value_bytes = unsafe {
            std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2)
        };
        let status = unsafe {
            RegSetValueExW(
                key.0, name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                0, REG_SZ, value_bytes.as_ptr(), value_bytes.len() as u32,
            )
        };
        if status != 0 { return Err(error("Writing protocol association", status)); }
        Ok(())
    }

    fn get_default(root: HKEY, path: &str) -> Result<Option<Vec<u16>>, String> {
        let Some(key) = open(root, path)? else { return Ok(None); };
        let mut length = 0u32;
        let status = unsafe {
            RegQueryValueExW(key.0, std::ptr::null(), std::ptr::null(),
                std::ptr::null_mut(), std::ptr::null_mut(), &mut length)
        };
        if status == 2 { return Ok(None); }
        if status != 0 { return Err(error("Reading protocol command", status)); }
        if length == 0 || length > 65536 || length % 2 != 0 {
            return Err("Malformed jka:// registry command".into());
        }
        let mut bytes = vec![0u8; length as usize];
        let status = unsafe {
            RegQueryValueExW(key.0, std::ptr::null(), std::ptr::null(),
                std::ptr::null_mut(), bytes.as_mut_ptr(), &mut length)
        };
        if status != 0 { return Err(error("Reading protocol command", status)); }
        if length as usize > bytes.len() || length % 2 != 0 {
            return Err("Malformed jka:// registry command".into());
        }
        let mut data: Vec<u16> = bytes[..length as usize].chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])).collect();
        while data.last() == Some(&0) { data.pop(); }
        Ok(Some(data))
    }

    fn command_for(executable: &Path) -> Vec<u16> {
        let mut result = vec![b'"' as u16];
        result.extend(executable.as_os_str().encode_wide());
        result.extend(OsStr::new("\" \"%1\"").encode_wide());
        result
    }

    fn icon_for(executable: &Path) -> Vec<u16> {
        let mut result = vec![b'"' as u16];
        result.extend(executable.as_os_str().encode_wide());
        result.extend(OsStr::new("\",0").encode_wide());
        result
    }

    pub(super) fn register(executable: &Path) -> Result<(), String> {
        let mut command = command_for(executable);
        command.push(0);
        let mut icon = icon_for(executable);
        icon.push(0);
        set_value(ROOT, None, &wide("URL:Jedi Academy server"))?;
        set_value(ROOT, Some("URL Protocol"), &wide(""))?;
        set_value(ICON, None, &icon)?;
        set_value(OPEN_COMMAND, None, &command)?;
        Ok(())
    }

    pub(super) fn status(executable: &Path) -> Result<super::RegistrationStatus, String> {
        use super::RegistrationStatus;
        // HKCR merges the per-user and machine-wide scheme. Query the active
        // class association rather than assuming our own per-user key wins.
        if let Some(command) = get_default(HKEY_CLASSES_ROOT, "jka\\shell\\open\\command")? {
            return Ok(if command == command_for(executable) {
                RegistrationStatus::OwnedByUs
            } else {
                RegistrationStatus::OtherHandler
            });
        }
        if open(HKEY_CLASSES_ROOT, SCHEME)?.is_some() {
            Ok(RegistrationStatus::OtherHandler) // Registered but no usable command.
        } else {
            Ok(RegistrationStatus::Unregistered)
        }
    }

    pub(super) fn unregister_if_owned(executable: &Path) -> Result<bool, String> {
        if get_default(HKEY_CURRENT_USER, OPEN_COMMAND)?.as_deref() != Some(command_for(executable).as_slice()) {
            return Ok(false); // Never delete another application's association.
        }
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(ROOT).as_ptr()) };
        if status != 0 { return Err(error("Removing protocol association", status)); }
        Ok(true)
    }
}

/// Explicit override for existing jka:// handlers, or safe uninstall. These
/// switches return before starting the updater, window, renderer or console.
pub(crate) fn registration_cli_entry() -> Option<i32> {
    #[cfg(windows)] {
        let mut args = std::env::args_os().skip(1);
        let action = args.next()?.to_string_lossy().into_owned();
        if action != "--register-jka-protocol" && action != "--unregister-jka-protocol" {
            return None;
        }
        if args.next().is_some() {
            std::eprintln!("Protocol registration switches take no arguments");
            return Some(1);
        }
        let result = std::env::current_exe()
            .map_err(|error| error.to_string())
            .and_then(|path| {
                if action == "--register-jka-protocol" {
                    windows_registration::register(&path)?;
                    Ok("Registered jka:// for this Windows user".to_owned())
                } else if windows_registration::unregister_if_owned(&path)? {
                    Ok("Unregistered jka:// for this Windows user".to_owned())
                } else {
                    Err("jka:// is not registered to this executable; left unchanged".into())
                }
            });
        return Some(match result {
            Ok(message) => { std::println!("{message}"); 0 }
            Err(error) => { std::eprintln!("{error}"); 1 }
        });
    }
    #[cfg(not(windows))]
    { None }
}

/// Read-only until the user explicitly clicks Register/Unregister in Setup.
#[cfg(windows)]
pub(crate) fn registration_status() -> Result<RegistrationStatus, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    windows_registration::status(&executable)
}

#[cfg(windows)]
pub(crate) fn register_explicit() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    windows_registration::register(&executable)
}

#[cfg(windows)]
pub(crate) fn register_if_unclaimed_explicit() -> Result<(), String> {
    // The UI's cached status could be stale after another app registers.
    // Its one-click Register option must never overwrite another handler.
    match registration_status()? {
        RegistrationStatus::Unregistered | RegistrationStatus::OwnedByUs => register_explicit(),
        RegistrationStatus::OtherHandler => Err("Another application now owns jka://. Refresh and confirm replacement first.".into()),
    }
}

#[cfg(windows)]
pub(crate) fn unregister_explicit() -> Result<bool, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    windows_registration::unregister_if_owned(&executable)
}

#[cfg(test)]
mod tests {
    use super::parse_jka_url;

    #[test]
    fn accepts_jka_server_links() {
        for (url, target) in [
            ("jka://192.168.0.1", "192.168.0.1"),
            ("jka://192.168.0.1:29070", "192.168.0.1:29070"),
            ("JKA://localhost/", "localhost"),
            ("jka://my-server.example.org:29071/", "my-server.example.org:29071"),
        ] {
            assert_eq!(parse_jka_url(url).unwrap(), target);
        }
    }

    #[test]
    fn rejects_commands_and_unsupported_uri_parts() {
        for url in [
            "jka://", "jka://host:0", "jka://host:65536", "jka://host:",
            "jka://host:port", "jka://host/path", "jka://host/?password=abc",
            "jka://host#fragment", "jka://user@host", "jka://host%0aquit",
            "jka://host;quit", "jka://host\nquit", "jka://host\\quit",
            "jka://[::1]", "https://host", "jka:///host",
        ] {
            assert!(parse_jka_url(url).is_err(), "unexpectedly accepted {url:?}");
        }
    }
}
