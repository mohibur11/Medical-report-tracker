//! Signing in to Google, and staying signed in.
//!
//! The installed-app flow: the app opens the system browser, Google asks which
//! account and whether to allow, and the answer comes back to a one-shot HTTP
//! listener on a loopback port. No password ever passes through this app, and
//! nothing is stored except a refresh token — sealed with DPAPI, because it is
//! standing permission to reach the user's Drive rather than data of their own.
//!
//! The scope requested is `drive.file`, which reaches only files this app itself
//! created. It cannot read the rest of the user's Drive, which is both the honest
//! thing to ask for and the reason no paid security assessment is needed to
//! publish the consent screen.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Deserialize;

use crate::db;
use crate::secret;

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
const USERINFO_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Only files this app creates. Deliberately not `drive` or `drive.readonly`:
/// those are restricted scopes, they would let this app read the user's whole
/// Drive, and neither is needed to keep a backup.
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file openid email";

pub const CLIENT_ID: &str = "google_client_id";
pub const CLIENT_SECRET: &str = "google_client_secret";

/// The OAuth client compiled in at build time, if the build had one.
///
/// Read from `src-tauri/google-client.env`, which is not in the repository: a
/// client ID committed to a public repo gets used by strangers against the
/// quota, and puts their app's name on somebody else's consent screen. Baking it
/// into the binary instead is what lets a phone say "choose an account" rather
/// than asking for a client ID and secret to be typed on a phone keyboard.
///
/// A client entered in the app still wins, so a build with nothing baked in
/// behaves exactly as it did before, and anyone who wants their own can use it.
const BAKED_CLIENT_ID: Option<&str> = option_env!("MRT_GOOGLE_CLIENT_ID");
const BAKED_CLIENT_SECRET: Option<&str> = option_env!("MRT_GOOGLE_CLIENT_SECRET");

/// The built-in client, but only if it is whole.
///
/// Both halves or neither: a build carrying an ID and no secret would look
/// configured, skip the setup screen, and then fail the token exchange with
/// `invalid_client` — worse than plainly asking for the credentials.
fn baked_client() -> Option<(String, String)> {
    let id = BAKED_CLIENT_ID?.trim();
    let secret = BAKED_CLIENT_SECRET.unwrap_or_default().trim();
    if id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((id.to_string(), secret.to_string()))
}

/// Is there a client to sign in with at all — stored, or built in?
fn have_client(conn: &Connection) -> bool {
    db::setting(conn, CLIENT_ID).is_some() || baked_client().is_some()
}
const REFRESH_TOKEN: &str = "google_refresh_token";
const ACCOUNT_EMAIL: &str = "google_account_email";

/// A sign-in that has been started and not yet answered.
///
/// Kept in the database rather than in a variable, because on a phone the flow
/// outlives the thing that started it: opening the browser sends this app to the
/// background, and Android is free to freeze it, drop the web view, or stop the
/// thread that was waiting. Anything held in memory is gone by the time the
/// answer arrives. These three survive it.
const PENDING_VERIFIER: &str = "google_pending_verifier";
const PENDING_STATE: &str = "google_pending_state";
const PENDING_REDIRECT: &str = "google_pending_redirect";

/// How long the browser half of the flow may take before it is given up on.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: Option<String>,
    pub connected: bool,
    /// False until an OAuth client id has been configured.
    pub configured: bool,
    /// A sign-in was started and Google's answer has not arrived. The screen uses
    /// this to offer the way to finish it by hand.
    pub pending: bool,
}

pub fn account(conn: &Connection) -> Account {
    let configured = have_client(conn);

    Account {
        email: db::setting(conn, ACCOUNT_EMAIL),
        connected: db::setting(conn, REFRESH_TOKEN).is_some(),
        configured,
        pending: db::setting(conn, PENDING_VERIFIER).is_some(),
    }
}

pub fn set_client(conn: &Connection, id: &str, secret_value: &str) -> Result<(), String> {
    let id = id.trim();
    if !id.ends_with(".apps.googleusercontent.com") {
        return Err(
            "That does not look like a Google client ID. It ends in .apps.googleusercontent.com."
                .into(),
        );
    }
    db::set_setting(conn, CLIENT_ID, id)?;
    // Desktop clients are public clients: this value cannot be kept secret in an
    // app the user runs, and Google treats it accordingly. Stored plainly because
    // pretending otherwise would be theatre.
    db::set_setting(conn, CLIENT_SECRET, secret_value.trim())
}

pub fn disconnect(conn: &Connection) -> Result<(), String> {
    // Best effort: tell Google to forget it too, so a stolen copy of the token is
    // dead rather than merely unreferenced.
    if let Ok(token) = refresh_token(conn) {
        let _ = reqwest::blocking::Client::new()
            .post(REVOKE_ENDPOINT)
            .form(&[("token", token.as_str())])
            .send();
    }
    for key in [REFRESH_TOKEN, ACCOUNT_EMAIL] {
        db::clear_setting(conn, key)?;
    }
    // A half-finished sign-in belongs to the account being forgotten.
    cancel(conn);
    // The uploaded-file map belongs to the account that owns those files.
    conn.execute("DELETE FROM drive_file", [])
        .map_err(|e| format!("cannot clear the upload record: {e}"))?;
    Ok(())
}

fn refresh_token(conn: &Connection) -> Result<String, String> {
    let sealed = db::setting(conn, REFRESH_TOKEN).ok_or("Not connected to Google Drive.")?;
    let bytes = base64_decode(&sealed)?;
    let plain = secret::unprotect(&bytes)?;
    String::from_utf8(plain).map_err(|_| "the stored sign-in is damaged".into())
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| "the stored sign-in is damaged".to_string())
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE: a random verifier, and the SHA-256 of it sent up front.
///
/// A loopback redirect is reachable by anything else running as this user, so the
/// authorization code alone must not be enough to obtain a token.
fn pkce_pair() -> (String, String) {
    use rand_core::RngCore;
    use sha2::{Digest, Sha256};

    let mut raw = [0u8; 64];
    rand_core::OsRng.fill_bytes(&mut raw);
    let verifier = base64_url_nopad(&raw);
    let challenge = base64_url_nopad(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    /// What was actually granted, space-separated. Not necessarily what was
    /// asked for: Google's consent screen lets the user untick permissions one
    /// by one, and a sign-in with Drive unticked succeeds — with a token that
    /// every Drive call then refuses.
    #[serde(default)]
    scope: Option<String>,
}

/// The sign-in went through, but without the one permission it was for.
pub const DRIVE_NOT_GRANTED: &str = "Google signed you in without access to Drive — the \
    \"See, edit, create and delete only the specific Google Drive files you use with this \
    app\" box on Google's consent screen was left unticked. Disconnect, sign in again, and \
    tick it.";

/// Refuse a token that cannot reach Drive now, at sign-in, rather than let it be
/// stored and fail on every file of the first backup.
fn require_drive(scope: Option<&str>) -> Result<(), String> {
    match scope {
        Some(granted) if !granted.split_whitespace().any(|s| s.ends_with("/auth/drive.file")) => {
            Err(DRIVE_NOT_GRANTED.into())
        }
        _ => Ok(()),
    }
}

/// What a completed sign-in yields, before any of it is stored.
pub struct Tokens {
    pub refresh: String,
    pub access: String,
    pub expires_in: u64,
    pub email: Option<String>,
}

/// Start a sign-in and return the URL to open.
///
/// Returns immediately. What it leaves behind — the verifier, the state, the
/// redirect it promised Google — is what lets the answer be collected later by
/// whoever gets it first: the listener if it survived, or the person pasting the
/// address out of the browser if it did not.
pub fn begin(conn: &Connection, redirect: &str) -> Result<String, String> {
    let (client_id, _) = client_credentials(conn)?;
    let (verifier, challenge) = pkce_pair();
    let state = base64_url_nopad(&{
        use rand_core::RngCore;
        let mut b = [0u8; 16];
        rand_core::OsRng.fill_bytes(&mut b);
        b
    });

    db::set_setting(conn, PENDING_VERIFIER, &verifier)?;
    db::set_setting(conn, PENDING_STATE, &state)?;
    db::set_setting(conn, PENDING_REDIRECT, redirect)?;

    Ok(format!(
        "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        percent_encode(&client_id),
        percent_encode(redirect),
        percent_encode(SCOPE),
        percent_encode(&challenge),
        percent_encode(&state),
    ))
}

/// Forget a sign-in that was started and never finished.
pub fn cancel(conn: &Connection) {
    for key in [PENDING_VERIFIER, PENDING_STATE, PENDING_REDIRECT] {
        let _ = db::clear_setting(conn, key);
    }
}

/// Read the answer, check it belongs to the sign-in that was started, and take
/// the pending flow with it.
///
/// `answer` is whatever the browser ended up holding: the whole address, or just
/// its query. The address bar keeps it even when the page itself failed to load,
/// which is the point — a redirect to a port nobody is listening on any more
/// still shows the code, and it is still good.
fn claim(conn: &Connection, answer: &str) -> Result<(String, String, String), String> {
    let verifier = db::setting(conn, PENDING_VERIFIER)
        .ok_or("No sign-in is waiting to be finished. Start one first.")?;
    let expected = db::setting(conn, PENDING_STATE).unwrap_or_default();
    let redirect = db::setting(conn, PENDING_REDIRECT).unwrap_or_default();

    let query = answer.split_once('?').map(|(_, q)| q).unwrap_or(answer).trim();
    let code = code_from_query(query, &expected)
        .ok_or("That address carries no sign-in answer. Copy the whole address from the browser.")??;

    cancel(conn);
    Ok((code, verifier, redirect))
}

/// Exchange the answer for tokens.
pub fn finish(conn: &Connection, answer: &str) -> Result<Tokens, String> {
    let (client_id, client_secret) = client_credentials(conn)?;
    let (code, verifier, redirect) = claim(conn, answer)?;

    let response = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .map_err(|e| format!("cannot reach Google: {e}"))?;

    if !response.status().is_success() {
        return Err(explain_token_error(&response.text().unwrap_or_default()));
    }

    let token: TokenResponse = response
        .json()
        .map_err(|e| format!("Google's reply could not be read: {e}"))?;
    require_drive(token.scope.as_deref())?;
    let refresh = token
        .refresh_token
        .ok_or("Google did not return a refresh token. Remove this app at myaccount.google.com/permissions and try again.")?;
    let email = fetch_email(&token.access_token);

    Ok(Tokens {
        refresh,
        access: token.access_token,
        expires_in: token.expires_in.unwrap_or(3600),
        email,
    })
}

/// The sign-in itself: open the browser, wait for the answer, exchange it.
///
/// Takes no database handle on purpose. This blocks for as long as the person
/// takes to choose an account and press Allow, and holding the connection across
/// that would freeze every other part of the app while they did it.
///
/// `open_url` is supplied by the caller because opening a browser is the one step
/// with no portable answer: a shell call on Windows, an intent on Android.
pub fn run_flow(
    client_id: &str,
    client_secret: &str,
    open_url: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Tokens, String> {
    #[cfg(target_os = "android")]
    {
        // The phone does not use this. Its browser leaves the app suspended, so
        // its sign-in is begin/finish and survives the app being stopped.
        let _ = (client_id, client_secret, open_url);
        return Err("Use begin and finish on Android.".into());
    }

    #[allow(unreachable_code)]
    {

    // Port 0 asks the OS for a free one; Google allows any port on loopback.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot listen for Google's reply: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the local port: {e}"))?
        .port();
    listener
        .set_nonblocking(false)
        .map_err(|e| format!("cannot configure the listener: {e}"))?;

    let redirect = format!("http://127.0.0.1:{port}");
    let (verifier, challenge) = pkce_pair();
    let state = base64_url_nopad(&{
        use rand_core::RngCore;
        let mut b = [0u8; 16];
        rand_core::OsRng.fill_bytes(&mut b);
        b
    });

    let url = format!(
        "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        percent_encode(&client_id),
        percent_encode(&redirect),
        percent_encode(SCOPE),
        percent_encode(&challenge),
        percent_encode(&state),
    );

    open_url(&url)?;

    let code = wait_for_code(listener, &state)?;

    let form = [
        ("code", code.as_str()),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect.as_str()),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier.as_str()),
    ];
    let response = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()
        .map_err(|e| format!("cannot reach Google: {e}"))?;

    if !response.status().is_success() {
        let body = response.text().unwrap_or_default();
        return Err(explain_token_error(&body));
    }

    let token: TokenResponse = response
        .json()
        .map_err(|e| format!("Google's reply could not be read: {e}"))?;
    require_drive(token.scope.as_deref())?;

    let refresh = token
        .refresh_token
        .ok_or("Google did not return a refresh token. Remove this app at myaccount.google.com/permissions and try again.")?;
    let email = fetch_email(&token.access_token);

    Ok(Tokens {
        refresh,
        access: token.access_token,
        expires_in: token.expires_in.unwrap_or(3600),
        email,
    })
    }
}

/// Keep what the sign-in returned. Sealed, because the refresh token is standing
/// permission to reach the user's Drive rather than data of their own.
pub fn store(conn: &Connection, tokens: Tokens) -> Result<Account, String> {
    let sealed = secret::protect(tokens.refresh.as_bytes())?;
    db::set_setting(conn, REFRESH_TOKEN, &base64_encode(&sealed))?;
    cache_access_token(conn, &tokens.access, tokens.expires_in);
    if let Some(email) = tokens.email {
        db::set_setting(conn, ACCOUNT_EMAIL, &email)?;
    }
    Ok(account(conn))
}

/// The client this installation signs in with, if one has been configured.
pub fn client_credentials(conn: &Connection) -> Result<(String, String), String> {
    // What the user entered, else what the build carries. In that order, so
    // somebody who wants their own Google project can still have one.
    if let Some(id) = db::setting(conn, CLIENT_ID) {
        return Ok((id, db::setting(conn, CLIENT_SECRET).unwrap_or_default()));
    }

    baked_client()
        .ok_or_else(|| "No Google client ID has been set. Add one in the Google Drive settings.".to_string())
}

/// Turn Google's JSON error into something a person can act on.
fn explain_token_error(body: &str) -> String {
    if body.contains("invalid_client") {
        "Google rejected the client ID or secret. Check both in the Google Drive settings.".into()
    } else if body.contains("redirect_uri_mismatch") {
        "Google rejected the redirect. The OAuth client must be of type Desktop app.".into()
    } else if body.contains("access_denied") {
        "Sign-in was declined.".into()
    } else {
        format!("Google refused the sign-in: {body}")
    }
}

/// Serve exactly one request: Google's redirect carrying the code.
fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot configure the listener: {e}"))?;

    let deadline = SystemTime::now() + CONSENT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("cannot read Google's reply: {e}"))?;
                // Keep listening until the request that actually carries the
                // answer arrives. A browser opens sockets of its own accord —
                // speculative connections, a favicon fetch — and answering the
                // first one and stopping means the real redirect finds nothing
                // listening, which looks to the user like a network failure.
                match handle_redirect(stream, expected_state) {
                    Outcome::Answer(result) => return result,
                    Outcome::NotItYet => continue,
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if SystemTime::now() > deadline {
                    return Err("Google did not answer within three minutes. Try again.".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("cannot accept Google's reply: {e}")),
        }
    }
}

/// The authorization code out of a redirect's query string.
///
/// `None` means the request was not the redirect at all. A browser opens sockets
/// of its own accord — speculative connections, a favicon fetch — and treating
/// the first one as the answer leaves the real redirect with nothing listening,
/// which looks to the user like a network failure.
fn code_from_query(query: &str, expected_state: &str) -> Option<Result<String, String>> {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let value = decode_component(value);
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }

    if code.is_none() && error.is_none() {
        return None;
    }

    Some(if let Some(e) = error.as_deref() {
        Err(if e == "access_denied" {
            "Sign-in was declined.".to_string()
        } else {
            format!("Google reported: {e}")
        })
    } else if state.as_deref() != Some(expected_state) {
        // Something other than the browser window we opened answered.
        Err("The reply from Google did not match this sign-in attempt.".to_string())
    } else {
        code.ok_or_else(|| "Google's reply carried no authorization code.".to_string())
    })
}

/// Whether a request was the redirect, or just browser noise.
enum Outcome {
    Answer(Result<String, String>),
    NotItYet,
}

fn handle_redirect(mut stream: std::net::TcpStream, expected_state: &str) -> Outcome {
    let Ok(clone) = stream.try_clone() else {
        return Outcome::NotItYet;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return Outcome::NotItYet;
    }

    // "GET /?code=...&state=... HTTP/1.1"
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();

    let Some(outcome) = code_from_query(query, expected_state) else {
        // Neither an answer nor an error: something the browser asked for on its own.
        let _ = write!(stream, "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        let _ = stream.flush();
        return Outcome::NotItYet;
    };

    let page = match &outcome {
        Ok(_) => "<h2>Connected</h2><p>You can close this tab and go back to Medicine Report Tracker.</p>",
        Err(_) => "<h2>Not connected</h2><p>Go back to Medicine Report Tracker to see why.</p>",
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Medicine Report Tracker</title>\
         <body style=\"font-family:system-ui;margin:3rem\">{page}</body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.flush();

    Outcome::Answer(outcome)
}

fn decode_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn fetch_email(access_token: &str) -> Option<String> {
    let response = reqwest::blocking::Client::new()
        .get(USERINFO_ENDPOINT)
        .bearer_auth(access_token)
        .send()
        .ok()?;
    let value: serde_json::Value = response.json().ok()?;
    value["email"].as_str().map(str::to_string)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_access_token(conn: &Connection, token: &str, expires_in: u64) {
    // Held in the database rather than in memory so a sync started right after a
    // restart does not need a round trip. Short-lived by construction: an hour.
    let _ = db::set_setting(conn, "google_access_token", token);
    let _ = db::set_setting(
        conn,
        "google_access_expiry",
        &(now_secs() + expires_in.saturating_sub(60)).to_string(),
    );
}

/// A usable access token, refreshed if the cached one is spent.
pub fn access_token(conn: &Connection) -> Result<String, String> {
    if let (Some(token), Some(expiry)) = (
        db::setting(conn, "google_access_token"),
        db::setting(conn, "google_access_expiry").and_then(|s| s.parse::<u64>().ok()),
    ) {
        if expiry > now_secs() {
            return Ok(token);
        }
    }

    let refresh = refresh_token(conn)?;
    let client_id = db::setting(conn, CLIENT_ID).ok_or("No Google client ID has been set.")?;
    let client_secret = db::setting(conn, CLIENT_SECRET).unwrap_or_default();

    let response = reqwest::blocking::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .map_err(|e| format!("cannot reach Google: {e}"))?;

    if !response.status().is_success() {
        let body = response.text().unwrap_or_default();
        if body.contains("invalid_grant") {
            return Err(
                "Google has expired this sign-in. Connect the Google account again."
                    .into(),
            );
        }
        return Err(explain_token_error(&body));
    }

    let token: TokenResponse = response
        .json()
        .map_err(|e| format!("Google's reply could not be read: {e}"))?;
    cache_access_token(conn, &token.access_token, token.expires_in.unwrap_or(3600));
    Ok(token.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let dir = std::env::temp_dir().join(format!("mrt-google-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        db::open(&dir.join("app.db")).unwrap()
    }

    #[test]
    fn a_pkce_challenge_is_the_hash_of_its_verifier_not_the_verifier() {
        use sha2::{Digest, Sha256};
        let (verifier, challenge) = pkce_pair();

        assert_ne!(verifier, challenge, "sending the verifier would defeat the point");
        assert_eq!(challenge, base64_url_nopad(&Sha256::digest(verifier.as_bytes())));
        assert!(verifier.len() >= 43 && verifier.len() <= 128, "outside the spec's range");
        assert!(
            !challenge.contains('+') && !challenge.contains('/') && !challenge.contains('='),
            "must be URL-safe and unpadded: {challenge}",
        );
    }

    #[test]
    fn two_sign_ins_never_share_a_verifier() {
        let (a, _) = pkce_pair();
        let (b, _) = pkce_pair();
        assert_ne!(a, b);
    }

    #[test]
    fn a_sign_in_with_drive_unticked_is_refused_at_once() {
        // What Google returns when only the identity boxes were ticked.
        let err = require_drive(Some("openid https://www.googleapis.com/auth/userinfo.email"))
            .unwrap_err();
        assert_eq!(err, DRIVE_NOT_GRANTED);
        assert!(err.contains("unticked"), "must say what to do");

        assert!(require_drive(Some(
            "https://www.googleapis.com/auth/drive.file openid https://www.googleapis.com/auth/userinfo.email"
        ))
        .is_ok());
        // Order and extras do not matter; the one scope does.
        assert!(require_drive(Some("email https://www.googleapis.com/auth/drive.file")).is_ok());
        // An answer that says nothing about scope is not evidence of a problem.
        assert!(require_drive(None).is_ok());
    }

    #[test]
    fn the_scope_asked_for_is_the_narrow_one() {
        assert!(SCOPE.contains("drive.file"));
        assert!(
            !SCOPE.contains("auth/drive ") && !SCOPE.contains("drive.readonly"),
            "a backup must never ask to read the user's whole Drive",
        );
    }

    #[test]
    fn a_url_is_encoded_so_a_redirect_cannot_smuggle_parameters() {
        assert_eq!(percent_encode("http://127.0.0.1:1234"), "http%3A%2F%2F127.0.0.1%3A1234");
        assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(percent_encode("-_.~"), "-_.~", "unreserved characters stay as they are");
    }

    /// A started sign-in, and the state Google would send back with it.
    fn started(conn: &Connection) -> String {
        set_client(conn, "x.apps.googleusercontent.com", "s").unwrap();
        begin(conn, "http://127.0.0.1:34813").unwrap();
        db::setting(conn, PENDING_STATE).unwrap()
    }

    #[test]
    fn a_started_sign_in_outlives_whatever_started_it() {
        // The phone's browser replaces this app on screen and Android is free to
        // stop it while it is away. Nothing held in memory survives that.
        let c = conn();
        let state = started(&c);

        assert!(account(&c).pending, "the screen must be able to offer a way to finish");
        assert!(db::setting(&c, PENDING_VERIFIER).is_some());
        assert_eq!(db::setting(&c, PENDING_REDIRECT).unwrap(), "http://127.0.0.1:34813");
        assert!(!state.is_empty());
    }

    #[test]
    fn the_address_from_a_failed_page_still_finishes_the_sign_in() {
        // Every way the redirect can fail — the app stopped, the port closed, the
        // network switched under Chrome — fails after Google has handed the code
        // over. It is in the address bar of the error page.
        let c = conn();
        let state = started(&c);

        let pasted = format!("http://127.0.0.1:34813/?state={state}&code=4%2F0Axyz&scope=drive.file");
        let (code, verifier, redirect) = claim(&c, &pasted).unwrap();

        assert_eq!(code, "4/0Axyz");
        assert!(!verifier.is_empty());
        assert_eq!(redirect, "http://127.0.0.1:34813");
        assert!(!account(&c).pending, "a claimed sign-in must not be offered twice");
    }

    #[test]
    fn a_bare_query_is_accepted_too() {
        // What the listener hands over, as opposed to what a person pastes.
        let c = conn();
        let state = started(&c);
        let (code, _, _) = claim(&c, &format!("state={state}&code=abc")).unwrap();
        assert_eq!(code, "abc");
    }

    #[test]
    fn an_answer_from_another_sign_in_leaves_this_one_open() {
        let c = conn();
        started(&c);

        let err = claim(&c, "http://127.0.0.1:34813/?state=elsewhere&code=abc").unwrap_err();
        assert!(err.contains("did not match"), "{err}");
        assert!(
            account(&c).pending,
            "refusing one paste must not throw away the sign-in — the right address may follow"
        );
    }

    #[test]
    fn pasting_something_that_is_not_an_answer_says_what_to_do() {
        let c = conn();
        started(&c);
        let err = claim(&c, "https://accounts.google.com/").unwrap_err();
        assert!(err.contains("Copy the whole address"), "{err}");
    }

    #[test]
    fn finishing_without_starting_is_refused() {
        let c = conn();
        set_client(&c, "x.apps.googleusercontent.com", "s").unwrap();
        let err = claim(&c, "http://127.0.0.1:1/?code=abc&state=x").unwrap_err();
        assert!(err.contains("No sign-in is waiting"), "{err}");
    }

    #[test]
    fn disconnecting_abandons_a_half_finished_sign_in() {
        let c = conn();
        started(&c);
        disconnect(&c).unwrap();
        assert!(!account(&c).pending);
    }

    #[test]
    fn browser_noise_is_not_mistaken_for_the_answer() {
        // A browser opens sockets nobody asked for. Treating the first as the
        // redirect leaves the real one with nothing listening, which is what made
        // the phone sign-in hang while the code sat unclaimed.
        assert!(code_from_query("", "st").is_none());
        assert!(code_from_query("favicon=1", "st").is_none());
    }

    #[test]
    fn a_redirect_from_a_different_sign_in_is_refused() {
        let out = code_from_query("code=abc&state=somebody-else", "mine").unwrap();
        assert!(out.unwrap_err().contains("did not match"));
    }

    #[test]
    fn a_declined_sign_in_says_so_plainly() {
        let out = code_from_query("error=access_denied&state=st", "st").unwrap();
        assert_eq!(out.unwrap_err(), "Sign-in was declined.");
    }

    #[test]
    fn the_code_comes_back_decoded() {
        // Google's codes contain a slash, which arrives percent-encoded.
        let out = code_from_query("code=4%2F0Axyz&state=st", "st").unwrap();
        assert_eq!(out.unwrap(), "4/0Axyz");
    }

    #[test]
    fn query_values_come_back_decoded() {
        assert_eq!(decode_component("4%2F0Ab_c-d"), "4/0Ab_c-d");
        assert_eq!(decode_component("one+two"), "one two");
        assert_eq!(decode_component("plain"), "plain");
    }

    #[test]
    fn a_client_id_that_is_not_one_is_refused_before_a_browser_opens() {
        let c = conn();
        let err = set_client(&c, "12345", "secret").unwrap_err();
        assert!(err.contains("apps.googleusercontent.com"), "{err}");
        assert!(db::setting(&c, CLIENT_ID).is_none(), "nothing should be stored");
    }

    #[test]
    fn a_client_id_is_kept_and_reported_as_configured() {
        let c = conn();
        set_client(&c, "123-abc.apps.googleusercontent.com", "shh").unwrap();

        let a = account(&c);
        assert!(a.configured);
        assert!(!a.connected, "having a client id is not being signed in");
        assert_eq!(a.email, None);
    }

    #[test]
    fn the_refresh_token_is_never_stored_as_it_was_given() {
        let c = conn();
        let sealed = secret::protect(b"1//real-refresh-token").unwrap();
        db::set_setting(&c, REFRESH_TOKEN, &base64_encode(&sealed)).unwrap();

        let stored = db::setting(&c, REFRESH_TOKEN).unwrap();
        assert!(!stored.contains("real-refresh-token"), "it must not be readable in the row");
        assert_eq!(refresh_token(&c).unwrap(), "1//real-refresh-token");
    }

    #[test]
    fn disconnecting_forgets_the_token_the_email_and_the_upload_map() {
        let c = conn();
        let sealed = secret::protect(b"token").unwrap();
        db::set_setting(&c, REFRESH_TOKEN, &base64_encode(&sealed)).unwrap();
        db::set_setting(&c, ACCOUNT_EMAIL, "someone@example.com").unwrap();
        c.execute(
            "INSERT INTO drive_file (rel_path, file_id, uploaded_at) VALUES ('a.jpg', 'id', datetime('now'))",
            [],
        )
        .unwrap();

        disconnect(&c).unwrap();

        let a = account(&c);
        assert!(!a.connected);
        assert_eq!(a.email, None);
        let left: i64 = c.query_row("SELECT count(*) FROM drive_file", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "those file ids belong to the account that was just forgotten");
    }

    #[test]
    fn a_cached_access_token_is_reused_until_it_is_nearly_spent() {
        let c = conn();
        cache_access_token(&c, "ya29.fresh", 3600);
        assert_eq!(access_token(&c).unwrap(), "ya29.fresh");
    }

    #[test]
    fn an_expired_access_token_is_not_handed_out() {
        let c = conn();
        db::set_setting(&c, "google_access_token", "ya29.stale").unwrap();
        db::set_setting(&c, "google_access_expiry", &(now_secs() - 10).to_string()).unwrap();

        // With no refresh token stored, the only honest answer is to say so
        // rather than return the stale one.
        let err = access_token(&c).unwrap_err();
        assert!(err.contains("Not connected"), "{err}");
    }

    #[test]
    fn google_errors_are_translated_into_something_actionable() {
        assert!(explain_token_error("{\"error\":\"invalid_client\"}").contains("client ID"));
        assert!(explain_token_error("{\"error\":\"redirect_uri_mismatch\"}").contains("Desktop app"));
        assert!(explain_token_error("{\"error\":\"access_denied\"}").contains("declined"));
    }
}
