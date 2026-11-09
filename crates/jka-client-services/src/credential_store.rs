//! Secure per-server account credential storage.
//!
//! Passwords are never written to DinurdoJK.cfg. On Windows they live in the
//! current user's Windows Credential Manager as generic credentials, keyed by
//! the exact resolved server `SocketAddr` (`IP:port`) and account type. That
//! endpoint is the trust boundary: a credential saved for one server is never
//! offered to another resolved address. A friendly server name is optional
//! display metadata only.

use std::net::SocketAddr;

#[cfg(any(windows, test))]
const TARGET_PREFIX: &str = "DinurdoJK/account/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccountType {
    Login,
    Admin,
    Rcon,
    /// The server's `g_password` (join password for private servers).
    Server,
}

impl AccountType {
    pub const ALL: [Self; 4] = [Self::Login, Self::Admin, Self::Rcon, Self::Server];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Login => "Login",
            Self::Admin => "Admin",
            Self::Rcon => "RCON",
            Self::Server => "Server",
        }
    }

    #[cfg(any(windows, test))]
    const fn key(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Admin => "admin",
            Self::Rcon => "rcon",
            Self::Server => "server",
        }
    }

    #[cfg(any(windows, test))]
    fn from_key(value: &str) -> Option<Self> {
        match value {
            "login" => Some(Self::Login),
            "admin" => Some(Self::Admin),
            "rcon" => Some(Self::Rcon),
            "server" => Some(Self::Server),
            _ => None,
        }
    }

    const fn sort_key(self) -> u8 {
        match self {
            Self::Login => 0,
            Self::Admin => 1,
            Self::Rcon => 2,
            Self::Server => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedAccount {
    pub server: SocketAddr,
    pub account_type: AccountType,
    pub server_name: Option<String>,
    /// Only `Login` accounts require a username. Admin and RCON entries leave
    /// this empty and use the credential blob as their password/secret.
    pub username: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

#[cfg(any(windows, test))]
fn target(server: SocketAddr, account_type: AccountType) -> String {
    format!("{TARGET_PREFIX}{}/{server}", account_type.key())
}

#[cfg(windows)]
fn secret_target(name: &str) -> String {
    format!("DinurdoJK/secret/{name}")
}

#[cfg(any(windows, test))]
fn parse_target(value: &str) -> Option<(SocketAddr, AccountType)> {
    let value = value.strip_prefix(TARGET_PREFIX)?;
    let (kind, server) = value.split_once('/')?;
    Some((server.parse().ok()?, AccountType::from_key(kind)?))
}

#[cfg(windows)]
mod platform {
    use super::{parse_target, secret_target, target, AccountType, Credential, SavedAccount, TARGET_PREFIX};
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

    pub fn save(
        server: SocketAddr,
        account_type: AccountType,
        server_name: Option<&str>,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        write_target(&target(server, account_type), server_name, username, password)
    }

    /// Generic secrets (service API keys), never listed with the accounts.
    pub fn save_secret(name: &str, secret: &str) -> Result<(), String> {
        write_target(&secret_target(name), None, "DinurdoJK", secret)
    }

    pub fn read_secret(name: &str) -> Result<Option<String>, String> {
        Ok(read_target(&secret_target(name))?.map(|credential| credential.password))
    }

    pub fn delete_secret(name: &str) -> Result<(), String> {
        delete_target(&secret_target(name))
    }

    fn write_target(target: &str, comment: Option<&str>, username: &str, password: &str) -> Result<(), String> {
        let mut target = wide(target);
        let mut comment = comment
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(wide);
        let mut username = wide(username);
        let mut password = password.as_bytes().to_vec();
        let blob_size = u32::try_from(password.len())
            .map_err(|_| "Account password is too long".to_owned())?;
        let credential = CredentialW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment
                .as_mut()
                .map_or(ptr::null_mut(), |value| value.as_mut_ptr()),
            LastWritten: FileTime {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
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

    pub fn read(
        server: SocketAddr,
        account_type: AccountType,
    ) -> Result<Option<Credential>, String> {
        read_target(&target(server, account_type))
    }

    fn read_target(target: &str) -> Result<Option<Credential>, String> {
        let target = wide(target);
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
            let bytes = if credential.CredentialBlob.is_null()
                || credential.CredentialBlobSize == 0
            {
                &[][..]
            } else {
                slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            };
            let password = String::from_utf8(bytes.to_vec())
                .map_err(|_| "Saved account password is not valid UTF-8".to_owned());
            (username, password)
        };
        unsafe { CredFree(raw.cast()) };
        let (username, password) = value;
        Ok(Some(Credential {
            username,
            password: password?,
        }))
    }

    pub fn delete(server: SocketAddr, account_type: AccountType) -> Result<(), String> {
        delete_target(&target(server, account_type))
    }

    fn delete_target(target: &str) -> Result<(), String> {
        let target = wide(target);
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

    pub fn list() -> Result<Vec<SavedAccount>, String> {
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
                let target_name = wide_ptr_string(credential.TargetName);
                let Some((server, account_type)) = parse_target(&target_name) else {
                    continue;
                };
                let server_name = wide_ptr_string(credential.Comment);
                let username = wide_ptr_string(credential.UserName);
                result.push(SavedAccount {
                    server,
                    account_type,
                    server_name: (!server_name.is_empty()).then_some(server_name),
                    username,
                });
            }
            CredFree(raw.cast());
        }
        result.sort_by_key(|entry| (entry.server.to_string(), entry.account_type.sort_key()));
        Ok(result)
    }

    pub const fn supported() -> bool {
        true
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{AccountType, Credential, SavedAccount};
    use std::net::SocketAddr;

    const UNSUPPORTED: &str =
        "secure saved accounts currently require Windows Credential Manager";

    pub fn save(
        _server: SocketAddr,
        _account_type: AccountType,
        _server_name: Option<&str>,
        _username: &str,
        _password: &str,
    ) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn read(
        _server: SocketAddr,
        _account_type: AccountType,
    ) -> Result<Option<Credential>, String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn delete(_server: SocketAddr, _account_type: AccountType) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn list() -> Result<Vec<SavedAccount>, String> {
        Ok(Vec::new())
    }
    pub fn save_secret(_name: &str, _secret: &str) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
    pub fn read_secret(_name: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn delete_secret(_name: &str) -> Result<(), String> {
        Ok(())
    }
    pub const fn supported() -> bool {
        false
    }
}

pub use platform::{delete, delete_secret, list, read, read_secret, save, save_secret, supported};

/// Only the Server credential is eligible for automatic join authentication.
/// `Login`, `Admin` and `Rcon` must never be offered during a handshake.
pub fn server_password(server: SocketAddr) -> Result<Option<String>, String> {
    let Some(credential) = read(server, AccountType::Server)? else {
        return Ok(None);
    };
    if !valid_server_password(&credential.password) {
        return Err(format!("Saved server password for {server} has invalid info-string characters"));
    }
    Ok(Some(credential.password))
}

/// Quake info strings cannot contain backslashes, semicolons, quotes or NULs.
/// Validate *before* building the connect packet instead of silently omitting
/// the saved password when Info_SetValueForKey refuses it.
pub fn valid_server_password(password: &str) -> bool {
    !password.is_empty()
        && !password.chars().any(|c| c.is_control() || matches!(c, '\\' | ';' | '"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips_exact_endpoint_and_type() {
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        for account_type in AccountType::ALL.iter().copied() {
            assert_eq!(
                parse_target(&target(server, account_type)),
                Some((server, account_type))
            );
        }
        assert!(parse_target("DinurdoJK/account/unknown/203.0.113.7:29070").is_none());
    }

    #[test]
    fn private_server_password_must_be_wire_safe() {
        assert!(valid_server_password("hello world"));
        assert!(!valid_server_password(""));
        assert!(!valid_server_password("foo\\bar"));
        assert!(!valid_server_password("foo;bar"));
        assert!(!valid_server_password("foo\"bar"));
        assert!(!valid_server_password("foo\nbar"));
    }
}
