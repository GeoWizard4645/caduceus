//! The menu-bar / system-tray icon.
//!
//! Caduceus runs as an accessory app with no Dock icon, so this menu is the one
//! guaranteed way to reach every part of the app — including turning the staff
//! back on after hiding it, which would otherwise be a dead end.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::hotkeys;
use crate::settings;
use crate::settings::SettingsManager;
use crate::tools::timekeeping::{Phase, TimekeepingRuntime};
use crate::window;

const ID_COMMAND_CENTER: &str = "command-center";
const ID_TOGGLE_ORB: &str = "toggle-staff";
const ID_CLIPBOARD: &str = "clipboard";
const ID_SETTINGS: &str = "settings";
const ID_STOP_AGENTS: &str = "stop-agents";
const ID_POMODORO_STATUS: &str = "pomodoro-status";
const ID_STOP_POMODORO: &str = "stop-pomodoro";
const ID_RESTART: &str = "restart";
const ID_QUIT: &str = "quit";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    let mut builder = TrayIconBuilder::with_id("caduceus-tray")
        // Keep a visible, unmistakable status item even if macOS fails to tint
        // the template image after an update or display-layout change.
        .title("Caduceus")
        .tooltip("Caduceus")
        .menu(&menu)
        // On macOS the left click opens the Command Center and the right click
        // opens the menu, which is what a menu-bar utility is expected to do.
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                let _ = window::toggle_command_center(tray.app_handle(), "tray");
            }
            // The menu itself is native (AppKit's own `NSMenu`, opened
            // outside this process's control on right-click), so there is no
            // "about to show" hook to refresh it from. The cursor entering
            // the icon's bounds is the closest thing to one: it fires right
            // before a click, on every platform, whichever button is used —
            // so a pomodoro's remaining time is at most a mouse-move stale
            // rather than stale since whenever the last phase transition
            // happened to be (which, for a 90-minute long break, is a while).
            TrayIconEvent::Enter { .. } => refresh(tray.app_handle()),
            _ => {}
        });

    // A monochrome template image so macOS tints it for light/dark menu bars.
    // Embedded at compile time: reading it from the bundle would fail under
    // `tauri dev`, where resources are not laid out the same way.
    const TRAY_PNG: &[u8] = include_bytes!("../icons/tray@2x.png");
    match tauri::image::Image::from_bytes(TRAY_PNG) {
        Ok(image) => {
            builder = builder
                .icon(image)
                .icon_as_template(cfg!(target_os = "macos"));
        }
        Err(e) => {
            log::warn!("tray template icon unusable ({e}); falling back to the app icon");
            if let Some(icon) = app.default_window_icon().cloned() {
                builder = builder.icon(icon);
            }
        }
    }

    builder.build(app)?;
    Ok(())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let staff_visible = window::staff(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(true);

    // Built with `Menu::new` + `.append()` one item at a time, rather than
    // the `Menu::with_items` array literal this used to be, because the
    // pomodoro section below is conditional — present only while a session
    // is running — and a fixed-size array can't express "N items, or N+2".
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_COMMAND_CENTER,
        "Open Command Center",
        true,
        Some(accelerator_hint(app)),
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_TOGGLE_ORB,
        if staff_visible {
            "Hide Staff"
        } else {
            "Show Staff"
        },
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_CLIPBOARD,
        "Clipboard History\u{2026}",
        true,
        None::<&str>,
    )?)?;

    // A running pomodoro is otherwise invisible outside the Time page — this
    // section is the actual fix for stray notifications, not decoration.
    // Anyone who can see the tray icon can see a session is live and end it
    // in one click, instead of having to remember Time → Pomodoro exists.
    if let Some(status) = pomodoro_status_label(app) {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        // Disabled: this line is a status readout, not a button. `enabled:
        // false` guarantees a click does nothing, rather than relying on the
        // event handler to ignore an id it does not recognise.
        menu.append(&MenuItem::with_id(
            app,
            ID_POMODORO_STATUS,
            status,
            false,
            None::<&str>,
        )?)?;
        menu.append(&MenuItem::with_id(
            app,
            ID_STOP_POMODORO,
            "Stop Pomodoro",
            true,
            None::<&str>,
        )?)?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_SETTINGS,
        "Settings\u{2026}",
        true,
        Some("CmdOrCtrl+,"),
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_STOP_AGENTS,
        "Stop All Agents",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_RESTART,
        "Restart Caduceus",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_QUIT,
        "Quit Caduceus",
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;

    Ok(menu)
}

/// The label for the running-pomodoro status line — phase and time
/// remaining — or `None` when nothing is running (or the runtime is not
/// registered yet, which should not happen after `setup` but is not worth a
/// panic if it somehow did). Callers add the whole pomodoro section only
/// when this returns something, so "no pomodoro" means no trace of one in
/// the menu at all.
fn pomodoro_status_label<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let runtime = app.try_state::<TimekeepingRuntime>()?;
    let status = runtime.pomodoro_status();
    if !status.running {
        return None;
    }

    let phase_label = match status.phase {
        Some(Phase::Work) => "Work",
        Some(Phase::ShortBreak) => "Short break",
        Some(Phase::LongBreak) => "Long break",
        None => "Pomodoro",
    };
    let minutes = status.remaining_secs / 60;
    let seconds = status.remaining_secs % 60;
    Some(format!(
        "Pomodoro — {phase_label} \u{b7} {minutes}:{seconds:02} left"
    ))
}

/// Show the hotkey the Command Center is actually reachable on, so the menu
/// never advertises a shortcut that does nothing.
///
/// # Why this prefers the runtime over settings
///
/// These two can legitimately disagree. If another application already holds
/// the configured accelerator, [`hotkeys::register_all`] moves the Command
/// Center to a working fallback **for this run** and deliberately does not
/// write that back into settings, so the user's chosen key returns the moment
/// whatever took it goes away. That is the right behaviour for the setting and
/// the wrong thing to print in a menu: what belongs here is the key that will
/// actually open the window if you press it.
///
/// Settings are the fallback for the window between app start and the first
/// registration, and [`settings::DEFAULT_COMMAND_CENTER_HOTKEY`] for the
/// window before settings load — one constant rather than a literal, so the
/// menu and the default can never drift apart.
fn accelerator_hint<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<hotkeys::HotkeyRuntime>()
        .and_then(|r| r.active_command_center())
        .or_else(|| {
            app.try_state::<SettingsManager>()
                .map(|s| s.with(|s| s.general.command_center_hotkey.clone()))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| settings::DEFAULT_COMMAND_CENTER_HOTKEY.into())
}

/// Rebuild the menu so the Show/Hide label matches reality.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id("caduceus-tray") else {
        // The status item is the app's primary navigation on macOS. If the OS
        // or an interrupted startup dropped it, any settings/hotkey refresh is
        // an opportunity to put it back instead of silently doing nothing.
        if let Err(e) = build(app) {
            log::error!("could not restore the tray icon: {e}");
        }
        return;
    };
    match build_menu(app) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                log::warn!("could not refresh the tray menu: {e}");
            }
        }
        Err(e) => log::warn!("could not rebuild the tray menu: {e}"),
    }
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let settings = app.state::<SettingsManager>().inner().clone();

    match event.id().as_ref() {
        ID_COMMAND_CENTER => {
            let _ = window::open_command_center(app, Default::default());
        }
        ID_TOGGLE_ORB => {
            if let Err(e) = window::toggle_staff(app, &settings) {
                log::error!("could not toggle the staff: {e}");
            }
            refresh(app);
        }
        ID_CLIPBOARD => {
            let _ = window::open_command_center(
                app,
                window::CommandCenterOpenPayload {
                    mode: "clipboard".into(),
                    ..Default::default()
                },
            );
        }
        ID_SETTINGS => {
            if let Err(e) = window::open_settings(app, None) {
                log::error!("could not open Settings: {e}");
            }
        }
        ID_STOP_AGENTS => {
            if let Some(runtime) = app.try_state::<crate::agent::AgentRuntime>() {
                runtime.stop_all();
            }
        }
        ID_STOP_POMODORO => {
            if let Some(runtime) = app.try_state::<TimekeepingRuntime>() {
                // `pomodoro_stop` already calls the `on_pomodoro_change`
                // callback wired up in `lib.rs`, which is what actually
                // rebuilds this menu — no explicit `refresh(app)` needed
                // here, and adding one would just rebuild it twice.
                runtime.pomodoro_stop();
            }
        }
        ID_RESTART => {
            crate::commands::restart_app(app.clone());
        }
        ID_QUIT => {
            // `exit` raises `ExitRequested`, which is where the teardown is
            // decided. Doing it here as well would run it twice — and this
            // handler is on the main thread, which is the one place it must
            // not block.
            app.exit(0);
        }
        other => log::debug!("unhandled tray menu item: {other}"),
    }
}
