//! The reminder engine.
//!
//! Stateless functions over (settings, state, now). Shells call:
//!   * [`due_items`]   — what should be presented right now,
//!   * [`delivery`]    — whether policy (DND, quiet hours) lets it through,
//!   * [`next_event`]  — when to wake up next (one timer, no polling).
//!
//! Every computation starts from the wall clock, never from accumulated intervals, so sleep/resume,
//! clock changes and restarts can't make reminders drift or double-fire.

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

/// Latest occurrence at or before `t` (looks back one week).
pub fn occurrence_at_or_before(r: &CustomReminder, t: NaiveDateTime) -> Option<NaiveDateTime> {
    let mut date = t.date();
    for _ in 0..8 {
        if occurs_on(r, date) {
            let occ = r.time.on(date);
            if occ <= t {
                return Some(occ);
            }
        }
        date = date.pred_opt()?;
    }
    None
}

/// First occurrence strictly after `t` (looks ahead one week; one-offs use their date).
pub fn occurrence_after(r: &CustomReminder, t: NaiveDateTime) -> Option<NaiveDateTime> {
    if r.days.is_empty() {
        let occ = r.time.on(r.date?);
        return (occ > t).then_some(occ);
    }
    let mut date = t.date();
    for _ in 0..8 {
        if occurs_on(r, date) {
            let occ = r.time.on(date);
            if occ > t {
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

/// Everything due at `now`, most important first. Does not apply delivery policy.
pub fn due_items<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> Vec<Due> {
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
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.occurrence.cmp(&b.occurrence)));
    out
}

/// The next instant (> now) at which anything changes: a reminder becomes due, a snooze or DND
/// ends, quiet hours start/end (the pet naps), or the day rolls over. Shells set exactly one
/// timer for this.
pub fn next_event<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> i64 {
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
            time: TimeOfDay::hm(h, m),
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
