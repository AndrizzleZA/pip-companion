//! Browser sign-in for installed apps: OAuth 2.0 authorization code + PKCE with a loopback
//! redirect (RFC 8252) — the approach Google recommends for desktop apps and the one Thunderbird,
//! gcloud and rclone use. The user signs in in their own browser; Pip never sees a password.
//!
//! Generic over the service ([`Service`]) so another sign-in connector (e.g. Microsoft) is a new
//! `Service` constant plus its own "who is this?" check — the flow itself is shared.

use crate::net::{self, form_encode};
use pip_core::connectors::SignInProblem;
use pip_core::email::b64url_encode;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_SIGNIN: u32 = WM_APP + 4;
/// How long a sign-in waits for the browser (2-step verification can take a while).
const SIGNIN_WAIT_SECS: u64 = 600;

/// One OAuth provider. Endpoints can be pointed at a local mock server in dev builds only.
pub struct Service {
    pub name: &'static str,
    auth: &'static str,
    token: &'static str,
    revoke: &'static str,
    pub scope: &'static str,
}

pub static GOOGLE: Service = Service {
    name: "Google",
    auth: "https://accounts.google.com/o/oauth2/v2/auth",
    token: "https://oauth2.googleapis.com/token",
    revoke: "https://oauth2.googleapis.com/revoke",
    scope: pip_core::email::gmail::SCOPE,
};

/// Real endpoint in release builds. Dev builds (only) let tests point it at tools/mock-google.js;
/// the override names aren't even compiled into release.
fn endpoint(url: &str) -> String {
    #[cfg(feature = "devtools")]
    {
        let var = if url.ends_with("/calendar/v3") {
            "PIP_GCAL_API"
        } else if url.ends_with("/auth") {
            "PIP_OAUTH_AUTH"
        } else if url.ends_with("/token") {
            "PIP_OAUTH_TOKEN"
        } else if url.ends_with("/revoke") {
            "PIP_OAUTH_REVOKE"
        } else {
            "PIP_GMAIL_API"
        };
        if let Ok(v) = std::env::var(var) {
            return v;
        }
    }
    url.to_string()
}

pub static GOOGLE_CALENDAR: Service = Service {
    name: "Google",
    auth: "https://accounts.google.com/o/oauth2/v2/auth",
    token: "https://oauth2.googleapis.com/token",
    revoke: "https://oauth2.googleapis.com/revoke",
    scope: pip_core::gcal::SCOPE,
};

pub fn gcal_api() -> String {
    endpoint(pip_core::gcal::API)
}

pub fn gmail_api() -> String {
    endpoint(pip_core::email::gmail::API)
}

/// The app's identity with the service. For installed apps this is *not* a secret (Google:
/// "installed apps cannot keep secrets"); security comes from PKCE and the user's consent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientKeys {
    pub id: String,
    pub secret: String,
}

/// Pip's own Google sign-in, compiled in from `app-windows/google-client.json` (see build.rs),
/// so users just click "Connect". Dev builds may override it from the environment for tests.
pub fn builtin_google_client() -> Option<ClientKeys> {
    #[cfg(feature = "devtools")]
    if let Ok(id) = std::env::var("PIP_GOOGLE_CLIENT_ID") {
        return Some(ClientKeys { id, secret: std::env::var("PIP_GOOGLE_CLIENT_SECRET").unwrap_or_default() });
    }
    let id = env!("PIP_BUILTIN_GOOGLE_ID");
    (!id.is_empty()).then(|| ClientKeys { id: id.into(), secret: env!("PIP_BUILTIN_GOOGLE_SECRET").into() })
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

/// Why a token call failed. `revoked` = the sign-in is no longer valid (reconnect needed).
pub struct TokenError {
    pub revoked: bool,
    pub message: String,
    pub problem: SignInProblem,
}

fn client_form(keys: &ClientKeys) -> String {
    let mut f = format!("client_id={}", form_encode(&keys.id));
    if !keys.secret.is_empty() {
        f += &format!("&client_secret={}", form_encode(&keys.secret));
    }
    f
}

fn token_call(service: &Service, form: &str, now: i64) -> Result<Tokens, TokenError> {
    let headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())];
    let r = net::send("POST", &endpoint(service.token), &headers, Some(form.as_bytes()))
        .map_err(|m| TokenError { revoked: false, message: m.clone(), problem: SignInProblem::Offline(m) })?;
    let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
    if r.status != 200 {
        let err = v["error"].as_str().unwrap_or("");
        let desc = v["error_description"].as_str().unwrap_or("");
        return Err(match err {
            "invalid_grant" => TokenError {
                revoked: true,
                message: format!("{} signed Pip out \u{2014} click Reconnect.", service.name),
                problem: SignInProblem::Other("the sign-in expired before it finished".into()),
            },
            "invalid_client" | "unauthorized_client" => TokenError {
                revoked: true,
                message: format!("{} no longer accepts this copy of Pip \u{2014} update Pip, then reconnect.", service.name),
                problem: SignInProblem::AppNotAllowed,
            },
            _ => {
                let detail = if desc.is_empty() { err.to_string() } else { format!("{err}: {desc}") };
                TokenError { revoked: false, message: format!("{} sign-in failed ({detail})", service.name), problem: SignInProblem::Other(detail) }
            }
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
pub fn refresh(service: &Service, keys: &ClientKeys, refresh_token: &str, now: i64) -> Result<Tokens, TokenError> {
    let form = format!("{}&refresh_token={}&grant_type=refresh_token", client_form(keys), form_encode(refresh_token));
    token_call(service, &form, now)
}

/// Best-effort revocation when an account is disconnected, so the permission disappears from the
/// user's account page too.
pub fn revoke_in_background(service: &'static Service, token: String) {
    let _ = std::thread::Builder::new().name("pip-revoke".into()).stack_size(128 * 1024).spawn(move || {
        let headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())];
        let _ = net::send("POST", &endpoint(service.revoke), &headers, Some(format!("token={}", form_encode(&token)).as_bytes()));
    });
}

/// A connected account, as returned by the service-specific check after sign-in.
pub struct Linked {
    pub refresh: String,
    pub access: String,
    pub expires_at: i64,
    /// Who signed in (shown to the user), e.g. the Gmail address.
    pub account: String,
    /// Service-specific starting point (Gmail: the history id to watch from).
    pub extra: String,
}

pub enum AuthResult {
    Ok(Linked),
    Failed(SignInProblem),
    Cancelled,
}

/// After the tokens arrive: "who is this, and does it work?" (runs on the sign-in thread).
pub type Verify = fn(access: &str) -> Result<(String, String), SignInProblem>;

/// A sign-in waiting for the browser to come back.
pub struct Pending {
    listener: TcpListener,
    verifier: String,
    state: String,
    redirect: String,
    keys: ClientKeys,
    service: &'static Service,
}

/// Binds the loopback port and builds the consent URL (UI thread; fast). `login_hint` preselects
/// the account when reconnecting; `choose_account` forces Google's account picker (only when
/// adding a second account — otherwise it's an extra click for people with one account).
pub fn prepare(service: &'static Service, keys: &ClientKeys, login_hint: Option<&str>, choose_account: bool) -> Result<(Pending, String), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Couldn't start sign-in ({e})"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = b64url_encode(&random_bytes(32));
    let challenge = b64url_encode(&sha256(verifier.as_bytes()));
    let state = b64url_encode(&random_bytes(16));
    let mut url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&access_type=offline&state={state}",
        endpoint(service.auth),
        form_encode(&keys.id),
        form_encode(&redirect),
        form_encode(service.scope)
    );
    // Always show the consent screen: it's the one step Google requires anyway, and it guarantees
    // a long-lived sign-in (no silent "signed out" later).
    url += if choose_account { "&prompt=consent%20select_account" } else { "&prompt=consent" };
    if let Some(h) = login_hint {
        url += &format!("&login_hint={}", form_encode(h));
    }
    Ok((Pending { listener, verifier, state, redirect, keys: keys.clone(), service }, url))
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The page the browser lands on after signing in. `ok` picks the happy or the "hmm" look.
fn page(ok: bool, title: &str, body: &str) -> String {
    let (icon, tint) = if ok { ("&#10003;", "#3BB08F") } else { ("!", "#E0A030") };
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Pip</title>\
<style>body{{font-family:\"Segoe UI\",system-ui,sans-serif;background:#f4f6f8;color:#1e2330;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}}\
div{{background:#fff;padding:36px 44px;border-radius:20px;box-shadow:0 8px 32px #0000000f;max-width:420px;text-align:center}}\
b{{display:inline-flex;width:56px;height:56px;border-radius:50%;background:{tint};color:#fff;font-size:28px;align-items:center;justify-content:center}}\
h2{{margin:16px 0 8px;font-weight:600}}p{{color:#6b7385;line-height:1.55;margin:0}}small{{display:block;margin-top:18px;color:#9aa1ae}}\
@media(prefers-color-scheme:dark){{body{{background:#14161b;color:#e8eaf0}}div{{background:#1e2129;box-shadow:none}}p{{color:#a0a7b4}}}}</style>\
<div><b>{icon}</b><h2>{}</h2><p>{}</p><small>You can close this tab and go back to Pip.</small></div>",
        html_escape(title),
        html_escape(body)
    );
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{html}", html.len())
}

/// Waits for the browser redirect, exchanges the code and runs `verify`, then posts `WM_SIGNIN`
/// with a boxed `AuthResult`. The thread lives only for the duration of one sign-in.
pub fn finish_in_background(hwnd: HWND, pending: Pending, cancel: Arc<AtomicBool>, verify: Verify) {
    let hwnd_bits = hwnd as usize;
    let _ = std::thread::Builder::new().name("pip-signin".into()).stack_size(256 * 1024).spawn(move || {
        let result = wait_and_exchange(pending, &cancel, verify);
        if cancel.load(Ordering::Relaxed) {
            return; // superseded or cancelled: the UI no longer waits for this one
        }
        let ptr = Box::into_raw(Box::new(result));
        unsafe {
            if PostMessageW(hwnd_bits as HWND, WM_SIGNIN, 0, ptr as isize) == 0 {
                drop(Box::from_raw(ptr));
            }
        }
    });
}

fn wait_and_exchange(p: Pending, cancel: &AtomicBool, verify: Verify) -> AuthResult {
    let _ = p.listener.set_nonblocking(true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(SIGNIN_WAIT_SECS);
    let (code, mut stream) = loop {
        if cancel.load(Ordering::Relaxed) {
            return AuthResult::Cancelled;
        }
        if std::time::Instant::now() > deadline {
            return AuthResult::Failed(SignInProblem::TimedOut);
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
        let state_ok = param("state").as_deref() == Some(p.state.as_str());
        if let Some(err) = param("error").filter(|_| state_ok) {
            return if err == "access_denied" {
                let _ = s.write_all(page(false, "No problem", "Nothing was connected. You can try again from Pip whenever you like.").as_bytes());
                AuthResult::Failed(SignInProblem::Declined)
            } else {
                let _ = s.write_all(page(false, "That didn\u{2019}t work", &format!("{} reported \u{201c}{err}\u{201d}. Please try again from Pip.", p.service.name)).as_bytes());
                AuthResult::Failed(SignInProblem::Other(err))
            };
        }
        match param("code") {
            Some(code) if state_ok => break (code, s),
            // Favicon, a stale tab or a forged request: answer and keep waiting for the real one.
            _ => {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        }
    };
    let fail = |stream: &mut std::net::TcpStream, problem: SignInProblem| {
        let _ = stream.write_all(page(false, "Not connected yet", &problem.explain(p.service.name)).as_bytes());
        AuthResult::Failed(problem)
    };
    let now = crate::platform::now_unix();
    let form = format!(
        "code={}&{}&redirect_uri={}&grant_type=authorization_code&code_verifier={}",
        form_encode(&code),
        client_form(&p.keys),
        form_encode(&p.redirect),
        form_encode(&p.verifier)
    );
    let tokens = match token_call(p.service, &form, now) {
        Ok(t) => t,
        Err(e) => return fail(&mut stream, e.problem),
    };
    // Granular consent: the user may have left the permission unticked.
    if !p.service.scope.split(' ').all(|s| tokens.scope.split(' ').any(|g| g == s)) {
        return fail(&mut stream, SignInProblem::MissingPermission);
    }
    let Some(refresh) = tokens.refresh.clone() else {
        return fail(&mut stream, SignInProblem::Other("no long-lived sign-in was granted".into()));
    };
    let (account, extra) = match verify(&tokens.access) {
        Ok(x) => x,
        Err(problem) => return fail(&mut stream, problem),
    };
    let _ = stream.write_all(page(true, "Pip is connected", &format!("Signed in as {account}.")).as_bytes());
    AuthResult::Ok(Linked { refresh, access: tokens.access, expires_at: tokens.expires_at, account, extra })
}

/// # Safety
/// `lparam` must come from a `WM_SIGNIN` message posted by [`finish_in_background`].
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

    #[test]
    fn secretless_clients_omit_the_secret() {
        assert_eq!(client_form(&ClientKeys { id: "a b".into(), secret: String::new() }), "client_id=a%20b");
        assert_eq!(client_form(&ClientKeys { id: "a".into(), secret: "s".into() }), "client_id=a&client_secret=s");
    }

    #[test]
    fn landing_page_escapes_text() {
        let p = page(true, "<x>", "a & b");
        assert!(p.contains("&lt;x&gt;") && p.contains("a &amp; b") && !p.contains("<x>"));
    }
}
