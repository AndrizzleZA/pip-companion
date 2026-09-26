//! Phone sync over the user's own Google Drive.
//!
//! Pip keeps one small encrypted file in Drive's *application data folder* — a per-app area that
//! is invisible in Drive, unreadable by other apps, and covered by a permission Google classes as
//! non-sensitive. Pip can't see anything else in the account: the permission grants access to its
//! own folder and nothing more.
//!
//! Files there: `pip-sync.bin` (the encrypted document), plus short-lived `pip-join-*` requests
//! from devices asking to be let in and `pip-grant-*` replies carrying the sync key sealed for
//! exactly one requester. Nothing readable, and nothing for the user to type.

use crate::net;
use pip_core::sync::{self, join, SyncDoc};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_DRIVESYNC: u32 = WM_APP + 6;
pub const DOC_NAME: &str = "pip-sync.bin";
/// The device that starts the sync keeps the key locally; newcomers ask to join.
pub const JOIN_PREFIX: &str = "pip-join-";
pub const GRANT_PREFIX: &str = "pip-grant-";
/// "Not now": left behind so the waiting device learns it was refused instead of asking forever.
pub const DENY_PREFIX: &str = "pip-no-";
/// Refuse anything absurd: the document is a few kilobytes in practice.
const MAX_BYTES: usize = 2 * 1024 * 1024;

pub fn join_name(id: &str) -> String {
    format!("{JOIN_PREFIX}{id}.bin")
}

pub fn grant_name(id: &str) -> String {
    format!("{GRANT_PREFIX}{id}.bin")
}

pub fn deny_name(id: &str) -> String {
    format!("{DENY_PREFIX}{id}.bin")
}

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

/// Everything in Pip's own folder, as (id, name). It holds a handful of files at most.
fn list(token: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!("{}/files?spaces=appDataFolder&fields=files(id,name)&pageSize=100", api());
    let r = net::send("GET", &url, &auth(token), None)?;
    if r.status == 401 || r.status == 403 {
        return Err("Google signed Pip out of Drive".into());
    }
    if r.status != 200 {
        return Err(format!("Drive answered HTTP {}", r.status));
    }
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
    Ok(v["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| Some((f["id"].as_str()?.to_string(), f["name"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default())
}

/// The id of a file in the app folder, if it exists yet.
fn find(token: &str, name: &str) -> Result<Option<String>, String> {
    Ok(list(token)?.into_iter().find(|(_, n)| n == name).map(|(id, _)| id))
}

fn fetch(token: &str, id: &str) -> Result<Option<Vec<u8>>, String> {
    let r = net::send("GET", &format!("{}/files/{id}?alt=media", api()), &auth(token), None)?;
    match r.status {
        200 if r.body.len() <= MAX_BYTES => Ok(Some(r.body)),
        200 => Err("The sync file is unexpectedly large; ignoring it".into()),
        404 => Ok(None),
        401 | 403 => Err("Google signed Pip out of Drive".into()),
        s => Err(format!("Drive answered HTTP {s}")),
    }
}

pub fn download(token: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(id) = find(token, name)? else { return Ok(None) };
    fetch(token, &id)
}

/// Tidies up a request or an answer once it has done its job.
fn remove(token: &str, name: &str) -> Result<(), String> {
    let Some(id) = find(token, name)? else { return Ok(()) };
    remove_id(token, &id)
}

fn remove_id(token: &str, id: &str) -> Result<(), String> {
    let r = net::send("DELETE", &format!("{}/files/{id}", api()), &auth(token), None)?;
    match r.status {
        200 | 204 | 404 => Ok(()),
        401 | 403 => Err("Google signed Pip out of Drive".into()),
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

/// A device waiting to be let in. The seed never leaves this device; only the public half goes up.
#[derive(Clone, Debug, PartialEq)]
pub struct Joining {
    pub id: String,
    pub seed: [u8; 32],
    pub device: String,
    pub created_at: i64,
}

impl Joining {
    fn request(&self) -> (join::Secret, join::Request) {
        join::new_request(&self.seed, self.id.clone(), self.device.clone(), self.created_at)
    }
}

/// What one sync attempt produced, handed back to the UI thread.
#[derive(Default)]
pub struct SyncResult {
    pub merged: Option<SyncDoc>,
    pub access: Option<(String, i64)>,
    pub error: Option<String>,
    pub signed_out: bool,
    /// Another device approved this one: here is the shared key.
    pub adopted_key: Option<sync::Key>,
    /// This device started the sync (its candidate key is now the shared one).
    pub created_key: Option<sync::Key>,
    /// The join request is now up there; remember it so the next round looks for the answer.
    pub join_published: bool,
    /// Still waiting for someone to tap Approve.
    pub waiting: bool,
    /// The other device tapped Not now.
    pub refused: bool,
    /// Devices asking to join, for the "… wants to sync" card.
    pub requests: Vec<join::Request>,
    /// The round got far enough to read them, so an empty list really means nobody is waiting.
    pub fetched_requests: bool,
}

pub struct SyncJob {
    pub keys: crate::oauth::ClientKeys,
    pub refresh: String,
    pub access: Option<(String, i64)>,
    pub key: Option<sync::Key>,
    /// Set while this device has no key: the request it is making, or has already made.
    pub joining: Option<Joining>,
    pub joining_published: bool,
    /// A key this device could start the sync with, used only if no other device has yet.
    pub candidate: Option<sync::Key>,
    /// The user tapped Approve: seal the key for this one requester.
    pub approve: Option<(join::Request, [u8; 32], [u8; 24])>,
    /// The user tapped Not now: drop these requests.
    pub deny: Vec<String>,
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

/// One round: answer whatever the user tapped, then fetch, merge and push back.
/// Blocking; worker thread only.
pub fn sync_once(job: &SyncJob) -> SyncResult {
    let mut out = SyncResult { access: job.access.clone(), ..SyncResult::default() };
    let mut token = match fresh_token(job, &mut out) {
        Some(t) => t,
        None => return out,
    };

    // An approval goes up first, so nothing else can hold the other device up.
    if let (Some((req, random, nonce)), Some(key)) = (&job.approve, job.key) {
        match join::grant(req, &key, random, nonce) {
            Ok(sealed) => {
                let answer = grant_name(&req.id);
                if let Err(e) = with_token(job, &mut out, &mut token, |t| upload(t, &answer, &sealed)) {
                    out.signed_out = e.contains("signed Pip out");
                    out.error = Some(e);
                    return out;
                }
                let asked = join_name(&req.id);
                let _ = with_token(job, &mut out, &mut token, |t| remove(t, &asked));
            }
            Err(e) => {
                out.error = Some(e);
                return out;
            }
        }
    }
    for id in &job.deny {
        // A dated marker, so the refused device stops asking and the folder tidies itself later.
        if let Ok(body) = serde_json::to_vec(&join::Request { id: id.clone(), created_at: job.now, ..Default::default() }) {
            let no = deny_name(id);
            let _ = with_token(job, &mut out, &mut token, |t| upload(t, &no, &body));
        }
        let asked = join_name(id);
        let _ = with_token(job, &mut out, &mut token, |t| remove(t, &asked));
    }

    let mut key = job.key;
    if key.is_none() {
        let Some(j) = &job.joining else {
            out.error = Some("Sync isn't set up on this device yet".into());
            return out;
        };
        let files = match with_token(job, &mut out, &mut token, list) {
            Ok(f) => f,
            Err(e) => {
                out.signed_out = e.contains("signed Pip out");
                out.error = Some(e);
                return out;
            }
        };
        let id_of = |name: &str| files.iter().find(|(_, n)| n == name).map(|(id, _)| id.clone());
        let answer = id_of(&grant_name(&j.id));
        let refusal = id_of(&deny_name(&j.id));
        let asked = id_of(&join_name(&j.id));
        if let Some(answer) = answer {
            // Someone tapped Approve: the key is in there, sealed for this device alone.
            let bytes = match fetch(&token, &answer) {
                Ok(Some(b)) => b,
                Ok(None) => Vec::new(),
                Err(e) => {
                    out.signed_out = e.contains("signed Pip out");
                    out.error = Some(e);
                    return out;
                }
            };
            let (secret, _) = j.request();
            match join::accept(&secret, &bytes) {
                Ok(k) => {
                    key = Some(k);
                    out.adopted_key = Some(k);
                    let _ = remove_id(&token, &answer);
                    if let Some(asked) = &asked {
                        let _ = remove_id(&token, asked);
                    }
                }
                Err(e) => {
                    out.error = Some(e);
                    return out;
                }
            }
        } else if let Some(refusal) = refusal {
            out.refused = true;
            let _ = remove_id(&token, &refusal);
            if let Some(asked) = &asked {
                let _ = remove_id(&token, asked);
            }
            return out;
        } else if job.joining_published && id_of(DOC_NAME).is_some() {
            // Nobody has tapped anything yet. Put the request back if it aged out of the folder.
            out.waiting = true;
            if asked.is_none() {
                if let Err(e) = publish(job, &mut out, &mut token, j) {
                    out.error = Some(e);
                }
            }
            return out;
        } else if id_of(DOC_NAME).is_some() {
            // This account already syncs: ask to be let in.
            if let Err(e) = publish(job, &mut out, &mut token, j) {
                out.error = Some(e);
                return out;
            }
            out.join_published = true;
            out.waiting = true;
            return out;
        } else {
            // Nothing to join — either this is the first device, or the sync this one was waiting
            // for is gone. Either way, waiting for an approval nobody can give would strand it.
            let Some(candidate) = job.candidate else {
                out.error = Some("Sync isn't set up on this device yet".into());
                return out;
            };
            if let Some(asked) = &asked {
                let _ = remove_id(&token, asked); // our own request, now pointless
            }
            key = Some(candidate);
            out.created_key = Some(candidate);
        }
    }
    let Some(key) = key else {
        out.error = Some("Sync isn't set up on this device yet".into());
        return out;
    };

    // This device holds the key, so it is the one that can let others in.
    let waiting = pending(job, &mut out, &mut token, job.now);
    out.requests = waiting;
    out.fetched_requests = true;

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

/// Leaves this device's join request where the others will see it.
fn publish(job: &SyncJob, out: &mut SyncResult, token: &mut String, j: &Joining) -> Result<(), String> {
    let (_, req) = j.request();
    let body = serde_json::to_vec(&req).map_err(|_| "Couldn't write the join request".to_string())?;
    let asked = join_name(&j.id);
    with_token(job, out, token, |t| upload(t, &asked, &body))
}

/// Requests from devices waiting to be let in; expired ones are swept up on the way past.
fn pending(job: &SyncJob, out: &mut SyncResult, token: &mut String, now: i64) -> Vec<join::Request> {
    let Ok(files) = with_token(job, out, token, list) else { return Vec::new() };
    let mut found = Vec::new();
    for (id, name) in files.iter().filter(|(_, n)| n.starts_with(DENY_PREFIX)) {
        // A refusal nobody came back for.
        if let Ok(Some(bytes)) = fetch(token, id) {
            if serde_json::from_slice::<join::Request>(&bytes).map(|r| join::is_stale(&r, now)).unwrap_or(true) {
                let _ = remove(token, name);
            }
        }
    }
    for (id, name) in files.iter().filter(|(_, n)| n.starts_with(JOIN_PREFIX)) {
        let Ok(Some(bytes)) = fetch(token, id) else { continue };
        let Ok(req) = serde_json::from_slice::<join::Request>(&bytes) else { continue };
        if join::is_stale(&req, now) {
            let _ = remove(token, name);
            continue;
        }
        found.push(req);
    }
    found
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
    fn a_request_and_its_answer_are_named_after_the_same_attempt() {
        assert_eq!(join_name("7f3a"), "pip-join-7f3a.bin");
        assert_eq!(grant_name("7f3a"), "pip-grant-7f3a.bin");
        assert!(join_name("7f3a").starts_with(JOIN_PREFIX));
        assert!(grant_name("7f3a").starts_with(GRANT_PREFIX));
    }

    #[test]
    fn a_waiting_device_can_open_the_approval_meant_for_it() {
        let j = Joining { id: "7f3a".into(), seed: [6u8; 32], device: "Andre\u{2019}s phone".into(), created_at: 100 };
        let (_, req) = j.request();
        // The request travels as plain JSON: a device name and a public key, nothing private.
        let wire: join::Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(wire, req);

        let key: sync::Key = [17u8; 32];
        let sealed = join::grant(&wire, &key, &[8u8; 32], &[2u8; 24]).unwrap();
        // Only the device that kept the seed can open it.
        let (secret, _) = j.request();
        assert_eq!(join::accept(&secret, &sealed).unwrap(), key);
    }
}
