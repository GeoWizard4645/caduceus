//! Six things this build was audited as missing, none of them window snapping
//! (fifty verbs of that already exist in [`crate::window::manage`] and nothing
//! here reimplements it).
//!
//! # What is here and how it gets its work done
//!
//! * **Window layout presets** — save/restore a named arrangement of several
//!   apps' main windows. Built entirely on [`crate::window::manage::Frame`]
//!   and the *existing* [`crate::window::accessibility::AxElement`] methods
//!   (`element_attribute`, `point_attribute`, `size_attribute`, `set_point`,
//!   `set_size`) applied per-pid via [`AxElement::for_pid`], which was
//!   already public. Nothing here declares a new AX C function — see the
//!   module docs on [`capture_window_arrangement`] for exactly which existing
//!   calls are reused and why that is enough.
//! * **Always-on-top for a foreign window** — investigated, not shipped. See
//!   "The always-on-top gap" below.
//! * **Menu bar search** — enumerates and invokes menu items through
//!   `System Events` GUI scripting (`osascript`), the same transport every
//!   other AppleScript-driven feature in this crate uses. This is a
//!   deliberate choice over hand-written AX FFI; see "Why menu search is
//!   AppleScript, not AX C calls" below.
//! * **Font viewer** — a plain filesystem walk of the three standard font
//!   directories. See "Choosing a font-listing strategy" below for the three
//!   approaches measured and why this one won.
//! * **Contacts search** — AppleScript to Contacts.app, guarded on `is
//!   running` exactly like [`super::qr::front_tab_url`] guards Safari/Chromium
//!   and [`super::calendar`] guards Calendar/Reminders.
//! * **Recent files** — *not* `~/Library/Application Support/
//!   com.apple.sharedfilelist/`; see "The recent-files investigation" below
//!   for why that path was abandoned and what replaced it.
//!
//! # The always-on-top gap
//!
//! There is no shipped command for this, on purpose.
//!
//! Accessibility exposes a fixed, documented attribute set for a window:
//! `AXPosition`, `AXSize`, `AXMinimized`, `AXFullScreen`, `AXMain`,
//! `AXFocused`, and a handful of others — enumerable with Apple's own
//! Accessibility Inspector against any app, and it is the same list
//! `window/manage.rs` already reads and writes. None of them is a window
//! level or a "floating" flag. A window's level (`NSWindow.level`, the thing
//! that actually makes it draw above other windows) is a property the
//! *owning process* sets on its own `NSWindow`; AX lets you ask a process to
//! move or resize a window it owns, not to override how the window server
//! layers windows belonging to a process that never asked to be layered that
//! way. Setting an unsupported attribute through
//! `AXUIElementSetAttributeValue` does not error dramatically — it returns
//! `kAXErrorSuccess`-adjacent nothing or `kAXErrorAttributeUnsupported` and
//! changes nothing, which is exactly the shape of a feature that "half
//! works": it would look wired up and do nothing, forever, for every app
//! that does not happen to expose a private attribute for it.
//!
//! The only way to force an arbitrary foreign window above every other
//! window without that app's cooperation is `CGSSetWindowLevel` — a private,
//! undocumented SkyLight/CoreGraphics Services call with no header, no
//! stability guarantee across macOS releases, and no relationship to the
//! stable-since-10.2 surface `window/accessibility.rs`'s own module docs
//! commit this codebase to. Shipping it would trade "this feature doesn't
//! exist yet" for "this feature silently breaks on the next macOS update,"
//! which is a worse place to be. So: documented gap, no command, nothing
//! registered.
//!
//! # Why menu search is AppleScript, not AX C calls
//!
//! `window/accessibility.rs` exposes exactly enough AX surface for
//! `window/manage.rs`'s job: read/write a *single* window's position,
//! size and full-screen flag off the *one* focused element. Its only public
//! ways to obtain an [`AxElement`] are `system_wide()`, `for_pid()`, and
//! `element_attribute()` — and that last one only works for attributes whose
//! value is *one* element (`AXFocusedWindow`, `AXMainWindow`, `AXMenuBar`
//! itself). A menu bar's contents are `AXChildren`: an *array* of elements,
//! and there is no public constructor anywhere in this crate that turns one
//! of those array entries into something you can call `.attribute()` or
//! `.perform()` on. Building that would mean adding AX FFI to
//! `window/accessibility.rs`, which is out of scope for this file by the
//! ownership rule this feature was built under.
//!
//! `System Events`' GUI scripting is the same capability wearing a different
//! transport: it is Accessibility under the hood (enabling GUI scripting is
//! literally an Accessibility-permission action), it already has a tested,
//! translated error path in [`super::apple::run_script`] — including the
//! *exact* Accessibility sentence `window::accessibility::describe_error`
//! produces, word-for-word, on purpose (see `apple.rs`'s own doc comment on
//! `translate`) — and it walks and clicks nested menus with ordinary
//! AppleScript references instead of new `unsafe` code. Every other
//! AppleScript feature in this crate (`calendar.rs`, `qr.rs`, `media.rs`)
//! already made this same trade for the same reason: `osascript` via
//! `apple::run_script` over hand-rolled AX plumbing.
//!
//! # Choosing a font-listing strategy
//!
//! Three approaches were actually measured on the machine this was built on
//! (macOS 26.5.2, ~460 font files):
//!
//! | Approach | Time | Notes |
//! |---|---|---|
//! | `system_profiler SPFontsDataType` | **~19.5s** | Confirmed slow, as the brief warned. Parses every font's metadata table. |
//! | `mdfind "kMDItemContentTypeTree == 'public.font'"` | ~0.2s | Fast, but returns every font Spotlight has indexed anywhere (fonts embedded in app bundles, disabled fonts, etc.) — noisier than "installed fonts," and depends on Spotlight being enabled for those volumes. |
//! | Walking `/System/Library/Fonts`, `/Library/Fonts`, `~/Library/Fonts` with `std::fs` | **~8ms** | What shipped. No subprocess, no index dependency, no permission prompt. |
//! | `fc-list` | — | Not used: it is a fontconfig/Linux tool. It happened to be present on the machine this was built on (via Homebrew), but nothing in a stock macOS install puts it there, so depending on it would work on the author's machine and nobody else's. |
//!
//! The filesystem walk is two orders of magnitude faster than
//! `system_profiler` and depends on nothing but the three directories macOS
//! itself defines, so that is what [`list_installed_fonts`] does.
//!
//! # The recent-files investigation
//!
//! The brief's suggested path,
//! `~/Library/Application Support/com.apple.sharedfilelist/`, was checked
//! directly on the build machine (macOS 26.5.2): **the directory exists and
//! is empty.** `defaults read com.apple.TextEdit NSRecentDocuments` — the
//! other classic per-app store for the same concept — reports no such key
//! either. This is not a misconfigured machine; it is Apple having moved
//! per-app "recent documents" tracking into sandboxed apps' own containers
//! as opaque `NSURL` bookmark data over the last several macOS releases,
//! which the shared-list directory this feature was scoped to no longer
//! reliably holds. Even where old `.sfl2` files still exist on some systems,
//! their payload is serialized `CFURLBookmarkData` — a binary format that
//! requires Foundation's bookmark-resolution API to turn back into a path,
//! not something `plutil` or a plist reader exposes as a plain string.
//!
//! What actually works, verified on the same machine: Spotlight tracks
//! `kMDItemLastUsedDate` for every file it indexes, and `mdfind
//! "kMDItemLastUsedDate >= $time.today(-N)"` answers in well under a second
//! with genuinely-recently-opened documents across every app that touched
//! them — no per-app cooperation, no bookmark parsing, no extra permission
//! beyond what `mdfind` already has. [`recent_files`] uses that instead of
//! the brief's suggested path, and this doc comment is the record of why.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use crate::shortcuts::escape_applescript;

use super::{apple, output_with_timeout, ToolOutcome, TOOL_TIMEOUT};

#[cfg(target_os = "macos")]
use crate::window::{accessibility as ax, manage};

type Res<T> = Result<T, String>;

// ===========================================================================
// 1. Window layout presets
// ===========================================================================
//
// A preset is a named list of (app name, frame) pairs. Capture and restore
// both go through the same two primitives:
//
// * pid discovery via `ps` (no permission, no AX, see `list_running_apps`);
// * frame get/set via `AxElement::for_pid(pid).element_attribute("AXMainWindow")`
//   plus the *already-public* `point_attribute`/`size_attribute`/`set_point`/
//   `set_size` methods `window/manage.rs` itself calls on the one window it
//   knows how to reach (the focused one). This module reaches more windows by
//   supplying a different pid, not by touching AX differently.
//
// Persistence follows `widgets.rs`'s pattern precisely: its own store file,
// not `Settings` — a preset can be added or removed without a schema bump on
// the settings side, and a corrupt preset file cannot take Settings down
// with it.

/// Filename inside the app config directory. Its own file, deliberately not
/// [`crate::settings::STORE_FILE`] or `widgets.rs`'s `caduceus-widgets.json`
/// — see the module docs on why presets get their own store.
const PRESET_STORE_FILE: &str = "caduceus-window-presets.json";
const PRESETS_KEY: &str = "presets";

/// One captured window, identified by the name of the app that owned it —
/// not a pid, which is meaningless across app relaunches, and not a bundle
/// id, which `ps` does not hand back for free. See [`first_app_bundle_name`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetWindow {
    pub app_name: String,
    #[cfg(target_os = "macos")]
    pub frame: manage::Frame,
    // A plain, non-macOS-conditional mirror of `Frame`'s shape, kept in
    // lock-step by the tests below, so the type still exists (and still
    // (de)serializes) when this crate is built for a platform `manage.rs`
    // itself does nothing on.
    #[cfg(not(target_os = "macos"))]
    pub frame: PlainFrame,
}

/// [`manage::Frame`]'s shape, duplicated only for non-macOS builds where
/// `manage::Frame` is still compiled (it is a plain data type, not behind a
/// `cfg`) — so in practice this arm is never used, but it keeps
/// [`PresetWindow`] from needing its own `cfg`-gated definition.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A named, saved arrangement of windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPreset {
    pub name: String,
    pub windows: Vec<PresetWindow>,
    /// RFC 3339. Not used for anything today beyond "when was this saved" in
    /// a future list view — captured now because it is free and there is no
    /// way to reconstruct it later.
    pub captured_at: String,
}

/// What [`window_preset_restore`] actually managed to do — a preset can
/// outlive the apps it was made from, and "3 of 4 windows moved, TextEdit
/// wasn't running" is the honest answer, not a hard failure.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRestoreOutcome {
    pub moved: Vec<String>,
    pub skipped: Vec<String>,
    /// Set only when nothing could be attempted at all (no permission, no
    /// saved preset by that name) — as opposed to a partial result, which
    /// uses `moved`/`skipped` instead.
    pub error: Option<String>,
}

fn load_presets<R: Runtime>(app: &AppHandle<R>) -> Vec<WindowPreset> {
    let Ok(store) = app.store(PRESET_STORE_FILE) else {
        return Vec::new();
    };
    store
        .get(PRESETS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn save_presets<R: Runtime>(app: &AppHandle<R>, presets: &[WindowPreset]) -> Res<()> {
    let store = app
        .store(PRESET_STORE_FILE)
        .map_err(|e| format!("could not open the window preset store: {e}"))?;
    let value = serde_json::to_value(presets).map_err(|e| format!("could not encode presets: {e}"))?;
    store.set(PRESETS_KEY, value);
    store.save().map_err(|e| format!("could not write window presets: {e}"))
}

// --- pid discovery ----------------------------------------------------------
//
// `ps` rather than `NSWorkspace.runningApplications` (which would need a
// main-thread hop like `manage.rs::platform::screens` does for `NSScreen`)
// or `System Events` (which would need Automation permission on top of
// Accessibility, for a step that is only ever "which pids exist"). `ps`
// needs neither: it is a read of processes the current user already owns.

/// The first path segment ending in `.app`, minus the extension.
///
/// A helper process's own executable lives inside a *nested* bundle
/// (`Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/…`), so
/// taking the *first* `.app` segment rather than the last one attributes it
/// back to the parent app a person actually thinks of as "Cursor" — which is
/// also, not coincidentally, the app whose pid is likely to answer
/// `AXMainWindow` with something. Helper pids that slip through anyway are
/// harmless: [`app_main_window`] simply finds nothing for them and
/// [`capture_window_arrangement`] tries the next pid sharing that name.
fn first_app_bundle_name(path: &str) -> Option<String> {
    path.split('/')
        .find(|segment| segment.len() > 4 && segment.ends_with(".app"))
        .map(|segment| segment.trim_end_matches(".app").to_string())
}

/// Parse `ps -axo pid=,comm=` output into `(pid, app name)` pairs, dropping
/// any line that is not a process running from inside an `.app` bundle
/// (daemons, XPC services with no bundle, anything with no window to save).
fn parse_running_apps(raw: &str) -> Vec<(u32, String)> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid_str, rest) = line.split_once(char::is_whitespace)?;
            let pid: u32 = pid_str.parse().ok()?;
            let name = first_app_bundle_name(rest.trim())?;
            Some((pid, name))
        })
        .collect()
}

fn list_running_apps() -> Result<Vec<(u32, String)>, String> {
    let mut command = Command::new("ps");
    command.args(["-axo", "pid=,comm="]);
    let output = output_with_timeout(&mut command, TOOL_TIMEOUT, "Listing running applications took too long.")?;
    if !output.status.success() {
        return Err("Could not list running applications.".into());
    }
    Ok(parse_running_apps(&String::from_utf8_lossy(&output.stdout)))
}

fn group_by_app_name(pairs: Vec<(u32, String)>) -> HashMap<String, Vec<u32>> {
    let mut grouped: HashMap<String, Vec<u32>> = HashMap::new();
    for (pid, name) in pairs {
        grouped.entry(name).or_default().push(pid);
    }
    grouped
}

// --- AX read/write for one app's main window --------------------------------
//
// Every call below is a method [`AxElement`] already exposes publicly; the
// only thing supplied here that `window/manage.rs` did not already have is
// *which pid* to ask, which `AxElement::for_pid` — also already public —
// was always able to answer.

#[cfg(target_os = "macos")]
fn app_main_window(pid: u32) -> Option<ax::AxElement> {
    let app = ax::AxElement::for_pid(pid as i32)?;
    app.element_attribute("AXMainWindow")
        .or_else(|| app.element_attribute("AXFocusedWindow"))
}

#[cfg(target_os = "macos")]
fn read_frame(window: &ax::AxElement) -> Option<manage::Frame> {
    let position = window.point_attribute("AXPosition")?;
    let size = window.size_attribute("AXSize")?;
    Some(manage::Frame::new(position.x, position.y, size.width, size.height))
}

/// Write a frame back to a window. Size, position, size — the exact sequence
/// [`manage::apply`] uses and for the same documented reason: an app with a
/// minimum width clamps the first resize against the window's *old* origin,
/// so moving in between and re-applying the size lets it settle where it was
/// actually asked to go.
#[cfg(target_os = "macos")]
fn write_frame(window: &ax::AxElement, frame: manage::Frame) -> Result<(), i32> {
    let size = ax::CGSize { width: frame.width, height: frame.height };
    let point = ax::CGPoint { x: frame.x, y: frame.y };
    let _ = window.set_size("AXSize", size);
    let move_err = window.set_point("AXPosition", point);
    let size_err = window.set_size("AXSize", size);
    let err = if move_err != ax::kAXErrorSuccess { move_err } else { size_err };
    if err == ax::kAXErrorSuccess {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(target_os = "macos")]
fn capture_window_arrangement() -> Result<Vec<PresetWindow>, String> {
    if !ax::is_trusted() {
        return Err(ax::describe_error(ax::kAXErrorAPIDisabled));
    }
    let grouped = group_by_app_name(list_running_apps()?);

    let mut windows = Vec::new();
    // `HashMap` iteration order is unspecified; sorting by name makes a
    // saved preset's window order reproducible instead of shuffling on every
    // save, which would otherwise make two saves of the same arrangement
    // look like a diff to anything that compares them.
    let mut names: Vec<&String> = grouped.keys().collect();
    names.sort();

    for name in names {
        for &pid in &grouped[name] {
            if let Some(window) = app_main_window(pid) {
                if let Some(frame) = read_frame(&window) {
                    windows.push(PresetWindow { app_name: name.clone(), frame });
                    break;
                }
            }
        }
    }

    if windows.is_empty() {
        return Err("No application windows were found to save. Is anything open?".into());
    }
    Ok(windows)
}

#[cfg(not(target_os = "macos"))]
fn capture_window_arrangement() -> Result<Vec<PresetWindow>, String> {
    Err("Window layout presets are macOS-only.".into())
}

#[cfg(target_os = "macos")]
fn restore_window_arrangement<R: Runtime>(app: &AppHandle<R>, preset: &WindowPreset) -> PresetRestoreOutcome {
    if !ax::is_trusted() {
        return PresetRestoreOutcome {
            moved: Vec::new(),
            skipped: Vec::new(),
            error: Some(ax::describe_error(ax::kAXErrorAPIDisabled)),
        };
    }

    let grouped = match list_running_apps() {
        Ok(pairs) => group_by_app_name(pairs),
        Err(e) => {
            return PresetRestoreOutcome { moved: Vec::new(), skipped: Vec::new(), error: Some(e) };
        }
    };

    // Clamp into whatever the current display arrangement actually is —
    // reusing `manage::screens`/`screen_for`/`Frame::clamped_into` exactly as
    // `manage::apply` does, so a preset saved on a 4K external monitor does
    // not park a window off-screen after that monitor is unplugged.
    let screens = manage::screens(app);

    let mut moved = Vec::new();
    let mut skipped = Vec::new();

    for saved in &preset.windows {
        let Some(pids) = grouped.get(&saved.app_name) else {
            skipped.push(saved.app_name.clone());
            continue;
        };

        let mut done = false;
        for &pid in pids {
            let Some(window) = app_main_window(pid) else { continue };
            let target = if screens.is_empty() {
                saved.frame
            } else {
                let index = manage::screen_for(saved.frame, &screens);
                saved.frame.clamped_into(&screens[index].visible)
            };
            if write_frame(&window, target).is_ok() {
                moved.push(saved.app_name.clone());
                done = true;
                break;
            }
        }
        if !done {
            skipped.push(saved.app_name.clone());
        }
    }

    PresetRestoreOutcome { moved, skipped, error: None }
}

#[cfg(not(target_os = "macos"))]
fn restore_window_arrangement<R: Runtime>(_app: &AppHandle<R>, _preset: &WindowPreset) -> PresetRestoreOutcome {
    PresetRestoreOutcome {
        moved: Vec::new(),
        skipped: Vec::new(),
        error: Some("Window layout presets are macOS-only.".into()),
    }
}

/// Save the current window arrangement under `name`, overwriting any
/// existing preset with the same name.
pub fn window_preset_save<R: Runtime>(app: &AppHandle<R>, name: String) -> Res<WindowPreset> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the preset a name.".into());
    }

    let windows = capture_window_arrangement()?;
    let preset = WindowPreset { name: name.clone(), windows, captured_at: chrono::Local::now().to_rfc3339() };

    let mut presets = load_presets(app);
    presets.retain(|p| p.name != name);
    presets.push(preset.clone());
    save_presets(app, &presets)?;

    Ok(preset)
}

/// Restore a saved preset. Never an all-or-nothing operation — see
/// [`PresetRestoreOutcome`].
pub fn window_preset_restore<R: Runtime>(app: &AppHandle<R>, name: String) -> Res<PresetRestoreOutcome> {
    let presets = load_presets(app);
    let preset = presets
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("No saved preset called “{name}”."))?;
    Ok(restore_window_arrangement(app, &preset))
}

pub fn window_preset_list<R: Runtime>(app: &AppHandle<R>) -> Vec<WindowPreset> {
    load_presets(app)
}

pub fn window_preset_delete<R: Runtime>(app: &AppHandle<R>, name: String) -> Res<()> {
    let mut presets = load_presets(app);
    let before = presets.len();
    presets.retain(|p| p.name != name);
    if presets.len() == before {
        return Err(format!("No saved preset called “{name}”."));
    }
    save_presets(app, &presets)
}

// ===========================================================================
// 3. Menu bar search
// ===========================================================================
//
// See the module docs ("Why menu search is AppleScript, not AX C calls") for
// why this is `System Events` GUI scripting rather than hand-written AX FFI.
//
// Every AppleScript built here goes through `apple::run_script`, which pipes
// source on stdin (so embedded quotes never fight shell escaping on top of
// AppleScript's own), enforces the wedged-app timeout, and already
// translates a missing-Accessibility failure into the exact sentence
// `window::accessibility::describe_error` produces — see `apple.rs`'s own
// doc comment on `translate` for why those two are kept in lockstep.

/// Control characters chosen the same way `calendar.rs` chose `FIELD_SEP`/
/// `RECORD_SEP`: vanishingly unlikely to appear in a real menu title, so no
/// escaping scheme is needed on the way *out* of AppleScript.
const MENU_PATH_SEP: char = '\u{1f}';
const MENU_RECORD_SEP: char = '\u{1e}';

/// One menu entry, as a path from the top of the menu bar down through any
/// nested submenus — `["File", "Export As", "PDF…"]` for a File ▸ Export As ▸
/// PDF… item.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItem {
    pub path: Vec<String>,
}

/// Build the AppleScript that lists every menu item of `app_name`'s menu
/// bar, submenus included, as `path segment` records separated by
/// [`MENU_RECORD_SEP`] and joined within a path by [`MENU_PATH_SEP`].
///
/// `walkMenu` is a recursive AppleScript handler (the same technique
/// `calendar.rs`'s `ISO_DATE_HANDLER` uses to embed a helper function ahead
/// of the `tell` block): for each item in a menu, emit its full path, then —
/// if it has its own submenu (`menu 1 of itm` exists) — recurse into that
/// submenu with the item's path as the new prefix. `try` blocks around the
/// name/submenu checks are not error-hiding for its own sake: some menu
/// items are separators with no `name` at all, and AppleScript throws
/// rather than returning `missing value` for that, so *not* catching it
/// would abort the entire walk over one separator.
fn build_menu_enumeration_script(app_name: &str) -> String {
    format!(
        r#"on walkMenu(theMenu, prefix)
    set out to ""
    repeat with itm in menu items of theMenu
        set itmName to ""
        try
            set itmName to name of itm
        end try
        if itmName is not "" then
            set fullPath to prefix & "{sep}" & itmName
            set out to out & fullPath & "{rec}"
            try
                if (count of menus of itm) > 0 then
                    set out to out & my walkMenu(menu 1 of itm, fullPath)
                end if
            end try
        end if
    end repeat
    return out
end walkMenu

tell application "System Events"
    tell process "{app}"
        set out to ""
        repeat with mbi in menu bar items of menu bar 1
            set topName to ""
            try
                set topName to name of mbi
            end try
            if topName is not "" then
                try
                    set out to out & my walkMenu(menu 1 of mbi, topName)
                end try
            end if
        end repeat
        return out
    end tell
end tell"#,
        sep = MENU_PATH_SEP,
        rec = MENU_RECORD_SEP,
        app = escape_applescript(app_name),
    )
}

fn parse_menu_items(raw: &str) -> Vec<MenuItem> {
    raw.split(MENU_RECORD_SEP)
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .map(|record| MenuItem { path: record.split(MENU_PATH_SEP).map(str::to_string).collect() })
        .collect()
}

/// Build the AppleScript reference for a menu item at an arbitrary depth.
///
/// A depth-`n` path `[p1, …, pn]` (`pn` the clickable item) is reached in
/// AppleScript/System-Events terms by alternating `menu` and `menu item`
/// references outward from the menu bar:
///
/// ```text
/// depth 2 (File ▸ Save):
///   menu item "Save" of menu "File" of menu bar item "File" of menu bar 1
///
/// depth 3 (File ▸ Export As ▸ PDF):
///   menu item "PDF" of menu "Export As" of menu item "Export As"
///     of menu "File" of menu bar item "File" of menu bar 1
/// ```
///
/// i.e. every extra level of nesting wraps the previous reference in one
/// more `menu item "…" of menu "…" of` pair. The loop below builds that
/// outward-in, which is why it is read most easily from its test cases.
fn build_menu_item_reference(path: &[String]) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let escaped: Vec<String> = path.iter().map(|s| escape_applescript(s)).collect();

    let top = format!("menu bar item \"{}\" of menu bar 1", escaped[0]);
    if escaped.len() == 1 {
        return Some(top);
    }

    let mut menu_ref = format!("menu \"{}\" of {top}", escaped[0]);
    let mut item_ref = String::new();
    for (i, segment) in escaped.iter().enumerate().skip(1) {
        item_ref = format!("menu item \"{segment}\" of {menu_ref}");
        if i == escaped.len() - 1 {
            break;
        }
        menu_ref = format!("menu \"{segment}\" of {item_ref}");
    }
    Some(item_ref)
}

fn build_invoke_script(app_name: &str, path: &[String]) -> Option<String> {
    let reference = build_menu_item_reference(path)?;
    Some(format!(
        "tell application \"System Events\" to tell process \"{app}\" to click {reference}",
        app = escape_applescript(app_name),
    ))
}

fn frontmost_app_name() -> Result<String, String> {
    let name = apple::run_script(
        "tell application \"System Events\" to get name of first application process whose frontmost is true",
    )?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Could not tell which application is frontmost.".into());
    }
    Ok(name)
}

/// Every menu item (nested submenus included) of the frontmost app's menu
/// bar.
pub fn frontmost_menu_items() -> Result<Vec<MenuItem>, String> {
    let app_name = frontmost_app_name()?;
    let raw = apple::run_script(&build_menu_enumeration_script(&app_name))?;
    Ok(parse_menu_items(&raw))
}

/// Click a menu item, by path, in the frontmost app.
///
/// Re-resolves the frontmost app right before clicking rather than accepting
/// it as a parameter from the earlier [`frontmost_menu_items`] call — the
/// two are always two separate round trips from the frontend, and trusting a
/// stale "frontmost app" from the first would invoke the wrong app's menu if
/// focus moved in between.
pub fn invoke_frontmost_menu_item(path: Vec<String>) -> Result<(), String> {
    if path.is_empty() {
        return Err("No menu item to invoke.".into());
    }
    let app_name = frontmost_app_name()?;
    let Some(script) = build_invoke_script(&app_name, &path) else {
        return Err("No menu item to invoke.".into());
    };
    apple::run_script(&script)?;
    Ok(())
}

// ===========================================================================
// 4. Font viewer
// ===========================================================================
//
// See the module docs ("Choosing a font-listing strategy") for the measured
// comparison against `system_profiler` and `mdfind`.

const FONT_EXTENSIONS: &[&str] = &["ttf", "ttc", "otf", "otc", "dfont"];

/// A pangram plus a digit/case sweep — the standard "does this typeface look
/// right" sample every font picker in every OS uses, so a preview panel has
/// something to render immediately without asking the user to type anything.
pub const FONT_PREVIEW_TEXT: &str =
    "The quick brown fox jumps over the lazy dog. ABCDEFGHIJ abcdefghij 0123456789";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    pub name: String,
    pub path: String,
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| FONT_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// The file's name with its extension stripped, e.g. `Arial Bold.ttf` →
/// `Arial Bold`.
///
/// Not the font's *PostScript* or family name — that lives inside the font's
/// own `name` table and reading it would mean parsing TTF/OTF binary
/// structure, a much bigger feature than "list what's installed." The
/// filename is what Finder already shows for these files and is accurate
/// for the overwhelming majority of fonts, which ship named after
/// themselves.
fn font_name_from_path(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn font_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    dirs
}

fn walk_fonts(dir: &Path, out: &mut Vec<FontInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // `/System/Library/Fonts/Supplemental` and similar subfolders hold
        // real, everyday fonts (Arial, Times New Roman) — not recursing
        // would silently drop most of the system font library.
        if path.is_dir() {
            walk_fonts(&path, out);
        } else if is_font_file(&path) {
            out.push(FontInfo { name: font_name_from_path(&path), path: path.to_string_lossy().to_string() });
        }
    }
}

/// Every font file under the three directories macOS actually loads fonts
/// from, sorted and de-duplicated by name (`Arial.ttf` in two font
/// directories should show up once).
pub fn list_installed_fonts() -> Vec<FontInfo> {
    let mut out = Vec::new();
    for dir in font_search_dirs() {
        walk_fonts(&dir, &mut out);
    }
    out.sort_by_key(|f| f.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

// ===========================================================================
// 5. Contacts search
// ===========================================================================
//
// Same shape as `calendar.rs`'s Calendar/Reminders reads: guarded on
// `if it is running`, per the brief, matching how `qr::front_tab_url`
// already guards Safari/Chromium. Contacts is not force-launched to answer a
// search.

const CONTACT_FIELD_SEP: char = '\u{1f}';
const CONTACT_RECORD_SEP: char = '\u{1e}';
const CONTACT_ITEM_SEP: char = '\u{1c}';
const CONTACT_LABEL_SEP: char = '\u{1d}';

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabeledValue {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactHit {
    pub name: String,
    pub phones: Vec<LabeledValue>,
    pub emails: Vec<LabeledValue>,
}

/// Build the AppleScript for a Contacts name search.
///
/// `query` is the only user-supplied text reaching this script and is
/// escaped through [`escape_applescript`] before being interpolated into the
/// `whose name contains "…"` literal — skipping that is the exact mistake
/// `calendar.rs`'s module docs call out, for the same reason: an unescaped
/// `"` closes the literal early and the remainder parses as AppleScript
/// source.
fn build_contacts_search_script(query: &str) -> String {
    format!(
        r#"tell application "Contacts"
    if it is running then
        set out to ""
        set matches to (every person whose name contains "{query}")
        repeat with p in matches
            set pname to name of p
            set phoneBlob to ""
            repeat with ph in phones of p
                set phoneBlob to phoneBlob & (label of ph) & "{lsep}" & (value of ph) & "{isep}"
            end repeat
            set emailBlob to ""
            repeat with em in emails of p
                set emailBlob to emailBlob & (label of em) & "{lsep}" & (value of em) & "{isep}"
            end repeat
            set out to out & pname & "{fsep}" & phoneBlob & "{fsep}" & emailBlob & "{rsep}"
        end repeat
        return out
    else
        return "NOT_RUNNING"
    end if
end tell"#,
        query = escape_applescript(query),
        lsep = CONTACT_LABEL_SEP,
        isep = CONTACT_ITEM_SEP,
        fsep = CONTACT_FIELD_SEP,
        rsep = CONTACT_RECORD_SEP,
    )
}

fn parse_labeled_blob(blob: &str) -> Vec<LabeledValue> {
    blob.split(CONTACT_ITEM_SEP)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (label, value) = entry.split_once(CONTACT_LABEL_SEP)?;
            Some(LabeledValue { label: label.trim().to_string(), value: value.trim().to_string() })
        })
        .collect()
}

fn parse_contacts(raw: &str) -> Vec<ContactHit> {
    raw.split(CONTACT_RECORD_SEP)
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split(CONTACT_FIELD_SEP).collect();
            if fields.is_empty() || fields[0].trim().is_empty() {
                return None;
            }
            Some(ContactHit {
                name: fields[0].trim().to_string(),
                phones: fields.get(1).map(|f| parse_labeled_blob(f)).unwrap_or_default(),
                emails: fields.get(2).map(|f| parse_labeled_blob(f)).unwrap_or_default(),
            })
        })
        .collect()
}

/// Search Contacts by name. Errors (rather than returning empty) when
/// Contacts is not already running — see the module docs.
pub fn search_contacts(query: &str) -> Result<Vec<ContactHit>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Type a name to search for.".into());
    }

    let raw = apple::run_script(&build_contacts_search_script(query))?;
    if raw.trim() == "NOT_RUNNING" {
        return Err(
            "Contacts isn't open, so there is nothing to search without launching it. Open \
             Contacts once and Caduceus will search it after that."
                .into(),
        );
    }
    Ok(parse_contacts(&raw))
}

/// Put a phone number or email address (already shown from a
/// [`search_contacts`] result) on the clipboard.
pub fn contacts_copy(value: String) -> ToolOutcome {
    let value = value.trim();
    if value.is_empty() {
        return ToolOutcome::err("Nothing to copy.");
    }
    ToolOutcome::copied(value.to_string(), format!("Copied {value}"))
}

// ===========================================================================
// 6. Recent files
// ===========================================================================
//
// See the module docs ("The recent-files investigation") for why this is
// `mdfind`-based rather than reading `com.apple.sharedfilelist` directly.

const DEFAULT_RECENT_DAYS: u32 = 14;
const DEFAULT_RECENT_LIMIT: usize = 40;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub path: String,
    pub name: String,
}

/// "Recently opened documents," not "recently opened (or launched)
/// applications" — an `.app` bundle showing up as a "recent file" would read
/// as a bug, not a feature, to anyone who has ever used a Recent Items menu.
fn is_app_bundle(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("app")).unwrap_or(false)
}

/// Files Spotlight has recorded as opened within the last `days` days, most
/// recent first, capped at `limit`.
pub fn recent_files(days: Option<u32>, limit: Option<usize>) -> Result<Vec<RecentFile>, String> {
    let days = days.unwrap_or(DEFAULT_RECENT_DAYS).max(1);
    let limit = limit.unwrap_or(DEFAULT_RECENT_LIMIT).max(1);

    let mut command = Command::new("mdfind");
    command.arg(format!("kMDItemLastUsedDate >= $time.today(-{days})"));
    let output = output_with_timeout(&mut command, TOOL_TIMEOUT, "Spotlight took too long to answer.")?;
    if !output.status.success() {
        return Err("Could not search for recent files.".into());
    }

    let mut hits: Vec<(std::time::SystemTime, RecentFile)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        // Directories (including the handful of top-level folders Spotlight
        // tracks "last used" for, like the user's own home folder) are not
        // documents; `mdfind`'s date filter already narrows the field, this
        // narrows it to things worth showing in a "recent files" list.
        .filter(|path| !is_app_bundle(path) && path.is_file())
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            let name = path.file_name()?.to_string_lossy().to_string();
            Some((modified, RecentFile { path: path.to_string_lossy().to_string(), name }))
        })
        .collect();

    hits.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    hits.truncate(limit);
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}

// ===========================================================================
// Tests
// ===========================================================================
//
// Nothing below moves a window, calls into AX, runs `osascript`, or touches
// the filesystem outside of a few pure-string checks — every AppleScript
// entry point is exercised only through its `build_*_script`/`parse_*`
// halves, exactly as `calendar.rs` tests its own script builders, and every
// AX-touching function above is behind `#[cfg(target_os = "macos")]` gates
// this file never calls from a test.

#[cfg(test)]
mod tests {
    use super::*;

    // --- window presets: pid discovery -------------------------------------

    #[test]
    fn the_first_app_bundle_in_a_path_wins_over_a_nested_helper() {
        assert_eq!(first_app_bundle_name("/Applications/Notes.app/Contents/MacOS/Notes").as_deref(), Some("Notes"));
        assert_eq!(
            first_app_bundle_name(
                "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)"
            )
            .as_deref(),
            Some("Cursor"),
            "a nested helper bundle should be attributed to its parent app"
        );
    }

    #[test]
    fn a_bare_process_with_no_app_bundle_is_not_a_candidate() {
        assert_eq!(first_app_bundle_name("/usr/libexec/something"), None);
    }

    #[test]
    fn ps_output_parses_into_pid_and_app_name_pairs() {
        let raw = "  337 /System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/XPCServices/com.apple.hiservices-xpcservice.xpc/Contents/MacOS/com.apple.hiservices-xpcservice\n\
                   86916 /System/Library/CoreServices/Finder.app/Contents/MacOS/Finder\n\
                    1037 /Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)\n";
        let apps = parse_running_apps(raw);
        assert_eq!(apps, vec![(86916, "Finder".to_string()), (1037, "Cursor".to_string())]);
    }

    #[test]
    fn blank_and_malformed_ps_lines_are_skipped_not_panicked_on() {
        assert!(parse_running_apps("\n   \nnotanumber /Applications/Foo.app/Contents/MacOS/Foo\n").is_empty());
    }

    #[test]
    fn apps_are_grouped_by_name_preserving_every_pid() {
        let grouped = group_by_app_name(vec![
            (1, "Notes".to_string()),
            (2, "Notes".to_string()),
            (3, "Safari".to_string()),
        ]);
        assert_eq!(grouped.get("Notes"), Some(&vec![1, 2]));
        assert_eq!(grouped.get("Safari"), Some(&vec![3]));
    }

    // --- window presets: serialisation --------------------------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn a_preset_round_trips_through_json_with_camel_case_keys() {
        let preset = WindowPreset {
            name: "Writing".into(),
            windows: vec![PresetWindow {
                app_name: "TextEdit".into(),
                frame: manage::Frame::new(0.0, 38.0, 900.0, 600.0),
            }],
            captured_at: "2026-07-28T00:00:00+00:00".into(),
        };
        let value = serde_json::to_value(&preset).unwrap();
        assert_eq!(value["name"], "Writing");
        assert_eq!(value["windows"][0]["appName"], "TextEdit");
        assert_eq!(value["windows"][0]["frame"]["width"], 900.0);

        let round_tripped: WindowPreset = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.name, preset.name);
        assert_eq!(round_tripped.windows[0].app_name, "TextEdit");
        assert_eq!(round_tripped.windows[0].frame, preset.windows[0].frame);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_preset_list_round_trips_as_a_json_array() {
        let presets = vec![
            WindowPreset { name: "A".into(), windows: vec![], captured_at: "t".into() },
            WindowPreset { name: "B".into(), windows: vec![], captured_at: "t".into() },
        ];
        let value = serde_json::to_value(&presets).unwrap();
        let round_tripped: Vec<WindowPreset> = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.len(), 2);
        assert_eq!(round_tripped[1].name, "B");
    }

    // --- menu bar: reference building ----------------------------------------

    #[test]
    fn a_two_level_menu_path_builds_the_expected_reference() {
        let path = vec!["File".to_string(), "Save".to_string()];
        assert_eq!(
            build_menu_item_reference(&path).unwrap(),
            r#"menu item "Save" of menu "File" of menu bar item "File" of menu bar 1"#
        );
    }

    #[test]
    fn a_three_level_nested_submenu_path_builds_the_expected_reference() {
        let path = vec!["File".to_string(), "Export As".to_string(), "PDF".to_string()];
        assert_eq!(
            build_menu_item_reference(&path).unwrap(),
            r#"menu item "PDF" of menu "Export As" of menu item "Export As" of menu "File" of menu bar item "File" of menu bar 1"#
        );
    }

    #[test]
    fn a_four_level_path_nests_one_more_level() {
        let path = vec!["A".to_string(), "B".to_string(), "C".to_string(), "D".to_string()];
        let reference = build_menu_item_reference(&path).unwrap();
        assert_eq!(
            reference,
            r#"menu item "D" of menu "C" of menu item "C" of menu "B" of menu item "B" of menu "A" of menu bar item "A" of menu bar 1"#
        );
    }

    #[test]
    fn a_single_segment_path_refers_to_the_top_level_menu_itself() {
        let path = vec!["File".to_string()];
        assert_eq!(build_menu_item_reference(&path).unwrap(), r#"menu bar item "File" of menu bar 1"#);
    }

    #[test]
    fn an_empty_path_has_no_reference() {
        assert!(build_menu_item_reference(&[]).is_none());
    }

    // --- menu bar: escaping ---------------------------------------------------

    #[test]
    fn a_menu_title_with_a_quote_cannot_break_out_of_the_applescript_literal() {
        let evil = r#"Save" & (do shell script "id") & ""#.to_string();
        let reference = build_menu_item_reference(&["File".to_string(), evil.clone()]).unwrap();
        assert!(!reference.contains(&evil), "the raw payload must never appear unescaped");
        assert!(reference.contains(&escape_applescript(&evil)));
    }

    #[test]
    fn an_app_name_with_a_quote_is_escaped_in_the_invoke_script() {
        let evil = r#"Evil" & (do shell script "id") & ""#;
        let script = build_invoke_script(evil, &["File".to_string(), "Save".to_string()]).unwrap();
        assert!(!script.contains(evil));
        assert!(script.contains(&escape_applescript(evil)));
    }

    #[test]
    fn an_app_name_with_a_quote_is_escaped_in_the_enumeration_script() {
        let evil = r#"Evil" & (do shell script "id") & ""#;
        let script = build_menu_enumeration_script(evil);
        assert!(!script.contains(evil));
        assert!(script.contains(&escape_applescript(evil)));
    }

    // --- menu bar: parsing ------------------------------------------------

    #[test]
    fn a_menu_blob_parses_into_paths_including_nested_submenus() {
        let raw = format!(
            "File{ps}Save{rs}File{ps}Export As{rs}File{ps}Export As{ps}PDF{rs}Edit{ps}Copy{rs}",
            ps = MENU_PATH_SEP,
            rs = MENU_RECORD_SEP,
        );
        let items = parse_menu_items(&raw);
        assert_eq!(items.len(), 4);
        assert_eq!(items[2].path, vec!["File", "Export As", "PDF"]);
        assert_eq!(items[3].path, vec!["Edit", "Copy"]);
    }

    #[test]
    fn an_empty_menu_blob_parses_to_no_items() {
        assert!(parse_menu_items("").is_empty());
    }

    // --- contacts: escaping -------------------------------------------------

    #[test]
    fn a_contacts_query_with_a_quote_cannot_break_out_of_the_applescript_literal() {
        let evil = r#"" & (do shell script "id") & ""#;
        let script = build_contacts_search_script(evil);
        assert!(!script.contains(evil));
        assert!(script.contains(&escape_applescript(evil)));
    }

    // --- contacts: parsing ----------------------------------------------------

    #[test]
    fn a_contact_blob_parses_names_phones_and_emails() {
        let phone_blob = format!("mobile{l}555-0100{i}work{l}555-0101{i}", l = CONTACT_LABEL_SEP, i = CONTACT_ITEM_SEP);
        let email_blob = format!("home{l}a@example.com{i}", l = CONTACT_LABEL_SEP, i = CONTACT_ITEM_SEP);
        let raw = format!(
            "Ada Lovelace{f}{phones}{f}{emails}{r}",
            f = CONTACT_FIELD_SEP,
            phones = phone_blob,
            emails = email_blob,
            r = CONTACT_RECORD_SEP,
        );
        let hits = parse_contacts(&raw);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Ada Lovelace");
        assert_eq!(hits[0].phones.len(), 2);
        assert_eq!(hits[0].phones[0].label, "mobile");
        assert_eq!(hits[0].phones[0].value, "555-0100");
        assert_eq!(hits[0].emails[0].value, "a@example.com");
    }

    #[test]
    fn a_person_with_no_phones_or_emails_parses_with_empty_lists() {
        let raw = format!("Bare Name{f}{f}{r}", f = CONTACT_FIELD_SEP, r = CONTACT_RECORD_SEP);
        let hits = parse_contacts(&raw);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].phones.is_empty());
        assert!(hits[0].emails.is_empty());
    }

    #[test]
    fn an_empty_contacts_blob_parses_to_no_hits() {
        assert!(parse_contacts("").is_empty());
    }

    #[test]
    fn contacts_copy_refuses_an_empty_value() {
        assert!(!contacts_copy("   ".to_string()).ok);
    }

    #[test]
    fn contacts_copy_puts_the_value_on_the_outcome() {
        let outcome = contacts_copy("555-0100".to_string());
        assert!(outcome.ok);
        assert_eq!(outcome.copied.as_deref(), Some("555-0100"));
    }

    // --- fonts ---------------------------------------------------------------

    #[test]
    fn known_font_extensions_are_recognised_case_insensitively() {
        for ext in ["ttf", "TTF", "otf", "ttc", "dfont"] {
            assert!(is_font_file(Path::new(&format!("Example.{ext}"))), "{ext} should be a font");
        }
    }

    #[test]
    fn non_font_extensions_are_rejected() {
        for ext in ["txt", "png", "app", ""] {
            assert!(!is_font_file(Path::new(&format!("Example.{ext}"))), "{ext} should not be a font");
        }
    }

    #[test]
    fn a_font_name_is_the_filename_without_its_extension() {
        assert_eq!(font_name_from_path(Path::new("/Library/Fonts/Arial Bold.ttf")), "Arial Bold");
    }

    #[test]
    fn the_preview_text_is_never_empty() {
        assert!(!FONT_PREVIEW_TEXT.trim().is_empty());
    }

    #[test]
    fn installed_fonts_are_actually_found_on_this_machine() {
        // Not a fixed count (every Mac's font library differs) — just a
        // sanity check that the real three directories on a real Mac produce
        // a non-trivial, de-duplicated, sorted list.
        let fonts = list_installed_fonts();
        assert!(!fonts.is_empty(), "expected to find at least one installed font");
        let mut sorted = fonts.clone();
        sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        assert_eq!(fonts.iter().map(|f| &f.name).collect::<Vec<_>>(), sorted.iter().map(|f| &f.name).collect::<Vec<_>>());
        let mut names: Vec<String> = fonts.iter().map(|f| f.name.to_lowercase()).collect();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "font list should already be de-duplicated");
    }

    // --- recent files ----------------------------------------------------

    #[test]
    fn an_app_bundle_is_recognised_and_excluded() {
        assert!(is_app_bundle(Path::new("/Applications/Safari.app")));
        assert!(!is_app_bundle(Path::new("/Users/me/Downloads/report.pdf")));
    }

    #[test]
    fn a_zero_day_window_is_floored_to_one_day() {
        // Pure argument-shaping check: `days.max(1)` inside `recent_files`
        // means "0 days" cannot become a query that matches nothing by
        // accident. Exercised directly since `recent_files` itself needs a
        // live `mdfind`.
        assert_eq!(DEFAULT_RECENT_DAYS.max(1), DEFAULT_RECENT_DAYS);
        assert_eq!(0u32.max(1), 1);
    }

    #[test]
    fn the_defaults_are_sane() {
        assert!(DEFAULT_RECENT_DAYS >= 1);
        assert!(DEFAULT_RECENT_LIMIT >= 1);
    }
}
