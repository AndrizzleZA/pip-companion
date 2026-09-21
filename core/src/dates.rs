//! Important dates (birthdays, anniversaries, deadlines) and public holidays.
//!
//! Everything here is computed offline: no network, no calendar account needed.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateKind {
    Birthday,
    Anniversary,
    /// One-off: a specific day in a specific year.
    Deadline,
    /// Yearly, no counting.
    Other,
}

impl DateKind {
    pub fn label(self) -> &'static str {
        match self {
            DateKind::Birthday => "Birthday",
            DateKind::Anniversary => "Anniversary",
            DateKind::Deadline => "Deadline",
            DateKind::Other => "Yearly",
        }
    }
    pub fn yearly(self) -> bool {
        self != DateKind::Deadline
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportantDate {
    pub id: String,
    pub title: String,
    pub kind: DateKind,
    pub month: u32,
    pub day: u32,
    /// Birth/start year (age, "10 years") or, for deadlines, the year it falls in.
    pub year: Option<i32>,
    /// Also remind this many days before (0 = only on the day).
    pub lead_days: u32,
    pub updated_at: i64,
}

impl Default for ImportantDate {
    fn default() -> Self {
        ImportantDate {
            id: String::new(),
            title: String::new(),
            kind: DateKind::Birthday,
            month: 1,
            day: 1,
            year: None,
            lead_days: 0,
            updated_at: 0,
        }
    }
}

/// The date `(month, day)` falls on in `year`; 29 Feb becomes 28 Feb in non-leap years.
pub fn date_in_year(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day).or_else(|| {
        (month == 2 && day == 29).then(|| NaiveDate::from_ymd_opt(year, 2, 28)).flatten()
    })
}

impl ImportantDate {
    /// Next occurrence on or after `from`.
    pub fn next_on_or_after(&self, from: NaiveDate) -> Option<NaiveDate> {
        if !self.kind.yearly() {
            let d = date_in_year(self.year?, self.month, self.day)?;
            return (d >= from).then_some(d);
        }
        let this = date_in_year(from.year(), self.month, self.day)?;
        if this >= from {
            Some(this)
        } else {
            date_in_year(from.year() + 1, self.month, self.day)
        }
    }

    /// "turns 30" / "10 years" for the occurrence on `on`.
    pub fn count_on(&self, on: NaiveDate) -> Option<i32> {
        match self.kind {
            DateKind::Birthday | DateKind::Anniversary => {
                self.year.map(|y| on.year() - y).filter(|n| *n > 0)
            }
            _ => None,
        }
    }

    /// Reminder days for the occurrence on `on`: the lead day (if any) and the day itself.
    pub fn reminder_days(&self, on: NaiveDate) -> Vec<NaiveDate> {
        let mut v = Vec::new();
        if self.lead_days > 0 {
            v.push(on - Duration::days(self.lead_days as i64));
        }
        v.push(on);
        v
    }

    /// One-line description for a given occurrence, e.g. "Mom's birthday (turns 60)".
    pub fn describe(&self, on: NaiveDate) -> String {
        let lower = self.title.to_lowercase();
        // "Mom" reads as "Mom's birthday"; "Mom's birthday" is left alone.
        let name = match self.kind {
            DateKind::Birthday if !lower.contains("birthday") && !lower.contains("bday") => format!("{}'s birthday", self.title),
            DateKind::Anniversary if !lower.contains("anniversary") => format!("{} anniversary", self.title),
            _ => self.title.clone(),
        };
        match (self.kind, self.count_on(on)) {
            (DateKind::Birthday, Some(n)) => format!("{name} (turns {n})"),
            (DateKind::Anniversary, Some(n)) => format!("{name} ({n} year{})", if n == 1 { "" } else { "s" }),
            _ => name,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Public holidays
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HolidayRegion {
    None,
    /// South Africa (Public Holidays Act 36 of 1994).
    Za,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Holiday {
    pub date: NaiveDate,
    pub name: String,
}

/// Western (Gregorian) Easter Sunday — anonymous Gregorian algorithm.
pub fn easter(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid Easter date")
}

/// Public holidays for `year`, including "observed" Mondays. One-off declared days (elections,
/// special occasions) are announced per year and cannot be computed.
pub fn holidays(region: HolidayRegion, year: i32) -> Vec<Holiday> {
    match region {
        HolidayRegion::None => Vec::new(),
        HolidayRegion::Za => za_holidays(year),
    }
}

fn za_holidays(year: i32) -> Vec<Holiday> {
    let d = |m: u32, day: u32| NaiveDate::from_ymd_opt(year, m, day).unwrap();
    let e = easter(year);
    let mut base = vec![
        (d(1, 1), "New Year's Day"),
        (d(3, 21), "Human Rights Day"),
        (e - Duration::days(2), "Good Friday"),
        (e + Duration::days(1), "Family Day"),
        (d(4, 27), "Freedom Day"),
        (d(5, 1), "Workers' Day"),
        (d(6, 16), "Youth Day"),
        (d(8, 9), "National Women's Day"),
        (d(9, 24), "Heritage Day"),
        (d(12, 16), "Day of Reconciliation"),
        (d(12, 25), "Christmas Day"),
        (d(12, 26), "Day of Goodwill"),
    ];
    // A holiday on a Sunday makes the following Monday a public holiday; Saturday holidays are not
    // moved. If that Monday is already a holiday (Christmas on a Sunday → Goodwill Monday), the
    // next free day is observed instead (27 Dec, as in 2011, 2016 and 2022).
    let mut observed = Vec::new();
    for (date, name) in &base {
        if date.weekday() == Weekday::Sun {
            let mut monday = *date + Duration::days(1);
            while base.iter().any(|(d, _)| *d == monday) || observed.iter().any(|(d, _): &(NaiveDate, String)| *d == monday) {
                monday += Duration::days(1);
            }
            observed.push((monday, format!("{name} (observed)")));
        }
    }
    base.sort_by_key(|(d, _)| *d);
    let mut out: Vec<Holiday> = base.into_iter().map(|(date, n)| Holiday { date, name: n.to_string() }).collect();
    out.extend(observed.into_iter().map(|(date, name)| Holiday { date, name }));
    out.sort_by_key(|h| h.date);
    out
}

/// Holidays between two dates (inclusive).
pub fn holidays_between(region: HolidayRegion, from: NaiveDate, to: NaiveDate) -> Vec<Holiday> {
    (from.year()..=to.year())
        .flat_map(|y| holidays(region, y))
        .filter(|h| h.date >= from && h.date <= to)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn easter_known_years() {
        assert_eq!(easter(2024), d(2024, 3, 31));
        assert_eq!(easter(2025), d(2025, 4, 20));
        assert_eq!(easter(2026), d(2026, 4, 5));
        assert_eq!(easter(2027), d(2027, 3, 28));
    }

    #[test]
    fn za_2026_matches_gazette() {
        let h = holidays(HolidayRegion::Za, 2026);
        let find = |n: &str| h.iter().find(|x| x.name == n).map(|x| x.date);
        assert_eq!(find("Good Friday"), Some(d(2026, 4, 3)));
        assert_eq!(find("Family Day"), Some(d(2026, 4, 6)));
        assert_eq!(find("Heritage Day"), Some(d(2026, 9, 24)));
        // Human Rights Day 2026 is a Saturday: not moved.
        assert!(!h.iter().any(|x| x.name.starts_with("Human Rights Day (observed)")));
        // Women's Day 2026 is a Sunday → Monday 10 August observed.
        assert!(h.iter().any(|x| x.date == d(2026, 8, 10) && x.name.contains("observed")));
        assert_eq!(h.len(), 13);
    }

    #[test]
    fn za_sunday_rule() {
        // 2027: Human Rights Day (21 Mar) is a Sunday → Monday 22 Mar observed.
        let h = holidays(HolidayRegion::Za, 2027);
        assert!(h.iter().any(|x| x.date == d(2027, 3, 22) && x.name.contains("observed")));
        // 2022: Christmas on Sunday → Goodwill Monday 26 is already a holiday → Tue 27 observed.
        let h = holidays(HolidayRegion::Za, 2022);
        assert!(h.iter().any(|x| x.date == d(2022, 12, 27)));
    }

    #[test]
    fn birthday_next_and_age() {
        let b = ImportantDate { title: "Mom".into(), kind: DateKind::Birthday, month: 3, day: 10, year: Some(1966), ..Default::default() };
        assert_eq!(b.next_on_or_after(d(2026, 9, 21)), Some(d(2027, 3, 10)));
        assert_eq!(b.next_on_or_after(d(2027, 3, 10)), Some(d(2027, 3, 10)));
        assert_eq!(b.describe(d(2027, 3, 10)), "Mom's birthday (turns 61)");
    }

    #[test]
    fn leap_day_and_deadline() {
        let b = ImportantDate { kind: DateKind::Birthday, month: 2, day: 29, ..Default::default() };
        assert_eq!(b.next_on_or_after(d(2027, 1, 1)), Some(d(2027, 2, 28)));
        assert_eq!(b.next_on_or_after(d(2028, 1, 1)), Some(d(2028, 2, 29)));
        let dl = ImportantDate { kind: DateKind::Deadline, month: 10, day: 31, year: Some(2026), ..Default::default() };
        assert_eq!(dl.next_on_or_after(d(2026, 9, 21)), Some(d(2026, 10, 31)));
        assert_eq!(dl.next_on_or_after(d(2026, 11, 1)), None);
    }
}
