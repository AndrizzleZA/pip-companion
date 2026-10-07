//! Gmail checks on a short-lived worker thread. Each check is one small "what changed since
//! history X" request; only messages that landed unread in the inbox are looked at, and only their
//! From/Subject headers (metadata scope). Results are posted back to the UI thread.

use crate::net;
use crate::oauth::{self, ClientKeys};
use pip_core::email::{gmail, important, EmailMsg, EmailSettings};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_MAILSYNC: u32 = WM_APP + 5;
/// Never examine more than this many new messages in one check (e.g. after a long time offline).
const MAX_NEW: usize = 40;

pub struct MailJob {
    pub account_id: String,
    pub refresh: String,
    pub access: Option<(String, i64)>,
    pub history_id: Option<String>,
}

#[derive(Default)]
pub struct MailResult {
    pub account_id: String,
    pub access: Option<(String, i64)>,
    pub history_id: Option<String>,
    pub important: Vec<EmailMsg>,
    /// New inbox messages looked at in this check.
    pub examined: usize,
    pub error: Option<String>,
    /// The refresh token is no longer valid — the user must sign in again.
    pub signed_out: bool,
}

enum Fetch {
    Ok(String),
    Unauthorized,
    NotFound,
    Err(String),
}

fn fetch(url: &str, token: &str) -> Fetch {
    let headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
    match net::send("GET", url, &headers, None) {
        Ok(r) if r.status == 200 => Fetch::Ok(String::from_utf8_lossy(&r.body).into_owned()),
        Ok(r) if r.status == 401 => Fetch::Unauthorized,
        Ok(r) if r.status == 404 => Fetch::NotFound,
        Ok(r) if r.status == 403 => Fetch::Err("Gmail refused access for now; Pip will try again later".into()),
        Ok(r) if r.status == 429 => Fetch::Err("Gmail asked Pip to slow down; trying again later".into()),
        Ok(r) => Fetch::Err(format!("Gmail answered HTTP {}", r.status)),
        Err(e) => Fetch::Err(e),
    }
}

fn check(keys: &ClientKeys, rules: &EmailSettings, job: &MailJob, now: i64) -> MailResult {
    let api = oauth::gmail_api();
    let mut res = MailResult { account_id: job.account_id.clone(), access: job.access.clone(), history_id: job.history_id.clone(), ..Default::default() };

    // A valid access token, refreshing if needed (and once more if Gmail says it's stale).
    let token = |force: bool, res: &mut MailResult| -> Option<String> {
        if !force {
            if let Some((t, exp)) = &res.access {
                if *exp > now + 60 {
                    return Some(t.clone());
                }
            }
        }
        match oauth::refresh(&oauth::GOOGLE, keys, &job.refresh, now) {
            Ok(t) => {
                res.access = Some((t.access.clone(), t.expires_at));
                Some(t.access)
            }
            Err(e) => {
                res.signed_out = e.revoked;
                res.error = Some(e.message);
                res.access = None;
                None
            }
        }
    };
    let mut get = |url: &str, res: &mut MailResult| -> Fetch {
        let Some(t) = token(false, res) else { return Fetch::Err(res.error.clone().unwrap_or_default()) };
        match fetch(url, &t) {
            Fetch::Unauthorized => match token(true, res) {
                Some(t) => fetch(url, &t),
                None => Fetch::Err(res.error.clone().unwrap_or_default()),
            },
            other => other,
        }
    };

    // First run (or history expired): remember where the mailbox is now; don't announce old mail.
    let baseline = |res: &mut MailResult, get: &mut dyn FnMut(&str, &mut MailResult) -> Fetch| match get(&gmail::profile_path(&api), res) {
        Fetch::Ok(body) => {
            if let Some((_, hid)) = gmail::parse_profile(&body) {
                res.history_id = Some(hid);
            }
        }
        Fetch::Err(e) => res.error = Some(e),
        _ => res.error = Some("Couldn't read the Gmail profile".into()),
    };

    let Some(start) = res.history_id.clone() else {
        baseline(&mut res, &mut get);
        return res;
    };

    let mut ids: Vec<String> = Vec::new();
    let mut page: Option<String> = None;
    let mut latest = None;
    for _ in 0..5 {
        match get(&gmail::history_path(&api, &start, page.as_deref()), &mut res) {
            Fetch::Ok(body) => {
                let Some(h) = gmail::parse_history(&body) else {
                    res.error = Some("Unexpected answer from Gmail".into());
                    return res;
                };
                for id in h.added {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                latest = h.history_id.or(latest);
                match h.next_page {
                    Some(t) => page = Some(t),
                    None => break,
                }
            }
            Fetch::NotFound => {
                // History too old (Gmail keeps about a week): start fresh from now.
                baseline(&mut res, &mut get);
                return res;
            }
            Fetch::Err(e) => {
                res.error = Some(e);
                return res;
            }
            Fetch::Unauthorized => {
                res.error = Some("Gmail didn't accept the sign-in".into());
                return res;
            }
        }
    }

    // Newest first, capped.
    ids.reverse();
    ids.truncate(MAX_NEW);
    for id in &ids {
        match get(&gmail::message_path(&api, id), &mut res) {
            Fetch::Ok(body) => {
                res.examined += 1;
                if let Some(m) = gmail::parse_message(&body, &job.account_id) {
                    if important(rules, &m).is_some() {
                        res.important.push(m);
                    }
                }
            }
            Fetch::NotFound => {} // deleted in the meantime
            Fetch::Err(e) => {
                res.error = Some(e);
                return res; // keep the old history id so these are retried next time
            }
            Fetch::Unauthorized => return res,
        }
    }
    if let Some(h) = latest {
        res.history_id = Some(h);
    }
    res
}

/// Sign-in check for Gmail: which address is this, and where does its mailbox history stand?
/// Only mail arriving after this point is ever considered, so connecting never floods you.
pub fn verify_gmail(access: &str) -> Result<(String, String), pip_core::connectors::SignInProblem> {
    use pip_core::connectors::SignInProblem;
    let headers = vec![("Authorization".to_string(), format!("Bearer {access}"))];
    match net::send("GET", &gmail::profile_path(&oauth::gmail_api()), &headers, None) {
        Ok(r) if r.status == 200 => {
            gmail::parse_profile(&String::from_utf8_lossy(&r.body)).ok_or_else(|| SignInProblem::Other("Gmail sent an unexpected answer".into()))
        }
        Ok(r) if r.status == 401 || r.status == 403 => Err(SignInProblem::MissingPermission),
        Ok(r) => Err(SignInProblem::Other(format!("Gmail answered HTTP {}", r.status))),
        Err(e) => Err(SignInProblem::Offline(e)),
    }
}

/// Starts a check in the background. `false` means the thread never started, and the caller has to
/// put itself back together; otherwise an answer always comes back, even an empty one.
pub fn start(hwnd: HWND, keys: ClientKeys, rules: EmailSettings, jobs: Vec<MailJob>) -> bool {
    let hwnd_bits = hwnd as usize;
    let now = crate::platform::now_unix();
    std::thread::Builder::new()
        .name("pip-mail".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            // No catch_unwind here. Release builds abort on panic (Cargo.toml), so catching was
            // only ever possible in a debug build - the one place a crash does least harm - and
            // the comment that used to sit here promised a resilience that shipped to nobody. A
            // panic in a mail check is a bug; it now leaves a report behind instead (crash.rs).
            let results: Vec<MailResult> = jobs.iter().map(|j| check(&keys, &rules, j, now)).collect();
            let ptr = Box::into_raw(Box::new(results));
            unsafe {
                if PostMessageW(hwnd_bits as HWND, WM_MAILSYNC, 0, ptr as isize) == 0 {
                    drop(Box::from_raw(ptr));
                }
            }
        })
        .is_ok()
}

/// # Safety
/// `lparam` must come from a `WM_MAILSYNC` message posted by [`start`].
pub unsafe fn take_result(lparam: isize) -> Box<Vec<MailResult>> {
    Box::from_raw(lparam as *mut Vec<MailResult>)
}
