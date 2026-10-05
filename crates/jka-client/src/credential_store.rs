//! Secure per-server jaPRO login storage.
//!
//! Passwords are never written to DinurdoJK.cfg. On Windows they live in the
//! current user's Windows Credential Manager as generic credentials, keyed by
//! the exact resolved server `SocketAddr` (`IP:port`). That exact endpoint key
//! is also the autologin trust boundary: a credential saved for one server is
//! never offered to another resolved address.

use std::net::SocketAddr;

#[cfg(any(windows, test))]
const TARGET_PREFIX: &str = "DinurdoJK/jaPRO/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedLogin {
    pub server: SocketAddr,
    pub username: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

#[cfg(any(windows, test))]
fn target(server: SocketAddr) -> String {
    format!("{TARGET_PREFIX}{server}")
}

#[cfg(any(windows, test))]
fn parse_target(value: &str) -> Option<SocketAddr> {
    value.strip_prefix(TARGET_PREFIX)?.parse().ok()
}

#[cfg(windows)]
mod platform {
    use super::{parse_target, target, Credential, SavedLogin, TARGET_PREFIX};
    use std::{
        ffi::{c_void, OsStr},
        net::SocketAddr,
        os::windows::ffi::OsStrExt,
        ptr,
        slice,
    };

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: i32 = 1168;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct FileTime {
        dwLowDateTime: u32,
        dwHighDateTime: u32,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct CredentialAttributeW {
        Keyword: *mut u16,
        Flags: u32,
        ValueSize: u32,
        Value: *mut u8,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct CredentialW {
        Flags: u32,
        Type: u32,
        TargetName: *mut u16,
        Comment: *mut u16,
        LastWritten: FileTime,
        CredentialBlobSize: u32,
        CredentialBlob: *mut u8,
        Persist: u32,
        AttributeCount: u32,
        Attributes: *mut CredentialAttributeW,
        TargetAlias: *mut u16,
        UserName: *mut u16,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            credential_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, credential_type: u32, flags: u32) -> i32;
        fn CredEnumerateW(
            filter: *const u16,
            flags: u32,
            count: *mut u32,
            credentials: *mut *mut *mut CredentialW,
        ) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    unsafe fn wide_ptr_string(value: *const u16) -> String {
        if value.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        while *value.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(slice::from_raw_parts(value, len))
    }

    fn last_error(context: &str) -> String {
        let code = unsafe { GetLastError() };
        format!("{context} failed (Windows error {code})")
    }

    pub fn save(server: SocketAddr, username: &str, password: &str) -> Result<(), String> {
        let mut target = wide(&target(server));
        let mut username = wide(username);
        let mut password = password.as_bytes().to_vec();
        let blob_size = u32::try_from(password.len()).map_err(|_| "jaPRO password is too long".to_owned())?;
        let credential = CredentialW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: FileTime { dwLowDateTime: 0, dwHighDateTime: 0 },
            CredentialBlobSize: blob_size,
            CredentialBlob: password.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: username.as_mut_ptr(),
        };
        let ok = unsafe { CredWriteW(&credential, 0) };
        // Avoid keeping an extra plaintext copy alive in this temporary buffer.
        password.fill(0);
        if ok == 0 {
            Err(last_error("CredWriteW"))
        } else {
            Ok(())
        }
    }

    pub fn read(server: SocketAddr) -> Result<Option<Credential>, String> {
        let target = wide(&target(server));
        let mut raw: *mut CredentialW = ptr::null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) };
        if ok == 0 {
            let code = unsafe { GetLastError() } as i32;
            if code == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(last_error("CredReadW"));
        }
        if raw.is_null() {
            return Ok(None);
        }
        let value = unsafe {
            let credential = &*raw;
            let username = wide_ptr_string(credential.UserName);
            let bytes = if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
                &[][..]
            } else {
                slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            };
            let password = String::from_utf8(bytes.to_vec())
                .map_err(|_| "Saved jaPRO password is not valid UTF-8".to_owned());
            (username, password)
        };
        unsafe { CredFree(raw.cast()) };
        let (username, password) = value;
        Ok(Some(Credential { username, password: password? }))
    }

    pub fn delete(server: SocketAddr) -> Result<(), String> {
        let target = wide(&target(server));
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let code = unsafe { GetLastError() } as i32;
            if code == ERROR_NOT_FOUND {
                return Ok(());
            }
            Err(last_error("CredDeleteW"))
        } else {
            Ok(())
        }
    }

    pub fn list() -> Result<Vec<SavedLogin>, String> {
        let filter = wide(&format!("{TARGET_PREFIX}*"));
        let mut count = 0u32;
        let mut raw: *mut *mut CredentialW = ptr::null_mut();
        let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut raw) };
        if ok == 0 {
            let code = unsafe { GetLastError() } as i32;
            if code == ERROR_NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(last_error("CredEnumerateW"));
        }
        if raw.is_null() || count == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        unsafe {
            for &entry in slice::from_raw_parts(raw, count as usize) {
                if entry.is_null() {
                    continue;
                }
                let credential = &*entry;
                let target = wide_ptr_string(credential.TargetName);
                let Some(server) = parse_target(&target) else { continue };
                let username = wide_ptr_string(credential.UserName);
                result.push(SavedLogin { server, username });
            }
            CredFree(raw.cast());
        }
        result.sort_by_key(|entry| entry.server);
        Ok(result)
    }

    pub const fn supported() -> bool { true }
}

#[cfg(not(windows))]
mod platform {
    use super::{Credential, SavedLogin};
    use std::net::SocketAddr;

    const UNSUPPORTED: &str = "secure saved jaPRO logins currently require Windows Credential Manager";

    pub fn save(_server: SocketAddr, _username: &str, _password: &str) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn read(_server: SocketAddr) -> Result<Option<Credential>, String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn delete(_server: SocketAddr) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn list() -> Result<Vec<SavedLogin>, String> {
        Ok(Vec::new())
    }
    pub const fn supported() -> bool { false }
}

pub use platform::{delete, list, read, save, supported};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips_exact_endpoint() {
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        assert_eq!(parse_target(&target(server)), Some(server));
        assert!(parse_target("DinurdoJK/jaPRO/203.0.113.7").is_none());
        assert!(parse_target("OtherClient/jaPRO/203.0.113.7:29070").is_none());
    }
}
