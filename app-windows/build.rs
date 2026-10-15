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

/// Which build this is, so a crash report can be matched to the exact binary that produced it —
/// and so to that binary's archived `.pdb`. The version cannot do this job: it has been 0.1.0 for
/// every build ever made, and a pile of symbol files with nothing to key them by is not an archive.
///
/// `PIP_BUILD` wins when set, because `tools/archive-symbols.ps1` passes the commit it is archiving
/// under and that must be exact. Falling back to asking git is for ordinary development builds,
/// where Cargo may not re-run this script the instant a commit lands; a stale stamp there costs
/// nothing, while a wrong one on a release would send you to the wrong source.
fn build_stamp() -> String {
    if let Ok(v) = std::env::var("PIP_BUILD") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match git(&["rev-parse", "--short=10", "HEAD"]).filter(|h| !h.is_empty()) {
        Some(hash) if git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty()) => format!("{hash}-dirty"),
        Some(hash) => hash,
        None => "unknown".to_string(),
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=PIP_BUILD");
    // HEAD's *contents* only change when the branch does, so committing on the same branch left
    // the stamp untouched: a build made after a commit still said `-dirty`, and one made after
    // committing a fix carried the hash of the commit before it. A stamp that names the wrong
    // source is worse than no stamp, because §34 reads a crash report by trusting it. The index
    // changes on every `git add` and every commit, which is exactly when the stamp goes stale.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    println!("cargo:rustc-env=PIP_BUILD={}", build_stamp());
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
