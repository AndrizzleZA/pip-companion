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
    /// Newest edit to the settings that sync as one block (water, quiet, alerts, pet name).
    /// Per-item settings (reminders, dates) carry their own timestamps.
    pub prefs_updated_at: i64,
    /// Newest change to what the companion looks like: character, colour, size, voice, liveliness,
    /// theme.
    ///
    /// Its own stamp rather than sharing `prefs_updated_at`, which §48 found the hard way. The
    /// phone keeps prefs and never draws Pip, so it holds default appearance with a recent prefs
    /// timestamp - and sharing the stamp let those defaults beat a real choice made on the PC. A
    /// record that merges on its own timestamp has to keep its own timestamp.
    pub appearance_updated_at: i64,
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
            prefs_updated_at: 0,
            appearance_updated_at: 0,
        }
    }
}

/// Which little creature this is. Only the crown differs — the body, the animations and the
/// expressions are shared, so a character costs a few lines of drawing and nothing at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Character {
    /// The original: a stem with two leaves.
    Sprout,
    Bunny,
    Kit,
    Tuft,
    /// Broad, low and bald: the outline is the whole character.
    Pebble,
    /// A small body under a wide ring of petals.
    Bloom,
    /// The smallest, with antennae.
    Moth,
}

/// A character this build has never heard of reads as Sprout.
///
/// The derived version failed the whole file on an unknown name, and one unparseable field costs
/// every setting: `load` moves the file aside as `.corrupt` and starts from defaults. A companion
/// added by a newer Pip, or a hand-edited name, is not worth someone's reminders and quiet hours.
///
/// What this does cost is the unknown name itself — it becomes "sprout" the next time settings are
/// saved, so going back to the newer build will not restore the choice. Keeping the raw string
/// would avoid that and is the better answer the day more than one version is in the wild.
impl<'de> Deserialize<'de> for Character {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(match name.as_str() {
            "bunny" => Character::Bunny,
            "kit" => Character::Kit,
            "tuft" => Character::Tuft,
            "pebble" => Character::Pebble,
            "bloom" => Character::Bloom,
            "moth" => Character::Moth,
            _ => Character::Sprout,
        })
    }
}

impl Character {
    pub const ALL: [Character; 7] = [
        Character::Sprout,
        Character::Bunny,
        Character::Kit,
        Character::Tuft,
        Character::Pebble,
        Character::Bloom,
        Character::Moth,
    ];

    /// Good days before this one turns up, or 0 for the ones that have always been here.
    ///
    /// A gift, not an unlock. Nothing that was ever free becomes conditional: the four originals
    /// and every colour stay available on the first day and on every day after, because taking
    /// back a companion someone already chose is the one thing §31 was built to prevent. Only
    /// characters that have never existed before can arrive, and nothing anywhere counts down to
    /// them — a companion that says "22 more days" is guilt-tripping, which §7 forbids and §28
    /// refused on purpose.
    pub fn arrives_at(self) -> u32 {
        match self {
            Character::Sprout | Character::Bunny | Character::Kit | Character::Tuft => 0,
            Character::Pebble => 7,
            Character::Bloom => 21,
            Character::Moth => 45,
        }
    }

    pub fn arrived(self, experience: u32) -> bool {
        experience >= self.arrives_at()
    }

    /// The characters to offer at this point. Everything else is simply not drawn, which is both
    /// the kindest option and the cheapest: there is no locked state to design.
    pub fn available(experience: u32) -> impl Iterator<Item = Character> {
        Character::ALL.into_iter().filter(move |c| c.arrived(experience))
    }

    /// Which character to actually draw. A settings file can name one that has not arrived — hand
    /// edited, carried back from a further-along device, or restored from a backup — and the
    /// answer is to show the original rather than to fail or to quietly rewrite their choice.
    /// The choice stays in the file, so it comes back by itself on the day it arrives.
    pub fn shown(self, experience: u32) -> Character {
        if self.arrived(experience) {
            self
        } else {
            Character::Sprout
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Character::Sprout => "Sprout",
            Character::Bunny => "Bunny",
            Character::Kit => "Toadstool",
            Character::Tuft => "Tuft",
            Character::Pebble => "Pebble",
            Character::Bloom => "Bloom",
            Character::Moth => "Moth",
        }
    }
}

/// How your companion sounds. Each is a handful of notes synthesised into memory when first
/// played — no files to ship, lose or load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Voice {
    /// The companion's own voice: each character sounds like itself. The default, and the only
    /// one that changes when you change who your companion is.
    Own,
    /// The original two-note chime.
    Chime,
    Bell,
    Blip,
    Coo,
}

impl Voice {
    pub const ALL: [Voice; 5] = [Voice::Own, Voice::Chime, Voice::Bell, Voice::Blip, Voice::Coo];

    pub fn label(self) -> &'static str {
        match self {
            Voice::Own => "Its own",
            Voice::Chime => "Chime",
            Voice::Bell => "Bell",
            Voice::Blip => "Blip",
            Voice::Coo => "Coo",
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

/// How hard the companion tries to be noticed when something is due.
///
/// One setting rather than the eight it could have been — repeat or not, how often, which
/// animations, whether per-reminder behaviour is on. Those are consequences of this choice, not
/// decisions anybody wants to make one at a time, and a screen of toggles is a control panel
/// where this is a decision.
///
/// The ladder stops. `Insistent` is two further attempts and then quiet, because a companion that
/// keeps going until it is answered is nagging, and §7 and §28 refuse that four times over. None
/// of these ever remark on being ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attention {
    /// The bubble, and nothing else. No animation, no sound, no second try.
    Still,
    /// One small gesture, a sound if sounds are on, and it waits.
    Subtle,
    /// A gesture that suits the reminder, and one more try if it goes unanswered.
    Playful,
    /// The same, bigger, and two further tries. Then it waits like all the others.
    Insistent,
}

impl Attention {
    pub const ALL: [Attention; 4] = [Attention::Still, Attention::Subtle, Attention::Playful, Attention::Insistent];

    pub fn label(self) -> &'static str {
        match self {
            Attention::Still => "Still",
            Attention::Subtle => "Subtle",
            Attention::Playful => "Playful",
            Attention::Insistent => "Insistent",
        }
    }

    /// How many *further* attempts after the first. The ceiling lives here and nowhere else.
    pub fn retries(self) -> u8 {
        match self {
            Attention::Still | Attention::Subtle => 0,
            Attention::Playful => 1,
            Attention::Insistent => 2,
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
    pub character: Character,
    pub voice: Voice,
    pub color: PetColor,
    pub size: PetSize,
    pub always_on_top: bool,
    /// Small clock pill next to the pet (ticks once a minute).
    pub show_clock: bool,
    pub liveliness: Liveliness,
    /// How hard it tries to be noticed when something is due.
    #[serde(default = "default_attention")]
    pub attention: Attention,
    pub theme: Theme,
    pub start_with_system: bool,
}

impl Default for PetSettings {
    fn default() -> Self {
        PetSettings {
            name: "Pip".into(),
            character: Character::Sprout,
            voice: Voice::Own,
            color: PetColor::Mint,
            size: PetSize::Medium,
            always_on_top: true,
            show_clock: false,
            liveliness: Liveliness::Normal,
            attention: default_attention(),
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
    /// Every time of day this goes off. One is the ordinary case; several for the things people
    /// actually repeat — pills morning and night, stretches through the day.
    ///
    /// Defaults to empty rather than to the struct default, so a record written by an older Pip
    /// (which has only `time`) is recognised as needing migration instead of quietly inheriting
    /// 09:00 and firing at the wrong hour.
    #[serde(default)]
    pub times: Vec<TimeOfDay>,
    /// The first of `times`, kept in step so a device still running an older Pip reads something
    /// sensible out of the synced document instead of nothing. Private: `times` is the truth, and
    /// a caller that set this one alone would silently change nothing. Crate-private so only the
    /// engine can touch it; the shells use `times` and the helpers below.
    pub(crate) time: TimeOfDay,
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
            times: vec![TimeOfDay::hm(9, 0)],
            time: TimeOfDay::hm(9, 0),
            days: DaySet::WEEKDAYS,
            date: None,
            important: false,
            enabled: true,
            updated_at: 0,
        }
    }
}

impl CustomReminder {
    /// A new reminder. Going through here (rather than filling the struct in) is what keeps the
    /// list of times and the legacy single time in step.
    pub fn new(id: impl Into<String>, title: impl Into<String>, times: Vec<TimeOfDay>, days: DaySet, now: i64) -> Self {
        let mut r = CustomReminder {
            id: id.into(),
            title: title.into(),
            times,
            days,
            updated_at: now,
            ..Default::default()
        };
        r.tidy_times();
        r
    }

    /// Keeps the list of times in order, without duplicates, and never empty — and keeps the
    /// legacy single `time` pointing at the first one. A reminder written by an older Pip arrives
    /// with only `time`, so that becomes its single time.
    pub fn tidy_times(&mut self) {
        if self.times.is_empty() {
            self.times = vec![self.time];
        }
        self.times.sort_by_key(|t| t.0);
        self.times.dedup_by_key(|t| t.0);
        self.times.truncate(12);
        self.time = self.times[0];
    }

    /// When it first goes off on a day it is due.
    pub fn first_time(&self) -> TimeOfDay {
        self.times.first().copied().unwrap_or(self.time)
    }

    /// Replaces the times, keeping them ordered and unique.
    pub fn set_times(&mut self, times: Vec<TimeOfDay>) {
        self.times = times;
        self.tidy_times();
    }

    /// Adds another time of day, an hour after the last one by default.
    pub fn add_time(&mut self) {
        let next = self.times.last().map(|t| t.add_minutes(60)).unwrap_or(TimeOfDay::hm(9, 0));
        self.times.push(next);
        self.tidy_times();
    }

    /// Removes one time; a reminder always keeps at least one.
    pub fn remove_time(&mut self, at: usize) {
        if self.times.len() > 1 && at < self.times.len() {
            self.times.remove(at);
            self.tidy_times();
        }
    }

    /// Nudges one time by `minutes`, then re-sorts (so times can cross over each other freely).
    pub fn shift_time(&mut self, at: usize, minutes: i32) {
        if let Some(t) = self.times.get_mut(at) {
            *t = t.add_minutes(minutes);
            self.tidy_times();
        }
    }

    /// "08:00" or "08:00, 14:00, 20:00" — what the row says under the title.
    pub fn times_label(&self) -> String {
        self.times.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")
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
            r.tidy_times();
        }
    }

    pub fn reminder_mut(&mut self, id: &str) -> Option<&mut CustomReminder> {
        self.reminders.iter_mut().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nothing_that_was_ever_free_becomes_conditional() {
        // The four originals were available from the first day Pip existed. Gating any of them
        // would take away a companion somebody already chose, which is the one thing growing up
        // was built never to do.
        for who in [Character::Sprout, Character::Bunny, Character::Kit, Character::Tuft] {
            assert_eq!(who.arrives_at(), 0, "{who:?} has always been here");
            assert!(who.arrived(0), "{who:?} must be there on day one");
        }
    }

    #[test]
    fn the_new_ones_arrive_gently_and_in_order() {
        assert_eq!(Character::Pebble.arrives_at(), 7);
        assert_eq!(Character::Bloom.arrives_at(), 21);
        assert_eq!(Character::Moth.arrives_at(), 45);
        assert_eq!(Character::available(0).count(), 4, "day one offers the four originals");
        assert_eq!(Character::available(6).count(), 4);
        assert_eq!(Character::available(7).count(), 5, "and nothing announces itself early");
        assert_eq!(Character::available(20).count(), 5);
        assert_eq!(Character::available(21).count(), 6);
        assert_eq!(Character::available(45).count(), 7);
    }

    #[test]
    fn a_character_that_has_not_arrived_shows_as_sprout_without_losing_the_choice() {
        // A hand-edited file, or one carried back from a device further along.
        assert_eq!(Character::Moth.shown(10), Character::Sprout);
        assert_eq!(Character::Moth.shown(45), Character::Moth, "and it comes back by itself");
        assert_eq!(Character::Tuft.shown(0), Character::Tuft, "the originals are never withheld");
    }

    #[test]
    fn a_character_this_build_has_never_heard_of_reads_as_sprout() {
        // The whole file used to fail on this, and one bad field costs every setting: storage
        // moves it aside as .corrupt and starts from defaults.
        let s: super::Settings = serde_json::from_str(r#"{"pet":{"character":"wyvern","name":"Zed"}}"#)
            .expect("an unknown character must not fail the file");
        assert_eq!(s.pet.character, super::Character::Sprout);
        assert_eq!(s.pet.name, "Zed", "and the rest of the settings survive it");
    }

    #[test]
    fn every_character_still_round_trips() {
        for who in super::Character::ALL {
            let json = serde_json::to_string(&who).unwrap();
            assert_eq!(serde_json::from_str::<super::Character>(&json).unwrap(), who, "{json}");
        }
    }

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

/// Playful is the default because it is the one that makes the companion feel like something
/// rather than a notification with a face, and it is still bounded: one further try, then quiet.
/// Anyone upgrading from a version without the setting gets it, which is a small change in
/// character and the point of having it.
fn default_attention() -> Attention {
    Attention::Playful
}
