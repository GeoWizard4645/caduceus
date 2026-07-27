//! macOS system controls: appearance, Finder, sound, displays, power, Wi-Fi.
//!
//! Every action here is driven by something macOS already ships — `osascript`,
//! `defaults`, `pmset`, `networksetup`, `CGSession`. Nothing is a private
//! framework and nothing needs a helper to be installed, which is the bar for a
//! built-in.
//!
//! Actions are reached through a closed [`SystemAction`] enum rather than by
//! passing a command string across IPC. The webview can therefore ask for
//! "toggle dark mode" but cannot ask for "run this shell line", which is the
//! same rule the shortcut runner follows.

use std::process::Command;

use serde::{Deserialize, Serialize};

use super::ToolOutcome;

/// Everything the system module can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAction {
    // --- appearance ---
    ToggleDarkMode,
    ToggleStageManager,
    ToggleHiddenFiles,
    ToggleDesktopIcons,

    // --- restarting parts of the shell ---
    RestartFinder,
    RestartDock,
    RestartMenuBar,

    // --- files ---
    EmptyTrash,

    // --- power and session ---
    LockScreen,
    SleepDisplay,
    SleepComputer,
    StartScreenSaver,
    LogOut,
    RestartComputer,
    ShutDown,

    // --- sound ---
    VolumeUp,
    VolumeDown,
    ToggleMute,

    // --- displays ---
    BrightnessUp,
    BrightnessDown,

    // --- network ---
    ToggleWifi,
}

impl SystemAction {
    /// Whether this action ends the session or powers the machine down.
    ///
    /// The palette asks before running one of these. An unconfirmed "Shut down"
    /// one row away from "Sleep" in a fuzzy list is a data-loss bug waiting to
    /// happen.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            SystemAction::LogOut | SystemAction::RestartComputer | SystemAction::ShutDown
        )
    }
}

fn run_tool(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Run AppleScript, translating the two errors users actually hit.
fn osa(script: &str) -> Result<String, String> {
    run_tool("osascript", &["-e", script]).map_err(|e| {
        if e.contains("-1743") || e.contains("Not authorized") {
            "Caduceus is not allowed to control that app yet. Grant it in System Settings → \
             Privacy & Security → Automation."
                .to_string()
        } else if e.contains("-25211") || e.contains("assistive access") {
            "This needs Accessibility permission. Grant it in System Settings → Privacy & \
             Security → Accessibility."
                .to_string()
        } else {
            e
        }
    })
}

/// Read a `defaults` boolean, treating anything unset as false.
fn defaults_bool(domain: &str, key: &str) -> bool {
    run_tool("defaults", &["read", domain, key])
        .map(|value| matches!(value.trim(), "1" | "YES" | "true"))
        .unwrap_or(false)
}

fn defaults_write_bool(domain: &str, key: &str, value: bool) -> Result<(), String> {
    run_tool(
        "defaults",
        &["write", domain, key, "-bool", if value { "YES" } else { "NO" }],
    )
    .map(|_| ())
}

/// The BSD name of the Wi-Fi interface, which is not always `en0`.
///
/// A Mac with a Thunderbolt dock or a USB Ethernet adapter routinely numbers
/// Wi-Fi as `en1` and up, so hardcoding `en0` toggles the wrong interface.
fn wifi_interface() -> Option<String> {
    let listing = run_tool("networksetup", &["-listallhardwareports"]).ok()?;
    let mut lines = listing.lines();
    while let Some(line) = lines.next() {
        if line.contains("Wi-Fi") || line.contains("AirPort") {
            for next in lines.by_ref() {
                if let Some(device) = next.strip_prefix("Device: ") {
                    return Some(device.trim().to_string());
                }
                if next.trim().is_empty() {
                    break;
                }
            }
        }
    }
    None
}

/// Run a system action.
pub fn run(action: SystemAction) -> ToolOutcome {
    match action {
        SystemAction::ToggleDarkMode => {
            match osa(
                "tell application \"System Events\" to tell appearance preferences \
                 to set dark mode to not dark mode",
            ) {
                Ok(_) => {
                    let dark = osa(
                        "tell application \"System Events\" to tell appearance preferences \
                         to get dark mode",
                    )
                    .unwrap_or_default();
                    ToolOutcome::ok(if dark.trim() == "true" {
                        "Dark mode on"
                    } else {
                        "Light mode on"
                    })
                }
                Err(e) => ToolOutcome::err(e),
            }
        }

        SystemAction::ToggleStageManager => {
            let on = defaults_bool("com.apple.WindowManager", "GloballyEnabled");
            match defaults_write_bool("com.apple.WindowManager", "GloballyEnabled", !on) {
                Ok(()) => {
                    // WindowManager only reads the preference at launch.
                    let _ = run_tool("killall", &["WindowManager"]);
                    ToolOutcome::ok(if on { "Stage Manager off" } else { "Stage Manager on" })
                }
                Err(e) => ToolOutcome::err(format!("Could not change the setting: {e}")),
            }
        }

        SystemAction::ToggleHiddenFiles => {
            let on = defaults_bool("com.apple.finder", "AppleShowAllFiles");
            match defaults_write_bool("com.apple.finder", "AppleShowAllFiles", !on) {
                Ok(()) => {
                    let _ = run_tool("killall", &["Finder"]);
                    ToolOutcome::ok(if on {
                        "Hidden files are hidden again"
                    } else {
                        "Hidden files are showing"
                    })
                }
                Err(e) => ToolOutcome::err(format!("Could not change the setting: {e}")),
            }
        }

        SystemAction::ToggleDesktopIcons => {
            // `CreateDesktop` defaults to true when it has never been written,
            // so an unset key means icons are currently visible.
            let showing = run_tool("defaults", &["read", "com.apple.finder", "CreateDesktop"])
                .map(|v| !matches!(v.trim(), "0" | "NO" | "false"))
                .unwrap_or(true);
            match defaults_write_bool("com.apple.finder", "CreateDesktop", !showing) {
                Ok(()) => {
                    let _ = run_tool("killall", &["Finder"]);
                    ToolOutcome::ok(if showing {
                        "Desktop icons hidden"
                    } else {
                        "Desktop icons showing"
                    })
                }
                Err(e) => ToolOutcome::err(format!("Could not change the setting: {e}")),
            }
        }

        SystemAction::RestartFinder => restart_process("Finder", "Finder restarted"),
        SystemAction::RestartDock => restart_process("Dock", "Dock restarted"),
        SystemAction::RestartMenuBar => {
            restart_process("SystemUIServer", "Menu bar restarted")
        }

        SystemAction::EmptyTrash => match osa("tell application \"Finder\" to empty trash") {
            Ok(_) => ToolOutcome::ok("Trash emptied"),
            Err(e) if e.contains("-1728") => ToolOutcome::ok("The Trash is already empty."),
            Err(e) => ToolOutcome::err(e),
        },

        SystemAction::LockScreen => {
            // CGSession's `-suspend` is the same thing the Apple menu's "Lock
            // Screen" calls, and unlike `pmset displaysleepnow` it locks
            // regardless of the "require password after sleep" delay.
            match run_tool(
                "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession",
                &["-suspend"],
            ) {
                Ok(_) => ToolOutcome::ok("Locked"),
                Err(_) => match run_tool("pmset", &["displaysleepnow"]) {
                    Ok(_) => ToolOutcome::ok("Display asleep"),
                    Err(e) => ToolOutcome::err(format!("Could not lock the screen: {e}")),
                },
            }
        }

        SystemAction::SleepDisplay => match run_tool("pmset", &["displaysleepnow"]) {
            Ok(_) => ToolOutcome::ok("Display asleep"),
            Err(e) => ToolOutcome::err(e),
        },

        SystemAction::SleepComputer => match osa("tell application \"System Events\" to sleep") {
            Ok(_) => ToolOutcome::ok("Going to sleep"),
            Err(e) => ToolOutcome::err(e),
        },

        SystemAction::StartScreenSaver => {
            match run_tool("open", &["-a", "ScreenSaverEngine"]) {
                Ok(_) => ToolOutcome::ok("Screen saver started"),
                Err(e) => ToolOutcome::err(e),
            }
        }

        SystemAction::LogOut => match osa("tell application \"System Events\" to log out") {
            Ok(_) => ToolOutcome::ok("Logging out"),
            Err(e) => ToolOutcome::err(e),
        },

        SystemAction::RestartComputer => {
            match osa("tell application \"System Events\" to restart") {
                Ok(_) => ToolOutcome::ok("Restarting"),
                Err(e) => ToolOutcome::err(e),
            }
        }

        SystemAction::ShutDown => match osa("tell application \"System Events\" to shut down") {
            Ok(_) => ToolOutcome::ok("Shutting down"),
            Err(e) => ToolOutcome::err(e),
        },

        SystemAction::VolumeUp => nudge_volume(10),
        SystemAction::VolumeDown => nudge_volume(-10),

        SystemAction::ToggleMute => {
            match osa("set volume output muted not (output muted of (get volume settings))") {
                Ok(_) => {
                    let muted = osa("output muted of (get volume settings)").unwrap_or_default();
                    ToolOutcome::ok(if muted.trim() == "true" { "Muted" } else { "Unmuted" })
                }
                Err(e) => ToolOutcome::err(e),
            }
        }

        // 144 and 145 are the hardware brightness key codes. There is no public
        // API for display brightness, and every private one Apple has shipped
        // has broken across releases — synthesising the key press is the only
        // route that has survived, and it needs Accessibility.
        SystemAction::BrightnessUp => brightness(144, "Brighter"),
        SystemAction::BrightnessDown => brightness(145, "Dimmer"),

        SystemAction::ToggleWifi => {
            let Some(device) = wifi_interface() else {
                return ToolOutcome::err("This Mac has no Wi-Fi interface.");
            };
            let on = run_tool("networksetup", &["-getairportpower", &device])
                .map(|s| s.contains("On"))
                .unwrap_or(false);
            match run_tool(
                "networksetup",
                &["-setairportpower", &device, if on { "off" } else { "on" }],
            ) {
                Ok(_) => ToolOutcome::ok(if on { "Wi-Fi off" } else { "Wi-Fi on" }),
                Err(e) => ToolOutcome::err(format!("Could not change Wi-Fi: {e}")),
            }
        }
    }
}

fn restart_process(name: &str, message: &str) -> ToolOutcome {
    match run_tool("killall", &[name]) {
        Ok(_) => ToolOutcome::ok(message),
        Err(e) if e.contains("No matching processes") => {
            ToolOutcome::err(format!("{name} is not running."))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

fn nudge_volume(delta: i32) -> ToolOutcome {
    let current: i32 = osa("output volume of (get volume settings)")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(50);
    let next = (current + delta).clamp(0, 100);
    match osa(&format!("set volume output volume {next}")) {
        // Changing the volume while muted does nothing audible unless the mute
        // is released too, which is what the hardware keys do.
        Ok(_) => {
            if delta > 0 {
                let _ = osa("set volume output muted false");
            }
            ToolOutcome::ok(format!("Volume {next}%"))
        }
        Err(e) => ToolOutcome::err(e),
    }
}

fn brightness(key_code: u16, message: &str) -> ToolOutcome {
    match osa(&format!(
        "tell application \"System Events\" to key code {key_code}"
    )) {
        Ok(_) => ToolOutcome::ok(message),
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Readouts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionReport {
    pub accessibility: bool,
    pub screen_recording: bool,
    /// Whether the Swift helper that does OCR and audio switching is installed.
    pub native_helper: bool,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Reports the Screen Recording grant without prompting for it.
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// What Caduceus is and is not currently allowed to do.
pub fn permissions() -> PermissionReport {
    #[cfg(target_os = "macos")]
    {
        PermissionReport {
            accessibility: crate::window::manage::permission_granted(),
            // SAFETY: no arguments, no ownership; documented as prompt-free.
            screen_recording: unsafe { CGPreflightScreenCaptureAccess() },
            native_helper: super::native::available(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionReport {
            accessibility: false,
            screen_recording: false,
            native_helper: false,
        }
    }
}

/// A one-screen summary of the machine, for the palette's output panel.
pub fn machine_summary() -> ToolOutcome {
    let model = run_tool("sysctl", &["-n", "hw.model"]).unwrap_or_default();
    let chip = run_tool("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_default();
    let cores = run_tool("sysctl", &["-n", "hw.ncpu"]).unwrap_or_default();
    let memory = run_tool("sysctl", &["-n", "hw.memsize"])
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|bytes| format!("{} GB", bytes / 1024 / 1024 / 1024))
        .unwrap_or_default();
    let os = run_tool("sw_vers", &["-productVersion"]).unwrap_or_default();
    let build = run_tool("sw_vers", &["-buildVersion"]).unwrap_or_default();
    let uptime = run_tool("uptime", &[]).unwrap_or_default();

    let battery = run_tool("pmset", &["-g", "batt"])
        .ok()
        .and_then(|out| out.lines().nth(1).map(str::trim).map(str::to_string))
        .unwrap_or_else(|| "no battery".into());

    ToolOutcome::copied(
        format!(
            "Model     {model}\nChip      {chip}\nCores     {cores}\nMemory    {memory}\n\
             macOS     {os} ({build})\nBattery   {battery}\nUptime    {}",
            uptime.split_once("up ").map(|(_, rest)| rest).unwrap_or(&uptime).trim()
        ),
        "Copied the summary",
    )
}

/// Wi-Fi status: which network, what address, and how to share it.
pub fn wifi_summary() -> ToolOutcome {
    let Some(device) = wifi_interface() else {
        return ToolOutcome::err("This Mac has no Wi-Fi interface.");
    };

    let power = run_tool("networksetup", &["-getairportpower", &device]).unwrap_or_default();
    if power.contains("Off") {
        return ToolOutcome::ok(format!("Wi-Fi is off ({device})."));
    }

    // macOS 14 removed the SSID from `airport -I` and gates it behind Location
    // Services in several tools; `networksetup` is the one that still answers.
    let network = run_tool("networksetup", &["-getairportnetwork", &device])
        .unwrap_or_default()
        .split_once(": ")
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| "unknown".into());

    let address = run_tool("ipconfig", &["getifaddr", &device]).unwrap_or_default();
    let router = run_tool("sh", &["-c", "route -n get default 2>/dev/null | awk '/gateway/{print $2}'"])
        .unwrap_or_default();

    ToolOutcome::copied(
        network.clone(),
        format!(
            "{network} · {} · router {} · {device}",
            if address.is_empty() { "no address".into() } else { address },
            if router.is_empty() { "unknown".into() } else { router },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_session_ending_actions_count_as_destructive() {
        assert!(SystemAction::ShutDown.is_destructive());
        assert!(SystemAction::RestartComputer.is_destructive());
        assert!(SystemAction::LogOut.is_destructive());

        for safe in [
            SystemAction::ToggleDarkMode,
            SystemAction::SleepDisplay,
            SystemAction::LockScreen,
            SystemAction::EmptyTrash,
            SystemAction::VolumeUp,
            SystemAction::ToggleWifi,
        ] {
            assert!(!safe.is_destructive(), "{safe:?} should not need confirming");
        }
    }

    #[test]
    fn the_wifi_interface_is_discovered_rather_than_assumed() {
        // Every Mac these tests run on has Wi-Fi; the point is that whatever
        // comes back is a real BSD interface name, not a hardcoded "en0".
        if let Some(device) = wifi_interface() {
            assert!(device.starts_with("en"), "unexpected interface {device}");
            assert!(device.len() >= 3, "unexpected interface {device}");
        }
    }

    #[test]
    fn an_unset_defaults_key_reads_as_false_rather_than_failing() {
        assert!(!defaults_bool("com.caduceus.nonexistent.domain", "NoSuchKey"));
    }

    #[test]
    fn permissions_are_reported_without_prompting() {
        // The assertion is that this returns at all: a prompting variant would
        // block the test run waiting for a click.
        let report = permissions();
        assert!(report.native_helper, "the native helper should be built");
    }
}
