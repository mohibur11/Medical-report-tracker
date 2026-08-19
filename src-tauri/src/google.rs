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
const REFRESH_TOKEN: &str = "google_refresh_token";
const ACCOUNT_EMAIL: &str = "google_account_email";

/// How long the browser half of the flow may take before the listener gives up.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: Option<String>,
    pub connected: bool,
    /// False until an OAuth client id has been configured.
    pub configured: bool,
}

pub fn account(conn: &Connection) -> Account {
    Account {
        email: db::setting(conn, ACCOUNT_EMAIL),
        connected: db::setting(conn, REFRESH_TOKEN).is_some(),
        configured: db::setting(conn, CLIENT_ID).is_some(),
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
        conn.execute("DELETE FROM app_setting WHERE key = ?1", rusqlite::params![key])
            .map_err(|e| format!("cannot forget the account: {e}"))?;
    }
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
}

/// What a completed sign-in yields, before any of it is stored.
pub struct Tokens {
    pub refresh: String,
    pub access: String,
    pub expires_in: u64,
    pub email: Option<String>,
}

/// The sign-in itself: open the browser, wait for the answer, exchange it.
///
/// Takes no database handle on purpose. This blocks for as long as the person
/// takes to choose an account and press Allow, and holding the connection across
/// that would freeze every other part of the app while they did it.
pub fn run_flow(client_id: &str, client_secret: &str) -> Result<Tokens, String> {

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

    open_in_browser(&url)?;

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
    let id = db::setting(conn, CLIENT_ID)
        .ok_or("No Google client ID has been set. Add one in the Google Drive settings.")?;
    Ok((id, db::setting(conn, CLIENT_SECRET).unwrap_or_default()))
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
                return handle_redirect(stream, expected_state);
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

fn handle_redirect(mut stream: std::net::TcpStream, expected_state: &str) -> Result<String, String> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| format!("cannot read the reply: {e}"))?,
    );
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| format!("cannot read the reply: {e}"))?;

    // "GET /?code=...&state=... HTTP/1.1"
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();

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

    let outcome = if let Some(e) = error.as_deref() {
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

    outcome
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

fn open_in_browser(url: &str) -> Result<(), String> {
    // rundll32 rather than `start`, which is a shell builtin and would need cmd
    // with its own quoting rules around a URL full of ampersands.
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot open the browser: {e}"))
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
