//! Compiles Pip's own Google sign-in identity into the app, so users just click "Connect Gmail".
//!
//! Put the "Desktop app" OAuth client file downloaded from the Google Cloud console at
//! `app-windows/google-client.json` (git-ignored), or point PIP_GOOGLE_CLIENT_JSON at it.
//! For installed apps Google treats this as an app identifier, not a confidential secret; it is
//! kept out of the repository only so secret scanners don't flag it.
//! Without the file, Pip builds fine but can't connect Google accounts (release builds warn).

use std::path::PathBuf;

fn field(json: &str, key: &str) -> String {
    // The file is tiny and machine-written: {"installed":{"client_id":"…","client_secret":"…",…}}
    let pat = format!("\"{key}\"");
    json.find(&pat)
        .and_then(|i| {
            let rest = &json[i + pat.len()..];
            let rest = &rest[rest.find(':')? + 1..];
            let start = rest.find('"')? + 1;
            let end = start + rest[start..].find('"')?;
            Some(rest[start..end].to_string())
        })
        .unwrap_or_default()
}

fn main() {
    let path = std::env::var_os("PIP_GOOGLE_CLIENT_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("google-client.json"));
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo:rerun-if-env-changed=PIP_GOOGLE_CLIENT_JSON");
    let json = std::fs::read_to_string(&path).unwrap_or_default();
    let (id, secret) = (field(&json, "client_id"), field(&json, "client_secret"));
    if !json.is_empty() && !id.ends_with(".apps.googleusercontent.com") {
        println!("cargo:warning={} doesn't look like a Google OAuth client file; ignoring it", path.display());
        println!("cargo:rustc-env=PIP_BUILTIN_GOOGLE_ID=");
        println!("cargo:rustc-env=PIP_BUILTIN_GOOGLE_SECRET=");
        return;
    }
    if id.is_empty() && std::env::var("PROFILE").as_deref() == Ok("release") {
        println!("cargo:warning=No {} — this build can't connect Google accounts", path.display());
    }
    println!("cargo:rustc-env=PIP_BUILTIN_GOOGLE_ID={id}");
    println!("cargo:rustc-env=PIP_BUILTIN_GOOGLE_SECRET={secret}");
}
