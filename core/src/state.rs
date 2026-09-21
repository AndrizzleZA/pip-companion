//! Persisted runtime state: what happened, as opposed to what the user configured.

use crate::email::EmailMsg;
use crate::settings::Settings;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
