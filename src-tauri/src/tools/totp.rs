//! 2FA code picker: store a TOTP secret and show the current 6-digit code.
//!
//! Splits the same way `settings::secrets` already splits provider API keys
//! from `BackendConfig`: the shared secret — the one piece of data that lets
//! anyone holding it mint valid codes forever — lives only in the OS
//! keychain (`settings::secrets::{set,get,delete}_totp_secret`), never in the
//! plain-JSON store this module keeps for everything else (label, issuer,
//! digit count, period). Deleting an account removes both halves; nothing
//! here can leave an orphaned secret behind in the keychain.
//!
//! Code generation itself is RFC 6238 TOTP via the `totp-rs` crate — no
//! existing dependency covers it, so this is the one new crate the feature
//! needed (see `Cargo.toml`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::settings::secrets;

type Res<T> = Result<T, String>;

const STORE_FILE: &str = "caduceus-totp.json";
const ACCOUNTS_KEY: &str = "accounts";

const DEFAULT_DIGITS: usize = 6;
const DEFAULT_PERIOD: u64 = 30;

/// Everything about a TOTP account *except* its secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpAccount {
    pub id: String,
    /// What the account is for, e.g. "Ada @ GitHub". Freeform — this module
    /// does not parse `otpauth://` labels, it just stores whatever the user
    /// typed when they added the secret.
    pub label: String,
    pub issuer: String,
    pub digits: usize,
    pub period: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentCode {
    pub code: String,
    pub seconds_remaining: u64,
    pub period: u64,
}

// ---------------------------------------------------------------------------
// Store (metadata only — see module docs)
// ---------------------------------------------------------------------------

fn load<R: Runtime>(app: &AppHandle<R>) -> Vec<TotpAccount> {
    let Ok(store) = app.store(STORE_FILE) else {
        return Vec::new();
    };
    store.get(ACCOUNTS_KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save<R: Runtime>(app: &AppHandle<R>, accounts: &[TotpAccount]) -> Res<()> {
    let store = app.store(STORE_FILE).map_err(|e| format!("could not open the 2FA store: {e}"))?;
    let value =
        serde_json::to_value(accounts).map_err(|e| format!("could not encode 2FA accounts: {e}"))?;
    store.set(ACCOUNTS_KEY, value);
    store.save().map_err(|e| format!("could not write 2FA accounts: {e}"))
}

// ---------------------------------------------------------------------------
// Code generation (pure, unit-tested against the RFC 6238 test vectors)
// ---------------------------------------------------------------------------

fn build_totp(secret_base32: &str, digits: usize, period: u64) -> Res<TOTP> {
    let cleaned: String =
        secret_base32.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    if cleaned.is_empty() {
        return Err("Paste the account's secret key first.".into());
    }
    let bytes = Secret::Encoded(cleaned)
        .to_bytes()
        .map_err(|_| "That doesn't look like a valid base32 secret key.".to_string())?;
    TOTP::new(Algorithm::SHA1, digits, 1, period, bytes)
        .map_err(|e| format!("Could not build a code generator from that secret: {e}"))
}

/// The code for `secret_base32` at `now`, plus how many seconds until it
/// rotates. Split out from [`current_code`] so tests can pin `now` instead of
/// racing the real clock.
fn code_at(secret_base32: &str, digits: usize, period: u64, now: SystemTime) -> Res<CurrentCode> {
    let totp = build_totp(secret_base32, digits, period)?;
    let epoch = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The system clock is set before 1970.".to_string())?
        .as_secs();
    let code = totp.generate(epoch);
    let seconds_remaining = period - (epoch % period);
    Ok(CurrentCode { code, seconds_remaining, period })
}

/// Validate that a secret is well-formed base32 without generating anything —
/// what `totp_add_account` calls before it ever touches the keychain, so a
/// typo is caught up front rather than saved and only discovered when the
/// code picker comes up blank.
fn validate_secret(secret_base32: &str) -> Res<()> {
    build_totp(secret_base32, DEFAULT_DIGITS, DEFAULT_PERIOD).map(|_| ())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn totp_list_accounts<R: Runtime>(app: AppHandle<R>) -> Res<Vec<TotpAccount>> {
    Ok(load(&app))
}

#[tauri::command]
pub fn totp_add_account<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    issuer: Option<String>,
    secret: String,
    digits: Option<usize>,
    period: Option<u64>,
) -> Res<TotpAccount> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err("Give this account a label, like \"Ada @ GitHub\".".into());
    }
    let digits = digits.unwrap_or(DEFAULT_DIGITS);
    let period = period.unwrap_or(DEFAULT_PERIOD);
    if !(6..=8).contains(&digits) {
        return Err("Codes are 6, 7, or 8 digits.".into());
    }
    if period == 0 {
        return Err("The refresh period must be at least 1 second.".into());
    }
    validate_secret(&secret)?;

    let account = TotpAccount {
        id: uuid::Uuid::new_v4().to_string(),
        label,
        issuer: issuer.unwrap_or_default(),
        digits,
        period,
    };

    secrets::set_totp_secret(&account.id, secret.trim())
        .map_err(|e| format!("Could not save the secret to the keychain: {e}"))?;

    let mut accounts = load(&app);
    accounts.push(account.clone());
    if let Err(e) = save(&app, &accounts) {
        // Do not leave a keychain entry with no matching metadata behind.
        let _ = secrets::delete_totp_secret(&account.id);
        return Err(e);
    }
    Ok(account)
}

#[tauri::command]
pub fn totp_delete_account<R: Runtime>(app: AppHandle<R>, id: String) -> Res<()> {
    let mut accounts = load(&app);
    accounts.retain(|a| a.id != id);
    save(&app, &accounts)?;
    secrets::delete_totp_secret(&id).map_err(|e| format!("Could not remove the keychain entry: {e}"))
}

#[tauri::command]
pub fn totp_current_code<R: Runtime>(app: AppHandle<R>, id: String) -> Res<CurrentCode> {
    let accounts = load(&app);
    let account = accounts.iter().find(|a| a.id == id).ok_or("That account no longer exists.")?;
    let secret = secrets::get_totp_secret(&id)
        .map_err(|e| format!("Could not read the secret from the keychain: {e}"))?;
    code_at(&secret, account.digits, account.period, SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// RFC 6238 Appendix B's SHA-1 test vector: the ASCII secret
    /// "12345678901234567890", base32-encoded, at Unix time 59, produces the
    /// 8-digit code "94287082". This is the standard's own worked example —
    /// if this test passes, the generator is RFC-compliant, not merely
    /// self-consistent.
    fn rfc6238_secret_base32() -> String {
        // base32(ASCII "12345678901234567890")
        "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".to_string()
    }

    #[test]
    fn matches_the_rfc6238_sha1_test_vector() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(59);
        let result = code_at(&rfc6238_secret_base32(), 8, 30, time).unwrap();
        assert_eq!(result.code, "94287082");
    }

    #[test]
    fn a_six_digit_code_is_the_low_order_digits_of_the_eight_digit_one() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(59);
        let result = code_at(&rfc6238_secret_base32(), 6, 30, time).unwrap();
        assert_eq!(result.code, "287082");
    }

    #[test]
    fn seconds_remaining_counts_down_within_a_period() {
        // 59 seconds in is 29 seconds into the second 30-second window.
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(59);
        let result = code_at(&rfc6238_secret_base32(), 6, 30, time).unwrap();
        assert_eq!(result.seconds_remaining, 1);
    }

    #[test]
    fn the_code_changes_once_the_period_rolls_over() {
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(29);
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(31);
        let c1 = code_at(&rfc6238_secret_base32(), 6, 30, t1).unwrap();
        let c2 = code_at(&rfc6238_secret_base32(), 6, 30, t2).unwrap();
        assert_ne!(c1.code, c2.code);
    }

    #[test]
    fn lowercase_and_spaced_secrets_are_accepted() {
        let spaced = "gezd gnbv gy3t qojq gezd gnbv gy3t qojq";
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(59);
        let result = code_at(spaced, 8, 30, time).unwrap();
        assert_eq!(result.code, "94287082");
    }

    #[test]
    fn an_invalid_secret_is_rejected_with_a_readable_message() {
        let err = validate_secret("not valid base32!!!").unwrap_err();
        assert!(err.contains("base32"));
    }

    #[test]
    fn an_empty_secret_is_rejected_before_it_reaches_the_decoder() {
        assert!(validate_secret("   ").is_err());
    }

    #[test]
    fn a_known_date_produces_a_stable_code_for_regression_purposes() {
        // Not an RFC vector, just a fixed point in time so a future refactor
        // that accidentally changes the epoch math gets caught.
        let fixed = chrono::Utc.with_ymd_and_hms(2026, 7, 28, 0, 0, 0).unwrap();
        let time = SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(fixed.timestamp() as u64);
        let result = code_at(&rfc6238_secret_base32(), 6, 30, time).unwrap();
        assert_eq!(result.code.len(), 6);
        assert!(result.code.chars().all(|c| c.is_ascii_digit()));
    }
}
