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
//! user's own devices have: the key is generated once, and reaches each further device through
//! a single Approve tap ([`join`]) — nothing is ever typed.

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
            deleted: state.sync_deleted.clone(),
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
        // Remember the merged tombstones, and forget ones older than three months.
        state.sync_deleted = self.deleted.clone();
        let cutoff = self.updated_at - 90 * 86_400;
        state.sync_deleted.retain(|_, when| *when >= cutoff);
        settings.reminders = self.reminders.clone();
        settings.reminders.sort_by_key(|r| r.first_time());
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
        here.reminders = vec![CustomReminder { id: "r1".into(), title: "Stretch".into(), times: vec![TimeOfDay::hm(9, 0)], updated_at: 10, ..Default::default() }];
        let state = State::default();
        let mine = SyncDoc::from_local(&here, &state, "pc", 20);

        let mut theirs = mine.clone();
        theirs.device = "phone".into();
        theirs.reminders[0].set_times(vec![TimeOfDay::hm(17, 30)]);
        theirs.reminders[0].updated_at = 30;
        theirs.updated_at = 30;

        let merged = mine.merge(&theirs);
        let mut applied = here.clone();
        let mut st = state.clone();
        merged.apply(&mut applied, &mut st);
        assert_eq!(applied.reminders[0].first_time(), TimeOfDay::hm(17, 30));
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
        .map_err(|_| "This sync data belongs to a different sync".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| format!("The sync data couldn't be read ({e})"))
}

/// Adding a device without typing anything.
///
/// The new device makes a key pair and leaves a join request; a device that already has the sync
/// key shows "… wants to sync" and, on approval, seals the key so that only the requester can
/// open it. The cloud only ever carries sealed bytes, and the user only ever taps Approve.
pub mod join {
    use super::Key;
    use blake2::{Blake2b512, Digest};
    use serde::{Deserialize, Serialize};
    use x25519_dalek::{PublicKey, StaticSecret};

    /// What a new device publishes while it waits to be let in.
    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Request {
        /// Random id for this attempt, used in the file name.
        pub id: String,
        /// Shown to the user on the approving device ("Andre's phone wants to sync").
        pub device: String,
        /// The requester's public key, 32 bytes as hex.
        pub public_key: String,
        pub created_at: i64,
    }

    /// A join request is only offered for this long, so an abandoned attempt disappears.
    pub const EXPIRES_SECS: i64 = 15 * 60;

    pub fn is_stale(r: &Request, now: i64) -> bool {
        now - r.created_at > EXPIRES_SECS
    }

    /// The private half stays on the requesting device only.
    pub struct Secret(StaticSecret);

    /// Makes the key pair for one join attempt. The random bytes come from the platform.
    pub fn new_request(random: &[u8; 32], id: String, device: String, now: i64) -> (Secret, Request) {
        let secret = StaticSecret::from(*random);
        let public = PublicKey::from(&secret);
        let req = Request { id, device, public_key: hex(public.as_bytes()), created_at: now };
        (Secret(secret), req)
    }

    fn shared(secret: &StaticSecret, other_public: &[u8; 32]) -> Key {
        let point = secret.diffie_hellman(&PublicKey::from(*other_public));
        let mut h = Blake2b512::new();
        h.update(b"pip-device-join-v1");
        h.update(point.as_bytes());
        let out = h.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&out[..32]);
        key
    }

    /// Approving device: seal the sync key for whoever made this request. The random bytes are an
    /// ephemeral key, so approving twice never repeats itself.
    pub fn grant(req: &Request, sync_key: &Key, random: &[u8; 32], nonce: &[u8; 24]) -> Result<Vec<u8>, String> {
        let their_public = unhex(&req.public_key).ok_or("That join request looks damaged")?;
        let ephemeral = StaticSecret::from(*random);
        let wrap = shared(&ephemeral, &their_public);
        let mut holder = super::SyncDoc::default();
        holder.device = hex(sync_key);
        let mut out = PublicKey::from(&ephemeral).as_bytes().to_vec();
        out.extend_from_slice(&super::seal(&wrap, nonce, &holder)?);
        Ok(out)
    }

    /// Requesting device: open what the approver left, recovering the sync key.
    pub fn accept(secret: &Secret, sealed: &[u8]) -> Result<Key, String> {
        if sealed.len() < 33 {
            return Err("The approval is incomplete".into());
        }
        let mut their_public = [0u8; 32];
        their_public.copy_from_slice(&sealed[..32]);
        let wrap = shared(&secret.0, &their_public);
        let holder = super::open(&wrap, &sealed[32..]).map_err(|_| "That approval was meant for another device".to_string())?;
        unhex(&holder.device).ok_or_else(|| "The shared key is damaged".to_string())
    }

    pub fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    pub fn unhex(s: &str) -> Option<[u8; 32]> {
        let b = s.as_bytes();
        if b.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(std::str::from_utf8(&b[i * 2..i * 2 + 2]).ok()?, 16).ok()?;
        }
        Some(out)
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
    fn a_new_device_is_let_in_with_one_approval() {
        // The phone asks to join.
        let (secret, req) = join::new_request(&[3u8; 32], "abc".into(), "Andre\u{2019}s phone".into(), 1_000);
        assert_eq!(req.device, "Andre\u{2019}s phone");
        assert!(!join::is_stale(&req, 1_100));
        assert!(join::is_stale(&req, 1_000 + join::EXPIRES_SECS + 1), "an abandoned request expires");

        // The PC approves, sealing the real key for that phone only.
        let sync_key: Key = [42u8; 32];
        let sealed = join::grant(&req, &sync_key, &[9u8; 32], &[4u8; 24]).unwrap();
        assert!(!sealed.windows(4).any(|w| w == [42u8; 4]), "the key must not travel in the clear");
        assert_eq!(join::accept(&secret, &sealed).unwrap(), sync_key);
    }

    #[test]
    fn an_approval_only_opens_on_the_device_that_asked() {
        let (_, req) = join::new_request(&[1u8; 32], "a".into(), "phone".into(), 0);
        let (other_secret, _) = join::new_request(&[2u8; 32], "b".into(), "someone else".into(), 0);
        let sealed = join::grant(&req, &[7u8; 32], &[8u8; 32], &[5u8; 24]).unwrap();
        assert!(join::accept(&other_secret, &sealed).is_err());
        assert!(join::accept(&other_secret, &sealed[..10]).is_err());
    }
}
