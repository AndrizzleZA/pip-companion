//! Google Calendar over its own API, for calendars connected with "Continue with Google".
//!
//! Only reading is possible with the permission Pip asks for (`calendar.readonly`). Google expands
//! repeating events for us (`singleEvents=true`), so none of the recurrence machinery in
//! [`crate::calendar`] is needed here — this module just asks for the same two-week window the
//! rest of Pip works with and turns the answer into [`Event`]s.

use crate::calendar::Event;
use crate::time::to_ts;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::Deserialize;

pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
pub const API: &str = "https://www.googleapis.com/calendar/v3";

/// One calendar in the user's list (their own, shared and subscribed ones).
#[derive(Clone, Debug, PartialEq)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    /// Ticked in the user's own Google Calendar — the ones they actually look at.
    pub shown: bool,
    pub primary: bool,
    /// Minutes before the start of the calendar's default popup reminder, if any.
    pub default_alarm_min: Option<i64>,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    items: Vec<ListItem>,
}

#[derive(Deserialize)]
struct ListItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    #[serde(rename = "accessRole")]
    access_role: String,
    #[serde(default)]
    #[serde(rename = "defaultReminders")]
    default_reminders: Vec<Reminder>,
}

#[derive(Deserialize, Clone)]
struct Reminder {
    #[serde(default)]
    method: String,
    #[serde(default)]
    minutes: i64,
}

fn popup_minutes(list: &[Reminder]) -> Option<i64> {
    list.iter().filter(|r| r.method == "popup").map(|r| r.minutes).min()
}

/// The calendars worth syncing: not deleted, still readable, and ticked in Google Calendar.
pub fn parse_calendar_list(json: &str) -> Vec<Calendar> {
    let parsed: ListResponse = serde_json::from_str(json).unwrap_or(ListResponse { items: Vec::new() });
    parsed
        .items
        .into_iter()
        .filter(|c| !c.deleted && !c.id.is_empty() && c.access_role != "none" && (c.selected || c.primary))
        .map(|c| Calendar {
            shown: c.selected || c.primary,
            primary: c.primary,
            default_alarm_min: popup_minutes(&c.default_reminders),
            name: if c.summary.is_empty() { c.id.clone() } else { c.summary },
            id: c.id,
        })
        .collect()
}

#[derive(Deserialize)]
struct EventsResponse {
    #[serde(default)]
    items: Vec<ApiEvent>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct ApiEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    start: Option<When>,
    #[serde(default)]
    end: Option<When>,
    #[serde(default)]
    reminders: Option<Reminders>,
    #[serde(default, rename = "eventType")]
    event_type: String,
    #[serde(default)]
    transparency: String,
}

#[derive(Deserialize)]
struct When {
    #[serde(default)]
    date: Option<String>,
    #[serde(default, rename = "dateTime")]
    date_time: Option<String>,
}

#[derive(Deserialize)]
struct Reminders {
    #[serde(default, rename = "useDefault")]
    use_default: bool,
    #[serde(default)]
    overrides: Vec<Reminder>,
}

fn day_start<Tz: TimeZone>(date: &str, tz: &Tz) -> Option<i64> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(to_ts(tz, NaiveDateTime::new(d, NaiveTime::MIN)))
}

fn moment<Tz: TimeZone>(w: &When, tz: &Tz) -> Option<(i64, bool)> {
    if let Some(dt) = &w.date_time {
        return chrono::DateTime::parse_from_rfc3339(dt).ok().map(|d| (d.timestamp(), false));
    }
    w.date.as_deref().and_then(|d| day_start(d, tz)).map(|t| (t, true))
}

/// Turns one page of `events.list` into occurrences. `default_alarm_min` is the calendar's own
/// default reminder, used for events that just say "remind me as usual".
pub fn parse_events<Tz: TimeZone>(json: &str, feed_id: &str, calendar_id: &str, default_alarm_min: Option<i64>, tz: &Tz) -> (Vec<Event>, Option<String>) {
    let parsed: EventsResponse = match serde_json::from_str(json) {
        Ok(p) => p,
        Err(_) => return (Vec::new(), None),
    };
    let mut out = Vec::new();
    for e in parsed.items {
        // Cancelled occurrences, declined-style placeholders and "working elsewhere"/focus blocks
        // aren't appointments the user wants a nudge about.
        if e.status == "cancelled" || matches!(e.event_type.as_str(), "workingLocation" | "focusTime") {
            continue;
        }
        let (Some(start), Some(end)) = (e.start.as_ref().and_then(|w| moment(w, tz)), e.end.as_ref().and_then(|w| moment(w, tz))) else {
            continue; // an event without times (Google returns these only for odd edge cases)
        };
        let alarm_min = match &e.reminders {
            Some(r) if !r.overrides.is_empty() => popup_minutes(&r.overrides),
            Some(r) if r.use_default => default_alarm_min,
            _ => None,
        };
        out.push(Event {
            uid: format!("{calendar_id}:{}", e.id),
            title: if e.summary.trim().is_empty() { "(no title)".into() } else { e.summary.trim().to_string() },
            start: start.0,
            end: end.0.max(start.0),
            all_day: start.1,
            alarm_min: alarm_min.filter(|_| e.transparency != "transparent"),
            feed_id: feed_id.to_string(),
        });
    }
    (out, parsed.next_page_token)
}

/// `GET /users/me/calendarList` — only the fields Pip uses, to keep the reply small.
pub fn calendar_list_path(api: &str) -> String {
    format!("{api}/users/me/calendarList?minAccessRole=reader&fields=items(id,summary,selected,primary,deleted,accessRole,defaultReminders)&maxResults=250")
}

/// `GET /calendars/{id}/events` for one window, with repeats already expanded by Google.
pub fn events_path(api: &str, calendar_id: &str, time_min: &str, time_max: &str, page: Option<&str>) -> String {
    let mut p = format!(
        "{api}/calendars/{}/events?singleEvents=true&orderBy=startTime&showDeleted=false&maxResults=250&timeMin={}&timeMax={}&fields=items(id,summary,status,start,end,reminders,eventType,transparency),nextPageToken",
        urlencode(calendar_id),
        urlencode(time_min),
        urlencode(time_max)
    );
    if let Some(t) = page {
        p.push_str("&pageToken=");
        p.push_str(&urlencode(t));
    }
    p
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Pip stores a Google-connected calendar as `gcal:<address>` in place of a subscription link.
pub const URL_PREFIX: &str = "gcal:";

pub fn feed_url(address: &str) -> String {
    format!("{URL_PREFIX}{address}")
}

/// The address a `gcal:` feed belongs to.
pub fn feed_address(url: &str) -> Option<&str> {
    url.strip_prefix(URL_PREFIX).map(str::trim).filter(|a| !a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap() // SAST
    }

    #[test]
    fn calendar_list_keeps_the_ones_you_look_at() {
        let json = r#"{"items":[
          {"id":"me@gmail.com","summary":"Andre","primary":true,"selected":true,"accessRole":"owner","defaultReminders":[{"method":"popup","minutes":30},{"method":"email","minutes":60}]},
          {"id":"fam@group.calendar.google.com","summary":"Family","selected":true,"accessRole":"reader","defaultReminders":[]},
          {"id":"hidden@group.calendar.google.com","summary":"Hidden","selected":false,"accessRole":"reader"},
          {"id":"old@group.calendar.google.com","deleted":true,"selected":true,"accessRole":"reader"}
        ]}"#;
        let list = parse_calendar_list(json);
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Andre", "Family"]);
        assert_eq!(list[0].default_alarm_min, Some(30)); // popup wins over email
        assert!(list[0].primary);
        assert_eq!(list[1].default_alarm_min, None);
    }

    #[test]
    fn events_become_occurrences() {
        let json = r#"{"items":[
          {"id":"e1","summary":"Dentist","status":"confirmed","start":{"dateTime":"2026-09-24T09:30:00+02:00"},"end":{"dateTime":"2026-09-24T10:00:00+02:00"},"reminders":{"useDefault":false,"overrides":[{"method":"popup","minutes":15},{"method":"email","minutes":120}]}},
          {"id":"e2","summary":"Leave","status":"confirmed","start":{"date":"2026-09-25"},"end":{"date":"2026-09-26"},"reminders":{"useDefault":true}},
          {"id":"e3","summary":"Gone","status":"cancelled","start":{"dateTime":"2026-09-24T11:00:00+02:00"},"end":{"dateTime":"2026-09-24T12:00:00+02:00"}},
          {"id":"e4","summary":"Desk","status":"confirmed","eventType":"workingLocation","start":{"date":"2026-09-24"},"end":{"date":"2026-09-25"}},
          {"id":"e5","status":"confirmed","start":{"dateTime":"2026-09-26T08:00:00+02:00"},"end":{"dateTime":"2026-09-26T08:30:00+02:00"},"reminders":{"useDefault":true}}
        ],"nextPageToken":"NEXT"}"#;
        let (evs, next) = parse_events(json, "feed1", "me@gmail.com", Some(10), &tz());
        assert_eq!(next.as_deref(), Some("NEXT"));
        assert_eq!(evs.len(), 3);
        assert_eq!((evs[0].title.as_str(), evs[0].alarm_min), ("Dentist", Some(15)));
        assert_eq!(evs[0].uid, "me@gmail.com:e1");
        assert_eq!(evs[0].feed_id, "feed1");
        assert!(!evs[0].all_day);
        assert!(evs[1].all_day && evs[1].alarm_min == Some(10)); // calendar default
        assert_eq!(evs[1].start, to_ts(&tz(), NaiveDate::from_ymd_opt(2026, 9, 25).unwrap().and_hms_opt(0, 0, 0).unwrap()));
        assert_eq!(evs[2].title, "(no title)");
    }

    #[test]
    fn bad_json_is_survivable() {
        assert_eq!(parse_events("not json", "f", "c", None, &tz()).0.len(), 0);
        assert!(parse_calendar_list("{}").is_empty());
    }

    #[test]
    fn paths_and_urls() {
        let p = events_path(API, "fam@group.calendar.google.com", "2026-09-21T00:00:00Z", "2026-10-07T00:00:00Z", Some("tok"));
        assert!(p.contains("singleEvents=true") && p.contains("fam%40group.calendar.google.com") && p.ends_with("&pageToken=tok"));
        assert!(calendar_list_path(API).contains("minAccessRole=reader"));
        assert_eq!(feed_address(&feed_url("me@gmail.com")), Some("me@gmail.com"));
        assert_eq!(feed_address("https://example.com/cal.ics"), None);
    }
}
