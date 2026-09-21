//! User settings. Everything the user can change lives here and nowhere else.
//!
//! `#[serde(default)]` everywhere means older/newer/hand-edited files always load: unknown fields
//! are ignored and missing ones take defaults, so a settings file can never brick the app.

use crate::calendar::CalendarFeed;
use crate::dates::{HolidayRegion, ImportantDate};
use crate::email::EmailSettings;
use crate::time::{DaySet, TimeOfDay};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub pet: PetSettings,
    pub water: WaterSettings,
    pub quiet: QuietSettings,
    pub notify: NotifySettings,
    pub reminders: Vec<CustomReminder>,
    pub dates: DatesSettings,
    pub email: EmailSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            pet: PetSettings::default(),
            water: WaterSettings::default(),
            quiet: QuietSettings::default(),
            notify: NotifySettings::default(),
            reminders: Vec::new(),
            dates: DatesSettings::default(),
            email: EmailSettings::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PetSize {
    Small,
    Medium,
    Large,
}

impl PetSize {
    pub fn factor(self) -> f32 {
        match self {
            PetSize::Small => 0.8,
            PetSize::Medium => 1.0,
            PetSize::Large => 1.3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Liveliness {
    /// Blinks now and then; no idle antics.
    Calm,
    Normal,
    /// More frequent little idle animations.
    Lively,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Body colour palette. Index into `PetColor::ALL`; names are stable for the file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PetColor {
    Mint,
    Peach,
    Lavender,
    Sky,
    Butter,
    Rose,
    Cloud,
}

impl PetColor {
    pub const ALL: [PetColor; 7] = [
        PetColor::Mint,
        PetColor::Peach,
        PetColor::Lavender,
        PetColor::Sky,
        PetColor::Butter,
        PetColor::Rose,
        PetColor::Cloud,
    ];
    /// Base body colour as 0xRRGGBB.
    pub fn rgb(self) -> u32 {
        match self {
            PetColor::Mint => 0x8FD9BE,
            PetColor::Peach => 0xFFC3A1,
            PetColor::Lavender => 0xC9B8FF,
            PetColor::Sky => 0x9DD2FF,
            PetColor::Butter => 0xFFE08A,
            PetColor::Rose => 0xFFB5C8,
            PetColor::Cloud => 0xE6E9EE,
        }
    }
    /// Deeper companion colour for buttons, toggles and highlights.
    pub fn accent(self) -> u32 {
        match self {
            PetColor::Mint => 0x2FA37F,
            PetColor::Peach => 0xE07A4C,
            PetColor::Lavender => 0x7C63E6,
            PetColor::Sky => 0x3A8FE0,
            PetColor::Butter => 0xC98F00,
            PetColor::Rose => 0xDD4F7A,
            PetColor::Cloud => 0x66758C,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            PetColor::Mint => "Mint",
            PetColor::Peach => "Peach",
            PetColor::Lavender => "Lavender",
            PetColor::Sky => "Sky",
            PetColor::Butter => "Butter",
            PetColor::Rose => "Rose",
            PetColor::Cloud => "Cloud",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PetSettings {
    pub name: String,
    pub color: PetColor,
    pub size: PetSize,
    pub always_on_top: bool,
    /// Small clock pill next to the pet (ticks once a minute).
    pub show_clock: bool,
    pub liveliness: Liveliness,
    pub theme: Theme,
    pub start_with_system: bool,
}

impl Default for PetSettings {
    fn default() -> Self {
        PetSettings {
            name: "Pip".into(),
            color: PetColor::Mint,
            size: PetSize::Medium,
            always_on_top: true,
            show_clock: false,
            liveliness: Liveliness::Normal,
            theme: Theme::System,
            start_with_system: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterSettings {
    pub enabled: bool,
    /// Minutes after the last drink before a gentle nudge.
    pub interval_min: u32,
    /// Active window: no water reminders outside it.
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    pub days: DaySet,
    /// Glasses per day; used for progress, mood and celebrations.
    pub daily_goal: u32,
}

impl Default for WaterSettings {
    fn default() -> Self {
        WaterSettings {
            enabled: true,
            interval_min: 60,
            start: TimeOfDay::hm(8, 0),
            end: TimeOfDay::hm(20, 0),
            days: DaySet::ALL,
            daily_goal: 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietSettings {
    pub enabled: bool,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    /// Reminders marked important may still appear (silently) during quiet hours.
    pub allow_important: bool,
}

impl Default for QuietSettings {
    fn default() -> Self {
        QuietSettings {
            enabled: true,
            start: TimeOfDay::hm(22, 0),
            end: TimeOfDay::hm(7, 0),
            allow_important: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifySettings {
    pub sound: bool,
    pub snooze_min: u32,
    /// Hold reminders while a fullscreen app (game, video, presentation) is in front.
    pub pause_in_fullscreen: bool,
    /// After this long unanswered, the bubble folds away into a small badge on the pet.
    pub bubble_minutes: u32,
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings { sound: true, snooze_min: 10, pause_in_fullscreen: true, bubble_minutes: 3 }
    }
}

/// When calendar events may notify you.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventAlerts {
    /// Only events that are important (own alert, important calendar, or a keyword match).
    Important,
    /// Every timed event.
    All,
    Off,
}

/// Important dates, public holidays and subscribed calendars.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatesSettings {
    /// When date reminders and the morning summary appear.
    pub notify_at: TimeOfDay,
    pub holidays: HolidayRegion,
    /// A quiet "today" card: holidays, all-day events and what's on.
    pub morning_summary: bool,
    pub items: Vec<ImportantDate>,
    pub calendars: Vec<CalendarFeed>,
    pub event_alerts: EventAlerts,
    /// Minutes before a timed event (used when the event has no alert of its own).
    pub event_lead_min: u32,
    /// Words that make an event important (case-insensitive).
    pub keywords: Vec<String>,
}

impl Default for DatesSettings {
    fn default() -> Self {
        DatesSettings {
            notify_at: TimeOfDay::hm(9, 0),
            holidays: HolidayRegion::Za,
            morning_summary: true,
            items: Vec::new(),
            calendars: Vec::new(),
            event_alerts: EventAlerts::Important,
            event_lead_min: 10,
            keywords: ["interview", "doctor", "dentist", "appointment", "flight", "exam", "deadline"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// A user-created reminder at a time of day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomReminder {
    /// Stable id (sync-ready).
    pub id: String,
    pub title: String,
    pub time: TimeOfDay,
    /// Repeat days. Empty = one-off on `date`.
    pub days: DaySet,
    pub date: Option<NaiveDate>,
    pub important: bool,
    pub enabled: bool,
    /// Unix seconds of the last edit — used by sync (last writer wins per record).
    pub updated_at: i64,
}

impl Default for CustomReminder {
    fn default() -> Self {
        CustomReminder {
            id: String::new(),
            title: "Reminder".into(),
            time: TimeOfDay::hm(9, 0),
            days: DaySet::WEEKDAYS,
            date: None,
            important: false,
            enabled: true,
            updated_at: 0,
        }
    }
}

impl Settings {
    /// Clamp values to sane ranges after loading or editing (hand-edited files, future versions).
    pub fn sanitize(&mut self) {
        self.version = SETTINGS_VERSION;
        let name: String = self.pet.name.trim().chars().take(24).collect();
        self.pet.name = if name.is_empty() { "Pip".into() } else { name };
        self.water.interval_min = self.water.interval_min.clamp(1, 24 * 60);
        self.water.daily_goal = self.water.daily_goal.clamp(1, 30);
        self.notify.snooze_min = self.notify.snooze_min.clamp(1, 240);
        self.notify.bubble_minutes = self.notify.bubble_minutes.clamp(1, 120);
        self.reminders.truncate(50);
        self.email.check_minutes = self.email.check_minutes.clamp(1, 60);
        self.email.accounts.truncate(5);
        self.dates.event_lead_min = self.dates.event_lead_min.clamp(0, 240);
        self.dates.items.truncate(200);
        self.dates.calendars.truncate(10);
        for d in &mut self.dates.items {
            d.month = d.month.clamp(1, 12);
            d.day = d.day.clamp(1, 31);
            d.lead_days = d.lead_days.min(60);
            d.title = d.title.chars().take(60).collect();
        }
        for r in &mut self.reminders {
            r.title = r.title.chars().take(60).collect();
            if r.title.trim().is_empty() {
                r.title = "Reminder".into();
            }
        }
    }

    pub fn reminder_mut(&mut self, id: &str) -> Option<&mut CustomReminder> {
        self.reminders.iter_mut().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_and_partial_files_load() {
        let s = Settings::default();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);

        // A partial / older file still loads with defaults for the rest.
        let partial: Settings =
            serde_json::from_str(r#"{"pet":{"name":"Mochi"},"futureField":1}"#).unwrap();
        assert_eq!(partial.pet.name, "Mochi");
        assert_eq!(partial.water.interval_min, 60);
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings::default();
        s.pet.name = "   ".into();
        s.water.interval_min = 0;
        s.sanitize();
        assert_eq!(s.pet.name, "Pip");
        assert_eq!(s.water.interval_min, 1);
    }
}
