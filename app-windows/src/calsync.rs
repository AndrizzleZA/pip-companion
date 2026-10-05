//! Calendar subscription sync. Runs on a short-lived worker thread (network + parsing never touch
//! the UI thread) and posts the result back. Feeds are cached on disk, so calendars keep working
//! offline and on startup without any network access.
//!
//! Only a two-week window of expanded occurrences is kept in memory.

use crate::net;
use chrono::{Duration, NaiveDate};
use pip_core::calendar::{self, Event};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_CALSYNC: u32 = WM_APP + 3;
/// Days of occurrences kept in memory, starting yesterday.
pub const WINDOW_DAYS: i64 = 16;

#[derive(Clone, Debug)]
pub struct FeedJob {
    pub id: String,
    pub url: String,
    /// Set for calendars connected with "Continue with Google" (`gcal:` feeds).
    pub google: Option<GoogleAuth>,
}

#[derive(Clone, Debug)]
pub struct GoogleAuth {
    pub keys: crate::oauth::ClientKeys,
    pub refresh: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Last successful contact with the server (Unix seconds).
    pub synced_at: i64,
    pub error: Option<String>,
    /// Occurrences in the current window.
    pub events: usize,
    /// Name announced by the feed (X-WR-CALNAME).
    pub name: Option<String>,
    /// A Google-connected calendar whose sign-in no longer works (offer Reconnect).
    pub signed_out: bool,
}

pub struct SyncResult {
    pub online: bool,
    pub feeds: Vec<(String, FeedMeta)>,
    pub events: Vec<Event>,
}

fn cal_dir(dir: &Path) -> PathBuf {
    dir.join("calendars")
}

pub fn load_meta(dir: &Path, id: &str) -> FeedMeta {
    std::fs::read(cal_dir(dir).join(format!("{id}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> bool {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).is_ok()
}

/// Removes a feed's cache (after it is deleted in settings).
pub fn forget(dir: &Path, id: &str) {
    let d = cal_dir(dir);
    let _ = std::fs::remove_file(d.join(format!("{id}.ics")));
    let _ = std::fs::remove_file(d.join(format!("{id}.json")));
}

/// Sync one feed and return its occurrences in the window. Blocking; worker thread only.
pub fn sync_feed(dir: &Path, job: &FeedJob, online: bool, from_day: NaiveDate, now: i64) -> (FeedMeta, Vec<Event>) {
    if pip_core::gcal::feed_address(&job.url).is_some() {
        return sync_google(dir, job, online, from_day, now);
    }
    let d = cal_dir(dir);
    let _ = std::fs::create_dir_all(&d);
    let ics_path = d.join(format!("{}.ics", job.id));
    let mut meta = load_meta(dir, &job.id);
    if online && !job.url.trim().is_empty() {
        // Only send validators if we still have the cached body they describe.
        let have_cache = ics_path.exists();
        let (etag, lm) = if have_cache { (meta.etag.as_deref(), meta.last_modified.as_deref()) } else { (None, None) };
        match net::get(&job.url, etag, lm) {
            Ok(resp) => {
                if let Some(body) = &resp.body {
                    // Never replace a good cache with something that isn't a calendar (login pages,
                    // error pages, a pasted website link...).
                    let looks_like_ics = body.windows(15).take(4096).any(|w| w == b"BEGIN:VCALENDAR");
                    if !looks_like_ics {
                        meta.error = Some("That link didn't return a calendar".into());
                        let _ = std::fs::write(d.join(format!("{}.json", job.id)), serde_json::to_vec_pretty(&meta).unwrap_or_default());
                        return (meta, cached_events(&ics_path, &job.id, from_day));
                    }
                    if !write_atomic(&ics_path, body) {
                        meta.error = Some("Couldn't save the calendar".into());
                    }
                }
                // A fresh body replaces the validators; a 304 keeps ours unless the server sent new ones.
                if resp.body.is_some() || resp.etag.is_some() {
                    meta.etag = resp.etag;
                }
                if resp.body.is_some() || resp.last_modified.is_some() {
                    meta.last_modified = resp.last_modified;
                }
                meta.synced_at = now;
                meta.error = None;
            }
            Err(e) => meta.error = Some(e),
        }
    }
    let mut events = Vec::new();
    if let Ok(bytes) = std::fs::read(&ics_path) {
        let text = String::from_utf8_lossy(&bytes);
        let cal = calendar::parse_ics(&text);
        meta.name = cal.name.clone();
        if cal.event_count() == 0 && !text.contains("BEGIN:VCALENDAR") {
            meta.error = Some("That link didn't return a calendar".into());
        }
        events = calendar::expand_days(&cal, &job.id, from_day - Duration::days(1), WINDOW_DAYS, &chrono::Local);
    }
    meta.events = events.len();
    if let Ok(json) = serde_json::to_vec_pretty(&meta) {
        write_atomic(&d.join(format!("{}.json", job.id)), &json);
    }
    (meta, events)
}

// ---------------------------------------------------------------------------------------------
// Calendars connected with "Continue with Google"
// ---------------------------------------------------------------------------------------------

fn day_start(day: NaiveDate) -> i64 {
    pip_core::time::to_ts(&chrono::Local, day.and_hms_opt(0, 0, 0).unwrap_or_default())
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// One authenticated GET against the Calendar API.
fn gcal_get(url: &str, token: &str) -> Result<String, (String, bool)> {
    let headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
    match net::send("GET", url, &headers, None) {
        Ok(r) if r.status == 200 => Ok(String::from_utf8_lossy(&r.body).into_owned()),
        Ok(r) if r.status == 401 || r.status == 403 => Err(("Google signed Pip out \u{2014} click Reconnect".into(), true)),
        Ok(r) if r.status == 429 || r.status == 503 => Err(("Google asked Pip to slow down; trying again later".into(), false)),
        Ok(r) => Err((format!("Google Calendar answered HTTP {}", r.status), false)),
        Err(e) => Err((e, false)),
    }
}

/// Fetches the window's events for every calendar the user has ticked in Google Calendar.
/// Falls back to the cached copy whenever the network (or the sign-in) isn't available.
fn sync_google(dir: &Path, job: &FeedJob, online: bool, from_day: NaiveDate, now: i64) -> (FeedMeta, Vec<Event>) {
    let d = cal_dir(dir);
    let _ = std::fs::create_dir_all(&d);
    let cache = d.join(format!("{}.events.json", job.id));
    let mut meta = load_meta(dir, &job.id);
    meta.name = pip_core::gcal::feed_address(&job.url).map(|a| a.to_string());
    let window_from = day_start(from_day - Duration::days(1));
    let window_to = day_start(from_day + Duration::days(WINDOW_DAYS - 1));

    let fetched = match (online, &job.google) {
        (true, Some(auth)) => Some(fetch_google(&auth.keys, &auth.refresh, &window_from, &window_to, &job.id, now)),
        _ => None,
    };
    match fetched {
        Some(Ok(events)) => {
            if let Ok(json) = serde_json::to_vec(&events) {
                write_atomic(&cache, &json);
            }
            meta.synced_at = now;
            meta.error = None;
            meta.signed_out = false;
            meta.events = events.len();
            save_meta(&d, &job.id, &meta);
            (meta, events)
        }
        other => {
            if let Some(Err((msg, signed_out))) = other {
                meta.error = Some(msg);
                meta.signed_out = signed_out;
            }
            // Keep showing what we last knew.
            let events: Vec<Event> = std::fs::read(&cache)
                .ok()
                .and_then(|b| serde_json::from_slice::<Vec<Event>>(&b).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.end > window_from && e.start < window_to)
                .collect();
            meta.events = events.len();
            save_meta(&d, &job.id, &meta);
            (meta, events)
        }
    }
}

fn fetch_google(keys: &crate::oauth::ClientKeys, refresh: &str, from: &i64, to: &i64, feed_id: &str, now: i64) -> Result<Vec<Event>, (String, bool)> {
    let token = crate::oauth::refresh(&crate::oauth::GOOGLE_CALENDAR, keys, refresh, now).map_err(|e| (e.message, e.revoked))?.access;
    let api = crate::oauth::gcal_api();
    let calendars = pip_core::gcal::parse_calendar_list(&gcal_get(&pip_core::gcal::calendar_list_path(&api), &token)?);
    let (time_min, time_max) = (rfc3339(*from), rfc3339(*to));
    let mut events = Vec::new();
    for cal in &calendars {
        let mut page: Option<String> = None;
        for _ in 0..10 {
            let body = gcal_get(&pip_core::gcal::events_path(&api, &cal.id, &time_min, &time_max, page.as_deref()), &token)?;
            let (mut evs, next) = pip_core::gcal::parse_events(&body, feed_id, &cal.id, cal.default_alarm_min, &chrono::Local);
            events.append(&mut evs);
            match next {
                Some(t) => page = Some(t),
                None => break,
            }
        }
    }
    events.sort_by_key(|e| e.start);
    Ok(events)
}

fn save_meta(d: &Path, id: &str, meta: &FeedMeta) {
    if let Ok(json) = serde_json::to_vec_pretty(meta) {
        write_atomic(&d.join(format!("{id}.json")), &json);
    }
}

/// Removes a Google-connected calendar's cached events.
pub fn forget_google(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(cal_dir(dir).join(format!("{id}.events.json")));
}

/// Sign-in check for Google Calendar: which account is this?
pub fn verify_gcal(access: &str) -> Result<(String, String), pip_core::connectors::SignInProblem> {
    use pip_core::connectors::SignInProblem;
    let url = format!("{}/users/me/calendarList/primary?fields=id", crate::oauth::gcal_api());
    match gcal_get(&url, access) {
        Ok(body) => {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            match v["id"].as_str() {
                Some(id) if !id.is_empty() => Ok((id.to_string(), String::new())),
                _ => Err(SignInProblem::Other("Google Calendar sent an unexpected answer".into())),
            }
        }
        Err((_, true)) => Err(SignInProblem::MissingPermission),
        Err((msg, _)) => Err(SignInProblem::Offline(msg)),
    }
}

/// Starts a sync on a worker thread; the result arrives as `WM_CALSYNC` with a boxed
/// `SyncResult` in lParam.
pub fn start(hwnd: HWND, dir: PathBuf, jobs: Vec<FeedJob>, online: bool, from_day: NaiveDate) {
    let hwnd_bits = hwnd as usize; // HWND is a plain handle; safe to move across threads
    let now = crate::platform::now_unix();
    let spawned = std::thread::Builder::new().name("pip-calsync".into()).stack_size(256 * 1024).spawn(move || {
        let mut result = SyncResult { online, feeds: Vec::new(), events: Vec::new() };
        for job in &jobs {
            let (meta, mut events) = sync_feed(&dir, job, online, from_day, now);
            result.feeds.push((job.id.clone(), meta));
            result.events.append(&mut events);
        }
        result.events.sort_by_key(|e| e.start);
        let ptr = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(hwnd_bits as HWND, WM_CALSYNC, 0, ptr as isize) == 0 {
                drop(Box::from_raw(ptr)); // window gone: don't leak
            }
        }
    });
    if spawned.is_err() {
        // Extremely unlikely, and the caller has already armed the next round, which will retry.
    }
}

/// Takes ownership of a `WM_CALSYNC` payload.
///
/// # Safety
/// `lparam` must come from a `WM_CALSYNC` message posted by [`start`].
pub unsafe fn take_result(lparam: isize) -> Box<SyncResult> {
    Box::from_raw(lparam as *mut SyncResult)
}

fn cached_events(ics_path: &Path, id: &str, from_day: NaiveDate) -> Vec<Event> {
    std::fs::read(ics_path)
        .map(|b| {
            let cal = calendar::parse_ics(&String::from_utf8_lossy(&b));
            calendar::expand_days(&cal, id, from_day - Duration::days(1), WINDOW_DAYS, &chrono::Local)
        })
        .unwrap_or_default()
}
