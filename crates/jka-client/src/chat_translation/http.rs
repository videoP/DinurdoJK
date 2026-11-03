//! Minimal blocking HTTP(S) for the translation workers, over WinHTTP like
//! the updater and race-ghost downloads. Only ever called off the main thread.
//! Errors never echo the URL: it can carry the chat text being translated.

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

struct Url {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<Url, String> {
    if url.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) || url.contains('\\') {
        return Err("translation URL contains unsafe characters".to_owned());
    }
    let (secure, rest, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest, 443)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest, 80)
    } else {
        return Err("translation URLs must use http:// or https://".to_owned());
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find(['/', '?']) {
        Some(index) if rest.as_bytes()[index] == b'/' => (&rest[..index], rest[index..].to_owned()),
        Some(index) => (&rest[..index], format!("/{}", &rest[index..])),
        None => (rest, "/".to_owned()),
    };
    if authority.contains('@') {
        return Err("translation URLs may not contain credentials".to_owned());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host, port.parse::<u16>().map_err(|_| "translation URL has an invalid port".to_owned())?)
        }
        _ => (authority, default_port),
    };
    if host.is_empty() {
        return Err("translation URL has no host".to_owned());
    }
    Ok(Url { secure, host: host.to_owned(), port, path })
}

/// `max_bytes` caps the response body; `timeout_ms` applies to each phase.
pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout_ms: i32,
    max_bytes: usize,
) -> Result<Response, String> {
    platform::request(&parse_url(url)?, method, headers, body, timeout_ms, max_bytes)
}

#[cfg(windows)]
mod platform {
    use super::{Response, Url};
    use std::{ffi::c_void, ptr};

    type HInternet = *mut c_void;
    const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
    const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;
    const WINHTTP_OPTION_REDIRECT_POLICY_NEVER: u32 = 0;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> HInternet;
        fn WinHttpConnect(session: HInternet, server: *const u16, port: u16, reserved: u32) -> HInternet;
        fn WinHttpOpenRequest(
            connect: HInternet,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept: *const *const u16,
            flags: u32,
        ) -> HInternet;
        fn WinHttpSetTimeouts(handle: HInternet, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
        fn WinHttpSetOption(handle: HInternet, option: u32, buffer: *mut c_void, length: u32) -> i32;
        fn WinHttpSendRequest(
            request: HInternet,
            headers: *const u16,
            headers_length: u32,
            optional: *mut c_void,
            optional_length: u32,
            total_length: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: HInternet, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            request: HInternet,
            info: u32,
            name: *const u16,
            buffer: *mut c_void,
            length: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpReadData(request: HInternet, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: HInternet) -> i32;
    }

    struct Handle(HInternet);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn request(
        url: &Url,
        method: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        timeout_ms: i32,
        max_bytes: usize,
    ) -> Result<Response, String> {
        let agent = wide("DinurdoJK/0.1 chat-translation");
        let session = Handle(unsafe {
            WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, ptr::null(), ptr::null(), 0)
        });
        if session.0.is_null() {
            return Err("WinHTTP open failed".to_owned());
        }
        unsafe { WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms) };
        let host = wide(&url.host);
        let connect = Handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), url.port, 0) });
        if connect.0.is_null() {
            return Err(format!("could not connect to {}", url.host));
        }
        let verb = wide(method);
        let path = wide(&url.path);
        let request = Handle(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                if url.secure { WINHTTP_FLAG_SECURE } else { 0 },
            )
        });
        if request.0.is_null() {
            return Err("WinHTTP request creation failed".to_owned());
        }
        // A redirect could forward the chat text (and an API key header) to
        // another host; translation endpoints never need one.
        let mut policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        unsafe {
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&mut policy as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        let mut header_text = String::from("Accept-Encoding: identity\r\n");
        for (name, value) in headers {
            header_text.push_str(&format!("{name}: {value}\r\n"));
        }
        let header_wide = wide(&header_text);
        let mut body = body.to_vec();
        let length = u32::try_from(body.len()).map_err(|_| "request body too large".to_owned())?;
        let sent = unsafe {
            WinHttpSendRequest(
                request.0,
                header_wide.as_ptr(),
                u32::MAX,
                if body.is_empty() { ptr::null_mut() } else { body.as_mut_ptr().cast() },
                length,
                length,
                0,
            )
        };
        if sent == 0 || unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
            return Err(format!("no response from {}", url.host));
        }
        let mut status = 0u32;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        if unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut status_len,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(format!("unreadable response from {}", url.host));
        }
        let mut out = Vec::new();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            if unsafe { WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read) } == 0 {
                return Err(format!("response from {} was cut off", url.host));
            }
            if read == 0 {
                break;
            }
            if out.len() + read as usize > max_bytes {
                return Err(format!("response from {} is too large", url.host));
            }
            out.extend_from_slice(&buffer[..read as usize]);
        }
        Ok(Response { status: status as u16, body: out })
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Response, Url};

    pub(super) fn request(
        _url: &Url,
        _method: &str,
        _headers: &[(&str, &str)],
        _body: &[u8],
        _timeout_ms: i32,
        _max_bytes: usize,
    ) -> Result<Response, String> {
        Err("chat translation needs Windows WinHTTP in this build".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split_into_host_port_and_path() {
        let url = parse_url("https://api.mymemory.translated.net/get?q=a").unwrap();
        assert!(url.secure);
        assert_eq!((url.host.as_str(), url.port, url.path.as_str()), ("api.mymemory.translated.net", 443, "/get?q=a"));
        let url = parse_url("http://localhost:5000").unwrap();
        assert_eq!((url.host.as_str(), url.port, url.path.as_str()), ("localhost", 5000, "/"));
        assert!(parse_url("ftp://x").is_err());
        assert!(parse_url("https://user:pw@host/").is_err());
    }
}
