//! Clipboard history: a background watcher, a SQLite store, and optional
//! encryption at rest.
//!
//! ```text
//!   OS clipboard ──poll──▶ watcher ──▶ [dedupe / exclude / encrypt] ──▶ SQLite
//!                                                                        │
//!                            Command Center ◀──── search / pin / paste ──┘
//! ```

pub mod crypto;
pub mod store;
pub mod watcher;

pub use store::{ClipboardEntry, ClipboardStore, EntryKind, StoreError, TransitionReport};
pub use watcher::{WatcherHandle, CLIPBOARD_CHANGED_EVENT};

use crate::settings::{secrets, ClipboardSettings};

/// Filename of the history database inside the app data directory.
pub const DB_FILE: &str = "clipboard.db";

/// Resolve the encryption key to use for reads/writes right now.
///
/// Returns `Ok(None)` when encryption is off, which is the normal case and not
/// an error.
pub fn active_key(cfg: &ClipboardSettings) -> Result<Option<[u8; 32]>, String> {
    if !cfg.encrypt_at_rest {
        return Ok(None);
    }
    secrets::get_or_create_clipboard_key()
        .map(Some)
        .map_err(|e| format!("clipboard encryption is on but the key is unavailable: {e}"))
}

/// Write a history entry back to the system clipboard.
///
/// Used when the user picks something out of the palette. Runs on a scratch
/// `arboard::Clipboard` rather than the watcher's, so it cannot deadlock the
/// watcher thread.
pub fn copy_entry_to_clipboard(store: &ClipboardStore, id: i64, cfg: &ClipboardSettings) -> Result<(), String> {
    let key = active_key(cfg)?;
    let Some((kind, bytes)) = store
        .get_content(id, key.as_ref())
        .map_err(|e| e.to_string())?
    else {
        return Err("That clipboard entry no longer exists.".into());
    };

    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;

    match kind {
        EntryKind::Text | EntryKind::Files => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            clipboard
                .set_text(text)
                .map_err(|e| format!("could not write to the clipboard: {e}"))
        }
        EntryKind::Image => {
            let img = image::load_from_memory(&bytes)
                .map_err(|e| format!("stored image is corrupt: {e}"))?
                .to_rgba8();
            let (w, h) = (img.width() as usize, img.height() as usize);
            clipboard
                .set_image(arboard::ImageData {
                    width: w,
                    height: h,
                    bytes: std::borrow::Cow::Owned(img.into_raw()),
                })
                .map_err(|e| format!("could not write the image to the clipboard: {e}"))
        }
    }
}

/// Turn the encryption toggle on or off, migrating existing history.
///
/// * **Off → on:** create (or reuse) the keychain key and encrypt every
///   plaintext row.
/// * **On → off:** decrypt every row back to plaintext, then delete the key.
///
/// Rows that cannot be decrypted are dropped and counted; see
/// [`store::ClipboardStore::transition_encryption`].
pub fn set_encryption(store: &ClipboardStore, enable: bool) -> Result<TransitionReport, String> {
    if enable {
        let key = secrets::get_or_create_clipboard_key()
            .map_err(|e| format!("could not create an encryption key: {e}"))?;
        // Any already-encrypted rows were written with this same key, so it is
        // both the old and the new key.
        store
            .transition_encryption(Some(&key), Some(&key))
            .map_err(|e| e.to_string())
    } else {
        let old = secrets::get_clipboard_key_opt();
        let report = store
            .transition_encryption(None, old.as_ref())
            .map_err(|e| e.to_string())?;
        // Only remove the key once nothing depends on it any more.
        if let Err(e) = secrets::delete_clipboard_key() {
            log::warn!("history was decrypted but the key could not be removed: {e}");
        }
        Ok(report)
    }
}
