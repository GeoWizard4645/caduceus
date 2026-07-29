//! Compare the running build to the latest GitHub release, and — since 4.2 —
//! do that on a schedule rather than only when someone presses a button. See
//! [`spawn_update_watcher`] for the background half of this file; everything
//! above it is the same manual check-and-install path that predates it,
//! unchanged except for the new [`UpdateCheck::homebrew_managed`] field.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::settings::{SettingsManager, UpdateMode};

const REPO: &str = "GeoWizard4645/caduceus";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current_version: String,
    pub update_available: bool,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub download_url: Option<String>,
    /// Whether this running copy was installed by Homebrew. The curl
    /// installer must never run against one of these — see
    /// [`is_homebrew_managed`] — so the frontend needs to know before it
    /// offers the "Update now" button.
    pub homebrew_managed: bool,
}

pub async fn check() -> UpdateCheck {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut out = UpdateCheck {
        current_version: current.clone(),
        update_available: false,
        latest_version: None,
        release_url: None,
        download_url: None,
        // Set unconditionally, before anything that can fail below, so a
        // network hiccup never hides "this is a brew install" from the UI —
        // that fact does not depend on GitHub answering.
        homebrew_managed: is_homebrew_managed(),
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent(format!("Caduceus/{}", current))
        .build()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let Ok(response) = client.get(&url).send().await else {
        return out;
    };
    if !response.status().is_success() {
        return out;
    }

    let Ok(body) = response.json::<serde_json::Value>().await else {
        return out;
    };

    let tag = body.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest = tag.trim_start_matches('v').to_string();
    if latest.is_empty() {
        return out;
    }

    out.latest_version = Some(latest.clone());
    out.release_url = body
        .get("html_url")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    out.download_url = pick_dmg_url(body.get("assets"));
    out.update_available = is_newer(&latest, &current);
    out
}

/// The one-liner from the website. A constant, and deliberately not built from
/// anything the UI passes in — this string becomes an executable file.
pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash";

/// The contents of the `.command` file, split out so it can be checked.
///
/// A shell script assembled by string formatting is a thing that compiles
/// happily and then fails at the one moment it runs, in a Terminal window, on
/// somebody else's machine. `bash -n` in the tests below is cheap insurance.
fn update_script() -> String {
    format!(
        r#"#!/bin/bash
echo "Updating Caduceus…"
echo "This is the same command as on the website:"
echo
echo "    {INSTALL_COMMAND}"
echo
{INSTALL_COMMAND}
status=$?
echo
if [ $status -eq 0 ]; then
  echo "Done. Caduceus should reopen on its own."
else
  echo "The update did not finish (exit $status). Caduceus is unchanged."
fi
echo "You can close this window."
"#
    )
}

/// Update in place by running the installer in Terminal.
///
/// # Why Terminal, and not a child process
///
/// The installer's update path quits the running copy (`osascript … to quit`,
/// then `pkill`) and `rm -rf`s the bundle before copying the new one. Run as a
/// child of Caduceus, it would be killing its own parent half way through, and
/// whether the rest of the script survives that depends on process-group
/// signalling nobody should be relying on. Terminal owns it instead, so
/// Caduceus quitting is exactly what the script expects rather than a hazard.
///
/// It is also the honest option. Caduceus is not notarised and asks people to
/// run this same command to install it; an update that replaces the app should
/// be the thing you can watch, not a silent download.
///
/// A `.command` file rather than `tell application "Terminal"`: AppleScript to
/// Terminal needs the Automation grant, and asking for permission to control a
/// terminal in order to run an update is a worse trade than writing a file.
#[cfg(target_os = "macos")]
pub fn run_installer() -> Result<(), String> {
    let path = write_update_script("caduceus-update.command")?;

    // `open` on a `.command` hands it to Terminal, which becomes its owner —
    // so it outlives this process quitting, which the script is about to do.
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Terminal: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub fn run_installer() -> Result<(), String> {
    Err("The installer is macOS-only.".into())
}

/// Write [`update_script`] out as an executable `.command` file and return its
/// path. Shared by [`run_installer`] (Terminal owns the file afterwards) and
/// [`run_installer_detached`] (nothing does — it runs the same script
/// headless).
#[cfg(target_os = "macos")]
fn write_update_script(filename: &str) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::temp_dir().join(filename);
    let script = update_script();

    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("Could not write the update script: {e}"))?;
    file.write_all(script.as_bytes())
        .map_err(|e| format!("Could not write the update script: {e}"))?;
    drop(file);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Could not make the update script runnable: {e}"))?;

    Ok(path)
}

/// Update in place without Terminal, for `UpdateMode::Auto`.
///
/// `run_installer`'s doc comment explains why the installer has to outlive
/// its parent — it quits Caduceus, deletes the bundle, and copies the new one
/// in, so whatever runs it cannot be a normal child process that dies with
/// Caduceus. Terminal solved that by owning the file instead. `auto` mode
/// needs the same survival property without a Terminal window popping up —
/// nobody who chose "install automatically" wants to be asked to watch it —
/// so this spawns the same script directly, detached from Caduceus's process
/// group with `process_group(0)` before the child execs.
///
/// A plain child process already outlives its parent quitting on macOS —
/// orphans are reparented to `launchd`, not killed, so `rm -rf`ing the bundle
/// out from under a *normally spawned* child would already survive `quit`.
/// What `process_group(0)` buys on top of that is independence from
/// *signals* sent to Caduceus's process group rather than to Caduceus's own
/// pid specifically — the same class of hazard Terminal ownership exists to
/// avoid, achieved here without a visible window instead of by handing the
/// file to another application. Stdio is discarded rather than inherited:
/// nothing is watching a headless run, and leaving Caduceus's own stdio
/// handles open in a process that outlives it would leave them dangling.
#[cfg(target_os = "macos")]
pub fn run_installer_detached() -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    let path = write_update_script("caduceus-auto-update.command")?;

    std::process::Command::new("/bin/bash")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start the update: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub fn run_installer_detached() -> Result<(), String> {
    Err("The installer is macOS-only.".into())
}

/// The Homebrew cask name Caduceus ships under.
const HOMEBREW_CASK: &str = "caduceus";

/// The two places Homebrew's own prefix can be on macOS: `/opt/homebrew` on
/// Apple Silicon, `/usr/local` on Intel. Never both at once, but checking
/// both costs nothing and does not depend on which Mac this is.
const HOMEBREW_PREFIXES: &[&str] = &["/opt/homebrew", "/usr/local"];

/// Whether the running copy of Caduceus was installed by Homebrew.
///
/// Auto-replacing a Homebrew-managed install with the curl installer would
/// leave `brew`'s bookkeeping pointing at a cask that is no longer what is
/// actually on disk — the next `brew upgrade` would then find "4.1.1" already
/// present (its own record, untouched) sitting next to a binary that is
/// actually newer, and either no-op confusingly or fail outright trying to
/// reconcile the two. So this has to be checked before anything runs.
///
/// Detected by looking for the Caskroom directory Homebrew leaves behind for
/// every installed cask, rather than by asking `brew` itself — a GUI app
/// launched from Finder or `launchd` does not inherit the shell's `PATH` the
/// way a Terminal-launched process does, and `brew` lives in `/opt/homebrew/bin`
/// or `/usr/local/bin`, neither of which is guaranteed to be on a GUI app's
/// `PATH`. `Command::new("brew")` would therefore reliably fail to find it in
/// exactly the case this needs to detect, on top of being slower than a
/// filesystem check for no benefit.
pub fn is_homebrew_managed() -> bool {
    HOMEBREW_PREFIXES
        .iter()
        .any(|prefix| caskroom_entry_exists(std::path::Path::new(prefix)))
}

/// `prefix` is a parameter (rather than this reading `HOMEBREW_PREFIXES`
/// directly) so the test below can point it at a temporary directory instead
/// of asking whether the machine actually running the tests has Homebrew.
fn caskroom_entry_exists(prefix: &std::path::Path) -> bool {
    prefix.join("Caskroom").join(HOMEBREW_CASK).is_dir()
}

/// Whether now is a bad moment to quit Caduceus out from under someone, for
/// `UpdateMode::Auto` to check before it does exactly that.
///
/// This only catches activity Caduceus itself is doing: a dictation or
/// push-to-talk recording (both drive the same [`crate::voice::VoiceRuntime`],
/// so one check covers both), and a meeting-notes or screen recording (both
/// drive [`crate::capture::recorder::RecorderRuntime`], differentiated only by
/// which mode it is running in, which does not matter here — either one is a
/// reason to wait). There is no macOS API for "is some *other* application
/// recording the screen right now", so a screen recorder running outside
/// Caduceus is invisible to this and stays invisible; this is a guard against
/// the update fighting with Caduceus's own features, not a promise about
/// every possible interruption.
fn is_busy<R: Runtime>(app: &AppHandle<R>) -> bool {
    let voice_busy = app
        .try_state::<crate::voice::VoiceRuntime>()
        .is_some_and(|voice| voice.is_recording());
    let capture_busy = app
        .try_state::<crate::capture::recorder::RecorderRuntime>()
        .is_some_and(|recorder| recorder.status().active);
    voice_busy || capture_busy
}

fn pick_dmg_url(assets: Option<&serde_json::Value>) -> Option<String> {
    let arr = assets?.as_array()?;
    for asset in arr {
        let name = asset.get("name")?.as_str()?;
        if name.ends_with(".dmg") && name.contains("universal") {
            return asset.get("browser_download_url")?.as_str().map(str::to_string);
        }
    }
    for asset in arr {
        let name = asset.get("name")?.as_str()?;
        if name.ends_with(".dmg") {
            return asset.get("browser_download_url")?.as_str().map(str::to_string);
        }
    }
    None
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn parse_version(raw: &str) -> (u32, u32, u32) {
    let mut parts = raw
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

// ---------------------------------------------------------------------------
// The background watcher
// ---------------------------------------------------------------------------

/// Wait this long after launch before the first automatic check, so asking
/// GitHub something is never on the path between the process starting and the
/// staff appearing.
const INITIAL_DELAY_SECS: u64 = 40;

/// The target cadence between checks. "Roughly every 12 hours" from the
/// design brief; see [`JITTER_SECS`] for why it is never exactly this.
const CHECK_INTERVAL_SECS: i64 = 12 * 60 * 60;

/// How far the actual wait is allowed to drift from [`CHECK_INTERVAL_SECS`],
/// plus or minus. Every install choosing "12 hours" precisely would mean every
/// install launched at a similar time of day hits GitHub's unauthenticated API
/// in the same few-minute window forever after — not enough installs exist for
/// that to matter today, but it costs one call to `getrandom` to not be the
/// app that assumes it never will.
const JITTER_SECS: i64 = 45 * 60;

/// The minimum time that must have passed since the last check before another
/// one is allowed to run, independent of the loop's own timing. This is the
/// actual guard against a restart storm: someone relaunching Caduceus ten
/// times in a minute gets ten watcher tasks that each ask "was the last check
/// recent?", see yes, and do nothing — rather than ten calls to GitHub.
/// Deliberately shorter than [`CHECK_INTERVAL_SECS`] so jitter can never push
/// two legitimate cycles close enough together to be mistaken for a restart.
const MIN_RECHECK_GAP_SECS: i64 = 6 * 60 * 60;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether enough time has passed since the last check to run another one.
/// `None` (never checked) is always due.
fn due_for_check(last_checked_at: Option<i64>, now: i64) -> bool {
    match last_checked_at {
        None => true,
        Some(last) => now.saturating_sub(last) >= MIN_RECHECK_GAP_SECS,
    }
}

/// Whether `latest` has already had its `Notify` moment. `None` on either side
/// is "no", never "yes" — an unknown latest version is never something that
/// was already announced.
fn already_announced(last_announced: Option<&str>, latest: Option<&str>) -> bool {
    matches!((last_announced, latest), (Some(a), Some(l)) if a == l)
}

/// A pseudo-random offset in `[-JITTER_SECS, JITTER_SECS]`, using the same
/// `getrandom` this codebase already reaches for elsewhere (see
/// `clipboard::crypto`, `settings::secrets`) rather than adding a `rand`
/// dependency for one call site. Falls back to no jitter — not zero wait — if
/// entropy is ever unavailable, which only makes the interval exactly
/// `CHECK_INTERVAL_SECS` rather than breaking anything.
fn random_jitter_secs() -> i64 {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_err() {
        return 0;
    }
    let span = (JITTER_SECS as u64) * 2 + 1;
    (u64::from_le_bytes(buf) % span) as i64 - JITTER_SECS
}

/// How long the watcher sleeps between ticks. A tick that finds nothing due
/// still waits this long before checking again — the loop itself is cheap and
/// does not need its own back-off.
fn next_wait_secs() -> u64 {
    (CHECK_INTERVAL_SECS + random_jitter_secs()).max(60) as u64
}

/// Show a macOS notification banner.
///
/// The same `display notification … with title …` shape
/// `tools::timekeeping::notify` uses, reused via the shared
/// `tools::apple::run_script` rather than duplicated: both title and body here
/// ultimately come from a GitHub release tag, and an unescaped quote in one
/// would turn the rest of the AppleScript line into script.
fn notify(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        crate::shortcuts::escape_applescript(body),
        crate::shortcuts::escape_applescript(title),
    );
    // Best-effort: Settings → Update already shows the same information, so a
    // notification that fails to display (no permission granted yet) has not
    // actually lost anything.
    let _ = crate::tools::apple::run_script(&script);
}

fn notify_update_available(check: &UpdateCheck) {
    let version = check.latest_version.as_deref().unwrap_or("a new version");
    notify(
        &format!("Caduceus {version} is available"),
        "Open Settings → Update to install it.",
    );
}

/// One tick of the background watcher: decide whether a check is due, run it
/// if so, and act on the result according to [`UpdateMode`]. Always persists
/// whatever it learned — even "checked, nothing new" — so [`due_for_check`]
/// has something to compare against next time.
async fn run_watcher_tick<R: Runtime>(app: &AppHandle<R>) {
    let Some(manager) = app.try_state::<SettingsManager>() else {
        return;
    };
    let settings = manager.get();

    if settings.update.mode == UpdateMode::Off {
        return;
    }
    if !due_for_check(settings.update.last_checked_at, now_unix()) {
        return;
    }

    let result = check().await;
    let mut next = settings.clone();
    next.update.last_checked_at = Some(now_unix());

    if !result.update_available {
        let _ = crate::settings::save(app, &next);
        return;
    }

    // `Auto` still behaves like `Notify` for this cycle — rather than doing
    // nothing at all — whenever installing automatically would be the wrong
    // call: a Homebrew-managed copy (Homebrew's own upgrade is the correct
    // path, see `is_homebrew_managed`) or a Mac that is busy with something
    // Caduceus itself started (see `is_busy`). Either way the next tick tries
    // the automatic install again from scratch.
    let can_auto_install =
        settings.update.mode == UpdateMode::Auto && !result.homebrew_managed && !is_busy(app);

    if can_auto_install {
        match run_installer_detached() {
            Ok(()) => {
                // Caduceus is about to quit as part of the install; nothing
                // after this point will run anyway, but persist first so a
                // restart before the update lands still has an up-to-date
                // `last_checked_at`.
                let _ = crate::settings::save(app, &next);
                return;
            }
            Err(e) => {
                log::warn!("caduceus: automatic update could not start ({e}); will retry next cycle");
            }
        }
    }

    let _ = app.emit(crate::window::UPDATE_AVAILABLE_EVENT, &result);

    if !already_announced(
        next.update.last_announced_version.as_deref(),
        result.latest_version.as_deref(),
    ) {
        notify_update_available(&result);
        next.update.last_announced_version = result.latest_version.clone();
    }

    let _ = crate::settings::save(app, &next);
}

/// Start the background updater. Call this once at launch; it runs for the
/// life of the process.
///
/// One task: sleep past startup, then loop forever doing one
/// [`run_watcher_tick`] per wake and sleeping a jittered ~12 hours in between.
/// Reads `UpdateMode` fresh out of [`SettingsManager`] on every tick rather
/// than capturing it once, so flipping the setting in Settings takes effect
/// from the next wake without restarting anything.
pub fn spawn_update_watcher<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(INITIAL_DELAY_SECS)).await;
        loop {
            run_watcher_tick(&app).await;
            tokio::time::sleep(std::time::Duration::from_secs(next_wait_secs())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated file has to be valid shell, and has to actually contain
    /// the install command rather than a mangled version of it.
    #[test]
    fn the_update_script_is_valid_shell() {
        let script = update_script();
        assert!(script.starts_with("#!/bin/bash\n"));
        assert!(script.contains(INSTALL_COMMAND));

        let dir = std::env::temp_dir().join(format!("caduceus-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("update.command");
        std::fs::write(&path, &script).unwrap();

        let out = std::process::Command::new("bash")
            .arg("-n")
            .arg(&path)
            .output()
            .expect("bash should be available");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            out.status.success(),
            "generated script is not valid shell: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// It must not silently succeed when the install failed.
    #[test]
    fn the_update_script_reports_a_failed_install() {
        let script = update_script();
        assert!(script.contains("status=$?"));
        assert!(script.contains("did not finish"));
    }

    #[test]
    fn newer_patch_is_detected() {
        assert!(is_newer("3.1.3", "3.1.2"));
        assert!(!is_newer("3.1.2", "3.1.2"));
        assert!(!is_newer("3.1.1", "3.1.2"));
    }

    // -- Homebrew detection --------------------------------------------------

    #[test]
    fn caskroom_entry_is_detected_when_present() {
        let dir = std::env::temp_dir().join(format!("caduceus-brew-test-yes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Caskroom").join(HOMEBREW_CASK)).unwrap();

        assert!(caskroom_entry_exists(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caskroom_entry_is_absent_for_a_plain_directory() {
        let dir = std::env::temp_dir().join(format!("caduceus-brew-test-no-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(!caskroom_entry_exists(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caskroom_entry_is_absent_for_a_different_cask() {
        let dir = std::env::temp_dir().join(format!("caduceus-brew-test-other-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Caskroom").join("some-other-app")).unwrap();

        assert!(!caskroom_entry_exists(&dir));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Rate limiting and announce-once -------------------------------------

    #[test]
    fn a_fresh_install_is_due_immediately() {
        assert!(due_for_check(None, 1_000_000));
    }

    #[test]
    fn a_recent_check_is_not_due_again() {
        let now = 1_000_000;
        assert!(!due_for_check(Some(now - 60), now));
        assert!(!due_for_check(Some(now - MIN_RECHECK_GAP_SECS + 1), now));
    }

    #[test]
    fn a_check_older_than_the_gap_is_due() {
        let now = 1_000_000;
        assert!(due_for_check(Some(now - MIN_RECHECK_GAP_SECS), now));
        assert!(due_for_check(Some(now - MIN_RECHECK_GAP_SECS - 1), now));
    }

    #[test]
    fn restart_storm_does_not_repeatedly_check() {
        // Ten "restarts" in the same second: only the first would have found
        // nothing to compare against.
        let now = 1_000_000;
        let mut last_checked_at = None;
        let mut checks_run = 0;
        for _ in 0..10 {
            if due_for_check(last_checked_at, now) {
                checks_run += 1;
                last_checked_at = Some(now);
            }
        }
        assert_eq!(checks_run, 1);
    }

    #[test]
    fn a_version_never_announced_is_not_already_announced() {
        assert!(!already_announced(None, Some("4.2.0")));
    }

    #[test]
    fn the_same_version_is_already_announced() {
        assert!(already_announced(Some("4.2.0"), Some("4.2.0")));
    }

    #[test]
    fn a_newer_version_than_the_one_announced_is_not_already_announced() {
        assert!(!already_announced(Some("4.1.1"), Some("4.2.0")));
    }

    #[test]
    fn an_unknown_latest_version_is_never_already_announced() {
        // Defensive: `check()` should never leave `latest_version` empty
        // alongside `update_available: true`, but if it ever did, silence
        // would be the wrong failure mode.
        assert!(!already_announced(Some("4.1.1"), None));
    }

    #[test]
    fn jittered_wait_stays_within_bounds() {
        for _ in 0..50 {
            let wait = next_wait_secs() as i64;
            assert!(wait >= CHECK_INTERVAL_SECS - JITTER_SECS);
            assert!(wait <= CHECK_INTERVAL_SECS + JITTER_SECS);
        }
    }
}
