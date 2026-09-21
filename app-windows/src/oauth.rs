//! Google sign-in for installed apps: OAuth 2.0 authorization code + PKCE with a loopback
//! redirect (RFC 8252). The user signs in in their own browser; Pip never sees a password.
//! Only the metadata scope is requested, so the resulting token cannot read, send or delete mail.

use crate::net::{self, form_encode};
use pip_core::email::{b64url_encode, gmail};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_MAILAUTH: u32 = WM_APP + 4;
/// Endpoints. Dev builds can point them at a local mock server (tools/mock-google.js);
/// release builds always talk to Google.
fn endpoint(var: &str, default: &str) -> String {
    if cfg!(feature = "devtools") {
        if let Ok(v) = std::env::var(var) {
            return v;
        }
    }
    default.to_string()
}
pub fn auth_url() -> String {
    endpoint("PIP_OAUTH_AUTH", "https://accounts.google.com/o/oauth2/v2/auth")
}
pub fn token_url() -> String {
    endpoint("PIP_OAUTH_TOKEN", "https://oauth2.googleapis.com/token")
}
pub fn gmail_api() -> String {
    endpoint("PIP_GMAIL_API", gmail::API)
}

#[derive(Clone, Debug)]
pub struct ClientKeys {
    pub id: String,
    pub secret: String,
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    unsafe { BCryptGenRandom(std::ptr::null_mut(), v.as_mut_ptr(), n as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    v
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32) };
    out
}

pub struct Tokens {
    pub access: String,
    pub expires_at: i64,
    pub refresh: Option<String>,
    pub scope: String,
}

/// Why a token call failed. `revoked` = the refresh token is no longer valid (sign in again).
pub struct TokenError {
    pub revoked: bool,
    pub message: String,
}

fn token_call(form: &str, now: i64) -> Result<Tokens, TokenError> {
    let headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())];
    let r = net::send("POST", &token_url(), &headers, Some(form.as_bytes())).map_err(|m| TokenError { revoked: false, message: m })?;
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
    if r.status != 200 {
        let err = v["error"].as_str().unwrap_or("");
        let desc = v["error_description"].as_str().unwrap_or("");
        return Err(match err {
            "invalid_grant" => TokenError {
                revoked: true,
                message: "Google signed Pip out. If this happens every week, open the consent screen and click “Publish app”, then sign in again.".into(),
            },
            "invalid_client" | "unauthorized_client" => TokenError {
                revoked: true,
                message: "Google didn't accept the Client ID/secret — copy them again from the Google Cloud console.".into(),
            },
            _ => TokenError { revoked: false, message: format!("Google sign-in failed ({err}{}{desc})", if desc.is_empty() { "" } else { ": " }) },
        });
    }
    Ok(Tokens {
        access: v["access_token"].as_str().unwrap_or("").to_string(),
        expires_at: now + v["expires_in"].as_i64().unwrap_or(3600),
        refresh: v["refresh_token"].as_str().map(String::from),
        scope: v["scope"].as_str().unwrap_or("").to_string(),
    })
}

/// Swaps a refresh token for a fresh access token. Blocking (worker threads only).
pub fn refresh(keys: &ClientKeys, refresh_token: &str, now: i64) -> Result<Tokens, TokenError> {
    let form = format!(
        "client_id={}&client_secret={}&refresh_token={}&grant_type=refresh_token",
        form_encode(&keys.id),
        form_encode(&keys.secret),
        form_encode(refresh_token)
    );
    token_call(&form, now)
}

/// Best-effort revocation when an account is disconnected.
pub fn revoke_in_background(token: String) {
    let _ = std::thread::Builder::new().name("pip-revoke".into()).stack_size(128 * 1024).spawn(move || {
        let headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())];
        let _ = net::send("POST", "https://oauth2.googleapis.com/revoke", &headers, Some(format!("token={}", form_encode(&token)).as_bytes()));
    });
}

pub enum AuthResult {
    Ok { refresh: String, access: String, expires_at: i64, address: String, history_id: String },
    Err(String),
    Cancelled,
}

/// A sign-in waiting for the browser to come back.
pub struct Pending {
    listener: TcpListener,
    verifier: String,
    state: String,
    redirect: String,
    keys: ClientKeys,
}

/// Binds the loopback port and builds the consent URL (UI thread; fast).
pub fn prepare(keys: &ClientKeys) -> Result<(Pending, String), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Couldn't start sign-in ({e})"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = b64url_encode(&random_bytes(32));
    let challenge = b64url_encode(&sha256(verifier.as_bytes()));
    let state = b64url_encode(&random_bytes(16));
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&access_type=offline&prompt=consent&state={state}",
        auth_url(),
        form_encode(&keys.id),
        form_encode(&redirect),
        form_encode(gmail::SCOPE)
    );
    Ok((Pending { listener, verifier, state, redirect, keys: keys.clone() }, url))
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn page(title: &str, body: &str) -> String {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Pip</title><body style=\"font-family:Segoe UI,system-ui,sans-serif;background:#f6f7f9;color:#1e2330;display:flex;align-items:center;justify-content:center;height:100vh;margin:0\"><div style=\"background:#fff;padding:32px 40px;border-radius:18px;box-shadow:0 6px 30px #0001;max-width:440px;text-align:center\"><div style=\"font-size:44px\">&#127793;</div><h2 style=\"margin:8px 0\">{title}</h2><p style=\"color:#6b7385;line-height:1.5\">{body}</p></div></body>"
    );
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len())
}

/// Waits (≤ 5 min) for the browser redirect, exchanges the code and reads the Gmail profile, then
/// posts `WM_MAILAUTH` with a boxed `AuthResult`.
pub fn finish_in_background(hwnd: HWND, pending: Pending, cancel: Arc<AtomicBool>) {
    let hwnd_bits = hwnd as usize;
    let _ = std::thread::Builder::new().name("pip-signin".into()).stack_size(256 * 1024).spawn(move || {
        let result = wait_and_exchange(pending, &cancel);
        let ptr = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(hwnd_bits as HWND, WM_MAILAUTH, 0, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

fn wait_and_exchange(p: Pending, cancel: &AtomicBool) -> AuthResult {
    let _ = p.listener.set_nonblocking(true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let (code, mut stream) = loop {
        if cancel.load(Ordering::Relaxed) {
            return AuthResult::Cancelled;
        }
        if std::time::Instant::now() > deadline {
            return AuthResult::Err("Sign-in timed out. Click “Sign in with Google” to try again.".into());
        }
        let (mut s, _) = match p.listener.accept() {
            Ok(x) => x,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        let _ = s.set_nonblocking(false);
        let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut buf = [0u8; 8192];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let param = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| url_decode(v)));
        if let Some(err) = param("error") {
            let _ = s.write_all(page("Sign-in cancelled", "No problem — Pip didn't connect to Gmail. You can close this tab.").as_bytes());
            return AuthResult::Err(if err == "access_denied" { "Sign-in was cancelled or access wasn't allowed.".into() } else { format!("Google reported: {err}") });
        }
        match (param("code"), param("state")) {
            (Some(code), Some(state)) if state == p.state => break (code, s),
            (Some(_), _) => {
                let _ = s.write_all(page("Something's off", "That sign-in response didn't match. Please try again from Pip.").as_bytes());
                return AuthResult::Err("Sign-in response didn't match (state check failed). Please try again.".into());
            }
            _ => {
                // Favicon or stray request: answer and keep waiting.
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        }
    };
    let now = crate::platform::now_unix();
    let form = format!(
        "code={}&client_id={}&client_secret={}&redirect_uri={}&grant_type=authorization_code&code_verifier={}",
        form_encode(&code),
        form_encode(&p.keys.id),
        form_encode(&p.keys.secret),
        form_encode(&p.redirect),
        form_encode(&p.verifier)
    );
    let tokens = match token_call(&form, now) {
        Ok(t) => t,
        Err(e) => {
            let _ = stream.write_all(page("Couldn't connect", &e.message).as_bytes());
            return AuthResult::Err(e.message);
        }
    };
    if !tokens.scope.contains("gmail.metadata") {
        let msg = "Pip needs permission to see your emails' sender and subject — please tick that box when signing in.";
        let _ = stream.write_all(page("One more permission needed", msg).as_bytes());
        return AuthResult::Err(msg.into());
    }
    let Some(refresh) = tokens.refresh.clone() else {
        let _ = stream.write_all(page("Couldn't connect", "Google didn't grant offline access. Please try again.").as_bytes());
        return AuthResult::Err("Google didn't return a long-lived sign-in. Please try again.".into());
    };
    let headers = vec![("Authorization".to_string(), format!("Bearer {}", tokens.access))];
    let profile = net::send("GET", &gmail::profile_path(&gmail_api()), &headers, None)
        .ok()
        .filter(|r| r.status == 200)
        .and_then(|r| gmail::parse_profile(&String::from_utf8_lossy(&r.body)));
    let Some((address, history_id)) = profile else {
        let msg = "Signed in, but Gmail didn't answer. Check that the Gmail API is enabled (step 2), then try again.";
        let _ = stream.write_all(page("Almost there", msg).as_bytes());
        return AuthResult::Err(msg.into());
    };
    let _ = stream.write_all(
        page("Pip is connected", &format!("Pip will gently let you know when something important arrives in {address}. It can only see senders and subjects. You can close this tab.")).as_bytes(),
    );
    AuthResult::Ok { refresh, access: tokens.access, expires_at: tokens.expires_at, address, history_id }
}

/// # Safety
/// `lparam` must come from a `WM_MAILAUTH` message posted by [`finish_in_background`].
pub unsafe fn take_result(lparam: isize) -> Box<AuthResult> {
    Box::from_raw(lparam as *mut AuthResult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_and_pkce_known_vector() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(b64url_encode(&sha256(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(random_bytes(32).len(), 32);
        assert_ne!(random_bytes(16), random_bytes(16));
    }

    #[test]
    fn url_decoding() {
        assert_eq!(url_decode("4%2F0Ab-x_y"), "4/0Ab-x_y");
        assert_eq!(url_decode("a+b%20c"), "a b c");
    }
}
