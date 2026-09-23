//! Phone sync over the user's own Google Drive.
//!
//! Pip keeps one small encrypted file in Drive's *application data folder* — a per-app area that
//! is invisible in Drive, unreadable by other apps, and covered by a permission Google classes as
//! non-sensitive. Pip can't see anything else in the account: the permission grants access to its
//! own folder and nothing more.
//!
//! Two files live there: `pip-sync.bin` (the encrypted document) and `pip-key.bin` (the sync key,
//! itself encrypted with the pairing code, so a second device can pick it up).

use crate::net;
use pip_core::sync::{self, SyncDoc};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_DRIVESYNC: u32 = WM_APP + 6;
pub const DOC_NAME: &str = "pip-sync.bin";
pub const KEY_NAME: &str = "pip-key.bin";
/// Refuse anything absurd: the document is a few kilobytes in practice.
const MAX_BYTES: usize = 2 * 1024 * 1024;

fn api() -> String {
    #[cfg(feature = "devtools")]
    if let Ok(v) = std::env::var("PIP_DRIVE_API") {
        return v;
    }
    "https://www.googleapis.com/drive/v3".into()
}

fn upload_api() -> String {
    #[cfg(feature = "devtools")]
    if let Ok(v) = std::env::var("PIP_DRIVE_UPLOAD") {
        return v;
    }
    "https://www.googleapis.com/upload/drive/v3".into()
}

fn auth(token: &str) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), format!("Bearer {token}"))]
}

/// The id of a file in the app folder, if it exists yet.
fn find(token: &str, name: &str) -> Result<Option<String>, String> {
    let url = format!("{}/files?spaces=appDataFolder&fields=files(id,name)&q=name%3D%27{name}%27", api());
    let r = net::send("GET", &url, &auth(token), None)?;
    if r.status == 401 || r.status == 403 {
        return Err("Google signed Pip out of Drive".into());
    }
    if r.status != 200 {
        return Err(format!("Drive answered HTTP {}", r.status));
    }
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
    // Trust nothing: only accept a file whose name is exactly the one asked for.
    Ok(v["files"]
        .as_array()
        .and_then(|files| files.iter().find(|f| f["name"].as_str() == Some(name)))
        .and_then(|f| f["id"].as_str())
        .map(String::from))
}

pub fn download(token: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(id) = find(token, name)? else { return Ok(None) };
    let r = net::send("GET", &format!("{}/files/{id}?alt=media", api()), &auth(token), None)?;
    match r.status {
        200 if r.body.len() <= MAX_BYTES => Ok(Some(r.body)),
        200 => Err("The sync file is unexpectedly large; ignoring it".into()),
        404 => Ok(None),
        s => Err(format!("Drive answered HTTP {s}")),
    }
}

/// Creates or replaces a file in the app folder (multipart: metadata + bytes in one request).
pub fn upload(token: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let existing = find(token, name)?;
    const BOUNDARY: &str = "pipsyncboundary";
    let meta = match &existing {
        Some(_) => format!("{{\"name\":\"{name}\"}}"),
        None => format!("{{\"name\":\"{name}\",\"parents\":[\"appDataFolder\"]}}"),
    };
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n").as_bytes());
    body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    let mut headers = auth(token);
    headers.push(("Content-Type".into(), format!("multipart/related; boundary={BOUNDARY}")));
    let (method, url) = match existing {
        Some(id) => ("PATCH", format!("{}/files/{id}?uploadType=multipart", upload_api())),
        None => ("POST", format!("{}/files?uploadType=multipart", upload_api())),
    };
    let r = net::send(method, &url, &headers, Some(&body))?;
    match r.status {
        200 | 201 => Ok(()),
        401 | 403 => Err("Google signed Pip out of Drive".into()),
        s => Err(format!("Drive wouldn't save the sync file (HTTP {s})")),
    }
}

/// What one sync attempt produced, handed back to the UI thread.
pub struct SyncResult {
    pub merged: Option<SyncDoc>,
    pub access: Option<(String, i64)>,
    pub error: Option<String>,
    pub signed_out: bool,
    /// The pairing code was consumed to fetch the key on this device.
    pub adopted_key: Option<sync::Key>,
    /// This device created the sync (its candidate key is now the shared one).
    pub created_key: Option<(sync::Key, String)>,
    /// Another device already syncs: Pip needs the pairing code from it.
    pub needs_code: bool,
}

pub struct SyncJob {
    pub keys: crate::oauth::ClientKeys,
    pub refresh: String,
    pub access: Option<(String, i64)>,
    pub key: Option<sync::Key>,
    /// Set when this device is joining an existing sync with a typed pairing code.
    pub pairing_code: Option<String>,
    /// A key this device could start sync with, used only if no other device has yet.
    pub candidate: Option<(sync::Key, String, [u8; 24])>,
    pub local: SyncDoc,
    pub nonce: [u8; 24],
    pub now: i64,
}

/// Runs one Drive call, refreshing the access token once if Drive says it is stale.
fn with_token<T>(job: &SyncJob, out: &mut SyncResult, token: &mut String, mut call: impl FnMut(&str) -> Result<T, String>) -> Result<T, String> {
    match call(token) {
        Err(e) if e.contains("signed Pip out") => {
            // Expired access tokens look exactly like this; try one fresh token before giving up.
            match crate::oauth::refresh(&crate::oauth::GOOGLE_SYNC, &job.keys, &job.refresh, job.now) {
                Ok(t) => {
                    *token = t.access.clone();
                    out.access = Some((t.access, t.expires_at));
                    call(token)
                }
                Err(e2) => {
                    out.signed_out = e2.revoked;
                    Err(e2.message)
                }
            }
        }
        other => other,
    }
}

/// One round: fetch, merge, push back. Blocking; worker thread only.
pub fn sync_once(job: &SyncJob) -> SyncResult {
    let mut out = SyncResult { merged: None, access: job.access.clone(), error: None, signed_out: false, adopted_key: None, created_key: None, needs_code: false };
    let mut token = match fresh_token(job, &mut out) {
        Some(t) => t,
        None => return out,
    };

    // Joining an existing sync: the key is in the cloud, wrapped with the pairing code.
    let mut key = job.key;
    if let Some(code) = &job.pairing_code {
        match with_token(job, &mut out, &mut token, |t| download(t, KEY_NAME)) {
            Ok(Some(bytes)) => match unwrap_key(code, &bytes) {
                Ok(k) => {
                    key = Some(k);
                    out.adopted_key = Some(k);
                }
                Err(e) => {
                    out.error = Some(e);
                    return out;
                }
            },
            Ok(None) => {
                out.error = Some("No sync data found in this Google account yet".into());
                return out;
            }
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        }
    }
    // No key yet: start the sync if nobody else has, otherwise ask for their pairing code.
    if key.is_none() {
        match with_token(job, &mut out, &mut token, |t| download(t, KEY_NAME)) {
            Ok(Some(_)) => {
                out.needs_code = true;
                out.error = Some("Another device already syncs. Type its pairing code to join.".into());
                return out;
            }
            Ok(None) => {
                let Some((candidate, code, nonce)) = job.candidate.clone() else {
                    out.error = Some("Sync isn't set up on this device yet".into());
                    return out;
                };
                match wrap_key(&code, &candidate, &nonce) {
                    Ok(wrapped) => {
                        if let Err(e) = with_token(job, &mut out, &mut token, |t| upload(t, KEY_NAME, &wrapped)) {
                            out.error = Some(e);
                            return out;
                        }
                        key = Some(candidate);
                        out.created_key = Some((candidate, code));
                    }
                    Err(e) => {
                        out.error = Some(e);
                        return out;
                    }
                }
            }
            Err(e) => {
                out.signed_out = e.contains("signed Pip out");
                out.error = Some(e);
                return out;
            }
        }
    }
    let Some(key) = key else {
        out.error = Some("Sync isn't set up on this device yet".into());
        return out;
    };

    let remote = match with_token(job, &mut out, &mut token, |t| download(t, DOC_NAME)) {
        Ok(Some(bytes)) => match sync::open(&key, &bytes) {
            Ok(doc) => Some(doc),
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        },
        Ok(None) => None,
        Err(e) => {
            out.signed_out = e.contains("signed Pip out");
            out.error = Some(e);
            return out;
        }
    };

    let merged = match &remote {
        Some(r) => job.local.merge(r),
        None => job.local.clone(),
    };
    // Only write when we actually have something new to say.
    if remote.as_ref() != Some(&merged) {
        let sealed = match sync::seal(&key, &job.nonce, &merged) {
            Ok(b) => b,
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        };
        if let Err(e) = with_token(job, &mut out, &mut token, |t| upload(t, DOC_NAME, &sealed)) {
            out.signed_out = e.contains("signed Pip out");
            out.error = Some(e);
            return out;
        }
    }
    out.merged = Some(merged);
    out
}

fn fresh_token(job: &SyncJob, out: &mut SyncResult) -> Option<String> {
    if let Some((t, exp)) = &job.access {
        if *exp > job.now + 60 {
            return Some(t.clone());
        }
    }
    match crate::oauth::refresh(&crate::oauth::GOOGLE_SYNC, &job.keys, &job.refresh, job.now) {
        Ok(t) => {
            out.access = Some((t.access.clone(), t.expires_at));
            Some(t.access)
        }
        Err(e) => {
            out.signed_out = e.revoked;
            out.error = Some(e.message);
            out.access = None;
            None
        }
    }
}

/// Wraps the sync key so it can wait in the cloud for the next device.
pub fn wrap_key(code: &str, key: &sync::Key, nonce: &[u8; 24]) -> Result<Vec<u8>, String> {
    let code_key = sync::pairing::key_from_code(code, b"pip-pairing")?;
    let mut holder = SyncDoc::default();
    holder.device = hex(key);
    sync::seal(&code_key, nonce, &holder)
}

pub fn unwrap_key(code: &str, bytes: &[u8]) -> Result<sync::Key, String> {
    let code_key = sync::pairing::key_from_code(code, b"pip-pairing")?;
    let holder = sync::open(&code_key, bytes).map_err(|_| "That pairing code doesn't match. Check it and try again.".to_string())?;
    unhex(&holder.device).ok_or_else(|| "The stored sync key is damaged".to_string())
}

fn hex(k: &sync::Key) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<sync::Key> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(std::str::from_utf8(&b[i * 2..i * 2 + 2]).ok()?, 16).ok()?;
    }
    Some(out)
}

pub fn start(hwnd: HWND, job: SyncJob) {
    let hwnd_bits = hwnd as usize;
    let _ = std::thread::Builder::new().name("pip-drivesync".into()).stack_size(256 * 1024).spawn(move || {
        let result = sync_once(&job);
        let ptr = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(hwnd_bits as HWND, WM_DRIVESYNC, 0, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

/// # Safety
/// `lparam` must come from a `WM_DRIVESYNC` message posted by [`start`].
pub unsafe fn take_result(lparam: isize) -> Box<SyncResult> {
    Box::from_raw(lparam as *mut SyncResult)
}

/// Sign-in check for sync: touching the app folder proves the permission works.
pub fn verify_drive(access: &str) -> Result<(String, String), pip_core::connectors::SignInProblem> {
    use pip_core::connectors::SignInProblem;
    match find(access, DOC_NAME) {
        Ok(_) => Ok(("your Google Drive".into(), String::new())),
        Err(e) if e.contains("signed Pip out") => Err(SignInProblem::MissingPermission),
        Err(e) => Err(SignInProblem::Offline(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_survives_the_pairing_code_round_trip() {
        let key: sync::Key = [17u8; 32];
        let code = sync::pairing::code_from_bytes(&[5; 10]);
        let wrapped = wrap_key(&code, &key, &[2u8; 24]).unwrap();
        assert_eq!(unwrap_key(&code, &wrapped).unwrap(), key);
        assert!(unwrap_key("ABCD-EFGH-JKLM-NPQR", &wrapped).is_err(), "a wrong code must not open it");
    }

    #[test]
    fn hex_round_trips() {
        let k: sync::Key = [0xAB; 32];
        assert_eq!(unhex(&hex(&k)).unwrap(), k);
        assert!(unhex("nonsense").is_none());
    }
}
