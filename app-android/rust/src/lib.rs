//! The bridge between the Android app and Pip's brain.
//!
//! Kotlin owns the phone: screens, sign-in, HTTP, alarms. Rust owns the parts that must behave
//! identically on every device — the merge rules and the encryption — so there is no second
//! implementation to keep in step.
//!
//! There is exactly one JNI entry point. Kotlin calls `Native.call("merge", jsonArgs)` and gets
//! JSON back; failures come back as `{"error": "…"}` rather than as exceptions, so the app can
//! show a sentence instead of a stack trace. Byte strings (sealed documents, keys, nonces) travel
//! as hex, which costs a few kilobytes and saves a dependency on both sides.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;
use pip_core::settings::{Character, CustomReminder, PetColor, Settings};
use pip_draw::creature::{self, Expr, Pose};
use pip_draw::tiny_skia::{Pixmap, Transform};
use pip_core::state::State;
use pip_core::schedule::{self, ItemKey};
use pip_core::sync::{self, join, SyncDoc};
use serde_json::{json, Value};

/// `com.pip.companion.Native.call(name, args)`.
#[no_mangle]
pub extern "system" fn Java_com_pip_companion_Native_call(mut env: JNIEnv, _class: JClass, name: JString, args: JString) -> jstring {
    let name: String = match env.get_string(&name) {
        Ok(s) => s.into(),
        Err(_) => return reply(&mut env, json!({ "error": "bad call" })),
    };
    let args: String = match env.get_string(&args) {
        Ok(s) => s.into(),
        Err(_) => return reply(&mut env, json!({ "error": "bad arguments" })),
    };
    let parsed: Value = serde_json::from_str(&args).unwrap_or(Value::Null);
    let out = match dispatch(&name, &parsed) {
        Ok(v) => v,
        Err(e) => json!({ "error": e }),
    };
    reply(&mut env, out)
}

fn reply(env: &mut JNIEnv, v: Value) -> jstring {
    match env.new_string(v.to_string()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn dispatch(name: &str, a: &Value) -> Result<Value, String> {
    match name {
        // What this phone would publish right now.
        "local_doc" => {
            let settings: Settings = from_field(a, "settings")?;
            let state: State = from_field(a, "state")?;
            let doc = SyncDoc::from_local(&settings, &state, str_field(a, "device")?, int_field(a, "now")?);
            to_value(&doc)
        }
        // The companion itself, drawn into an RGBA square for Compose to show as a Bitmap.
        //
        // The same `pip_draw::creature::draw` the desktop calls, through the same `Pose`: there is
        // no second drawing path and no phone-shaped copy of the creature to keep in step (§50).
        //
        // `experience` rather than a stage, because the phone must respect arrivals exactly as the
        // desktop does: a character that has not arrived yet is shown as Sprout (§43), and the
        // growing-up stage is derived here rather than trusted from the caller.
        "draw" => {
            let who: Character = from_field(a, "character")?;
            let color: PetColor = from_field(a, "color")?;
            let experience = int_field(a, "experience")?.max(0) as u32;
            // Mapped by hand rather than derived: serde has no business in a drawing crate for the
            // sake of six words.
            let expr = match a.get("expr").and_then(Value::as_str).unwrap_or("content") {
                "happy" => Expr::Happy,
                "proud" => Expr::Proud,
                "thirsty" => Expr::Thirsty,
                "asleep" => Expr::Asleep,
                "carried" => Expr::Carried,
                "content" => Expr::Content,
                other => return Err(format!("no such expression: {other}")),
            };
            let size = a.get("size").and_then(Value::as_u64).unwrap_or(220).clamp(32, 1024) as u32;

            let mut pose = Pose::base(expr);
            pose.stage = match experience {
                e if e >= pip_core::state::GROWN => 2,
                e if e >= pip_core::state::GROWING => 1,
                _ => 0,
            };
            let mut pm = Pixmap::new(size, size).ok_or("that size is not a pixmap")?;
            // The creature is authored in a 100-unit square, so one scale puts it anywhere.
            let scale = size as f32 / 100.0;
            creature::draw(&mut pm.as_mut(), &pose, color.rgb(), who.shown(experience), Transform::from_scale(scale, scale));

            Ok(json!({ "w": size, "h": size, "rgba": base64(pm.data()) }))
        }
        // Local and remote, reconciled. Order never matters.
        "merge" => {
            let local: SyncDoc = from_field(a, "local")?;
            let remote: SyncDoc = from_field(a, "remote")?;
            to_value(&local.merge(&remote))
        }
        // The merged document written back into the phone's own settings and state.
        "apply" => {
            let doc: SyncDoc = from_field(a, "doc")?;
            let mut settings: Settings = from_field(a, "settings")?;
            let mut state: State = from_field(a, "state")?;
            doc.apply(&mut settings, &mut state);
            Ok(json!({ "settings": to_value(&settings)?, "state": to_value(&state)? }))
        }
        // Something the user deleted here, so it stays deleted everywhere.
        "tombstone" => {
            let mut doc: SyncDoc = from_field(a, "doc")?;
            let ids: Vec<String> = from_field(a, "ids")?;
            doc.tombstone(ids, int_field(a, "now")?);
            to_value(&doc)
        }
        "seal" => {
            let doc: SyncDoc = from_field(a, "doc")?;
            let bytes = sync::seal(&key_field(a, "key")?, &nonce_field(a, "nonce")?, &doc)?;
            Ok(json!({ "bytes": join::hex(&bytes) }))
        }
        "open" => {
            let doc = sync::open(&key_field(a, "key")?, &bytes_field(a, "bytes")?)?;
            to_value(&doc)
        }
        // Asking to be let in: the seed stays on this phone, the request goes up.
        "join_request" => {
            let (_, req) = join::new_request(&key_field(a, "seed")?, str_field(a, "id")?.to_string(), str_field(a, "device")?.to_string(), int_field(a, "now")?);
            to_value(&req)
        }
        // Opening the approval that came back.
        "join_accept" => {
            let (secret, _) = join::new_request(&key_field(a, "seed")?, String::new(), String::new(), 0);
            let key = join::accept(&secret, &bytes_field(a, "bytes")?)?;
            Ok(json!({ "key": join::hex(&key) }))
        }
        // Letting another device in from the phone, which works exactly as it does on the PC.
        "join_grant" => {
            let req: join::Request = from_field(a, "request")?;
            let sealed = join::grant(&req, &key_field(a, "key")?, &key_field(a, "random")?, &nonce_field(a, "nonce")?)?;
            Ok(json!({ "bytes": join::hex(&sealed) }))
        }
        "join_stale" => {
            let req: join::Request = from_field(a, "request")?;
            Ok(json!({ "stale": join::is_stale(&req, int_field(a, "now")?) }))
        }
        // --- when things go off ---------------------------------------------------------------
        // The phone asks the same engine the PC uses, so an alarm rings at the same minute on both.

        // Add or change one reminder. Goes through the engine so the times end up ordered and
        // unique, the legacy single time stays in step, and the phone can't write something the PC
        // would read differently.
        "save_reminder" => {
            let mut settings: Settings = from_field(a, "settings")?;
            let mut r: CustomReminder = from_field(a, "reminder")?;
            r.updated_at = int_field(a, "now")?;
            r.tidy_times();
            match settings.reminders.iter_mut().find(|x| x.id == r.id) {
                Some(existing) => *existing = r,
                None => settings.reminders.push(r),
            }
            settings.sanitize();
            to_value(&settings)
        }
        // Remove one, and remember that it was removed so a device that has been offline can't
        // bring it back.
        "delete_reminder" => {
            let mut settings: Settings = from_field(a, "settings")?;
            let mut state: State = from_field(a, "state")?;
            let id = str_field(a, "id")?.to_string();
            settings.reminders.retain(|r| r.id != id);
            state.sync_deleted.insert(id, int_field(a, "now")?);
            Ok(json!({ "settings": to_value(&settings)?, "state": to_value(&state)? }))
        }
        // When the next reminder goes off. The phone arms one alarm for this and nothing else:
        // `next_event` also reports quiet-hours boundaries and the day rolling over, and waking a
        // phone — and lighting the status-bar alarm icon — for those would be a lie and a tax on
        // the battery.
        "next_alert" => {
            let settings: Settings = from_field(a, "settings")?;
            let state: State = from_field(a, "state")?;
            let now = int_field(a, "now")?;
            let zone = zone(a)?;
            let dates = schedule::date_moments(&settings, now, &zone)
                .into_iter()
                .filter(|(m, id)| {
                    // Already dealt with, here or on the PC — the ack travels with the sync.
                    let key = schedule::date_state_key(id);
                    *m > now && state.reminders.get(&key).map_or(0, |r| r.acked) < *m
                })
                .map(|(m, _)| m);
            let at = settings
                .reminders
                .iter()
                .filter_map(|r| schedule::reminder_next(r, &state, now, &zone))
                .chain(dates)
                .min();
            Ok(json!({ "at": at }))
        }
        // The next instant anything is due, so the shell can arm one alarm for it.
        "next_event" => {
            let settings: Settings = from_field(a, "settings")?;
            let state: State = from_field(a, "state")?;
            let now = int_field(a, "now")?;
            Ok(json!({ "at": schedule::next_event(&settings, &state, now, &zone(a)?) }))
        }
        // What the water card shows: today's count against the goal, and whether water is on at
        // all. The day is rolled first, so a phone opened after midnight shows 0 and not yesterday.
        "water" => {
            let settings: Settings = from_field(a, "settings")?;
            let mut state: State = from_field(a, "state")?;
            let now = int_field(a, "now")?;
            let today = pip_core::time::local(&zone(a)?, now).date();
            let rolled = state.roll_day(&settings, today);
            Ok(json!({
                "enabled": settings.water.enabled,
                "count": state.water.count,
                "goal": settings.water.daily_goal,
                // Only handed back when the roll changed something, so the phone writes as rarely
                // as the PC does.
                "state": if rolled { to_value(&state)? } else { Value::Null },
            }))
        }
        // "Had a glass". The same core call the PC makes, so a glass counts the same on both.
        "drink" => {
            let settings: Settings = from_field(a, "settings")?;
            let mut state: State = from_field(a, "state")?;
            let now = int_field(a, "now")?;
            let today = pip_core::time::local(&zone(a)?, now).date();
            let count = state.log_drink(&settings, now, today);
            Ok(json!({ "count": count, "goal": settings.water.daily_goal, "state": to_value(&state)? }))
        }
        // Everything due right now, as the phone needs to show it.
        "due" => {
            let settings: Settings = from_field(a, "settings")?;
            let state: State = from_field(a, "state")?;
            let now = int_field(a, "now")?;
            let items: Vec<Value> = schedule::due_items(&settings, &state, now, &zone(a)?)
                .into_iter()
                .filter_map(|d| match &d.key {
                    // Reminders ring; dates are a quiet note. Water is left out — the phone shows
                    // it on a card but never rings for it, because the PC already nudges hourly
                    // and two devices buzzing about the same glass is what Pip exists to avoid.
                    // Calendar events have nowhere on the phone to open.
                    ItemKey::Date(id) => {
                        let item = settings.dates.items.iter().find(|x| &x.id == id)?;
                        let today = pip_core::time::local(&zone(a).ok()?, now).date();
                        let on = item.next_on_or_after(today)?;
                        let weekday = pip_core::time::weekday_name(chrono::Datelike::weekday(&on));
                        let name = settings.pet.name.clone();
                        let (title, body) = pip_core::messages::date_reminder(
                            item.kind,
                            &item.describe(on),
                            (on - today).num_days(),
                            weekday,
                            &name,
                        );
                        Some(json!({
                            "kind": "date",
                            "id": id,
                            "title": title,
                            "body": body,
                            "occurrence": d.occurrence,
                            "late": d.late,
                        }))
                    }
                    ItemKey::Custom(id) => {
                        let r = settings.reminders.iter().find(|r| &r.id == id)?;
                        Some(json!({
                            "kind": "reminder",
                            "id": id,
                            "title": r.title,
                            "occurrence": d.occurrence,
                            "important": r.important,
                            "late": d.late,
                        }))
                    }
                    _ => None,
                })
                .collect();
            Ok(json!({ "items": items }))
        }
        // "Done": this occurrence is answered and won't come back.
        "ack" => {
            let mut settings: Settings = from_field(a, "settings")?;
            let mut state: State = from_field(a, "state")?;
            schedule::ack_custom(&mut settings, &mut state, str_field(a, "id")?, int_field(a, "occurrence")?, int_field(a, "now")?);
            Ok(json!({ "settings": to_value(&settings)?, "state": to_value(&state)? }))
        }
        // "Got it" on a date. Stored the same way the PC stores it, and carried by progress.acked,
        // so dismissing a birthday here doesn't leave it waiting on the PC.
        "ack_date" => {
            let mut state: State = from_field(a, "state")?;
            schedule::ack_date(&mut state, str_field(a, "id")?, int_field(a, "occurrence")?);
            to_value(&state)
        }
        // What's coming up, for the screen. Same wording the PC uses, so the two never disagree
        // about whose birthday it is or how old they are turning.
        "dates" => {
            let settings: Settings = from_field(a, "settings")?;
            let now = int_field(a, "now")?;
            let today = pip_core::time::local(&zone(a)?, now).date();
            let mut out: Vec<(i64, Value)> = settings
                .dates
                .items
                .iter()
                .filter_map(|d| {
                    let on = d.next_on_or_after(today)?;
                    let days = (on - today).num_days();
                    Some((
                        days,
                        json!({
                            "id": d.id,
                            "what": d.describe(on),
                            "days": days,
                            "when": pip_core::time::long_date(on),
                        }),
                    ))
                })
                .collect();
            out.sort_by_key(|(days, _)| *days);
            Ok(json!({ "items": out.into_iter().map(|(_, v)| v).collect::<Vec<_>>() }))
        }
        // "Snooze": ask again at `until`.
        "snooze" => {
            let mut state: State = from_field(a, "state")?;
            schedule::snooze_custom(&mut state, str_field(a, "id")?, int_field(a, "until")?);
            to_value(&state)
        }
        _ => Err(format!("Pip doesn't know how to {name}")),
    }
}

/// The phone's current offset from UTC. Passed in rather than discovered, so the engine behaves
/// exactly as it does on the PC and a test can pin it.
fn zone(a: &Value) -> Result<chrono::FixedOffset, String> {
    let minutes = a.get("tz_offset_min").and_then(Value::as_i64).unwrap_or(0);
    chrono::FixedOffset::east_opt((minutes * 60) as i32).ok_or_else(|| "impossible time zone".to_string())
}

/// RGBA pixels as base64.
///
/// The rest of this bridge sends bytes as hex, which is right for a key or a nonce and wrong for a
/// frame: a 220-pixel square is 193 KB, and hex would make the reply 387 KB where base64 makes it
/// 258 KB. Android decodes it with `android.util.Base64`, so this still costs no dependency on
/// either side - which was the point of the hex convention in the first place.
fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { A[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn from_field<T: serde::de::DeserializeOwned>(a: &Value, field: &str) -> Result<T, String> {
    serde_json::from_value(a.get(field).cloned().unwrap_or(Value::Null)).map_err(|e| format!("{field}: {e}"))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn str_field<'a>(a: &'a Value, field: &str) -> Result<&'a str, String> {
    a.get(field).and_then(Value::as_str).ok_or_else(|| format!("{field} is missing"))
}

fn int_field(a: &Value, field: &str) -> Result<i64, String> {
    a.get(field).and_then(Value::as_i64).ok_or_else(|| format!("{field} is missing"))
}

fn key_field(a: &Value, field: &str) -> Result<[u8; 32], String> {
    join::unhex(str_field(a, field)?).ok_or_else(|| format!("{field} is not a 32-byte value"))
}

fn nonce_field(a: &Value, field: &str) -> Result<[u8; 24], String> {
    let b = bytes_field(a, field)?;
    b.try_into().map_err(|_| format!("{field} is not a 24-byte value"))
}

fn bytes_field(a: &Value, field: &str) -> Result<Vec<u8>, String> {
    let s = str_field(a, field)?.as_bytes();
    if s.len() % 2 != 0 {
        return Err(format!("{field} is damaged"));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in s.chunks(2) {
        let hex = std::str::from_utf8(pair).map_err(|_| format!("{field} is damaged"))?;
        out.push(u8::from_str_radix(hex, 16).map_err(|_| format!("{field} is damaged"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The phone and the PC must agree on every byte, so the bridge is exercised end to end here:
    /// seal on one side, open on the other, with only JSON crossing between.
    #[test]
    fn a_document_survives_the_round_trip_through_json() {
        let doc = dispatch("local_doc", &json!({ "settings": {}, "state": {}, "device": "Andre\u{2019}s phone", "now": 1_000 })).unwrap();
        let key = join::hex(&[7u8; 32]);
        let sealed = dispatch("seal", &json!({ "doc": doc, "key": key, "nonce": join::hex(&[0u8; 32])[..48] })).unwrap();
        let back = dispatch("open", &json!({ "key": key, "bytes": sealed["bytes"] })).unwrap();
        assert_eq!(back["device"], "Andre\u{2019}s phone");
        let merged = dispatch("merge", &json!({ "local": doc, "remote": back })).unwrap();
        assert_eq!(merged["device"], "Andre\u{2019}s phone");
    }

    #[test]
    fn a_phone_can_ask_to_join_and_open_the_answer() {
        let seed = join::hex(&[3u8; 32]);
        let req = dispatch("join_request", &json!({ "seed": seed, "id": "ab12", "device": "phone", "now": 100 })).unwrap();
        let key = join::hex(&[42u8; 32]);
        let sealed = dispatch("join_grant", &json!({ "request": req, "key": key, "random": join::hex(&[9u8; 32]), "nonce": join::hex(&[0u8; 32])[..48] })).unwrap();
        let opened = dispatch("join_accept", &json!({ "seed": seed, "bytes": sealed["bytes"] })).unwrap();
        assert_eq!(opened["key"], key);
    }

    /// The phone arms its alarm from this, so it has to agree with the engine down to the minute.
    #[test]
    fn the_phone_is_told_when_the_next_alert_goes_off() {
        let settings = json!({
            "reminders": [{
                "id": "r1",
                "title": "Take pills",
                "times": ["08:00", "14:00", "20:00"],
                "days": 127,
                "enabled": true,
                "updated_at": 1
            }],
            "water": { "enabled": false },
            "dates": { "morning_summary": false }
        });
        // 2026-09-21, 09:00 on a phone two hours ahead of UTC: of 08:00/14:00/20:00, 14:00 is next.
        let args = json!({ "settings": settings, "state": {}, "now": 1_789_974_000, "tz_offset_min": 120 });
        let at = dispatch("next_alert", &args).unwrap()["at"].as_i64().unwrap();
        assert_eq!(at, 1_789_974_000 + 5 * 3600, "14:00 local");

        // At that moment it is due, and it knows what to say.
        let mut at_due = args.clone();
        at_due["now"] = json!(at);
        let items = dispatch("due", &at_due).unwrap();
        assert_eq!(items["items"][0]["title"], "Take pills");
        assert_eq!(items["items"][0]["occurrence"], at);

        // Done: it stops being due, and the next one is the evening.
        let acked = dispatch(
            "ack",
            &json!({ "settings": settings, "state": {}, "id": "r1", "occurrence": at, "now": at + 30 }),
        )
        .unwrap();
        let mut after = at_due.clone();
        after["state"] = acked["state"].clone();
        after["now"] = json!(at + 60);
        assert_eq!(dispatch("due", &after).unwrap()["items"].as_array().unwrap().len(), 0);
        assert_eq!(dispatch("next_alert", &after).unwrap()["at"], at + 6 * 3600, "20:00 local");

        // Nothing to ring for: the phone arms no alarm at all rather than one for a quiet-hours
        // boundary it can do nothing about.
        let none = json!({ "settings": { "reminders": [] }, "state": {}, "now": 1_789_974_000, "tz_offset_min": 120 });
        assert!(dispatch("next_alert", &none).unwrap()["at"].is_null());
    }

    #[test]
    fn a_birthday_wakes_the_phone_and_says_the_same_thing_the_pc_would() {
        // 2026-05-20 08:00 local (two hours east); Mum's birthday is the 21st, warned a day ahead.
        let now = 1_779_256_800;
        let settings = json!({
            "pet": { "name": "Pip" },
            "dates": {
                "notify_at": "09:00",
                "items": [{ "id": "mum", "title": "Mum", "kind": "birthday", "month": 5, "day": 21, "year": 1960, "lead_days": 1 }],
            },
        });
        let args = json!({ "settings": settings, "state": {}, "now": now, "tz_offset_min": 120 });

        // The screen knows what is coming and how it is worded.
        let list = dispatch("dates", &args).unwrap();
        assert_eq!(list["items"][0]["what"], "Mum's birthday (turns 66)");
        assert_eq!(list["items"][0]["days"], 1, "tomorrow");

        // An alarm is armed for the lead day's 09:00, not only the day itself.
        let at = dispatch("next_alert", &args).unwrap()["at"].as_i64().unwrap();
        assert_eq!(at, now + 3600, "09:00 local today, the lead day");

        // At that moment it is due, with the PC's own wording.
        let mut at_due = args.clone();
        at_due["now"] = json!(at);
        let due = dispatch("due", &at_due).unwrap();
        assert_eq!(due["items"][0]["kind"], "date");
        assert_eq!(due["items"][0]["id"], "mum");
        assert!(
            due["items"][0]["title"].as_str().unwrap().contains("Mum"),
            "got {:?}",
            due["items"][0]["title"]
        );

        // "Got it" settles it here, and nothing is armed for it again.
        let state = dispatch("ack_date", &json!({ "state": {}, "id": "mum", "occurrence": at })).unwrap();
        let after = json!({ "settings": settings, "state": state, "now": at, "tz_offset_min": 120 });
        assert!(dispatch("due", &after).unwrap()["items"].as_array().unwrap().is_empty(), "dealt with");

        // And the PC hears about it, because acks travel with the ordinary sync.
        let doc = dispatch("local_doc", &json!({ "settings": settings, "state": state, "device": "phone", "now": at })).unwrap();
        let acked = doc["progress"]["acked"].as_object().unwrap();
        assert!(acked.values().any(|v| v == at), "the ack is in the document: {acked:?}");
    }

    /// Editing on the phone has to leave the document in the shape the PC expects.
    #[test]
    fn a_reminder_edited_on_the_phone_is_tidied_the_same_way() {
        let saved = dispatch(
            "save_reminder",
            &json!({
                "settings": {},
                "reminder": { "id": "r1", "title": "Take pills", "times": ["20:00", "08:00", "08:00"], "days": 31, "enabled": true },
                "now": 1_000,
            }),
        )
        .unwrap();
        let r = &saved["reminders"][0];
        assert_eq!(r["times"], json!(["08:00", "20:00"]), "sorted, duplicate dropped");
        assert_eq!(r["time"], "08:00", "and the field an older Pip reads");
        assert_eq!(r["updated_at"], 1_000);

        // Changing it replaces it rather than adding a second one.
        let again = dispatch(
            "save_reminder",
            &json!({ "settings": saved, "reminder": { "id": "r1", "title": "Pills", "times": ["09:00"], "days": 31 }, "now": 2_000 }),
        )
        .unwrap();
        assert_eq!(again["reminders"].as_array().unwrap().len(), 1);
        assert_eq!(again["reminders"][0]["title"], "Pills");

        // Deleting leaves a tombstone, so an offline PC can't resurrect it.
        let gone = dispatch("delete_reminder", &json!({ "settings": again, "state": {}, "id": "r1", "now": 3_000 })).unwrap();
        assert_eq!(gone["settings"]["reminders"].as_array().unwrap().len(), 0);
        assert_eq!(gone["state"]["sync_deleted"]["r1"], 3_000);
    }

    #[test]
    fn a_glass_logged_on_the_phone_counts_like_one_logged_on_the_pc() {
        let settings = json!({ "water": { "enabled": true, "daily_goal": 8 } });
        // 2026-10-06 09:00 UTC, two hours east.
        let now = 1_791_270_000;
        let args = json!({ "settings": settings, "state": {}, "now": now, "tz_offset_min": 120 });

        let shown = dispatch("water", &args).unwrap();
        assert_eq!(shown["count"], 0);
        assert_eq!(shown["goal"], 8);
        assert_eq!(shown["enabled"], true);

        let after = dispatch("drink", &args).unwrap();
        assert_eq!(after["count"], 1, "the card counts the glass");
        let state = after["state"].clone();

        // And again, from the state the first one handed back.
        let again =
            dispatch("drink", &json!({ "settings": settings, "state": state, "now": now + 60, "tz_offset_min": 120 })).unwrap();
        assert_eq!(again["count"], 2);

        // The PC drank three; the merge keeps the higher count, so neither device loses a glass.
        let mine = dispatch(
            "local_doc",
            &json!({ "settings": settings, "state": again["state"], "device": "phone", "now": now + 60 }),
        )
        .unwrap();
        let theirs = dispatch(
            "local_doc",
            &json!({
                "settings": settings,
                "state": { "water": { "day": "2026-10-06", "count": 3 } },
                "device": "pc",
                "now": now,
            }),
        )
        .unwrap();
        let merged = dispatch("merge", &json!({ "local": mine, "remote": theirs })).unwrap();
        assert_eq!(merged["progress"]["water"]["2026-10-06"], 3, "a glass drunk is a glass drunk");
    }

    #[test]
    fn the_water_card_starts_the_day_at_zero() {
        let settings = json!({ "water": { "enabled": true, "daily_goal": 8 } });
        let yesterday = json!({ "water": { "day": "2026-10-05", "count": 7 } });
        let shown = dispatch(
            "water",
            &json!({ "settings": settings, "state": yesterday, "now": 1_791_270_000, "tz_offset_min": 120 }),
        )
        .unwrap();
        assert_eq!(shown["count"], 0, "yesterday's glasses aren't today's");
        assert!(!shown["state"].is_null(), "and the rolled day is handed back to be saved");
    }

    #[test]
    fn a_bad_call_explains_itself_instead_of_panicking() {
        assert!(dispatch("nonsense", &json!({})).unwrap_err().contains("nonsense"));
        assert!(dispatch("open", &json!({ "key": "short", "bytes": "00" })).is_err());
        assert!(dispatch("local_doc", &json!({})).is_err());
    }
}

#[cfg(test)]
mod real_data {
    use super::*;

    /// The date actually on Andre's two devices: 22 September, saved without a name, warned a day
    /// ahead. Checks the phone shows something sensible rather than "'s birthday".
    #[test]
    fn the_untitled_birthday_on_the_real_devices_reads_properly() {
        let settings = json!({
            "pet": { "name": "Pip" },
            "dates": {
                "notify_at": "09:00",
                "items": [{
                    "id": "d6ab2b53da05a", "title": "", "kind": "birthday",
                    "month": 9, "day": 22, "year": null, "lead_days": 1, "updated_at": 1_790_096_701,
                }],
            },
        });
        // 2026-10-06, the day this was written: the 22nd has passed, so the next one is 2027's.
        let now = 1_791_273_943;
        let list = dispatch("dates", &json!({ "settings": settings, "now": now, "tz_offset_min": 120 })).unwrap();
        assert_eq!(list["items"][0]["what"], "Birthday");
        assert_eq!(list["items"][0]["days"], 351);
        assert_eq!(list["items"][0]["when"], "Wednesday 22 September", "the PC's own wording");
    }
}

#[cfg(test)]
mod the_claim_this_module_makes {
    /// §19 and §33 say the bridge is *one function*. §50 found they had also said it was "eight
    /// calls" in four places, where it was twenty-eight — a number nobody could maintain.
    ///
    /// The count is gone from the document and the durable claim is pinned here instead. This
    /// reads its own source, which is unusual for a test and is the point: the thing being
    /// asserted is a fact about the shape of this file, and a second entry point would be exactly
    /// the kind of change that gets made without anyone opening the design document (§51).
    #[test]
    fn there_is_exactly_one_way_in() {
        // Only the module above the tests, for the same reason the secrets test does it.
        let source = include_str!("lib.rs").split("#[cfg(test)]").next().unwrap_or_default();
        let entry_points = source.matches("extern \"system\"").count();
        assert_eq!(entry_points, 1, "the design document says the bridge is one JNI function; make it true or change §19");
    }
}
