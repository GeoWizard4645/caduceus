//! Real app icons for shortcuts, extracted from the user's own installed apps.
//!
//! # Why extraction instead of a bundled logo set
//!
//! The product ask was "every major brand and app's glyph" for shortcut icons.
//! Shipping copies of other companies' logos inside Caduceus is the wrong way
//! to get there: it goes stale the moment any of them rebrand, it is a lot of
//! bytes for marks most users will never pick, and — this is the one that
//! actually matters — Caduceus is GPL, and redistributing a pile of third-party
//! trademarks inside a GPL'd bundle is a legal question nobody asked for.
//!
//! [`crate::apps`] already knows about every `.app` on this Mac, because the
//! launcher needs that list anyway. Reading the icon out of the bundle the
//! user already has installed sidesteps all three problems at once: the icon
//! is always current, it costs no bundle size, and there is no trademark
//! question because we are not distributing it — we are reading a file that
//! is already on the user's disk, the same way Finder does.
//!
//! # How extraction works
//!
//! Every `.app` names its own icon file via `CFBundleIconFile` in
//! `Contents/Info.plist`, pointing at an `.icns` under `Contents/Resources`.
//! `plutil -convert json -o -` normalises that plist (binary or XML, macOS
//! ships both) to JSON on stdout without touching the file on disk, which
//! means reading it needs no new plist-parsing dependency — just a bounded
//! subprocess and `serde_json`, both already in the tree. The `.icns` itself
//! is then handed to `sips`, also built into macOS, which is the same tool
//! [`crate::tools`] already uses elsewhere for image work.
//!
//! # Where the result is cached, and why there
//!
//! `tauri.conf.json`'s asset-protocol scope only allows the webview to load
//! files from a handful of directories under the app config dir — see
//! `$APPCONFIG/staff`, `/appearance`, `/icons` and `/shortcut-icons` there.
//! This module writes into `/shortcut-icons`, the same directory
//! [`crate::shortcuts::icons`] already uses for user-uploaded custom images,
//! rather than claiming one of its own. That is not just convenience: it
//! means an extracted icon becomes an ordinary `image:<filename>` token, the
//! exact format `ShortcutIcon.tsx` and `shortcuts::icons::resolve_path`
//! already know how to resolve, so a shortcut can point at one with no
//! changes needed to either of those files. This module calls
//! [`crate::shortcuts::icons::icons_dir`] and
//! [`crate::shortcuts::icons::icon_token`] — both already `pub` — rather than
//! duplicating the directory logic or inventing a second token format.
//!
//! Filenames are content-addressed (`appicon-<app>-<hash of the .icns
//! bytes>.png`), for the same reason `shortcuts::icons::import_icon` hashes
//! uploaded images: a fixed name for "Slack's icon" would mean that if Slack
//! ever ships a new mark and the user re-extracts it, the webview goes on
//! showing the old one from its asset cache, because the URL never changed.
//! Hashing the source `.icns` bytes (not the converted PNG's) also means a
//! second shortcut pointed at the same app is a free cache hit — extraction
//! never re-runs `sips` for a `.icns` it has already converted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tauri::{AppHandle, Runtime};

use crate::tools::output_with_timeout;

/// `plutil` reads a file already on local disk; this only guards against a
/// truly wedged process (a corrupt plist that sends it spinning), not real
/// work.
const PLIST_TIMEOUT: Duration = Duration::from_secs(5);

/// `sips` converting a small `.icns` is near-instant; this is generous enough
/// to survive a loaded machine without ever being the reason a click hangs.
const SIPS_TIMEOUT: Duration = Duration::from_secs(10);

/// `.icns` files are small (a few hundred KB, occasionally a few MB for an
/// app that bundles every resolution up to a giant preview size). Anything
/// past this is almost certainly not a normal icon, and reading it fully into
/// memory to hash it would be wasted work.
const MAX_ICNS_BYTES: u64 = 20 * 1024 * 1024;

/// Read an application bundle's icon, convert it to PNG, cache it under the
/// shared shortcut-icons directory, and return an `image:<filename>` token
/// ready to store as a shortcut's `icon` field.
///
/// `app_bundle_path` is expected to be one of the paths
/// [`crate::apps::AppIndex`] already returned to the frontend — this does not
/// re-scan `/Applications`, it just trusts the caller to have picked a real
/// `.app` (and checks that below).
pub fn extract_app_icon<R: Runtime>(
    app: &AppHandle<R>,
    app_bundle_path: &str,
) -> Result<String, String> {
    let bundle = PathBuf::from(app_bundle_path);
    ensure_is_app_bundle(&bundle)?;

    let icns_path = resolve_icns_path(&bundle)?;
    let icns_bytes = read_bounded(&icns_path)?;
    let hash = content_hash(&icns_bytes);

    let app_name = sanitize_name(
        bundle
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app"),
    );
    let filename = format!("appicon-{app_name}-{hash}.png");

    // Reuses `shortcuts::icons`' directory and token format on purpose — see
    // the module doc. Neither function is reimplemented here; both are
    // called as-is from the unmodified module.
    let dir = crate::shortcuts::icons::icons_dir(app)?;
    let dest = dir.join(&filename);

    if !dest.is_file() {
        convert_icns_to_png(&icns_path, &dir, &dest)?;
    }

    Ok(crate::shortcuts::icons::icon_token(&filename))
}

/// Confirms `path` looks like an application bundle before anything shells
/// out against it — a `.app` extension and an existing directory, which is
/// the same bar [`crate::apps`] itself uses when it discovers these paths.
fn ensure_is_app_bundle(path: &Path) -> Result<(), String> {
    if path.extension().and_then(|e| e.to_str()) != Some("app") {
        return Err("that does not look like an application (expected a .app bundle)".into());
    }
    if !path.is_dir() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(())
}

/// Run `sips` to convert `icns_path` into a PNG, then move it into place under
/// `dest`.
///
/// Converts into a uniquely-named temporary file first and renames it into
/// place, rather than having `sips` write `dest` directly: `sips` is given a
/// bounded timeout and could in principle be killed mid-write, and a renamed
/// file only ever appears atomically complete or not at all, so a killed
/// `sips` never leaves a half-written file sitting at the name callers are
/// about to treat as cached and valid.
fn convert_icns_to_png(icns_path: &Path, dir: &Path, dest: &Path) -> Result<(), String> {
    let tmp = dir.join(format!(".appicon-{}.png.tmp", uuid::Uuid::new_v4()));

    let mut command = Command::new("sips");
    command
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(icns_path)
        .arg("--out")
        .arg(&tmp);

    let output = output_with_timeout(&mut command, SIPS_TIMEOUT, "sips did not answer")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "sips could not convert {}: {}",
            icns_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not cache the extracted icon: {e}")
    })
}

/// Read a file's bytes, refusing anything implausibly large for an `.icns`
/// rather than reading an unbounded amount into memory.
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_ICNS_BYTES {
        return Err(format!(
            "{} is unexpectedly large for an icon ({} MB) — refusing to read it",
            path.display(),
            meta.len() / (1024 * 1024)
        ));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

/// Find the `.icns` file for an application bundle.
///
/// Prefers whatever `Info.plist`'s `CFBundleIconFile` names, since that is
/// the authoritative answer. Falls back to scanning `Contents/Resources` for
/// `.icns` files when the key is missing or stale (seen on a handful of apps
/// whose `Info.plist` lags behind an internal rename) — a single `.icns`
/// found that way is unambiguous, and among several, one literally named
/// `AppIcon.icns` is the overwhelmingly common convention.
fn resolve_icns_path(bundle: &Path) -> Result<PathBuf, String> {
    let resources = bundle.join("Contents/Resources");
    let plist_path = bundle.join("Contents/Info.plist");

    if let Some(name) = bundle_icon_file_name(&plist_path)? {
        let mut candidate = resources.join(&name);
        // `CFBundleIconFile` conventionally omits the extension.
        if candidate.extension().is_none() {
            candidate.set_extension("icns");
        }
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&resources)
        .map_err(|e| format!("could not read {}: {e}", resources.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("icns"))
        .collect();
    // Deterministic order: picking "the first one" should mean the same file
    // every time, not whatever order the filesystem happened to return.
    candidates.sort();

    if let Some(preferred) = candidates
        .iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some("AppIcon"))
    {
        return Ok(preferred.clone());
    }

    candidates
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} has no .icns icon Caduceus can read", bundle.display()))
}

/// Read `CFBundleIconFile` out of an application's `Info.plist`, via `plutil`.
///
/// `plutil -convert json -o -` rather than a plist-parsing crate: mixed
/// binary/XML plists are exactly what `plutil` exists to normalise, it ships
/// on every Mac that can run this app at all, and `-o -` writes the
/// conversion to stdout instead of overwriting the source file. That means
/// this never touches anything inside the app bundle it is reading.
fn bundle_icon_file_name(plist_path: &Path) -> Result<Option<String>, String> {
    if !plist_path.is_file() {
        return Err(format!("no Info.plist at {}", plist_path.display()));
    }

    let mut command = Command::new("plutil");
    command
        .arg("-convert")
        .arg("json")
        .arg("-o")
        .arg("-")
        .arg(plist_path);

    let output = output_with_timeout(&mut command, PLIST_TIMEOUT, "plutil did not answer")?;
    if !output.status.success() {
        return Err(format!(
            "plutil could not read {}: {}",
            plist_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Info.plist was not valid JSON once converted: {e}"))?;
    Ok(icon_file_name_from_plist_json(&value))
}

/// Pulled out of [`bundle_icon_file_name`] so the interesting part — which
/// key, and what to do if the value looks unsafe — is unit-testable without
/// shelling out to `plutil`.
fn icon_file_name_from_plist_json(plist: &serde_json::Value) -> Option<String> {
    let raw = plist.get("CFBundleIconFile")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    // Defense in depth: this string comes from inside the app bundle, which
    // is not data we generated, so treat it as untrusted the same way
    // `shortcuts::icons::resolve_path` treats a stored icon token — take only
    // the file name component, so a mischievous `../../../etc/whatever`
    // can't walk this out of `Contents/Resources`.
    Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

/// Keep only characters safe in a filename, mirroring
/// `shortcuts::icons::import_icon`'s treatment of a shortcut id — an app name
/// only appears in the cache filename for a human skimming the directory, so
/// it does not need to be reversible, only stable and safe.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "app".to_string()
    } else {
        cleaned
    }
}

/// A short, stable, non-cryptographic digest of an icon's bytes.
///
/// FNV-1a, the same algorithm `shortcuts::icons::import_icon` uses for the
/// same reason: this only names a cache file, so the property that matters is
/// that different bytes usually produce different names, not that it resists
/// a deliberate collision. Duplicated here rather than imported because
/// `shortcuts::icons`'s copy is a private helper, and hashing eight bytes at a
/// time is cheap enough that sharing it is not worth a visibility change to a
/// module this task does not own.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:08x}")
}

// ---------------------------------------------------------------------------
// Tauri command wrappers
//
// These are `pub` and annotated so they are ready to register, but this
// module does not add itself to `commands.rs` or to the `generate_handler!`
// list in `lib.rs` — both are owned by other work happening in parallel. See
// this task's final report for exactly what to wire up.
// ---------------------------------------------------------------------------

/// Extract and cache the icon for an installed app, returning an
/// `image:<filename>` token the frontend can store as a shortcut's `icon`.
///
/// `app_path` is the absolute bundle path as returned by
/// [`crate::commands::list_installed_apps`] (i.e. `InstalledApp::path`).
#[tauri::command]
pub fn extract_app_icon_cmd<R: Runtime>(
    app: AppHandle<R>,
    app_path: String,
) -> Result<String, String> {
    extract_app_icon(&app, &app_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A private, per-test scratch directory under the OS temp dir — avoids
    /// pulling in a `tempfile` dependency for tests that only need "a
    /// directory nothing else is using right now".
    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "caduceus-appicons-test-{label}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    // --- icon_file_name_from_plist_json: pure, no filesystem or subprocess --

    #[test]
    fn reads_the_bundle_icon_file_key() {
        let plist = serde_json::json!({ "CFBundleIconFile": "AppIcon" });
        assert_eq!(
            icon_file_name_from_plist_json(&plist),
            Some("AppIcon".to_string())
        );
    }

    #[test]
    fn keeps_an_explicit_extension_as_is() {
        let plist = serde_json::json!({ "CFBundleIconFile": "AppIcon.icns" });
        assert_eq!(
            icon_file_name_from_plist_json(&plist),
            Some("AppIcon.icns".to_string())
        );
    }

    #[test]
    fn missing_key_resolves_to_nothing_rather_than_erroring() {
        let plist = serde_json::json!({ "CFBundleName": "Example" });
        assert_eq!(icon_file_name_from_plist_json(&plist), None);
    }

    #[test]
    fn blank_value_is_treated_as_missing() {
        let plist = serde_json::json!({ "CFBundleIconFile": "   " });
        assert_eq!(icon_file_name_from_plist_json(&plist), None);
    }

    #[test]
    fn a_path_traversal_attempt_is_reduced_to_a_bare_filename() {
        // Info.plist is data from inside the bundle, not data we produced —
        // this proves a hostile or merely corrupt value can't walk the
        // result outside Contents/Resources.
        let plist = serde_json::json!({ "CFBundleIconFile": "../../../etc/passwd" });
        assert_eq!(
            icon_file_name_from_plist_json(&plist),
            Some("passwd".to_string())
        );
    }

    // --- content_hash: pure ------------------------------------------------

    #[test]
    fn identical_bytes_hash_identically() {
        let a = content_hash(b"same bytes");
        let b = content_hash(b"same bytes");
        assert_eq!(a, b);
    }

    #[test]
    fn different_bytes_hash_differently() {
        let a = content_hash(b"the old logo");
        let b = content_hash(b"the new logo");
        assert_ne!(a, b, "changing the icon's bytes must change its cache key");
    }

    #[test]
    fn hash_is_lowercase_hex_at_least_eight_chars() {
        // `{:08x}` zero-pads to a minimum of 8, but a u64's hex form can run
        // up to 16 — the guarantee that matters for a filename is "always at
        // least 8, always hex, always lowercase", not an exact width.
        let hash = content_hash(b"anything");
        assert!(hash.len() >= 8, "hash too short: {hash}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    // --- sanitize_name: pure ------------------------------------------------

    #[test]
    fn strips_spaces_and_punctuation_from_app_names() {
        assert_eq!(sanitize_name("Google Chrome"), "GoogleChrome");
        assert_eq!(sanitize_name("1Password 7"), "1Password7");
    }

    #[test]
    fn keeps_hyphens() {
        assert_eq!(sanitize_name("Visual-Studio-Code"), "Visual-Studio-Code");
    }

    #[test]
    fn a_name_that_is_all_punctuation_falls_back_to_app() {
        assert_eq!(sanitize_name("😀😀😀"), "app");
        assert_eq!(sanitize_name(""), "app");
    }

    // --- ensure_is_app_bundle: filesystem, no specific app required --------

    #[test]
    fn rejects_a_path_that_is_not_dot_app() {
        let dir = scratch_dir("not-an-app");
        let not_an_app = dir.join("Notes.txt");
        std::fs::write(&not_an_app, b"hi").unwrap();
        let err = ensure_is_app_bundle(&not_an_app).unwrap_err();
        assert!(err.contains(".app"), "unexpected message: {err}");
    }

    #[test]
    fn rejects_a_dot_app_path_that_does_not_exist() {
        let dir = scratch_dir("missing-app");
        let missing = dir.join("Nonexistent.app");
        let err = ensure_is_app_bundle(&missing).unwrap_err();
        assert!(err.contains("does not exist"), "unexpected message: {err}");
    }

    #[test]
    fn accepts_a_directory_that_ends_in_dot_app() {
        let dir = scratch_dir("real-app");
        let bundle = dir.join("Example.app");
        std::fs::create_dir_all(&bundle).unwrap();
        assert!(ensure_is_app_bundle(&bundle).is_ok());
    }

    // --- resolve_icns_path: filesystem fixtures we build ourselves, never a
    // real installed app -----------------------------------------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn resolves_via_the_info_plist_icon_file_key() {
        let dir = scratch_dir("plist-resolve");
        let bundle = dir.join("Fixture.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict>\n\
              <key>CFBundleIconFile</key><string>MyIcon</string>\n\
              </dict></plist>",
        )
        .unwrap();
        // The extension is deliberately omitted from the plist value, as is
        // conventional, so the file on disk must have it for this to pass.
        std::fs::write(resources.join("MyIcon.icns"), b"not a real icns, just bytes").unwrap();

        let resolved = resolve_icns_path(&bundle).expect("should resolve via Info.plist");
        assert_eq!(resolved, resources.join("MyIcon.icns"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn falls_back_to_the_only_icns_file_when_the_plist_key_is_missing() {
        let dir = scratch_dir("plist-fallback-single");
        let bundle = dir.join("Fixture.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict>\n\
              <key>CFBundleName</key><string>Fixture</string>\n\
              </dict></plist>",
        )
        .unwrap();
        std::fs::write(resources.join("OnlyIcon.icns"), b"bytes").unwrap();

        let resolved = resolve_icns_path(&bundle).expect("should fall back to the lone .icns");
        assert_eq!(resolved, resources.join("OnlyIcon.icns"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn falls_back_to_app_icon_dot_icns_among_several_when_the_plist_key_is_missing() {
        let dir = scratch_dir("plist-fallback-preferred");
        let bundle = dir.join("Fixture.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict></dict></plist>",
        )
        .unwrap();
        std::fs::write(resources.join("DocumentIcon.icns"), b"bytes").unwrap();
        std::fs::write(resources.join("AppIcon.icns"), b"bytes").unwrap();

        let resolved = resolve_icns_path(&bundle).expect("should prefer AppIcon.icns");
        assert_eq!(resolved, resources.join("AppIcon.icns"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_with_no_icns_anywhere_is_a_clear_error() {
        let dir = scratch_dir("no-icns");
        let bundle = dir.join("Fixture.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
              <plist version=\"1.0\"><dict></dict></plist>",
        )
        .unwrap();

        let err = resolve_icns_path(&bundle).unwrap_err();
        assert!(err.contains("no .icns"), "unexpected message: {err}");
    }

    // --- cache-key / filename shape, end to end on a fixture bundle --------

    #[cfg(target_os = "macos")]
    #[test]
    fn the_same_icon_bytes_produce_the_same_cache_filename() {
        let dir = scratch_dir("stable-filename");
        let icns = dir.join("Icon.icns");
        std::fs::write(&icns, b"identical bytes").unwrap();

        let bytes_a = read_bounded(&icns).unwrap();
        let bytes_b = read_bounded(&icns).unwrap();
        assert_eq!(content_hash(&bytes_a), content_hash(&bytes_b));
    }

    #[test]
    fn read_bounded_refuses_a_file_over_the_size_cap() {
        let dir = scratch_dir("oversized");
        let huge = dir.join("Huge.icns");
        // Sparse file: seek past the cap and write one byte, rather than
        // actually writing 20MB+ of zeros to disk for a unit test.
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::File::create(&huge).unwrap();
            f.seek(SeekFrom::Start(MAX_ICNS_BYTES + 1)).unwrap();
            f.write_all(&[0]).unwrap();
        }
        let err = read_bounded(&huge).unwrap_err();
        assert!(err.contains("unexpectedly large"), "unexpected message: {err}");
    }
}
