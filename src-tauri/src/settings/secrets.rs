//! OS keychain access for everything Orbit must never write to disk.
//!
//! Two kinds of secret live here:
//!
//! * **Provider API keys** — one entry per configured backend, keyed by backend
//!   id, plus one for the speech-to-text endpoint.
//! * **The clipboard encryption key** — 32 random bytes, generated on first use
//!   and never exported.
//!
//! Backends are: macOS Keychain, Windows Credential Manager, and the
//! freedesktop Secret Service (GNOME Keyring / KWallet) on Linux. On a headless
//! Linux box with no Secret Service running, every call here fails; callers
//! must degrade gracefully rather than panic (see [`SecretError`]).

use base64::Engine as _;
use keyring::Entry;
use thiserror::Error;

/// Keychain "service" name. Shows up in Keychain Access as the item's service.
const SERVICE: &str = "com.orbit.desktop";

/// Account name for the clipboard encryption key.
const CLIPBOARD_KEY_ACCOUNT: &str = "clipboard-encryption-key";

/// Account name for the speech-to-text endpoint key.
const STT_KEY_ACCOUNT: &str = "stt-api-key";

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("no OS keychain is available on this system: {0}")]
    Unavailable(String),
    #[error("keychain entry not found")]
    NotFound,
    #[error("keychain error: {0}")]
    Other(String),
}

impl From<keyring::Error> for SecretError {
    fn from(e: keyring::Error) -> Self {
        match e {
            keyring::Error::NoEntry => SecretError::NotFound,
            keyring::Error::PlatformFailure(inner) => SecretError::Unavailable(inner.to_string()),
            other => SecretError::Other(other.to_string()),
        }
    }
}

pub type SecretResult<T> = Result<T, SecretError>;

fn entry(account: &str) -> SecretResult<Entry> {
    Entry::new(SERVICE, account).map_err(SecretError::from)
}

/// Keychain account name for a backend's API key.
fn backend_account(backend_id: &str) -> String {
    format!("backend.{backend_id}.api_key")
}

// ---------------------------------------------------------------------------
// Provider API keys
// ---------------------------------------------------------------------------

pub fn set_backend_api_key(backend_id: &str, key: &str) -> SecretResult<()> {
    if key.is_empty() {
        return delete_backend_api_key(backend_id);
    }
    entry(&backend_account(backend_id))?
        .set_password(key)
        .map_err(SecretError::from)
}

pub fn get_backend_api_key(backend_id: &str) -> SecretResult<String> {
    entry(&backend_account(backend_id))?
        .get_password()
        .map_err(SecretError::from)
}

/// Convenience for call sites that treat "no key" and "keychain broken" the
/// same way: both mean "send the request without an Authorization header",
/// which is exactly right for local model servers.
pub fn get_backend_api_key_opt(backend_id: &str) -> Option<String> {
    match get_backend_api_key(backend_id) {
        Ok(k) if !k.is_empty() => Some(k),
        Ok(_) => None,
        Err(SecretError::NotFound) => None,
        Err(e) => {
            log::warn!("could not read API key for backend {backend_id}: {e}");
            None
        }
    }
}

pub fn delete_backend_api_key(backend_id: &str) -> SecretResult<()> {
    match entry(&backend_account(backend_id))?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::from(e)),
    }
}

pub fn has_backend_api_key(backend_id: &str) -> bool {
    matches!(get_backend_api_key(backend_id), Ok(k) if !k.is_empty())
}

// ---------------------------------------------------------------------------
// Speech-to-text key
// ---------------------------------------------------------------------------

pub fn set_stt_api_key(key: &str) -> SecretResult<()> {
    if key.is_empty() {
        return match entry(STT_KEY_ACCOUNT)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::from(e)),
        };
    }
    entry(STT_KEY_ACCOUNT)?
        .set_password(key)
        .map_err(SecretError::from)
}

pub fn get_stt_api_key_opt() -> Option<String> {
    match entry(STT_KEY_ACCOUNT).and_then(|e| e.get_password().map_err(SecretError::from)) {
        Ok(k) if !k.is_empty() => Some(k),
        _ => None,
    }
}

pub fn has_stt_api_key() -> bool {
    get_stt_api_key_opt().is_some()
}

// ---------------------------------------------------------------------------
// Clipboard encryption key
// ---------------------------------------------------------------------------

/// Fetch the clipboard encryption key, generating and storing one on first use.
///
/// The key is stored base64-encoded because the keychain APIs are string-typed
/// on every platform.
///
/// **Losing this key makes existing encrypted history unreadable.** That is the
/// intended behaviour: the whole point of the toggle is that history is
/// worthless without the key material, and Orbit deliberately provides no
/// escrow or export path for it.
pub fn get_or_create_clipboard_key() -> SecretResult<[u8; 32]> {
    let e = entry(CLIPBOARD_KEY_ACCOUNT)?;
    match e.get_password() {
        Ok(encoded) => decode_key(&encoded),
        Err(keyring::Error::NoEntry) => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key)
                .map_err(|err| SecretError::Other(format!("CSPRNG unavailable: {err}")))?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(key);
            e.set_password(&encoded)?;
            Ok(key)
        }
        Err(err) => Err(SecretError::from(err)),
    }
}

/// Read the key without creating one. Used when decrypting old rows: if the key
/// is gone, those rows are simply unreadable and get surfaced as such.
pub fn get_clipboard_key_opt() -> Option<[u8; 32]> {
    let encoded = entry(CLIPBOARD_KEY_ACCOUNT).ok()?.get_password().ok()?;
    decode_key(&encoded).ok()
}

/// Destroy the clipboard key. Called when the user turns encryption off *after*
/// history has been decrypted back to plaintext.
pub fn delete_clipboard_key() -> SecretResult<()> {
    match entry(CLIPBOARD_KEY_ACCOUNT)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::from(e)),
    }
}

fn decode_key(encoded: &str) -> SecretResult<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| SecretError::Other(format!("stored key is not valid base64: {e}")))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| SecretError::Other("stored key is not 32 bytes".into()))
}

/// Probe whether a keychain is usable at all, so the UI can warn up front on
/// systems (headless Linux, mainly) where secret storage will not work.
pub fn keychain_available() -> bool {
    match Entry::new(SERVICE, "orbit-availability-probe") {
        Ok(e) => !matches!(
            e.get_password(),
            Err(keyring::Error::PlatformFailure(_)) | Err(keyring::Error::NoStorageAccess(_))
        ),
        Err(_) => false,
    }
}
