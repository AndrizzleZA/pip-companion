//! "What's on" for a day: holidays, important dates and calendar events, plus the rules that
//! decide which of them are allowed to interrupt (important) and which only inform.

use crate::calendar::Event;
use crate::dates::{holidays_between, ImportantDate};
use crate::settings::{EventAlerts, Settings};
use crate::time::{local, to_ts};
use chrono::{Duration, NaiveDate, TimeZone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgendaKind {
    Holiday,
    Date,
    Event,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgendaItem {
    pub kind: AgendaKind,
    /// Start time for timed events; None for all-day things.
    pub time: Option<i64>,
    pub text: String,
    pub important: bool,
}

fn keyword_hit(s: &Settings, title: &str) -> bool {
    let t = title.to_lowercase();
    s.dates.keywords.iter().map(|k| k.trim().to_lowercase()).any(|k| !k.is_empty() && t.contains(&k))
}

/// May this calendar event notify the user (as opposed to only appearing on the agenda)?
pub fn event_important(s: &Settings, e: &Event) -> bool {
    let feed_important = s.dates.calendars.iter().any(|c| c.id == e.feed_id && c.important);
    match s.dates.event_alerts {
        EventAlerts::Off => false,
        EventAlerts::All => !e.all_day || feed_important || keyword_hit(s, &e.title),
        EventAlerts::Important => feed_important || keyword_hit(s, &e.title) || (!e.all_day && e.alarm_min.is_some()),
    }
}

/// When an important event should nudge: its own alert time, or `event_lead_min` before;
/// all-day events at the daily notify time.
pub fn event_alert_at<Tz: TimeZone>(s: &Settings, e: &Event, tz: &Tz) -> i64 {
    if e.all_day {
        return to_ts(tz, s.dates.notify_at.on(local(tz, e.start).date()));
    }
    e.start - e.alarm_min.unwrap_or(s.dates.event_lead_min as i64) * 60
}

/// Important dates whose occurrence is `day`.
pub fn dates_on(s: &Settings, day: NaiveDate) -> Vec<(&ImportantDate, NaiveDate)> {
    s.dates.items.iter().filter_map(|d| d.next_on_or_after(day).filter(|o| *o == day).map(|o| (d, o))).collect()
}

pub fn agenda_for_day<Tz: TimeZone>(s: &Settings, events: &[Event], day: NaiveDate, tz: &Tz) -> Vec<AgendaItem> {
    let mut out = Vec::new();
    for h in holidays_between(s.dates.holidays, day, day) {
        out.push(AgendaItem { kind: AgendaKind::Holiday, time: None, text: format!("{} (public holiday)", h.name), important: false });
    }
    for (d, on) in dates_on(s, day) {
        out.push(AgendaItem { kind: AgendaKind::Date, time: None, text: d.describe(on), important: true });
    }
    let start = to_ts(tz, day.and_hms_opt(0, 0, 0).unwrap());
    let end = to_ts(tz, (day + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap());
    let mut evs: Vec<&Event> = events.iter().filter(|e| e.start < end && e.end.max(e.start + 1) > start).collect();
    evs.sort_by_key(|e| (!e.all_day, e.start));
    for e in evs {
        out.push(AgendaItem {
            kind: AgendaKind::Event,
            time: (!e.all_day).then_some(e.start),
            text: e.title.clone(),
            important: event_important(s, e),
        });
    }
    out
}

/// Lines for the morning summary: today's agenda plus a heads-up for tomorrow's holiday.
/// Empty when there is nothing worth saying.
pub fn brief_lines<Tz: TimeZone>(s: &Settings, events: &[Event], today: NaiveDate, tz: &Tz, fmt_time: impl Fn(i64) -> String) -> Vec<String> {
    let mut lines: Vec<String> = agenda_for_day(s, events, today, tz)
        .into_iter()
        .map(|a| match a.time {
            Some(t) => format!("{}  {}", fmt_time(t), a.text),
            None => a.text,
        })
        .collect();
    if let Some(h) = holidays_between(s.dates.holidays, today + Duration::days(1), today + Duration::days(1)).first() {
        lines.push(format!("Tomorrow: {} (public holiday)", h.name));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::CalendarFeed;
    use crate::dates::{DateKind, ImportantDate};
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(7200).unwrap()
    }
    fn ts(d: u32, h: u32, m: u32) -> i64 {
        to_ts(&tz(), NaiveDate::from_ymd_opt(2026, 9, d).unwrap().and_hms_opt(h, m, 0).unwrap())
    }
    fn ev(title: &str, start: i64, alarm: Option<i64>, feed: &str) -> Event {
        Event { uid: title.into(), title: title.into(), start, end: start + 1800, all_day: false, alarm_min: alarm, feed_id: feed.into() }
    }

    #[test]
    fn importance_rules() {
        let mut s = Settings::default();
        s.dates.calendars.push(CalendarFeed { id: "work".into(), important: true, ..Default::default() });
        assert!(event_important(&s, &ev("Dentist check-up", 0, None, "home"))); // keyword
        assert!(event_important(&s, &ev("Lunch", 0, Some(10), "home"))); // own alert
        assert!(!event_important(&s, &ev("Lunch", 0, None, "home"))); // informational
        assert!(event_important(&s, &ev("1:1", 0, None, "work"))); // important calendar
        s.dates.event_alerts = EventAlerts::Off;
        assert!(!event_important(&s, &ev("Dentist", 0, Some(5), "work")));
    }

    #[test]
    fn agenda_and_brief() {
        let mut s = Settings::default();
        s.dates.items.push(ImportantDate { id: "m".into(), title: "Mom's birthday".into(), kind: DateKind::Birthday, month: 9, day: 23, year: Some(1966), ..Default::default() });
        let events = vec![ev("Stand-up", ts(23, 9, 30), None, "f")];
        let day = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let a = agenda_for_day(&s, &events, day, &tz());
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].text, "Mom's birthday (turns 60)");
        // 23 Sep: tomorrow (24th) is Heritage Day.
        let lines = brief_lines(&s, &events, day, &tz(), |_| "09:30".into());
        assert_eq!(lines.last().unwrap(), "Tomorrow: Heritage Day (public holiday)");
        assert_eq!(event_alert_at(&s, &events[0], &tz()), ts(23, 9, 20));
    }
}
