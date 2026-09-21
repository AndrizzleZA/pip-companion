//! Small calendar/time value types shared by settings and the engine.

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Minutes since local midnight (0..1440). Serialised as "HH:MM" so settings stay readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct TimeOfDay(pub u16);

impl TimeOfDay {
    pub const fn hm(h: u16, m: u16) -> Self {
        TimeOfDay(h * 60 + m)
    }
    pub fn hour(self) -> u16 {
        self.0 / 60
    }
    pub fn minute(self) -> u16 {
        self.0 % 60
    }
    /// Adds `delta` minutes, wrapping around midnight.
    pub fn add_minutes(self, delta: i32) -> Self {
        TimeOfDay((self.0 as i32 + delta).rem_euclid(1440) as u16)
    }
    pub fn to_naive(self) -> NaiveTime {
        NaiveTime::from_hms_opt(self.hour() as u32, self.minute() as u32, 0).unwrap()
    }
    pub fn of(t: NaiveDateTime) -> Self {
        TimeOfDay((t.time().hour_min()) as u16)
    }
    pub fn on(self, date: NaiveDate) -> NaiveDateTime {
        date.and_time(self.to_naive())
    }
}

trait HourMin {
    fn hour_min(&self) -> u32;
}
impl HourMin for NaiveTime {
    fn hour_min(&self) -> u32 {
        use chrono::Timelike;
        self.hour() * 60 + self.minute()
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl From<TimeOfDay> for String {
    fn from(t: TimeOfDay) -> String {
        t.to_string()
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let (h, m) = s.split_once(':').ok_or_else(|| format!("bad time '{s}'"))?;
        let h: u16 = h.trim().parse().map_err(|_| format!("bad hour in '{s}'"))?;
        let m: u16 = m.trim().parse().map_err(|_| format!("bad minute in '{s}'"))?;
        if h > 23 || m > 59 {
            return Err(format!("time out of range '{s}'"));
        }
        Ok(TimeOfDay::hm(h, m))
    }
}

/// Is `t` inside the daily window [start, end)? Windows may wrap past midnight (22:00–07:00).
/// An empty window (start == end) contains nothing.
pub fn in_window(t: TimeOfDay, start: TimeOfDay, end: TimeOfDay) -> bool {
    if start <= end {
        t >= start && t < end
    } else {
        t >= start || t < end
    }
}

/// Set of weekdays as a bitmask, bit 0 = Monday … bit 6 = Sunday.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct DaySet(pub u8);

impl DaySet {
    pub const ALL: DaySet = DaySet(0b111_1111);
    pub const WEEKDAYS: DaySet = DaySet(0b001_1111);
    pub const NONE: DaySet = DaySet(0);

    pub fn contains(self, d: Weekday) -> bool {
        self.0 & (1 << d.num_days_from_monday()) != 0
    }
    pub fn toggle(&mut self, index_from_monday: u32) {
        self.0 ^= 1 << index_from_monday;
    }
    pub fn has_index(self, index_from_monday: u32) -> bool {
        self.0 & (1 << index_from_monday) != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 & 0x7f == 0
    }
    /// Human summary: "Every day", "Weekdays", "Weekends", "Mon, Wed".
    pub fn describe(self) -> String {
        match self.0 & 0x7f {
            0 => "Once".into(),
            0x7f => "Every day".into(),
            0x1f => "Weekdays".into(),
            0x60 => "Weekends".into(),
            m => {
                const N: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                (0..7).filter(|i| m & (1 << i) != 0).map(|i| N[i]).collect::<Vec<_>>().join(", ")
            }
        }
    }
}

/// Local wall-clock time for a Unix timestamp.
pub fn local<Tz: TimeZone>(tz: &Tz, ts: i64) -> NaiveDateTime {
    tz.timestamp_opt(ts, 0).single().map(|d| d.naive_local()).unwrap_or_else(|| {
        chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default().naive_utc()
    })
}

/// Unix timestamp for a local wall-clock time. DST gaps resolve forward, overlaps to the earlier
/// instant, so a reminder is never silently skipped.
pub fn to_ts<Tz: TimeZone>(tz: &Tz, t: NaiveDateTime) -> i64 {
    match tz.from_local_datetime(&t) {
        chrono::LocalResult::Single(d) => d.timestamp(),
        chrono::LocalResult::Ambiguous(a, _) => a.timestamp(),
        chrono::LocalResult::None => {
            // Inside a DST gap: try an hour later.
            let later = t + chrono::Duration::hours(1);
            tz.from_local_datetime(&later).earliest().map(|d| d.timestamp()).unwrap_or(0)
        }
    }
}

pub fn weekday_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

pub fn month_name(m: u32) -> &'static str {
    const M: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ];
    M[(m.clamp(1, 12) - 1) as usize]
}

/// "Monday 21 September"
pub fn long_date(d: NaiveDate) -> String {
    format!("{} {} {}", weekday_name(d.weekday()), d.day(), month_name(d.month()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_wraps_midnight() {
        let (s, e) = (TimeOfDay::hm(22, 0), TimeOfDay::hm(7, 0));
        assert!(in_window(TimeOfDay::hm(23, 30), s, e));
        assert!(in_window(TimeOfDay::hm(6, 59), s, e));
        assert!(!in_window(TimeOfDay::hm(7, 0), s, e));
        assert!(!in_window(TimeOfDay::hm(12, 0), s, e));
        assert!(!in_window(TimeOfDay::hm(12, 0), TimeOfDay::hm(9, 0), TimeOfDay::hm(9, 0)));
    }

    #[test]
    fn time_of_day_roundtrip() {
        let t: TimeOfDay = serde_json::from_str("\"08:05\"").unwrap();
        assert_eq!(t, TimeOfDay::hm(8, 5));
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"08:05\"");
        assert!(serde_json::from_str::<TimeOfDay>("\"25:00\"").is_err());
        assert_eq!(TimeOfDay::hm(23, 45).add_minutes(30), TimeOfDay::hm(0, 15));
    }

    #[test]
    fn dayset_describe() {
        assert_eq!(DaySet::ALL.describe(), "Every day");
        assert_eq!(DaySet::WEEKDAYS.describe(), "Weekdays");
        assert_eq!(DaySet(0b101).describe(), "Mon, Wed");
    }
}
