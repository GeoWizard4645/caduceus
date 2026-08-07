//! Launch-at-login.
//!
//! A thin wrapper over `tauri-plugin-autostart`, which registers a macOS
//! LaunchAgent, a Windows `Run` registry entry, or an XDG `.desktop` autostart
//! file as appropriate.
//!
//! Everything here is best-effort and reported rather than fatal: on a locked-
//! down machine (managed Windows, immutable Linux image) registration can fail,
//! and that should surface as a message in Settings rather than stopping the
//! app from running.
//!
//! # The login item records a path, and the plugin will not notice it going bad
//!
//! Two things about `tauri-plugin-autostart` drive most of this file.
//!
//! First, what it writes is the **absolute path of the running executable**,
//! taken from `current_exe()` at startup. For the `LaunchAgent` launcher
//! Caduceus uses, that is the binary inside the bundle
//! (`…/Caduceus.app/Contents/MacOS/Caduceus`) — the plugin only trims back to
//! the `.app` for the `AppleScript` launcher.
//!
//! Second, its `is_enabled()` on macOS is literally "does
//! `~/Library/LaunchAgents/Caduceus.plist` exist". It never reads the file. So
//! a login item pointing at a binary that has since moved, been deleted, or was
//! never durable in the first place reports itself as perfectly enabled forever,
//! and a `sync_with_settings` that only reconciles the on/off bit will never
//! rewrite it. That is not hypothetical: a login item here pointed into
//! `src-tauri/target/release/bundle/macos/` — a Cargo build tree — so login was
//! starting a stale build-directory copy instead of the installed app, and a
//! `cargo clean` would have silently broken launch-at-login altogether.
//!
//! Hence [`sync_with_settings`] reconciles the *path* as well as the flag, and
//! [`set_enabled`] refuses to enrol a binary running out of a build tree at all.
//!
//! [`autostart_status`] and [`autostart_set_enabled`] put both checks in front
//! of a human: a live read of what the OS actually has, independent of
//! whatever `general.launch_at_login` says, and a single entry point for
//! changing it that keeps the OS registration and the saved preference from
//! ever disagreeing about what was last asked for.

use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::ManagerExt;

use crate::settings::{self, SettingsManager};

/// The executable path the autostart plugin would record in the login item if
/// it registered right now.
///
/// Mirrors the plugin's own derivation — `current_exe()`, canonicalised — so
/// the comparison in [`registered_path_is_current`] is against the same string
/// the plugin would write, not an approximation of it.
fn current_launch_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    // `canonicalize` matches what the plugin does; falling back to the raw path
    // keeps a symlinked-but-readable install working rather than bailing out.
    let exe = exe.canonicalize().unwrap_or(exe);
    Some(exe.display().to_string())
}

/// Whether this executable lives in a Cargo build tree rather than an installed
/// bundle.
///
/// A `cargo run`, a `cargo tauri dev`, or a locally built release bundle all
/// produce a perfectly working binary at a path that exists only until the next
/// `cargo clean` — and `launch_at_login` defaults to `true`, so without this
/// check the first dev run on a fresh profile quietly enrols the build tree as
/// the user's login item. That is how the stale entry described in the module
/// header got there.
fn is_build_tree_path(path: &str) -> bool {
    path.contains("/target/debug/")
        || path.contains("/target/release/")
        || path.contains("\\target\\debug\\")
        || path.contains("\\target\\release\\")
}

/// The LaunchAgent file `auto-launch` reads and writes for us.
///
/// Rebuilt here rather than asked for, because `AutoLaunchManager` exposes only
/// `enable`/`disable`/`is_enabled` — the underlying `AutoLaunch`'s
/// `get_app_path` is not re-exported. The name is the app's product name, which
/// is what the plugin passes as `app_name`.
#[cfg(target_os = "macos")]
fn launch_agent_plist<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", app.package_info().name)),
    )
}

/// Whether the registered login item still points at the running executable.
///
/// `true` whenever the question cannot be answered — no login item, an
/// unreadable file, an unknown `current_exe` — because the only thing this
/// gates is a corrective rewrite, and rewriting on a bad reading would mean
/// re-registering on every single launch.
///
/// The match is against the exact `<string>…</string>` element `auto-launch`
/// writes into `ProgramArguments`, rather than a bare substring search, so a
/// path that merely *prefixes* another (`/Applications/Caduceus.app` inside
/// `/Applications/Caduceus.app.old/…`) cannot read as a match.
#[cfg(target_os = "macos")]
fn registered_path_is_current<R: Runtime>(app: &AppHandle<R>) -> bool {
    let (Some(plist), Some(path)) = (launch_agent_plist(app), current_launch_path()) else {
        return true;
    };
    let Ok(contents) = std::fs::read_to_string(&plist) else {
        return true;
    };
    contents.contains(&format!("<string>{path}</string>"))
}

#[cfg(not(target_os = "macos"))]
fn registered_path_is_current<R: Runtime>(_app: &AppHandle<R>) -> bool {
    // Windows records the path in a registry value and Linux in a `.desktop`
    // file; neither is read back here yet, so nothing claims a stale entry.
    true
}

/// The message [`set_enabled`] returns when asked to enrol a build-tree
/// binary. Pulled out to its own function so the exact wording is covered by
/// a test independent of constructing a real `AppHandle`.
fn build_tree_refusal(path: &str) -> String {
    format!(
        "This copy of Caduceus is running from a build directory ({path}), so it was \
         not registered to launch at login — that path disappears on the next `cargo \
         clean`. Turn this on from the installed copy instead."
    )
}

pub fn set_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    // Only ever refuse to *add* one. Removing a login item is always allowed —
    // a developer turning it off should not be blocked from doing so just
    // because the copy they are running happens to be a build.
    if enabled {
        if let Some(path) = current_launch_path() {
            if is_build_tree_path(&path) {
                return Err(build_tree_refusal(&path));
            }
        }
    }

    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| {
        format!(
            "Could not {} launch at login: {e}",
            if enabled { "enable" } else { "disable" }
        )
    })
}

/// Whether the OS currently has Caduceus registered to launch at login.
///
/// Read from the OS rather than from settings, so a login item removed by hand
/// is reflected accurately. Note this answers "is there an entry", not "is the
/// entry any good" — see [`registered_path_is_current`] for the other half.
pub fn is_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Reconcile the OS state with the saved preference at startup.
///
/// Reconciles the recorded path too, not just the on/off flag — see the module
/// header for why an enabled-but-stale login item is otherwise permanent.
pub fn sync_with_settings<R: Runtime>(app: &AppHandle<R>, wanted: bool) {
    if is_enabled(app) != wanted {
        if let Err(e) = set_enabled(app, wanted) {
            log::warn!("{e}");
        }
        return;
    }

    // The flag already agrees. If it says "on", the entry still has to point at
    // a binary that exists — this copy — or login silently starts something
    // else, or nothing at all.
    if wanted && !registered_path_is_current(app) {
        match current_launch_path() {
            Some(path) if is_build_tree_path(&path) => {
                // Leave a good entry alone rather than repointing it at a build
                // tree. A dev build running alongside an installed one must not
                // rewrite the installed copy's login item.
                log::debug!("not repointing the login item at a build-tree binary ({path})");
            }
            Some(path) => {
                log::info!("login item points elsewhere; repointing it at {path}");
                if let Err(e) = set_enabled(app, true) {
                    log::warn!("{e}");
                }
            }
            None => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// A snapshot of launch-at-login as it actually stands, for Settings to show
/// alongside — not instead of — the saved preference.
///
/// `enabled` alone is not enough to tell someone their login item is fine:
/// see the module docs for how a login item can report itself as enabled
/// forever while quietly pointing at a binary that no longer exists.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartStatus {
    /// Whether the OS currently has a login item for Caduceus.
    pub enabled: bool,
    /// `false` when a login item exists but points somewhere other than this
    /// running copy — a build tree since `cargo clean`ed, a moved or deleted
    /// install, or a different copy of the app entirely. Vacuously `true`
    /// when `enabled` is `false`: with no login item, there is nothing to be
    /// stale.
    pub path_current: bool,
    /// Whether this running copy is itself a build-tree binary, so enabling
    /// from here would be refused — mirrors [`set_enabled`]'s error.
    pub build_tree: bool,
}

fn status<R: Runtime>(app: &AppHandle<R>) -> AutostartStatus {
    AutostartStatus {
        enabled: is_enabled(app),
        path_current: registered_path_is_current(app),
        build_tree: current_launch_path().is_some_and(|p| is_build_tree_path(&p)),
    }
}

/// Query launch-at-login as it actually stands right now — see
/// [`AutostartStatus`] for why that can differ from the saved preference.
#[tauri::command]
pub fn autostart_status<R: Runtime>(app: AppHandle<R>) -> AutostartStatus {
    status(&app)
}

/// Turn launch-at-login on or off directly, persisting the choice exactly
/// like saving the whole settings tree with a changed `general.launch_at_login`
/// does (see `commands::update_settings`). This is a second door to the same
/// room, not a different one: without the write-back here, a toggle through
/// this command would look like it took, then silently revert the next time
/// [`sync_with_settings`] runs at the following startup.
#[tauri::command]
pub fn autostart_set_enabled<R: Runtime>(
    app: AppHandle<R>,
    settings: tauri::State<'_, SettingsManager>,
    enabled: bool,
) -> Result<AutostartStatus, String> {
    set_enabled(&app, enabled)?;

    let mut cfg = settings.get();
    if cfg.general.launch_at_login != enabled {
        cfg.general.launch_at_login = enabled;
        settings::save(&app, &cfg)?;
    }

    Ok(status(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cargo_build_tree_binary_is_recognised() {
        assert!(is_build_tree_path(
            "/Users/x/Code/Caduceus/src-tauri/target/release/bundle/macos/Caduceus.app/Contents/MacOS/Caduceus"
        ));
        assert!(is_build_tree_path(
            "/Users/x/Code/Caduceus/src-tauri/target/debug/Caduceus"
        ));
        assert!(is_build_tree_path("C:\\src\\caduceus\\target\\debug\\Caduceus.exe"));
    }

    #[test]
    fn an_installed_bundle_is_not_mistaken_for_a_build_tree() {
        assert!(!is_build_tree_path(
            "/Applications/Caduceus.app/Contents/MacOS/Caduceus"
        ));
        // "target" as an ordinary directory name is not a Cargo build tree —
        // only the `target/debug` and `target/release` pair is.
        assert!(!is_build_tree_path("/Users/x/target/Caduceus.app/Contents/MacOS/Caduceus"));
        assert!(!is_build_tree_path(
            "/Users/x/Applications/On Target/Caduceus.app/Contents/MacOS/Caduceus"
        ));
    }

    #[test]
    fn the_build_tree_refusal_names_the_offending_path_and_says_why() {
        let message = build_tree_refusal("/Users/x/Code/Caduceus/src-tauri/target/debug/Caduceus");
        assert!(
            message.contains("target/debug/Caduceus"),
            "must name the offending path so it can actually be found: {message}"
        );
        assert!(
            message.to_lowercase().contains("cargo clean"),
            "must say why a build-tree path is not durable: {message}"
        );
    }

    /// Not a test of the OS registration itself — that needs a real
    /// `AppHandle`, which nothing in this module's test suite constructs (see
    /// every other test here, which exercises pure path logic only). This
    /// pins down the part that *is* pure: the same running copy always
    /// computes the same launch path, which is what makes `enable` on top of
    /// `tauri-plugin-autostart` idempotent in the first place — it always
    /// overwrites the one fixed-name login item (`Caduceus.plist` on macOS)
    /// with the same content, rather than appending a new one.
    #[test]
    fn the_launch_path_is_stable_across_repeated_reads() {
        assert_eq!(current_launch_path(), current_launch_path());
    }
}
