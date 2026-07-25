//! Optional encryption-at-rest for clipboard history.
//!
//! # Threat model
//!
//! This protects clipboard history from **another process or user reading the
//! SQLite file off disk** — a synced backup, a shared machine, a stolen laptop
//! with the disk mounted elsewhere. It does *not* protect against malware
//! running as you while Caduceus is unlocked, because at that point the attacker
//! can ask the OS keychain for the key exactly like Caduceus does.
//!
//! # Construction
//!
//! ChaCha20-Poly1305 (AEAD) with a 256-bit key from the OS keychain and a fresh
//! random 96-bit nonce per record. Records are stored as `nonce || ciphertext`,
//! where the Poly1305 tag is appended to the ciphertext by the AEAD.
//!
//! A random nonce per record is safe here because the key is per-machine and
//! the number of records is bounded by `max_items` — nowhere near the ~2^32
//! records where birthday collisions on a 96-bit nonce start to matter.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use thiserror::Error;

pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("could not generate random bytes: {0}")]
    Random(String),
    #[error("encryption failed")]
    Encrypt,
    /// Wrong key, truncated record, or tampering — deliberately not
    /// distinguished, since telling them apart leaks information.
    #[error("could not decrypt this entry (the encryption key may have changed)")]
    Decrypt,
    #[error("record is too short to be a valid encrypted entry")]
    Malformed,
}

pub type CryptoResult<T> = Result<T, CryptoError>;

fn cipher(key: &[u8; KEY_LEN]) -> ChaCha20Poly1305 {
    // Both lengths are fixed by the array types, so neither conversion can fail.
    ChaCha20Poly1305::new(&Key::from(*key))
}

/// Encrypt a record. Output layout: `nonce (12 bytes) || ciphertext || tag`.
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> CryptoResult<Vec<u8>> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|e| CryptoError::Random(e.to_string()))?;
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher(key)
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a record produced by [`encrypt`].
pub fn decrypt(key: &[u8; KEY_LEN], record: &[u8]) -> CryptoResult<Vec<u8>> {
    if record.len() <= NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce_bytes, ciphertext) = record.split_at(NONCE_LEN);
    let nonce = Nonce::try_from(nonce_bytes).map_err(|_| CryptoError::Malformed)?;
    cipher(key)
        .decrypt(&nonce, ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

/// Encrypt a UTF-8 string.
pub fn encrypt_str(key: &[u8; KEY_LEN], s: &str) -> CryptoResult<Vec<u8>> {
    encrypt(key, s.as_bytes())
}

/// Decrypt to a UTF-8 string, replacing invalid sequences rather than failing:
/// a preview that renders with a replacement character is more useful than an
/// error row.
pub fn decrypt_str(key: &[u8; KEY_LEN], record: &[u8]) -> CryptoResult<String> {
    Ok(String::from_utf8_lossy(&decrypt(key, record)?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_LEN] {
        [7u8; KEY_LEN]
    }

    #[test]
    fn round_trips() {
        let key = test_key();
        let msg = b"the quick brown fox";
        let ct = encrypt(&key, msg).unwrap();
        assert_ne!(&ct[NONCE_LEN..], msg.as_slice());
        assert_eq!(decrypt(&key, &ct).unwrap(), msg);
    }

    #[test]
    fn nonce_is_fresh_per_record() {
        let key = test_key();
        let a = encrypt(&key, b"same").unwrap();
        let b = encrypt(&key, b"same").unwrap();
        assert_ne!(a, b, "identical plaintexts must not produce identical records");
    }

    #[test]
    fn wrong_key_fails_closed() {
        let ct = encrypt(&test_key(), b"secret").unwrap();
        assert!(matches!(decrypt(&[9u8; KEY_LEN], &ct), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn tampering_is_detected() {
        let key = test_key();
        let mut ct = encrypt(&key, b"secret").unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0x01;
        assert!(decrypt(&key, &ct).is_err());
    }

    #[test]
    fn short_records_are_rejected() {
        assert!(matches!(
            decrypt(&test_key(), &[0u8; NONCE_LEN]),
            Err(CryptoError::Malformed)
        ));
    }

    #[test]
    fn round_trips_unicode() {
        let key = test_key();
        let s = "caf\u{e9} \u{1f30d} \u{4f60}\u{597d}";
        let ct = encrypt_str(&key, s).unwrap();
        assert_eq!(decrypt_str(&key, &ct).unwrap(), s);
    }
}
