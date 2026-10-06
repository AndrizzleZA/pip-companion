//! Persisted runtime state: what happened, as opposed to what the user configured.

use crate::email::EmailMsg;
use crate::settings::Settings;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Good days needed to reach the second and third stages of growing up.
pub const GROWING: u32 = 15;
pub const GROWN: u32 = 45;

pub const HISTORY_DAYS: usize = 30;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub version: u32,
    /// When the companion was first started (Unix seconds).
    pub created_at: i64,
    /// Desktop shell: top-left of the creature box in physical pixels.
    pub desktop_pos: Option<[i32; 2]>,
    pub pet_hidden: bool,
    pub water: WaterState,
    pub reminders: BTreeMap<String, ReminderState>,
    /// Do-not-disturb end (Unix seconds).
    pub dnd_until: Option<i64>,
    /// 0..=100; grows with pets and completed reminders, decays slowly.
    pub affection: u32,
    pub last_interaction: i64,
    /// Days that went well, one at most per day. Never goes down, and a missed day is never
    /// mentioned — this is something earned, not a streak to break.
    pub experience: u32,
    /// Set by any interaction and read once when the day turns over. A flag rather than a date
    /// comparison, so the day boundary stays the only place that needs to know about calendars.
    pub touched: bool,
    /// Rotates message variants so the same line doesn't repeat back to back.
    pub msg_seq: u32,
    pub streak: u32,
    pub best_streak: u32,
    /// Calendar event occurrences already handled (key → when), pruned after a few days.
    pub events_acked: BTreeMap<String, i64>,
    /// Day the morning summary was last shown.
    pub brief_day: Option<NaiveDate>,
    /// Per email account: where incremental sync continues from.
    pub mail: BTreeMap<String, MailState>,
    /// Important emails not yet acknowledged (newest last).
    pub mail_pending: Vec<EmailMsg>,
    /// Recently notified message ids, so nothing is announced twice.
    pub mail_seen: Vec<String>,
    /// Reminders/dates deleted here: id → when. Sync carries these so another device
    /// doesn't hand the deleted item back. Pruned after three months.
    pub sync_deleted: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MailState {
    /// Gmail historyId to continue from.
    pub history_id: Option<String>,
    pub last_check: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterState {
    /// Local date that `count` belongs to.
    pub day: Option<NaiveDate>,
    pub count: u32,
    /// Interval counts from here (last drink, or a dismissed reminder).
    pub anchor: Option<i64>,
    pub snooze_until: Option<i64>,
    /// When the companion first saw the user today — progress is only expected from here on,
    /// so installing it (or booting) at noon never makes the pet "thirsty".
    pub seen_since: Option<i64>,
    pub history: Vec<DayCount>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DayCount {
    pub date: NaiveDate,
    pub count: u32,
    pub goal: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReminderState {
    /// Latest occurrence (Unix seconds) the user completed or dismissed.
    pub acked: i64,
    pub snooze_until: Option<i64>,
}

impl State {
    /// How grown the companion is: 0 newly arrived, 1 growing up, 2 fully grown.
    ///
    /// Earned rather than waited for. A companion left alone for a month is exactly where it was.
    pub fn stage(&self) -> u8 {
        match self.experience {
            e if e >= GROWN => 2,
            e if e >= GROWING => 1,
            _ => 0,
        }
    }

    pub fn new(now: i64) -> Self {
        State { version: 1, created_at: now, affection: 50, ..Default::default() }
    }

    /// Moves the water counter onto `today`, archiving the previous day and updating the streak.
    /// Returns true if the day changed.
    pub fn roll_day(&mut self, settings: &Settings, today: NaiveDate) -> bool {
        match self.water.day {
            Some(d) if d == today => false,
            Some(d) => {
                let goal = settings.water.daily_goal;
                self.water.history.push(DayCount { date: d, count: self.water.count, goal });
                if self.water.history.len() > HISTORY_DAYS {
                    let excess = self.water.history.len() - HISTORY_DAYS;
                    self.water.history.drain(..excess);
                }
                // Streaks only count days the user asked to be reminded on; gaps longer than a
                // day (computer off) end the streak gently rather than pretending.
                let tracked = settings.water.days.contains(d.weekday());
                if tracked && settings.water.enabled {
                    if self.water.count >= goal && (today - d).num_days() == 1 {
                        self.streak += 1;
                        self.best_streak = self.best_streak.max(self.streak);
                    } else {
                        self.streak = 0;
                    }
                }
                // A good day: the goal was met, or they spent a moment with it. Either is enough,
                // so someone with water switched off still grows.
                let met_goal = settings.water.enabled && goal > 0 && self.water.count >= goal;
                if met_goal || self.touched {
                    self.experience = self.experience.saturating_add(1);
                }
                self.touched = false;
                self.water.day = Some(today);
                self.water.count = 0;
                self.water.snooze_until = None;
                // Affection drifts back toward neutral overnight.
                self.affection = (self.affection + 50) / 2;
                true
            }
            None => {
                self.water.day = Some(today);
                true
            }
        }
    }

    /// Records a glass of water. Returns the new count for today.
    pub fn log_drink(&mut self, settings: &Settings, now: i64, today: NaiveDate) -> u32 {
        self.roll_day(settings, today);
        self.water.count += 1;
        self.water.anchor = Some(now);
        self.water.snooze_until = None;
        self.bump_affection(4, now);
        self.water.count
    }

    /// Takes back the last glass (misclick).
    pub fn undo_drink(&mut self) {
        self.water.count = self.water.count.saturating_sub(1);
    }

    pub fn bump_affection(&mut self, by: u32, now: i64) {
        self.affection = (self.affection + by).min(100);
        self.last_interaction = now;
        self.touched = true;
    }

    pub fn next_msg(&mut self) -> u32 {
        self.msg_seq = self.msg_seq.wrapping_add(1);
        self.msg_seq
    }

    pub fn dnd_active(&self, now: i64) -> bool {
        self.dnd_until.is_some_and(|t| t > now)
    }

    /// Drops state for reminders that no longer exist.
    pub fn prune(&mut self, settings: &Settings) {
        self.reminders.retain(|id, _| {
            if let Some(d) = id.strip_prefix("date:") {
                settings.dates.items.iter().any(|x| x.id == d)
            } else {
                id.starts_with("ev:") || id == "mail" || settings.reminders.iter().any(|r| &r.id == id)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn streak_counts_consecutive_goal_days() {
        let s = Settings::default();
        let mut st = State::new(0);
        st.roll_day(&s, d(2026, 9, 1));
        st.water.count = 8;
        st.roll_day(&s, d(2026, 9, 2));
        assert_eq!(st.streak, 1);
        st.water.count = 8;
        st.roll_day(&s, d(2026, 9, 3));
        assert_eq!(st.streak, 2);
        st.water.count = 3;
        st.roll_day(&s, d(2026, 9, 4));
        assert_eq!(st.streak, 0);
        assert_eq!(st.best_streak, 2);
        assert_eq!(st.water.history.len(), 3);
        assert_eq!(st.water.count, 0);
    }

    #[test]
    fn gap_day_breaks_streak() {
        let s = Settings::default();
        let mut st = State::new(0);
        st.roll_day(&s, d(2026, 9, 1));
        st.water.count = 9;
        st.roll_day(&s, d(2026, 9, 5));
        assert_eq!(st.streak, 0);
    }
}

impl State {
    /// Forgets acknowledged calendar occurrences that started more than two days ago.
    pub fn prune_events(&mut self, now: i64) {
        self.events_acked.retain(|k, _| {
            k.rsplit_once('@').and_then(|(_, t)| t.parse::<i64>().ok()).is_none_or(|t| t > now - 2 * 86_400)
        });
    }
}

impl State {
    /// Queues newly found important emails (deduplicated). Returns how many were new.
    pub fn add_mail(&mut self, msgs: Vec<EmailMsg>) -> usize {
        let mut added = 0;
        for m in msgs {
            if self.mail_seen.contains(&m.id) {
                continue;
            }
            self.mail_seen.push(m.id.clone());
            self.mail_pending.push(m);
            added += 1;
        }
        let excess = self.mail_seen.len().saturating_sub(500);
        self.mail_seen.drain(..excess);
        let excess = self.mail_pending.len().saturating_sub(20);
        self.mail_pending.drain(..excess);
        added
    }
}

#[cfg(test)]
mod earning_good_days {
    use super::*;
    use crate::settings::Settings;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn setup() -> (State, Settings) {
        let mut s = Settings::default();
        s.water.enabled = true;
        s.water.daily_goal = 4;
        let mut st = State::new(1_800_000_000);
        st.roll_day(&s, d(2026, 9, 1)); // first sight of a day sets the clock, earns nothing
        assert_eq!(st.experience, 0);
        (st, s)
    }

    #[test]
    fn meeting_the_goal_earns_a_day() {
        let (mut st, s) = setup();
        st.water.count = 4;
        st.roll_day(&s, d(2026, 9, 2));
        assert_eq!(st.experience, 1);
    }

    #[test]
    fn so_does_a_moment_together_when_water_is_off() {
        let (mut st, mut s) = setup();
        s.water.enabled = false;
        st.bump_affection(1, 1_800_000_000); // a pat on the head
        st.roll_day(&s, d(2026, 9, 2));
        assert_eq!(st.experience, 1, "someone with water switched off still grows");
    }

    #[test]
    fn a_quiet_day_simply_earns_nothing() {
        let (mut st, s) = setup();
        st.experience = 7;
        st.roll_day(&s, d(2026, 9, 2)); // no water, no interaction
        assert_eq!(st.experience, 7, "it never goes down, and nothing is said");
    }

    #[test]
    fn a_day_counts_once_however_much_happens() {
        let (mut st, s) = setup();
        st.water.count = 99;
        st.bump_affection(1, 1_800_000_000);
        st.bump_affection(1, 1_800_000_000);
        st.roll_day(&s, d(2026, 9, 2));
        assert_eq!(st.experience, 1, "one per calendar day, at most");
    }

    #[test]
    fn yesterdays_attention_doesnt_pay_for_today() {
        let (mut st, s) = setup();
        st.bump_affection(1, 1_800_000_000);
        st.roll_day(&s, d(2026, 9, 2));
        assert_eq!(st.experience, 1);
        st.roll_day(&s, d(2026, 9, 3)); // a day with nothing in it
        assert_eq!(st.experience, 1, "the flag was spent");
    }

    #[test]
    fn a_week_away_costs_nothing_and_earns_the_last_good_day() {
        let (mut st, s) = setup();
        st.bump_affection(1, 1_800_000_000);
        st.roll_day(&s, d(2026, 9, 9)); // computer off for a week
        assert_eq!(st.experience, 1, "the day they were there still counts");
    }

    #[test]
    fn the_stages_land_where_they_should() {
        let mut st = State::new(0);
        for (exp, stage) in [(0, 0), (14, 0), (15, 1), (44, 1), (45, 2), (10_000, 2)] {
            st.experience = exp;
            assert_eq!(st.stage(), stage, "{exp} good days");
        }
    }
}
