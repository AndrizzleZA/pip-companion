//! The reminder engine.
//!
//! Stateless functions over (settings, state, now). Shells call:
//!   * [`due_items`]   — what should be presented right now,
//!   * [`delivery`]    — whether policy (DND, quiet hours) lets it through,
//!   * [`next_event`]  — when to wake up next (one timer, no polling).
//!
//! Every computation starts from the wall clock, never from accumulated intervals, so sleep/resume,
//! clock changes and restarts can't make reminders drift or double-fire.

use crate::agenda::{brief_lines, event_alert_at, event_important};
use crate::calendar::Event;
use crate::settings::{CustomReminder, Settings};
use crate::state::{ReminderState, State};
use crate::time::{in_window, local, to_ts, TimeOfDay};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone};

/// Occurrences missed by more than this (e.g. PC was off) are dropped instead of shown late.
pub const MISSED_WINDOW_SECS: i64 = 12 * 3600;
/// Shown as "earlier today" when presented more than this late.
pub const LATE_AFTER_SECS: i64 = 5 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemKey {
    Water,
    Custom(String),
    /// An important date (id).
    Date(String),
    /// A calendar event occurrence (Event::key).
    Event(String),
    /// The morning summary.
    Brief,
    /// Important email(s) waiting — one grouped item.
    Mail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Bubble only; never a sound or a system toast.
    Low,
    Normal,
    /// Important: may break through quiet hours (silently) if the user allows it.
    High,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub key: ItemKey,
    /// The scheduled instant this presentation is for (Unix seconds).
    pub occurrence: i64,
    pub priority: Priority,
    /// True when shown well after its time (e.g. computer was asleep).
    pub late: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Now { sound: bool },
    HoldUntil(i64),
}

// ---------------------------------------------------------------------------------------------
// Quiet hours / DND
// ---------------------------------------------------------------------------------------------

pub fn is_quiet<Tz: TimeZone>(s: &Settings, now: i64, tz: &Tz) -> bool {
    s.quiet.enabled
        && s.quiet.start != s.quiet.end
        && in_window(TimeOfDay::of(local(tz, now)), s.quiet.start, s.quiet.end)
}

/// Next time the local clock reads `t` strictly after `now`.
pub fn next_time_of_day<Tz: TimeZone>(t: TimeOfDay, now: i64, tz: &Tz) -> i64 {
    let l = local(tz, now);
    let mut date = l.date();
    for _ in 0..3 {
        let ts = to_ts(tz, t.on(date));
        if ts > now {
            return ts;
        }
        date = date.succ_opt().unwrap_or(date);
    }
    now + 86_400
}

/// Whether an item of `priority` may be shown now, and with sound.
pub fn delivery<Tz: TimeZone>(
    s: &Settings,
    st: &State,
    priority: Priority,
    now: i64,
    tz: &Tz,
) -> Delivery {
    if let Some(until) = st.dnd_until.filter(|&t| t > now) {
        return Delivery::HoldUntil(until);
    }
    if is_quiet(s, now, tz) {
        if priority == Priority::High && s.quiet.allow_important {
            return Delivery::Now { sound: false };
        }
        return Delivery::HoldUntil(next_time_of_day(s.quiet.end, now, tz));
    }
    Delivery::Now { sound: s.notify.sound && priority >= Priority::Normal }
}

// ---------------------------------------------------------------------------------------------
// Water
// ---------------------------------------------------------------------------------------------

/// When the next water reminder is due (may be in the past = due now). None if nothing is
/// scheduled within the next week (disabled, no days, interval longer than the window...).
pub fn water_next<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> Option<i64> {
    let w = &s.water;
    if !w.enabled || w.days.is_empty() || w.start == w.end {
        return None;
    }
    let interval = w.interval_min as i64 * 60;
    let today = local(tz, now).date();
    // Start a day early so windows that wrap past midnight (e.g. 20:00–02:00) are covered.
    let mut day = today.pred_opt()?;
    for _ in 0..9 {
        if w.days.contains(day.weekday()) {
            let ws = to_ts(tz, w.start.on(day));
            let end_day = if w.end > w.start { day } else { day.succ_opt()? };
            let we = to_ts(tz, w.end.on(end_day));
            if we > now {
                let anchor = st.water.anchor.unwrap_or(ws).max(ws);
                let mut cand = (anchor + interval).max(st.water.snooze_until.unwrap_or(0));
                if is_quiet(s, cand, tz) {
                    cand = next_time_of_day(s.quiet.end, cand, tz);
                }
                if cand < we {
                    return Some(cand.max(ws));
                }
            }
        }
        day = day.succ_opt()?;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Custom reminders
// ---------------------------------------------------------------------------------------------

fn occurs_on(r: &CustomReminder, date: NaiveDate) -> bool {
    if r.days.is_empty() {
        r.date == Some(date)
    } else {
        r.days.contains(date.weekday())
    }
}

/// The times this reminder goes off on a day it is due, earliest first. Older records carry a
/// single `time` and no list, so fall back to that rather than going silent.
fn times_of(r: &CustomReminder) -> Vec<TimeOfDay> {
    if r.times.is_empty() {
        vec![r.first_time()]
    } else {
        let mut t = r.times.clone();
        t.sort_by_key(|x| x.0);
        t
    }
}

/// Latest occurrence at or before `t` (looks back one week).
pub fn occurrence_at_or_before(r: &CustomReminder, t: NaiveDateTime) -> Option<NaiveDateTime> {
    let times = times_of(r);
    let mut date = t.date();
    for _ in 0..8 {
        if occurs_on(r, date) {
            // Latest of the day's times that has already passed.
            if let Some(occ) = times.iter().rev().map(|x| x.on(date)).find(|&occ| occ <= t) {
                return Some(occ);
            }
        }
        date = date.pred_opt()?;
    }
    None
}

/// First occurrence strictly after `t` (looks ahead one week; one-offs use their date).
pub fn occurrence_after(r: &CustomReminder, t: NaiveDateTime) -> Option<NaiveDateTime> {
    let times = times_of(r);
    if r.days.is_empty() {
        let date = r.date?;
        return times.iter().map(|x| x.on(date)).find(|&occ| occ > t);
    }
    let mut date = t.date();
    for _ in 0..8 {
        if occurs_on(r, date) {
            if let Some(occ) = times.iter().map(|x| x.on(date)).find(|&occ| occ > t) {
                return Some(occ);
            }
        }
        date = date.succ_opt()?;
    }
    None
}

fn custom_due<Tz: TimeZone>(
    r: &CustomReminder,
    rs: Option<&ReminderState>,
    now: i64,
    tz: &Tz,
) -> Option<i64> {
    if !r.enabled {
        return None;
    }
    let occ = to_ts(tz, occurrence_at_or_before(r, local(tz, now))?);
    let acked = rs.map_or(0, |x| x.acked);
    let snoozed = rs.and_then(|x| x.snooze_until).is_some_and(|t| t > now);
    (occ > acked && now - occ <= MISSED_WINDOW_SECS && !snoozed).then_some(occ)
}

fn custom_next_future<Tz: TimeZone>(
    r: &CustomReminder,
    rs: Option<&ReminderState>,
    now: i64,
    tz: &Tz,
) -> Option<i64> {
    if !r.enabled {
        return None;
    }
    let snooze = rs.and_then(|x| x.snooze_until).filter(|&t| t > now);
    let next = occurrence_after(r, local(tz, now)).map(|t| to_ts(tz, t));
    match (snooze, next) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// When the next occurrence of reminder `r` is, for display ("next: Tue 09:00").
pub fn reminder_next<Tz: TimeZone>(r: &CustomReminder, st: &State, now: i64, tz: &Tz) -> Option<i64> {
    custom_next_future(r, st.reminders.get(&r.id), now, tz)
}

// ---------------------------------------------------------------------------------------------
// Engine entry points
// ---------------------------------------------------------------------------------------------

/// Everything due at `now` (no calendar events), most important first.
pub fn due_items<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> Vec<Due> {
    due_items_in(s, st, &[], now, tz)
}

/// Everything due at `now`, including calendar `events`. Does not apply delivery policy.
pub fn due_items_in<Tz: TimeZone>(s: &Settings, st: &State, events: &[Event], now: i64, tz: &Tz) -> Vec<Due> {
    let mut out = Vec::new();
    if let Some(t) = water_next(s, st, now, tz).filter(|&t| t <= now) {
        let met_goal = st.water.day == Some(local(tz, now).date())
            && st.water.count >= s.water.daily_goal;
        out.push(Due {
            key: ItemKey::Water,
            occurrence: t,
            priority: if met_goal { Priority::Low } else { Priority::Normal },
            late: now - t > LATE_AFTER_SECS,
        });
    }
    for r in &s.reminders {
        if let Some(occ) = custom_due(r, st.reminders.get(&r.id), now, tz) {
            out.push(Due {
                key: ItemKey::Custom(r.id.clone()),
                occurrence: occ,
                priority: if r.important { Priority::High } else { Priority::Normal },
                late: now - occ > LATE_AFTER_SECS,
            });
        }
    }
    dates_events_due(s, st, events, now, tz, &mut out);
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.occurrence.cmp(&b.occurrence)));
    out
}

/// The next instant (> now) at which anything changes: a reminder becomes due, a snooze or DND
/// ends, quiet hours start/end (the pet naps), or the day rolls over. Shells set exactly one
/// timer for this.
pub fn next_event<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> i64 {
    next_event_in(s, st, &[], now, tz)
}

/// Like [`next_event`], also considering calendar `events`.
pub fn next_event_in<Tz: TimeZone>(s: &Settings, st: &State, events: &[Event], now: i64, tz: &Tz) -> i64 {
    let mut best = next_time_of_day(TimeOfDay(0), now, tz); // midnight rollover
    let mut consider = |t: Option<i64>| {
        if let Some(t) = t.filter(|&t| t > now) {
            best = best.min(t);
        }
    };
    consider(water_next(s, st, now, tz));
    for r in &s.reminders {
        consider(custom_next_future(r, st.reminders.get(&r.id), now, tz));
    }
    consider(st.dnd_until);
    if s.quiet.enabled && s.quiet.start != s.quiet.end {
        consider(Some(next_time_of_day(s.quiet.start, now, tz)));
        consider(Some(next_time_of_day(s.quiet.end, now, tz)));
    }
    for t in dates_events_future(s, st, events, now, tz) {
        consider(Some(t));
    }
    best
}

// ---------------------------------------------------------------------------------------------
// User responses
// ---------------------------------------------------------------------------------------------

/// "Done" on a custom reminder (or dismiss — both mean handled). One-offs switch themselves off.
pub fn ack_custom(s: &mut Settings, st: &mut State, id: &str, occurrence: i64, now: i64) {
    let rs = st.reminders.entry(id.to_string()).or_default();
    rs.acked = rs.acked.max(occurrence);
    rs.snooze_until = None;
    if let Some(r) = s.reminder_mut(id) {
        if r.days.is_empty() {
            r.enabled = false;
            r.updated_at = now;
        }
    }
}

pub fn snooze_custom(st: &mut State, id: &str, until: i64) {
    st.reminders.entry(id.to_string()).or_default().snooze_until = Some(until);
}

/// A reminder was created or edited: occurrences before `now` count as handled, so editing a
/// 09:00 reminder at 10:00 doesn't immediately fire a "missed" notice.
pub fn reminder_edited(st: &mut State, id: &str, now: i64) {
    let rs = st.reminders.entry(id.to_string()).or_default();
    rs.acked = now;
    rs.snooze_until = None;
}

/// Dismissing a water reminder restarts the interval from now without counting a glass.
pub fn dismiss_water(st: &mut State, now: i64) {
    st.water.anchor = Some(now);
    st.water.snooze_until = None;
}

pub fn snooze_water(st: &mut State, until: i64) {
    st.water.snooze_until = Some(until);
}

/// Next local date on which a one-off reminder at `time` would fire (today if still ahead).
pub fn next_date_for<Tz: TimeZone>(time: TimeOfDay, now: i64, tz: &Tz) -> NaiveDate {
    local(tz, next_time_of_day(time, now, tz)).date()
}

pub fn minutes(n: i64) -> i64 {
    Duration::minutes(n).num_seconds()
}

// ---------------------------------------------------------------------------------------------
// Important dates, calendar events, morning summary
// ---------------------------------------------------------------------------------------------

/// State key for snoozing the grouped email item.
pub const MAIL_STATE_KEY: &str = "mail";

/// State key for an important date's reminders.
pub fn date_state_key(id: &str) -> String {
    format!("date:{id}")
}

/// State key for a snoozed calendar event.
pub fn event_state_key(key: &str) -> String {
    format!("ev:{key}")
}

/// Reminder moments for the next occurrence of each important date: (moment, id).
fn date_moments<Tz: TimeZone>(s: &Settings, now: i64, tz: &Tz) -> Vec<(i64, String)> {
    let today = local(tz, now).date();
    let mut out = Vec::new();
    for d in &s.dates.items {
        // An occurrence whose lead day is today may be up to lead_days ahead.
        if let Some(on) = d.next_on_or_after(today) {
            for day in d.reminder_days(on) {
                out.push((to_ts(tz, s.dates.notify_at.on(day)), d.id.clone()));
            }
        }
    }
    out
}

fn snoozed(st: &State, key: &str, now: i64) -> Option<i64> {
    st.reminders.get(key).and_then(|r| r.snooze_until).filter(|&t| t > now)
}

fn dates_events_due<Tz: TimeZone>(s: &Settings, st: &State, events: &[Event], now: i64, tz: &Tz, out: &mut Vec<Due>) {
    let today = local(tz, now).date();
    // Dates: the latest moment today that hasn't been handled (shown all day once due).
    for (m, id) in date_moments(s, now, tz) {
        let key = date_state_key(&id);
        let acked = st.reminders.get(&key).map_or(0, |r| r.acked);
        if m <= now && local(tz, m).date() == today && m > acked && snoozed(st, &key, now).is_none() {
            if !out.iter().any(|d| d.key == ItemKey::Date(id.clone())) {
                out.push(Due { key: ItemKey::Date(id), occurrence: m, priority: Priority::Normal, late: now - m > LATE_AFTER_SECS });
            }
        }
    }
    // Events: from their alert time until they end (at least 15 minutes after start).
    for e in events.iter().filter(|e| event_important(s, e)) {
        let at = event_alert_at(s, e, tz);
        let key = e.key();
        if at <= now
            && now < e.end.max(e.start + 900)
            && !st.events_acked.contains_key(&key)
            && snoozed(st, &event_state_key(&key), now).is_none()
        {
            out.push(Due { key: ItemKey::Event(key), occurrence: at, priority: Priority::Normal, late: now > e.start });
        }
    }
    // Important email: one grouped item while anything is waiting.
    if !st.mail_pending.is_empty() && snoozed(st, MAIL_STATE_KEY, now).is_none() {
        let newest = st.mail_pending.iter().map(|m| m.received).max().unwrap_or(now);
        out.push(Due { key: ItemKey::Mail, occurrence: newest, priority: Priority::Normal, late: false });
    }
    // Morning summary: once a day from notify time, only when there is something to say.
    if s.dates.morning_summary && st.brief_day != Some(today) {
        let m = to_ts(tz, s.dates.notify_at.on(today));
        if m <= now && !brief_lines(s, events, today, tz, |_| String::new()).is_empty() {
            out.push(Due { key: ItemKey::Brief, occurrence: m, priority: Priority::Low, late: false });
        }
    }
}

fn dates_events_future<Tz: TimeZone>(s: &Settings, st: &State, events: &[Event], now: i64, tz: &Tz) -> Vec<i64> {
    let mut v: Vec<i64> = date_moments(s, now, tz).into_iter().map(|(m, _)| m).collect();
    for e in events.iter().filter(|e| event_important(s, e)) {
        v.push(event_alert_at(s, e, tz));
        v.push(e.end.max(e.start + 900)); // stop presenting once it's over
    }
    for r in st.reminders.values() {
        v.extend(r.snooze_until);
    }
    let today = local(tz, now).date();
    if s.dates.morning_summary && st.brief_day != Some(today) {
        v.push(to_ts(tz, s.dates.notify_at.on(today)));
    }
    v
}

pub fn ack_date(st: &mut State, id: &str, occurrence: i64) {
    let rs = st.reminders.entry(date_state_key(id)).or_default();
    rs.acked = rs.acked.max(occurrence);
    rs.snooze_until = None;
}

pub fn ack_event(st: &mut State, key: &str, now: i64) {
    st.events_acked.insert(key.to_string(), now);
    st.reminders.remove(&event_state_key(key));
}

pub fn ack_brief(st: &mut State, today: NaiveDate) {
    st.brief_day = Some(today);
}

/// "Got it" on the email bubble: everything currently waiting is handled.
pub fn ack_mail(st: &mut State) {
    st.mail_pending.clear();
    st.reminders.remove(MAIL_STATE_KEY);
}

/// Snooze for any keyed item that stores its state in `reminders` (dates, events).
pub fn snooze_key(st: &mut State, state_key: String, until: i64) {
    st.reminders.entry(state_key).or_default().snooze_until = Some(until);
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::DaySet;
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap() // SAST
    }
    /// Monday 21 Sep 2026 at hh:mm local.
    fn at(h: u32, m: u32) -> i64 {
        on(21, h, m)
    }
    fn on(day: u32, h: u32, m: u32) -> i64 {
        let t = NaiveDate::from_ymd_opt(2026, 9, day).unwrap().and_hms_opt(h, m, 0).unwrap();
        to_ts(&tz(), t)
    }
    fn setup() -> (Settings, State) {
        let s = Settings::default();
        let st = State::new(0);
        (s, st)
    }

    #[test]
    fn first_water_of_day_is_start_plus_interval() {
        let (s, st) = setup();
        assert_eq!(water_next(&s, &st, at(7, 30), &tz()), Some(at(9, 0)));
        assert!(due_items(&s, &st, at(8, 59), &tz()).is_empty());
        let due = due_items(&s, &st, at(9, 0), &tz());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].key, ItemKey::Water);
        assert!(!due[0].late);
    }

    #[test]
    fn water_interval_counts_from_last_drink() {
        let (s, mut st) = setup();
        st.water.anchor = Some(at(10, 15));
        assert_eq!(water_next(&s, &st, at(10, 20), &tz()), Some(at(11, 15)));
    }

    #[test]
    fn water_stops_at_window_end_and_resumes_tomorrow() {
        let (s, mut st) = setup();
        st.water.anchor = Some(at(19, 30)); // 20:30 would be after the 20:00 end
        assert_eq!(water_next(&s, &st, at(19, 35), &tz()), Some(on(22, 9, 0)));
    }

    #[test]
    fn water_respects_days() {
        let (mut s, st) = setup();
        s.water.days = DaySet(0b100_0000); // Sunday only; the 21st is a Monday
        assert_eq!(water_next(&s, &st, at(7, 0), &tz()), Some(on(27, 9, 0)));
    }

    #[test]
    fn water_snooze_overrides() {
        let (s, mut st) = setup();
        st.water.anchor = Some(at(9, 0));
        snooze_water(&mut st, at(10, 10));
        assert_eq!(water_next(&s, &st, at(10, 1), &tz()), Some(at(10, 10)));
    }

    #[test]
    fn water_pushed_out_of_quiet_hours() {
        let (mut s, mut st) = setup();
        s.water.end = TimeOfDay::hm(23, 0);
        st.water.anchor = Some(at(21, 30)); // 22:30 is quiet → next day
        assert_eq!(water_next(&s, &st, at(21, 31), &tz()), Some(on(22, 9, 0)));
    }

    #[test]
    fn overdue_water_after_wake_is_late() {
        let (s, mut st) = setup();
        st.water.anchor = Some(at(9, 0));
        let due = due_items(&s, &st, at(12, 0), &tz());
        assert_eq!(due[0].occurrence, at(10, 0));
        assert!(due[0].late);
    }

    fn reminder(days: DaySet, h: u16, m: u16) -> CustomReminder {
        CustomReminder {
            id: "r1".into(),
            title: "Stand-up".into(),
            times: vec![TimeOfDay::hm(h, m)],
            days,
            ..Default::default()
        }
    }

    /// One reminder, several times a day — pills morning, noon and night.
    fn reminder_at(days: DaySet, times: &[(u16, u16)]) -> CustomReminder {
        CustomReminder {
            id: "r1".into(),
            title: "Take pills".into(),
            times: times.iter().map(|&(h, m)| TimeOfDay::hm(h, m)).collect(),
            days,
            ..Default::default()
        }
    }

    #[test]
    fn custom_repeating_fires_once_per_occurrence() {
        let (mut s, mut st) = setup();
        s.water.enabled = false;
        s.reminders.push(reminder(DaySet::WEEKDAYS, 9, 30));
        assert!(due_items(&s, &st, at(9, 29), &tz()).is_empty());
        assert_eq!(next_event(&s, &st, at(9, 0), &tz()), at(9, 30));
        let due = due_items(&s, &st, at(9, 30), &tz());
        assert_eq!(due.len(), 1);
        ack_custom(&mut s, &mut st, "r1", due[0].occurrence, at(9, 31));
        assert!(due_items(&s, &st, at(9, 45), &tz()).is_empty());
        // Tomorrow again.
        assert_eq!(due_items(&s, &st, on(22, 9, 30), &tz()).len(), 1);
    }

    #[test]
    fn a_reminder_can_go_off_several_times_a_day() {
        let (mut s, mut st) = setup();
        s.water.enabled = false;
        s.reminders.push(reminder_at(DaySet::ALL, &[(8, 0), (14, 0), (20, 0)]));

        // Each time of day is its own occurrence, and acknowledging one doesn't silence the rest.
        let pills = s.reminders[0].clone();
        let next_of = |st: &State, now: i64| reminder_next(&pills, st, now, &tz());
        assert_eq!(next_of(&st, at(7, 0)), Some(at(8, 0)));
        let morning = due_items(&s, &st, at(8, 0), &tz());
        assert_eq!(morning.len(), 1);
        ack_custom(&mut s, &mut st, "r1", morning[0].occurrence, at(8, 1));
        assert!(due_items(&s, &st, at(8, 30), &tz()).is_empty());

        assert_eq!(next_of(&st, at(8, 30)), Some(at(14, 0)));
        let afternoon = due_items(&s, &st, at(14, 0), &tz());
        assert_eq!(afternoon.len(), 1);
        assert_ne!(afternoon[0].occurrence, morning[0].occurrence);

        // And the last one rolls over to the first time tomorrow.
        ack_custom(&mut s, &mut st, "r1", afternoon[0].occurrence, at(14, 1));
        assert_eq!(next_of(&st, at(14, 2)), Some(at(20, 0)));
        // …and after the last one, the first time tomorrow.
        assert_eq!(next_of(&st, at(20, 1)), Some(on(22, 8, 0)));
    }

    #[test]
    fn times_stay_in_order_and_a_reminder_always_keeps_one() {
        let mut r = reminder_at(DaySet::ALL, &[(20, 0), (8, 0), (8, 0)]);
        r.tidy_times();
        assert_eq!(r.times_label(), "08:00, 20:00", "sorted, and the duplicate dropped");
        assert_eq!(r.first_time(), TimeOfDay::hm(8, 0));

        r.remove_time(0);
        assert_eq!(r.times_label(), "20:00");
        r.remove_time(0);
        assert_eq!(r.times_label(), "20:00", "the last time can't be removed");

        r.add_time();
        assert_eq!(r.times_label(), "20:00, 21:00", "a new time lands an hour later");

        r.shift_time(0, -90);
        assert_eq!(r.times_label(), "18:30, 21:00");
    }

    /// Reminders written by an older Pip carry a single `time` and no list; they must keep working,
    /// on this device and in anything it syncs.
    #[test]
    fn a_reminder_from_an_older_pip_still_goes_off() {
        let old: CustomReminder = serde_json::from_str(
            r#"{"id":"r1","title":"Stand-up","time":"09:30","days":31,"enabled":true,"updated_at":5}"#,
        )
        .unwrap();
        assert_eq!(old.times, Vec::new(), "no list in the old format");
        assert_eq!(old.first_time(), TimeOfDay::hm(9, 30));

        let (mut s, st) = setup();
        s.water.enabled = false;
        s.reminders.push(old);
        assert_eq!(next_event(&s, &st, at(9, 0), &tz()), at(9, 30), "and it still fires");

        // Loading settings migrates it into the list, and keeps writing the old field for devices
        // that haven't been updated yet.
        s.sanitize();
        assert_eq!(s.reminders[0].times, vec![TimeOfDay::hm(9, 30)]);
        let json = serde_json::to_string(&s.reminders[0]).unwrap();
        assert!(json.contains("\"time\":\"09:30\""), "older Pip still reads a single time: {json}");
    }

    #[test]
    fn custom_missed_window_and_edit_protection() {
        let (mut s, mut st) = setup();
        s.water.enabled = false;
        s.reminders.push(reminder(DaySet::ALL, 9, 0));
        // Created at 10:00 → the 09:00 occurrence is not "missed".
        reminder_edited(&mut st, "r1", at(10, 0));
        assert!(due_items(&s, &st, at(10, 0), &tz()).is_empty());
        // Next day, PC off until 22:00 (13h late) → dropped, not shown.
        assert!(due_items(&s, &st, on(22, 22, 0), &tz()).is_empty());
        // ...but 2 h late is shown as late.
        let d = due_items(&s, &st, on(22, 11, 0), &tz());
        assert!(d[0].late);
    }

    #[test]
    fn custom_snooze_and_one_off() {
        let (mut s, mut st) = setup();
        s.water.enabled = false;
        let mut r = reminder(DaySet::NONE, 14, 0);
        r.date = Some(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
        s.reminders.push(r);
        let d = due_items(&s, &st, at(14, 0), &tz());
        assert_eq!(d.len(), 1);
        snooze_custom(&mut st, "r1", at(14, 10));
        assert!(due_items(&s, &st, at(14, 5), &tz()).is_empty());
        assert_eq!(next_event(&s, &st, at(14, 5), &tz()), at(14, 10));
        assert_eq!(due_items(&s, &st, at(14, 10), &tz()).len(), 1);
        ack_custom(&mut s, &mut st, "r1", d[0].occurrence, at(14, 11));
        assert!(!s.reminders[0].enabled, "one-off disables itself");
    }

    #[test]
    fn delivery_policy() {
        let (s, mut st) = setup();
        assert_eq!(delivery(&s, &st, Priority::Normal, at(12, 0), &tz()), Delivery::Now { sound: true });
        assert_eq!(delivery(&s, &st, Priority::Low, at(12, 0), &tz()), Delivery::Now { sound: false });
        // Quiet hours: hold until 07:00, important passes silently.
        assert_eq!(
            delivery(&s, &st, Priority::Normal, at(23, 0), &tz()),
            Delivery::HoldUntil(on(22, 7, 0))
        );
        assert_eq!(delivery(&s, &st, Priority::High, at(23, 0), &tz()), Delivery::Now { sound: false });
        // DND holds everything.
        st.dnd_until = Some(at(13, 0));
        assert_eq!(delivery(&s, &st, Priority::High, at(12, 0), &tz()), Delivery::HoldUntil(at(13, 0)));
    }

    #[test]
    fn next_event_includes_midnight_and_quiet_edges() {
        let (mut s, st) = setup();
        s.water.enabled = false;
        assert_eq!(next_event(&s, &st, at(21, 0), &tz()), at(22, 0)); // quiet starts
        s.quiet.enabled = false;
        assert_eq!(next_event(&s, &st, at(21, 0), &tz()), on(22, 0, 0)); // midnight
    }


    fn quiet_setup() -> (Settings, State) {
        let (mut s, st) = setup();
        s.water.enabled = false;
        s.dates.morning_summary = false;
        (s, st)
    }

    #[test]
    fn birthday_heads_up_and_day_of() {
        use crate::dates::{DateKind, ImportantDate};
        let (mut s, mut st) = quiet_setup();
        s.dates.items.push(ImportantDate { id: "mom".into(), title: "Mom".into(), kind: DateKind::Birthday, month: 9, day: 24, lead_days: 3, ..Default::default() });
        // Lead day is Mon 21st at 09:00.
        assert!(due_items(&s, &st, at(8, 59), &tz()).is_empty());
        assert_eq!(next_event(&s, &st, at(8, 0), &tz()), at(9, 0));
        let d = due_items(&s, &st, at(9, 0), &tz());
        assert_eq!(d[0].key, ItemKey::Date("mom".into()));
        ack_date(&mut st, "mom", d[0].occurrence);
        assert!(due_items(&s, &st, at(15, 0), &tz()).is_empty());
        // Tuesday: nothing. Thursday 24th (the day): due again, and still due late that evening.
        assert!(due_items(&s, &st, on(22, 10, 0), &tz()).is_empty());
        assert_eq!(due_items(&s, &st, on(24, 20, 0), &tz()).len(), 1);
        // A missed day is not carried into the next.
        assert!(due_items(&s, &st, on(25, 9, 30), &tz()).is_empty());
    }

    #[test]
    fn important_event_alerts_until_it_ends() {
        use crate::calendar::Event;
        let (s, mut st) = quiet_setup();
        let dentist = Event { uid: "x".into(), title: "Dentist".into(), start: at(14, 0), end: at(14, 30), all_day: false, alarm_min: None, feed_id: "f".into() };
        let lunch = Event { uid: "y".into(), title: "Lunch".into(), start: at(12, 0), end: at(13, 0), all_day: false, alarm_min: None, feed_id: "f".into() };
        let evs = vec![lunch, dentist.clone()];
        assert!(due_items_in(&s, &st, &evs, at(12, 0), &tz()).is_empty(), "lunch is informational");
        assert_eq!(next_event_in(&s, &st, &evs, at(12, 0), &tz()), at(13, 50));
        let d = due_items_in(&s, &st, &evs, at(13, 50), &tz());
        assert_eq!(d[0].key, ItemKey::Event(dentist.key()));
        assert!(due_items_in(&s, &st, &evs, at(14, 31), &tz()).is_empty(), "over");
        snooze_key(&mut st, event_state_key(&dentist.key()), at(13, 55));
        assert!(due_items_in(&s, &st, &evs, at(13, 52), &tz()).is_empty());
        assert_eq!(due_items_in(&s, &st, &evs, at(13, 55), &tz()).len(), 1);
        ack_event(&mut st, &dentist.key(), at(13, 56));
        assert!(due_items_in(&s, &st, &evs, at(13, 57), &tz()).is_empty());
    }

    #[test]
    fn morning_summary_once_and_only_with_content() {
        let (mut s, mut st) = quiet_setup();
        s.dates.morning_summary = true;
        // Mon 21 Sep 2026: no holiday today, none tomorrow, no events → nothing to say.
        assert!(due_items(&s, &st, at(10, 0), &tz()).is_empty());
        // Wed 23 Sep: tomorrow is Heritage Day → summary due from 09:00, Low priority, once.
        let d = due_items(&s, &st, on(23, 9, 0), &tz());
        assert_eq!((d[0].key.clone(), d[0].priority), (ItemKey::Brief, Priority::Low));
        ack_brief(&mut st, NaiveDate::from_ymd_opt(2026, 9, 23).unwrap());
        assert!(due_items(&s, &st, on(23, 12, 0), &tz()).is_empty());
    }


    #[test]
    fn important_mail_grouped_deduped_snoozed() {
        use crate::email::EmailMsg;
        let (s, mut st) = quiet_setup();
        let m = |id: &str| EmailMsg { id: id.into(), account_id: "a".into(), from_name: "Mom".into(), from_addr: "m@x.com".into(), subject: "Hi".into(), labels: vec![], received: at(10, 0) };
        assert_eq!(st.add_mail(vec![m("1"), m("2")]), 2);
        assert_eq!(st.add_mail(vec![m("2")]), 0, "never announced twice");
        let d = due_items(&s, &st, at(10, 1), &tz());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].key, ItemKey::Mail);
        snooze_key(&mut st, MAIL_STATE_KEY.to_string(), at(10, 30));
        assert!(due_items(&s, &st, at(10, 5), &tz()).is_empty());
        assert_eq!(next_event(&s, &st, at(10, 5), &tz()), at(10, 30));
        ack_mail(&mut st);
        assert!(due_items(&s, &st, at(10, 40), &tz()).is_empty());
        let (title, lines) = crate::messages::mail_card(&[m("3")]);
        assert_eq!((title.as_str(), lines[0].as_str()), ("Email from Mom", "“Hi”"));
    }

    #[test]
    fn goal_met_lowers_priority() {
        let (s, mut st) = setup();
        st.water.day = Some(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
        st.water.count = 8;
        st.water.anchor = Some(at(12, 0));
        let d = due_items(&s, &st, at(13, 0), &tz());
        assert_eq!(d[0].priority, Priority::Low);
    }
}
