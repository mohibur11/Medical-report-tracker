//! Keeping one secret out of a database that is deliberately not encrypted.
//!
//! Everything else in this app is plaintext on purpose: the vault has to stay
//! browsable, and the password is an honest screen lock rather than encryption.
//! A Google refresh token is different in kind. It is not the user's data, it is
//! standing permission to reach their Drive, and it stays valid until revoked —
//! so a copied database file should not carry one.
//!
//! DPAPI ties the ciphertext to the Windows user account. Copying the database to
//! another machine, or opening it as another user, yields nothing; the same user
//! on the same machine unwraps it without being asked for anything.

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(&mut input, None, None, None, None, 0, &mut output)
            .map_err(|e| format!("cannot protect the token: {e}"))?;

        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut _));
        Ok(bytes)
    }
}

#[cfg(windows)]
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let mut input = CRYPT_INTEGER_BLOB {
        cbData: sealed.len() as u32,
        pbData: sealed.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(&mut input, None, None, None, None, 0, &mut output)
            .map_err(|_| "the stored Google sign-in cannot be read on this account".to_string())?;

        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut _));
        Ok(bytes)
    }
}

/// Elsewhere this is a no-op, and the caller must not pretend otherwise.
#[cfg(not(windows))]
pub fn protect(_plain: &[u8]) -> Result<Vec<u8>, String> {
    Err("no secret store on this platform".into())
}

#[cfg(not(windows))]
pub fn unprotect(_sealed: &[u8]) -> Result<Vec<u8>, String> {
    Err("no secret store on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_survives_the_round_trip() {
        let token = b"1//0gLONG-looking-refresh-token_value";
        let sealed = protect(token).expect("this account should be able to protect data");
        assert_ne!(sealed.as_slice(), token, "it must not be stored as it was given");
        assert_eq!(unprotect(&sealed).unwrap(), token);
    }

    #[test]
    fn tampered_ciphertext_is_refused_rather_than_returning_rubbish() {
        let mut sealed = protect(b"refresh-token").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(unprotect(&sealed).is_err());
    }

    #[test]
    fn the_failure_message_says_what_to_do_about_it() {
        let err = unprotect(b"not protected at all").unwrap_err();
        assert!(err.contains("Google sign-in"), "{err}");
    }
}
