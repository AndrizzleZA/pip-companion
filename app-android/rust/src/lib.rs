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
use pip_core::settings::Settings;
use pip_core::state::State;
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
        _ => Err(format!("Pip doesn't know how to {name}")),
    }
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

    #[test]
    fn a_bad_call_explains_itself_instead_of_panicking() {
        assert!(dispatch("nonsense", &json!({})).unwrap_err().contains("nonsense"));
        assert!(dispatch("open", &json!({ "key": "short", "bytes": "00" })).is_err());
        assert!(dispatch("local_doc", &json!({})).is_err());
    }
}
