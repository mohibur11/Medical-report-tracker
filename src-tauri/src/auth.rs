//! The app lock.
//!
//! BE CLEAR ABOUT WHAT THIS IS. The vault is plaintext by design — browsable in
//! Explorer, copyable to a USB stick, readable without this app ever running
//! again. That portability is the whole point of requirement 3.2, and encrypting
//! the files would destroy it.
//!
//! So this password is an honest screen lock: it stops someone idly opening the
//! app on an unlocked machine. It does NOT protect the scans, because anyone with
//! access to the Windows account can open the vault folder directly. Real
//! protection at rest is BitLocker, and the UI says so rather than implying more.
//!
//! What it is still worth doing properly: Argon2id rather than a fast hash, so
//! that a copied database cannot be brute-forced at GPU speed, and constant-time
//! verification so the check itself leaks nothing.
//!
//! The `wrapped_dek` and `dek_dpapi` columns exist in the schema from migration
//! 001 and stay NULL. They are there so that adding encryption later is a code
//! change, not a data migration. Choosing an AEAD and a recovery-code scheme is a
//! decision worth making when encryption actually ships, not guessed at now.

use argon2::{Algorithm, Argon2, Params, Version};
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand_core::OsRng;
use rusqlite::{params, Connection};

/// 46 MiB, one pass, one lane. Memory cost is what defeats GPU cracking; the
/// parameters live in the stored PHC string, so raising them later re-hashes on
/// next unlock rather than invalidating anything.
const MEMORY_KIB: u32 = 46 * 1024;
const ITERATIONS: u32 = 1;
const PARALLELISM: u32 = 1;

fn hasher() -> Result<Argon2<'static>, String> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, None)
        .map_err(|e| format!("bad argon2 parameters: {e}"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Has the user set a password? False means the app opens unlocked.
pub fn is_enabled(conn: &Connection) -> Result<bool, String> {
    let hash: Option<String> = conn
        .query_row("SELECT pw_hash FROM users ORDER BY created_at LIMIT 1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(hash.map(|h| !h.is_empty()).unwrap_or(false))
}

pub fn email(conn: &Connection) -> Result<String, String> {
    conn.query_row("SELECT email FROM users ORDER BY created_at LIMIT 1", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

/// Set or replace the password.
///
/// `email` is stored but decorative today. It is collected now because it is the
/// join key if accounts ever become real, and it is never used as a primary key.
pub fn set_password(
    conn: &Connection,
    user_id: &str,
    email_addr: &str,
    password: &str,
) -> Result<(), String> {
    if password.chars().count() < 8 {
        return Err("Use at least 8 characters.".into());
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = hasher()?
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| format!("cannot hash password: {e}"))?
        .to_string();

    conn.execute(
        "UPDATE users SET pw_hash = ?2, email = ?3, updated_at = datetime('now') WHERE id = ?1",
        params![user_id, hash, email_addr.trim()],
    )
    .map_err(|e| format!("cannot save password: {e}"))?;
    Ok(())
}

/// Verify a password in constant time.
pub fn verify(conn: &Connection, password: &str) -> Result<bool, String> {
    let stored: String = conn
        .query_row("SELECT pw_hash FROM users ORDER BY created_at LIMIT 1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;

    if stored.is_empty() {
        // No password set — nothing to unlock.
        return Ok(true);
    }

    let parsed = PasswordHash::new(&stored).map_err(|e| format!("stored hash is unreadable: {e}"))?;
    Ok(hasher()?
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Change the password, requiring the current one.
pub fn change_password(
    conn: &Connection,
    user_id: &str,
    current: &str,
    next: &str,
) -> Result<(), String> {
    if !verify(conn, current)? {
        return Err("That is not the current password.".into());
    }
    let email_addr = email(conn)?;
    set_password(conn, user_id, &email_addr, next)
}

/// Remove the lock. Requires the current password — otherwise anyone who reached
/// an unlocked session could quietly disable it.
pub fn disable(conn: &Connection, user_id: &str, current: &str) -> Result<(), String> {
    if !verify(conn, current)? {
        return Err("That is not the current password.".into());
    }
    conn.execute(
        "UPDATE users SET pw_hash = '', updated_at = datetime('now') WHERE id = ?1",
        params![user_id],
    )
    .map_err(|e| format!("cannot remove password: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        dir: std::path::PathBuf,
        conn: Connection,
        user: String,
    }

    impl Fx {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("mrt-auth-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(&dir).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            Fx { dir, conn, user }
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn a_fresh_install_is_unlocked() {
        let f = Fx::new();
        assert!(!is_enabled(&f.conn).unwrap());
        // With no password set, verification cannot be a gate.
        assert!(verify(&f.conn, "anything").unwrap());
    }

    #[test]
    fn a_set_password_verifies_and_a_wrong_one_does_not() {
        let f = Fx::new();
        set_password(&f.conn, &f.user, "someone@example.com", "correct horse battery").unwrap();

        assert!(is_enabled(&f.conn).unwrap());
        assert!(verify(&f.conn, "correct horse battery").unwrap());
        assert!(!verify(&f.conn, "correct horse batteryy").unwrap());
        assert!(!verify(&f.conn, "").unwrap());
    }

    #[test]
    fn the_password_is_never_stored_in_a_recoverable_form() {
        let f = Fx::new();
        let secret = "correct horse battery";
        set_password(&f.conn, &f.user, "a@b.c", secret).unwrap();

        let stored: String = f.conn
            .query_row("SELECT pw_hash FROM users", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains(secret));
        assert!(stored.starts_with("$argon2id$"), "expected an Argon2id PHC string, got {stored}");
    }

    #[test]
    fn the_stored_parameters_are_memory_hard() {
        // A fast hash would make a copied database trivially crackable, which is
        // the one thing a local password can actually resist.
        let f = Fx::new();
        set_password(&f.conn, &f.user, "a@b.c", "correct horse battery").unwrap();
        let stored: String = f.conn.query_row("SELECT pw_hash FROM users", [], |r| r.get(0)).unwrap();
        assert!(stored.contains(&format!("m={MEMORY_KIB}")), "got {stored}");
        assert!(stored.contains(&format!("t={ITERATIONS}")));
    }

    #[test]
    fn two_users_with_the_same_password_get_different_hashes() {
        let f = Fx::new();
        set_password(&f.conn, &f.user, "a@b.c", "correct horse battery").unwrap();
        let first: String = f.conn.query_row("SELECT pw_hash FROM users", [], |r| r.get(0)).unwrap();

        set_password(&f.conn, &f.user, "a@b.c", "correct horse battery").unwrap();
        let second: String = f.conn.query_row("SELECT pw_hash FROM users", [], |r| r.get(0)).unwrap();

        assert_ne!(first, second, "a fresh salt must be generated each time");
    }

    #[test]
    fn short_passwords_are_refused() {
        let f = Fx::new();
        assert!(set_password(&f.conn, &f.user, "a@b.c", "short").unwrap_err().contains("8 characters"));
    }

    #[test]
    fn changing_the_password_requires_the_current_one() {
        let f = Fx::new();
        set_password(&f.conn, &f.user, "a@b.c", "first password").unwrap();

        assert!(change_password(&f.conn, &f.user, "wrong", "second password")
            .unwrap_err().contains("not the current password"));
        assert!(verify(&f.conn, "first password").unwrap(), "a failed change must not alter anything");

        change_password(&f.conn, &f.user, "first password", "second password").unwrap();
        assert!(verify(&f.conn, "second password").unwrap());
        assert!(!verify(&f.conn, "first password").unwrap());
    }

    #[test]
    fn removing_the_lock_requires_the_current_password() {
        let f = Fx::new();
        set_password(&f.conn, &f.user, "a@b.c", "first password").unwrap();

        assert!(disable(&f.conn, &f.user, "wrong").is_err());
        assert!(is_enabled(&f.conn).unwrap(), "a failed attempt must leave the lock on");

        disable(&f.conn, &f.user, "first password").unwrap();
        assert!(!is_enabled(&f.conn).unwrap());
    }

    #[test]
    fn the_email_is_stored_but_is_not_the_identity() {
        let f = Fx::new();
        set_password(&f.conn, &f.user, "  someone@example.com  ", "correct horse battery").unwrap();
        assert_eq!(email(&f.conn).unwrap(), "someone@example.com");

        // The primary key is still the ULID it always was.
        let id: String = f.conn.query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(id, f.user);
        assert_eq!(id.len(), 26);
    }

    #[test]
    fn the_dek_columns_stay_null_because_nothing_is_encrypted_yet() {
        // They exist so adding encryption later is a code change, not a data
        // migration. Claiming otherwise would overstate what this lock does.
        let f = Fx::new();
        set_password(&f.conn, &f.user, "a@b.c", "correct horse battery").unwrap();
        let (dek, dpapi): (Option<Vec<u8>>, Option<Vec<u8>>) = f.conn
            .query_row("SELECT wrapped_dek, dek_dpapi FROM users", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(dek.is_none() && dpapi.is_none());
    }
}
