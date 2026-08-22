use std::path::Path;

fn main() {
    bake_google_client();
    tauri_build::build()
}

/// Compile the Google OAuth client into the binary, if one is configured here.
///
/// It is read from `google-client.env` beside this file, which is not in the
/// repository and never will be — a client ID committed to a public repo gets
/// used by strangers against the quota, and shows their app's name on somebody
/// else's consent screen. Keeping it out of the source but inside the build is
/// what lets the phone say "choose an account" instead of asking for credentials
/// nobody wants to type on a phone keyboard.
///
/// Without the file the app behaves exactly as before: it asks for a client, and
/// what the user enters is stored in their own database.
fn bake_google_client() {
    let path = Path::new("google-client.env");
    println!("cargo:rerun-if-changed=google-client.env");
    println!("cargo:rerun-if-env-changed=MRT_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=MRT_GOOGLE_CLIENT_SECRET");

    // An environment variable wins, so a CI build can supply its own.
    for key in ["MRT_GOOGLE_CLIENT_ID", "MRT_GOOGLE_CLIENT_SECRET"] {
        if let Ok(value) = std::env::var(key) {
            println!("cargo:rustc-env={key}={value}");
        }
    }

    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if !matches!(key, "MRT_GOOGLE_CLIENT_ID" | "MRT_GOOGLE_CLIENT_SECRET") {
            continue;
        }
        if std::env::var(key).is_err() {
            println!("cargo:rustc-env={key}={}", value.trim().trim_matches('"'));
        }
    }
}
