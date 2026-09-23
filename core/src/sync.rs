//! Syncing Pip between devices (desktop ↔ phone).
//!
//! The rules are deliberately boring, because sync bugs are miserable:
//!   * every record carries `updated_at`, and the newest edit wins — per record, never per file,
//!     so editing a reminder here and a birthday there never loses one of them;
//!   * deletions are remembered as tombstones, otherwise a stale device would resurrect them;
//!   * progress (water, streak, what you've already acknowledged) merges upward: the highest
//!     count, the longest streak, the union of acknowledgements. Nothing ever nags twice.
//!
//! Everything is encrypted before it leaves the device ([`seal`]), with a key that only the
//! user's own devices have: the key is generated once, and moves to the next device through a
//! short pairing code the user types in ([`pairing`]).

use crate::dates::ImportantDate;
use crate::settings::{CustomReminder, NotifySettings, QuietSettings, Settings, WaterSettings};
use crate::state::{DayCount, State};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FORMAT: u32 = 1;

/// Everything worth carrying between devices. Email and calendar connections are deliberately
/// absent: each device signs in to those itself, so no access token ever travels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncDoc {
    pub format: u32,
    /// Which device wrote this revision, for "last synced from …".
    pub device: String,
    pub updated_at: i64,
    pub prefs: Option<Prefs>,
    pub reminders: Vec<CustomReminder>,
    pub dates: Vec<ImportantDate>,
    /// Records the user deleted: id → when. Keeps them deleted everywhere.
    pub deleted: BTreeMap<String, i64>,
    pub progress: Progress,
}

/// Settings that aren't lists: the whole block is one record with one timestamp.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub updated_at: i64,
    pub pet_name: String,
    pub water: WaterSettings,
    pub quiet: QuietSettings,
    pub notify: NotifySettings,
    pub holidays: crate::dates::HolidayRegion,
    pub morning_summary: bool,
    pub notify_at: crate::time::TimeOfDay,
    pub event_alerts: crate::settings::EventAlerts,
    pub event_lead_min: u32,
    pub date_keywords: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        let d = Settings::default();
        Prefs {
            updated_at: 0,
            pet_name: d.pet.name,
            water: d.water,
            quiet: d.quiet,
            notify: d.notify,
            holidays: d.dates.holidays,
            morning_summary: d.dates.morning_summary,
            notify_at: d.dates.notify_at,
            event_alerts: d.dates.event_alerts,
            event_lead_min: d.dates.event_lead_min,
            date_keywords: d.dates.keywords,
        }
    }
}

/// What you've actually done — merged upward, never downward.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    /// Glasses of water per day (the higher count wins: a glass drunk is a glass drunk).
    pub water: BTreeMap<NaiveDate, u32>,
    pub streak: u32,
    pub best_streak: u32,
    /// Reminder/date/event keys already dealt with → when, so nothing is announced twice.
    pub acked: BTreeMap<String, i64>,
}

fn newest(a: i64, b: i64) -> i64 {
    a.max(b)
}

impl SyncDoc {
    /// Snapshot of this device.
    pub fn from_local(settings: &Settings, state: &State, device: &str, now: i64) -> SyncDoc {
        let mut water: BTreeMap<NaiveDate, u32> = state.water.history.iter().map(|d| (d.date, d.count)).collect();
        if let Some(day) = state.water.day {
            let entry = water.entry(day).or_default();
            *entry = (*entry).max(state.water.count);
        }
        let mut acked: BTreeMap<String, i64> = state.reminders.iter().filter(|(_, r)| r.acked > 0).map(|(k, r)| (k.clone(), r.acked)).collect();
        acked.extend(state.events_acked.iter().map(|(k, v)| (k.clone(), *v)));
        SyncDoc {
            format: FORMAT,
            device: device.to_string(),
            updated_at: now,
            prefs: Some(Prefs {
                updated_at: settings.prefs_updated_at,
                pet_name: settings.pet.name.clone(),
                water: settings.water.clone(),
                quiet: settings.quiet.clone(),
                notify: settings.notify.clone(),
                holidays: settings.dates.holidays,
                morning_summary: settings.dates.morning_summary,
                notify_at: settings.dates.notify_at,
                event_alerts: settings.dates.event_alerts,
                event_lead_min: settings.dates.event_lead_min,
                date_keywords: settings.dates.keywords.clone(),
            }),
            reminders: settings.reminders.clone(),
            dates: settings.dates.items.clone(),
            deleted: BTreeMap::new(),
            progress: Progress { water, streak: state.streak, best_streak: state.best_streak, acked },
        }
    }

    /// Combines this document with one from another device. Order doesn't matter: merging
    /// A into B gives the same result as B into A.
    pub fn merge(&self, other: &SyncDoc) -> SyncDoc {
        let mut out = SyncDoc {
            format: FORMAT,
            device: if other.updated_at > self.updated_at { other.device.clone() } else { self.device.clone() },
            updated_at: newest(self.updated_at, other.updated_at),
            prefs: match (&self.prefs, &other.prefs) {
                (Some(a), Some(b)) => Some(if b.updated_at > a.updated_at { b.clone() } else { a.clone() }),
                (a, b) => a.clone().or_else(|| b.clone()),
            },
            reminders: Vec::new(),
            dates: Vec::new(),
            deleted: self.deleted.clone(),
            progress: Progress::default(),
        };
        for (id, when) in &other.deleted {
            let e = out.deleted.entry(id.clone()).or_insert(*when);
            *e = newest(*e, *when);
        }

        // Lists: newest edit per id, minus anything deleted after that edit.
        let mut reminders: BTreeMap<&str, &CustomReminder> = BTreeMap::new();
        for r in self.reminders.iter().chain(other.reminders.iter()) {
            match reminders.get(r.id.as_str()) {
                Some(cur) if cur.updated_at >= r.updated_at => {}
                _ => {
                    reminders.insert(&r.id, r);
                }
            }
        }
        out.reminders = reminders
            .into_values()
            .filter(|r| out.deleted.get(&r.id).is_none_or(|d| *d < r.updated_at))
            .cloned()
            .collect();

        let mut dates: BTreeMap<&str, &ImportantDate> = BTreeMap::new();
        for d in self.dates.iter().chain(other.dates.iter()) {
            match dates.get(d.id.as_str()) {
                Some(cur) if cur.updated_at >= d.updated_at => {}
                _ => {
                    dates.insert(&d.id, d);
                }
            }
        }
        out.dates = dates
            .into_values()
            .filter(|d| out.deleted.get(&d.id).is_none_or(|t| *t < d.updated_at))
            .cloned()
            .collect();

        // Progress only ever grows.
        out.progress.water = self.progress.water.clone();
        for (day, count) in &other.progress.water {
            let e = out.progress.water.entry(*day).or_default();
            *e = (*e).max(*count);
        }
        out.progress.streak = self.progress.streak.max(other.progress.streak);
        out.progress.best_streak = self.progress.best_streak.max(other.progress.best_streak);
        out.progress.acked = self.progress.acked.clone();
        for (k, when) in &other.progress.acked {
            let e = out.progress.acked.entry(k.clone()).or_insert(*when);
            *e = newest(*e, *when);
        }
        out
    }

    /// Writes the merged result back into this device's settings and state.
    pub fn apply(&self, settings: &mut Settings, state: &mut State) {
        if let Some(p) = &self.prefs {
            if p.updated_at >= settings.prefs_updated_at {
                settings.pet.name = p.pet_name.clone();
                settings.water = p.water.clone();
                settings.quiet = p.quiet.clone();
                settings.notify = p.notify.clone();
                settings.dates.holidays = p.holidays;
                settings.dates.morning_summary = p.morning_summary;
                settings.dates.notify_at = p.notify_at;
                settings.dates.event_alerts = p.event_alerts;
                settings.dates.event_lead_min = p.event_lead_min;
                settings.dates.keywords = p.date_keywords.clone();
            }
        }
        settings.reminders = self.reminders.clone();
        settings.reminders.sort_by_key(|r| r.time);
        settings.dates.items = self.dates.clone();
        state.streak = state.streak.max(self.progress.streak);
        state.best_streak = state.best_streak.max(self.progress.best_streak);
        for (day, count) in &self.progress.water {
            if state.water.day == Some(*day) {
                state.water.count = state.water.count.max(*count);
            } else if let Some(h) = state.water.history.iter_mut().find(|h| h.date == *day) {
                h.count = h.count.max(*count);
            } else {
                state.water.history.push(DayCount { date: *day, count: *count, goal: settings.water.daily_goal });
            }
        }
        state.water.history.sort_by_key(|h| h.date);
        for (key, when) in &self.progress.acked {
            if let Some(r) = state.reminders.get_mut(key) {
                r.acked = r.acked.max(*when);
            } else {
                let e = state.events_acked.entry(key.clone()).or_insert(*when);
                *e = newest(*e, *when);
            }
        }
    }

    /// Marks records as deleted so other devices drop them too.
    pub fn tombstone(&mut self, ids: impl IntoIterator<Item = String>, now: i64) {
        for id in ids {
            self.deleted.insert(id, now);
        }
        let del = self.deleted.clone();
        self.reminders.retain(|r| del.get(&r.id).is_none_or(|d| *d < r.updated_at));
        self.dates.retain(|d| del.get(&d.id).is_none_or(|t| *t < d.updated_at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::TimeOfDay;

    fn reminder(id: &str, title: &str, updated: i64) -> CustomReminder {
        CustomReminder { id: id.into(), title: title.into(), updated_at: updated, ..Default::default() }
    }

    fn doc(device: &str, now: i64) -> SyncDoc {
        SyncDoc { format: FORMAT, device: device.into(), updated_at: now, ..Default::default() }
    }

    #[test]
    fn newest_edit_of_each_record_wins() {
        let mut pc = doc("pc", 100);
        pc.reminders = vec![reminder("a", "Vitamins", 50), reminder("b", "Stand up", 40)];
        let mut phone = doc("phone", 120);
        phone.reminders = vec![reminder("a", "Vitamins at 8", 90), reminder("c", "Call mom", 110)];

        let merged = pc.merge(&phone);
        let titles: Vec<_> = merged.reminders.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, vec!["Vitamins at 8", "Stand up", "Call mom"]);
        // Merging the other way round gives the same thing.
        assert_eq!(merged, phone.merge(&pc));
    }

    #[test]
    fn deletions_stay_deleted_but_a_later_edit_wins() {
        let mut pc = doc("pc", 100);
        pc.reminders = vec![reminder("a", "Vitamins", 50)];
        let mut phone = doc("phone", 120);
        phone.deleted.insert("a".into(), 80);
        assert!(pc.merge(&phone).reminders.is_empty(), "deleted on the phone, so gone here too");

        // But if this device edited it *after* the deletion, the edit wins.
        pc.reminders = vec![reminder("a", "Vitamins", 90)];
        assert_eq!(pc.merge(&phone).reminders.len(), 1);
    }

    #[test]
    fn progress_only_grows() {
        let day = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let mut pc = doc("pc", 100);
        pc.progress.water.insert(day, 5);
        pc.progress.streak = 4;
        pc.progress.acked.insert("date:birthday".into(), 500);
        let mut phone = doc("phone", 90);
        phone.progress.water.insert(day, 3);
        phone.progress.streak = 7;
        phone.progress.acked.insert("ev:standup@123".into(), 600);

        let m = pc.merge(&phone);
        assert_eq!(m.progress.water[&day], 5);
        assert_eq!(m.progress.streak, 7);
        assert_eq!(m.progress.acked.len(), 2);
    }

    #[test]
    fn round_trips_through_local_settings() {
        let mut s = Settings::default();
        s.pet.name = "Pip".into();
        s.reminders = vec![reminder("a", "Vitamins", 50)];
        s.prefs_updated_at = 70;
        let mut st = State::default();
        st.streak = 3;
        let d = SyncDoc::from_local(&s, &st, "pc", 100);

        let mut s2 = Settings::default();
        s2.pet.name = "Bud".into();
        let mut st2 = State::default();
        d.apply(&mut s2, &mut st2);
        assert_eq!(s2.reminders.len(), 1);
        assert_eq!(s2.pet.name, "Pip");
        assert_eq!(st2.streak, 3);
    }

    #[test]
    fn older_prefs_never_overwrite_newer_ones() {
        let mut s = Settings::default();
        s.prefs_updated_at = 900;
        s.water.daily_goal = 10;
        let st = State::default();
        let mut incoming = SyncDoc::from_local(&Settings::default(), &st, "phone", 100);
        incoming.prefs.as_mut().unwrap().updated_at = 100; // older
        incoming.prefs.as_mut().unwrap().water.daily_goal = 2;

        let mut mine = s.clone();
        let mut my_state = st.clone();
        incoming.apply(&mut mine, &mut my_state);
        assert_eq!(mine.water.daily_goal, 10, "the newer local preference survives");
    }

    #[test]
    fn a_reminder_time_edit_on_the_phone_reaches_the_pc() {
        let mut here = Settings::default();
        here.reminders = vec![CustomReminder { id: "r1".into(), title: "Stretch".into(), time: TimeOfDay::hm(9, 0), updated_at: 10, ..Default::default() }];
        let state = State::default();
        let mine = SyncDoc::from_local(&here, &state, "pc", 20);

        let mut theirs = mine.clone();
        theirs.device = "phone".into();
        theirs.reminders[0].time = TimeOfDay::hm(17, 30);
        theirs.reminders[0].updated_at = 30;
        theirs.updated_at = 30;

        let merged = mine.merge(&theirs);
        let mut applied = here.clone();
        let mut st = state.clone();
        merged.apply(&mut applied, &mut st);
        assert_eq!(applied.reminders[0].time, TimeOfDay::hm(17, 30));
    }
}

// ---------------------------------------------------------------------------------------------
// Encryption: nothing leaves a device readable
// ---------------------------------------------------------------------------------------------

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

/// Bytes of the key that only the user's own devices hold.
pub type Key = [u8; 32];

/// Encrypts a document. `nonce` must be 24 fresh random bytes from the platform's generator
/// (the core never invents randomness itself, so each shell uses its own trusted source).
/// The result is `nonce || ciphertext`, which is what gets stored in the cloud.
pub fn seal(key: &Key, nonce: &[u8; 24], doc: &SyncDoc) -> Result<Vec<u8>, String> {
    let plain = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let sealed = cipher
        .encrypt(XNonce::from_slice(nonce), Payload { msg: &plain, aad: b"pip-sync-v1" })
        .map_err(|_| "Couldn't encrypt the sync data".to_string())?;
    let mut out = Vec::with_capacity(24 + sealed.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Decrypts what [`seal`] produced. Any tampering (or a wrong key) is an error, never garbage.
pub fn open(key: &Key, bytes: &[u8]) -> Result<SyncDoc, String> {
    if bytes.len() < 25 {
        return Err("The sync data is incomplete".into());
    }
    let (nonce, body) = bytes.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    let plain = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad: b"pip-sync-v1" })
        .map_err(|_| "This sync data was written with a different pairing code".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| format!("The sync data couldn't be read ({e})"))
}

/// Moving the key to a second device: the PC shows a short code, the phone types it in.
///
/// The code itself is never the key. It unlocks a copy of the real key that the first device
/// leaves in the cloud, so a guessed code alone is useless without access to the account, and a
/// stolen cloud file is useless without the code.
pub mod pairing {
    use super::Key;
    use argon2::{Algorithm, Argon2, Params, Version};

    /// Characters that can't be confused when read aloud or typed (no I, O, 0, 1).
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    /// 16 characters of that alphabet = 80 bits, shown as four groups of four.
    pub const CODE_LEN: usize = 16;

    /// Formats 10 random bytes as the code the user reads off the screen.
    pub fn code_from_bytes(random: &[u8; 10]) -> String {
        let mut bits = 0u32;
        let mut have = 0u32;
        let mut out = String::with_capacity(CODE_LEN + 3);
        for (i, b) in random.iter().enumerate() {
            bits = (bits << 8) | *b as u32;
            have += 8;
            while have >= 5 {
                have -= 5;
                let idx = ((bits >> have) & 31) as usize;
                if out.len() % 5 == 4 {
                    out.push('-');
                }
                out.push(ALPHABET[idx] as char);
            }
            let _ = i;
        }
        out
    }

    /// Accepts the code however the user typed it: spaces, dashes, lower case, O for 0.
    pub fn normalise(typed: &str) -> String {
        typed
            .chars()
            .filter_map(|c| match c.to_ascii_uppercase() {
                'O' => Some('0'),
                'I' | 'L' => Some('1'),
                c if c.is_ascii_alphanumeric() => Some(c),
                _ => None,
            })
            .collect()
    }

    /// Turns the typed code into a key-encryption key. Deliberately slow (Argon2id), so guessing
    /// codes against a stolen file is impractical.
    pub fn key_from_code(code: &str, salt: &[u8]) -> Result<Key, String> {
        // Argon2 wants at least 8 bytes of salt; short app-provided salts are padded with a
        // constant so callers can pass something readable like b"pip-sync".
        let mut salted = salt.to_vec();
        while salted.len() < 16 {
            salted.extend_from_slice(b"pip-sync-padding");
        }
        let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| e.to_string())?;
        let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        a2.hash_password_into(normalise(code).as_bytes(), &salted, &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    fn key() -> Key {
        [7u8; 32]
    }

    #[test]
    fn sealed_data_is_unreadable_and_round_trips() {
        let mut d = SyncDoc { format: FORMAT, device: "pc".into(), updated_at: 42, ..Default::default() };
        d.dates.push(crate::dates::ImportantDate { id: "d1".into(), title: "Mom's birthday".into(), ..Default::default() });
        let bytes = seal(&key(), &[9u8; 24], &d).unwrap();

        assert!(!String::from_utf8_lossy(&bytes).contains("birthday"), "the payload must not be readable");
        assert_eq!(open(&key(), &bytes).unwrap(), d);
    }

    #[test]
    fn the_wrong_key_or_a_tampered_file_is_refused() {
        let d = SyncDoc { format: FORMAT, device: "pc".into(), updated_at: 1, ..Default::default() };
        let bytes = seal(&key(), &[1u8; 24], &d).unwrap();
        assert!(open(&[8u8; 32], &bytes).is_err(), "a different key must not decrypt");

        let mut tampered = bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key(), &tampered).is_err(), "a changed byte must be caught");
        assert!(open(&key(), &bytes[..10]).is_err());
    }

    #[test]
    fn pairing_codes_are_readable_and_forgiving() {
        let code = pairing::code_from_bytes(&[0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE]);
        assert_eq!(code.len(), pairing::CODE_LEN + 3, "four groups of four");
        assert!(code.chars().all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()));

        // Typed back in any reasonable way, it unlocks the same key.
        let salt = b"pip-sync-salt";
        let a = pairing::key_from_code(&code, salt).unwrap();
        let b = pairing::key_from_code(&code.to_lowercase().replace('-', " "), salt).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, pairing::key_from_code("ABCD-EFGH-JKLM-NPQR", salt).unwrap());
    }

    #[test]
    fn a_key_travels_to_the_second_device_through_the_code() {
        // Device 1: a real key, wrapped with the code key and left in the cloud.
        let data_key: Key = [42u8; 32];
        let code = pairing::code_from_bytes(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let code_key = pairing::key_from_code(&code, b"salt").unwrap();
        let mut holder = SyncDoc::default();
        holder.device = "wrapped-key".into();
        holder.deleted.insert(hex(&data_key), 0); // the wrapped payload rides in a document
        let wrapped = seal(&code_key, &[3u8; 24], &holder).unwrap();

        // Device 2: types the code, recovers the same key.
        let recovered_key = pairing::key_from_code(&format!(" {} ", code.to_lowercase()), b"salt").unwrap();
        let out = open(&recovered_key, &wrapped).unwrap();
        assert_eq!(out.deleted.keys().next().unwrap(), &hex(&data_key));
    }

    fn hex(k: &Key) -> String {
        k.iter().map(|b| format!("{b:02x}")).collect()
    }
}
