//! The background clipboard watcher.
//!
//! # Why polling
//!
//! macOS exposes clipboard changes only as a monotonically increasing
//! `changeCount` that you have to read; there is no notification. Windows has
//! `AddClipboardFormatListener` and Linux has selection ownership events, but
//! `arboard` — the cross-platform clipboard crate Orbit uses — does not surface
//! either. Rather than three platform-specific code paths plus a polling
//! fallback, Orbit polls on all three. At the default 700ms the cost is
//! unmeasurable (one clipboard read, and a hash only when the content changed).
//!
//! The interval is a setting, so anyone who wants 200ms responsiveness or 5s
//! frugality can have it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use arboard::Clipboard;
use image::ImageEncoder;

use super::store::{ClipboardStore, EntryKind, NewEntry};
use crate::settings::SettingsManager;

/// Longest side of the stored thumbnail, in pixels.
const THUMBNAIL_MAX: u32 = 220;

/// Characters kept in the searchable preview of a text entry.
const PREVIEW_CHARS: usize = 400;

/// Event emitted when a new entry lands, so an open Command Center updates live.
pub const CLIPBOARD_CHANGED_EVENT: &str = "orbit://clipboard-changed";

/// Handle used to stop the watcher (on quit, or when the user disables history).
#[derive(Clone, Default)]
pub struct WatcherHandle {
    stop: Arc<AtomicBool>,
}

impl WatcherHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Start the watcher on a dedicated OS thread.
///
/// A plain thread rather than a tokio task: `arboard::Clipboard` is not `Send`
/// on every platform and wants to live for the lifetime of the loop, and the
/// work is blocking I/O with a sleep, which is exactly what threads are for.
pub fn spawn<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    store: ClipboardStore,
    settings: SettingsManager,
) -> WatcherHandle {
    let handle = WatcherHandle::default();
    let stop_flag = handle.clone();

    std::thread::Builder::new()
        .name("orbit-clipboard".into())
        .spawn(move || {
            let mut clipboard = match Clipboard::new() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("clipboard is unavailable on this system, history disabled: {e}");
                    return;
                }
            };

            // Seed from the database so a restart does not re-capture whatever
            // happens to still be on the clipboard.
            let mut last_hash = store.latest_hash().unwrap_or_default();
            let mut prune_countdown: u32 = 0;

            loop {
                if stop_flag.should_stop() {
                    log::debug!("clipboard watcher stopping");
                    return;
                }

                let cfg = settings.with(|s| s.clipboard.clone());
                let interval = Duration::from_millis(cfg.poll_interval_ms);

                if !cfg.enabled {
                    std::thread::sleep(interval);
                    continue;
                }

                match read_clipboard(&mut clipboard, &cfg) {
                    Ok(Some(candidate)) if candidate.hash != last_hash => {
                        last_hash = candidate.hash.clone();

                        // Source-app detection is deliberately done *after* the
                        // change check so we never shell out on an idle tick.
                        let source = detect_frontmost_app();

                        if is_excluded(&cfg, source.as_deref())
                            || (cfg.respect_concealed_marker && clipboard_is_concealed())
                        {
                            log::debug!("skipping clipboard entry from an excluded/concealed source");
                            continue;
                        }

                        let key = if cfg.encrypt_at_rest {
                            crate::settings::secrets::get_clipboard_key_opt()
                        } else {
                            None
                        };
                        if cfg.encrypt_at_rest && key.is_none() {
                            log::error!(
                                "clipboard encryption is on but the key is unavailable; \
                                 skipping capture rather than writing plaintext"
                            );
                            continue;
                        }

                        let entry = NewEntry {
                            source_app: source,
                            ..candidate
                        };

                        match store.insert(entry, key.as_ref()) {
                            Ok((id, inserted)) => {
                                use tauri::Emitter;
                                let _ = app.emit(CLIPBOARD_CHANGED_EVENT, id);
                                if inserted {
                                    prune_countdown += 1;
                                }
                            }
                            Err(e) => log::error!("could not store clipboard entry: {e}"),
                        }

                        // Pruning every insert would VACUUM-thrash; every 20 is
                        // often enough to keep the table near its limit.
                        if prune_countdown >= 20 {
                            prune_countdown = 0;
                            if let Err(e) = store.prune(cfg.max_items, cfg.max_age_days) {
                                log::warn!("clipboard prune failed: {e}");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log::trace!("clipboard read failed (usually harmless): {e}"),
                }

                std::thread::sleep(interval);
            }
        })
        .expect("failed to spawn clipboard watcher thread");

    handle
}

/// Read whatever is currently on the clipboard, in priority order:
/// text → image. (File lists are read from the text flavour, since every
/// platform also exposes copied files as newline-separated paths.)
fn read_clipboard(
    clipboard: &mut Clipboard,
    cfg: &crate::settings::ClipboardSettings,
) -> Result<Option<NewEntry>, arboard::Error> {
    if cfg.capture_text || cfg.capture_files {
        match clipboard.get_text() {
            Ok(text) if !text.trim().is_empty() => {
                if text.len() > cfg.max_entry_bytes {
                    log::debug!("skipping {}-byte clipboard text (over the limit)", text.len());
                    return Ok(None);
                }
                let looks_like_files = looks_like_file_list(&text);
                if looks_like_files && !cfg.capture_files {
                    return Ok(None);
                }
                if !looks_like_files && !cfg.capture_text {
                    return Ok(None);
                }
                let kind = if looks_like_files {
                    EntryKind::Files
                } else {
                    EntryKind::Text
                };
                return Ok(Some(NewEntry {
                    kind,
                    hash: hash_bytes(text.as_bytes()),
                    preview: make_preview(&text),
                    content: text.into_bytes(),
                    source_app: None,
                    thumbnail: None,
                    width: None,
                    height: None,
                }));
            }
            Ok(_) => {}
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(e) => log::trace!("clipboard text read: {e}"),
        }
    }

    if cfg.capture_images {
        match clipboard.get_image() {
            Ok(img) => {
                let (w, h) = (img.width as u32, img.height as u32);
                let raw = img.bytes.into_owned();
                if raw.len() > cfg.max_entry_bytes {
                    log::debug!("skipping {}-byte clipboard image (over the limit)", raw.len());
                    return Ok(None);
                }
                let Some(buffer) = image::RgbaImage::from_raw(w, h, raw) else {
                    return Ok(None);
                };
                let png = encode_png(&buffer)?;
                let thumbnail = make_thumbnail(&buffer).ok();
                return Ok(Some(NewEntry {
                    kind: EntryKind::Image,
                    hash: hash_bytes(&png),
                    preview: format!("Image \u{2014} {w}\u{d7}{h}"),
                    content: png,
                    source_app: None,
                    thumbnail,
                    width: Some(w),
                    height: Some(h),
                }));
            }
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(e) => log::trace!("clipboard image read: {e}"),
        }
    }

    Ok(None)
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, arboard::Error> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| arboard::Error::Unknown {
            description: format!("PNG encode failed: {e}"),
        })?;
    Ok(out)
}

fn make_thumbnail(img: &image::RgbaImage) -> Result<Vec<u8>, arboard::Error> {
    let thumb = image::imageops::thumbnail(
        img,
        THUMBNAIL_MAX.min(img.width()),
        // `thumbnail` does not preserve aspect ratio on its own, so compute the
        // matching height.
        ((THUMBNAIL_MAX.min(img.width()) as f32 / img.width().max(1) as f32)
            * img.height() as f32)
            .round()
            .max(1.0) as u32,
    );
    encode_png(&thumb)
}

/// Heuristic: several lines that all look like absolute paths to things that
/// exist. Copying files puts exactly that on the text flavour of the clipboard.
fn looks_like_file_list(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() || lines.len() > 200 {
        return false;
    }
    lines.iter().all(|l| {
        let absolute = l.starts_with('/')
            || l.starts_with("file://")
            || (l.len() > 2 && l.as_bytes()[1] == b':' && l.as_bytes()[0].is_ascii_alphabetic());
        absolute && std::path::Path::new(l.trim_start_matches("file://")).exists()
    })
}

fn make_preview(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= PREVIEW_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(PREVIEW_CHARS).collect();
        format!("{truncated}\u{2026}")
    }
}

/// Content digest used for de-duplication.
///
/// `DefaultHasher` is not cryptographic, and does not need to be: a collision
/// costs one skipped history entry, and there is no adversary who benefits.
fn hash_bytes(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}:{}", h.finish(), bytes.len())
}

fn is_excluded(cfg: &crate::settings::ClipboardSettings, source: Option<&str>) -> bool {
    let Some(app) = source else { return false };
    let app_lower = app.to_lowercase();
    cfg.excluded_apps
        .iter()
        .any(|x| !x.is_empty() && app_lower.contains(&x.to_lowercase()))
}

// ---------------------------------------------------------------------------
// Platform probes (all best-effort — see docs/PLATFORM_SUPPORT.md)
// ---------------------------------------------------------------------------

/// Name of the frontmost application, used for the exclusion list and the
/// "copied from" label.
///
/// On macOS this shells out to `lsappinfo`, which — unlike the usual
/// `System Events` AppleScript — needs no Automation permission, so Orbit never
/// triggers a scary consent prompt just to label a clipboard row.
fn detect_frontmost_app() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let front = Command::new("lsappinfo").arg("front").output().ok()?;
        let asn = String::from_utf8_lossy(&front.stdout).trim().to_string();
        if asn.is_empty() {
            return None;
        }
        let info = Command::new("lsappinfo")
            .args(["info", "-only", "name", &asn])
            .output()
            .ok()?;
        // Output looks like: "LSDisplayName"="Safari"
        let text = String::from_utf8_lossy(&info.stdout);
        let name = text.split('=').nth(2)?.trim().trim_matches('"').to_string();
        (!name.is_empty()).then_some(name)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::process::Command;
        // Only works under X11 with xdotool installed; Wayland has no portable
        // equivalent. Absence is fine — the field is optional.
        let out = Command::new("xdotool")
            .args(["getactivewindow", "getwindowclassname"])
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    #[cfg(target_os = "windows")]
    {
        // Would need a direct Win32 call (GetForegroundWindow +
        // QueryFullProcessImageName). Not implemented; the exclusion list falls
        // back to the concealed-content check.
        None
    }
}

/// Whether the current clipboard carries the `org.nspasteboard.ConcealedType`
/// marker that password managers set to mean "do not record this".
///
/// Honouring this convention is why Orbit can be trusted to run a clipboard
/// history at all. Implemented via JXA, which reads *our own* process's
/// pasteboard and therefore needs no permissions.
fn clipboard_is_concealed() -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        const SCRIPT: &str = r#"ObjC.import('AppKit');
var t = $.NSPasteboard.generalPasteboard.types;
var out = [];
for (var i = 0; i < t.count; i++) { out.push(ObjC.unwrap(t.objectAtIndex(i))); }
out.join(',')"#;
        let Ok(out) = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", SCRIPT])
            .output()
        else {
            return false;
        };
        let types = String::from_utf8_lossy(&out.stdout).to_lowercase();
        types.contains("org.nspasteboard.concealedtype")
            || types.contains("org.nspasteboard.autogeneratedtype")
    }

    #[cfg(not(target_os = "macos"))]
    {
        // No cross-platform equivalent exists. Windows has
        // `ExcludeClipboardContentFromMonitorProcessing`, which arboard does not
        // expose; Linux has no convention at all.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ClipboardSettings;

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        assert_eq!(make_preview("  a\n\n b \t c "), "a b c");
        let long = "x".repeat(PREVIEW_CHARS + 50);
        let p = make_preview(&long);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1); // + ellipsis
        assert!(p.ends_with('\u{2026}'));
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
    }

    #[test]
    fn exclusion_matches_case_insensitively_on_substrings() {
        let cfg = ClipboardSettings::default();
        assert!(is_excluded(&cfg, Some("1Password 8")));
        assert!(is_excluded(&cfg, Some("bitwarden")));
        assert!(!is_excluded(&cfg, Some("Safari")));
        assert!(!is_excluded(&cfg, None));
    }

    #[test]
    fn empty_exclusion_patterns_never_match() {
        let cfg = ClipboardSettings {
            excluded_apps: vec![String::new()],
            ..Default::default()
        };
        assert!(!is_excluded(&cfg, Some("anything")));
    }

    #[test]
    fn plain_text_is_not_mistaken_for_a_file_list() {
        assert!(!looks_like_file_list("just some words"));
        assert!(!looks_like_file_list("/definitely/not/a/real/path/xyzzy"));
        assert!(!looks_like_file_list(""));
    }

    #[test]
    fn real_paths_are_detected_as_a_file_list() {
        let dir = std::env::temp_dir();
        let f = dir.join("orbit-file-list-test.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(looks_like_file_list(f.to_str().unwrap()));
        let _ = std::fs::remove_file(f);
    }
}
