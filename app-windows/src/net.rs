//! Minimal HTTPS GET over WinHTTP — built into Windows, uses the OS certificate store, proxy
//! settings and TLS stack, so Pip ships no networking or crypto library of its own.
//! Only called from short-lived worker threads.

use crate::platform::wide;
use std::ffi::c_void;
use windows_sys::Win32::Networking::WinHttp::*;

pub const MAX_BODY: usize = 10 * 1024 * 1024;

pub struct Response {
    /// None for 304 Not Modified.
    pub body: Option<Vec<u8>>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

pub struct Target {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub secure: bool,
}

/// Accepts https:// and webcal:// (served over HTTPS). Plain http is refused: calendar
/// links are secrets and must not travel unencrypted.
pub fn parse_url(url: &str) -> Result<Target, String> {
    let url = url.trim();
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("webcal://").or_else(|| url.strip_prefix("webcals://")) {
        r
    } else if cfg!(feature = "devtools") && url.starts_with("http://127.0.0.1") {
        &url[7..] // dev builds only: local mock servers
    } else if url.starts_with("http://") {
        return Err("Only secure (https) calendar links are supported".into());
    } else {
        return Err("That doesn't look like a calendar link".into());
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err("That doesn't look like a calendar link".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "Invalid port in link".to_string())?),
        None => (authority, 443),
    };
    let secure = !(cfg!(feature = "devtools") && url.starts_with("http://127.0.0.1"));
    Ok(Target { host: host.to_string(), port, path: path.to_string(), secure })
}

fn header(req: *mut c_void, level: u32) -> Option<String> {
    unsafe {
        let mut len: u32 = 0;
        WinHttpQueryHeaders(req, level, std::ptr::null(), std::ptr::null_mut(), &mut len, std::ptr::null_mut());
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize / 2) + 1];
        if WinHttpQueryHeaders(req, level, std::ptr::null(), buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut()) == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize / 2]))
    }
}

/// A raw HTTP exchange: network problems are `Err`, any HTTP status is `Ok`.
pub struct Raw {
    pub status: u32,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Calendar download with validators and friendly status errors.
pub fn get(url: &str, etag: Option<&str>, last_modified: Option<&str>) -> Result<Response, String> {
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(e) = etag {
        headers.push(("If-None-Match".into(), e.into()));
    }
    if let Some(m) = last_modified {
        headers.push(("If-Modified-Since".into(), m.into()));
    }
    let r = send("GET", url, &headers, None)?;
    match r.status {
        304 => Ok(Response { body: None, etag: r.etag, last_modified: r.last_modified }),
        200 => Ok(Response { body: Some(r.body), etag: r.etag, last_modified: r.last_modified }),
        401 | 403 => Err("The calendar link was refused (it may have been reset)".into()),
        404 => Err("Calendar not found — check the link".into()),
        s => Err(format!("The calendar server answered HTTP {s}")),
    }
}

/// One HTTPS request. `headers` are extra request headers; `body` is sent as-is.
pub fn send(method: &str, url: &str, headers: &[(String, String)], body: Option<&[u8]>) -> Result<Raw, String> {
    let t = parse_url(url)?;
    unsafe {
        let ua = wide(concat!("Pip/", env!("CARGO_PKG_VERSION"), ""));
        let session = Handle(WinHttpOpen(ua.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
        if session.0.is_null() {
            return Err("Networking is unavailable".into());
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let flags: u32 = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
        WinHttpSetOption(session.0, WINHTTP_OPTION_DECOMPRESSION, &flags as *const u32 as *const c_void, 4);
        let conn = Handle(WinHttpConnect(session.0, wide(&t.host).as_ptr(), t.port, 0));
        if conn.0.is_null() {
            return Err(format!("Couldn't reach {}", t.host));
        }
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            wide(method).as_ptr(),
            wide(&t.path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            if t.secure { WINHTTP_FLAG_SECURE } else { 0 },
        ));
        if req.0.is_null() {
            return Err("Couldn't create the request".into());
        }
        let mut extra = String::new();
        for (k, v) in headers {
            extra.push_str(&format!("{k}: {v}\r\n"));
        }
        if !extra.is_empty() {
            let w = wide(&extra);
            WinHttpAddRequestHeaders(req.0, w.as_ptr(), (w.len() - 1) as u32, WINHTTP_ADDREQ_FLAG_ADD);
        }
        let (bp, bl) = body.map_or((std::ptr::null(), 0u32), |b| (b.as_ptr() as *const c_void, b.len() as u32));
        if WinHttpSendRequest(req.0, std::ptr::null(), 0, bp, bl, bl, 0) == 0 || WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
            let code = windows_sys::Win32::Foundation::GetLastError();
            return Err(match code {
                12007 => "No internet connection (couldn't resolve the server)".into(),
                12002 => "The server timed out".into(),
                12029 | 12030 => "Couldn't connect to the server".into(),
                12175 | 12045 | 12044 => "The server's security certificate was rejected".into(),
                c => format!("Network error ({c})"),
            });
        }
        let mut status: u32 = 0;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
        );
        let etag = header(req.0, WINHTTP_QUERY_ETAG);
        let last_modified = header(req.0, WINHTTP_QUERY_LAST_MODIFIED);
        let mut body = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(req.0, &mut avail) == 0 {
                return Err("The download was interrupted".into());
            }
            if avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            if WinHttpReadData(req.0, body[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                return Err("The download was interrupted".into());
            }
            body.truncate(start + read as usize);
            if body.len() > MAX_BODY {
                return Err("The response is too large (over 10 MB)".into());
            }
        }
        Ok(Raw { status, body, etag, last_modified })
    }
}

/// application/x-www-form-urlencoded value encoding.
pub fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let t = parse_url("webcal://p01-caldav.icloud.com/published/2/abc").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.path.as_str()), ("p01-caldav.icloud.com", 443, "/published/2/abc"));
        let t = parse_url("https://calendar.google.com/calendar/ical/x%40gmail.com/private-1/basic.ics").unwrap();
        assert_eq!(t.path, "/calendar/ical/x%40gmail.com/private-1/basic.ics");
        assert!(parse_url("http://insecure.example/cal.ics").is_err());
        assert!(parse_url("hello").is_err());
        assert_eq!(parse_url("https://h:8443/a?b=c").unwrap().port, 8443);
    }
}
