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
        // Extremely unlikely; the next scheduled sync will try again.
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
