//! The menu-bar / system-tray icon.
//!
//! Caduceus runs as an accessory app with no Dock icon, so this menu is the one
//! guaranteed way to reach every part of the app — including turning the staff
//! back on after hiding it, which would otherwise be a dead end.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::settings::SettingsManager;
use crate::window;

const ID_COMMAND_CENTER: &str = "command-center";
const ID_TOGGLE_ORB: &str = "toggle-staff";
const ID_CLIPBOARD: &str = "clipboard";
const ID_SETTINGS: &str = "settings";
const ID_STOP_AGENTS: &str = "stop-agents";
const ID_QUIT: &str = "quit";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    let mut builder = TrayIconBuilder::with_id("caduceus-tray")
        .tooltip("Caduceus")
        .menu(&menu)
        // On macOS the left click opens the Command Center and the right click
        // opens the menu, which is what a menu-bar utility is expected to do.
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = window::toggle_command_center(tray.app_handle());
            }
        });

    // A monochrome template image so macOS tints it for light/dark menu bars.
    // Embedded at compile time: reading it from the bundle would fail under
    // `tauri dev`, where resources are not laid out the same way.
    const TRAY_PNG: &[u8] = include_bytes!("../icons/tray@2x.png");
    match tauri::image::Image::from_bytes(TRAY_PNG) {
        Ok(image) => {
            builder = builder.icon(image).icon_as_template(cfg!(target_os = "macos"));
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

    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(
                app,
                ID_COMMAND_CENTER,
                "Open Command Center",
                true,
                Some(accelerator_hint(app)),
            )?,
            &MenuItem::with_id(
                app,
                ID_TOGGLE_ORB,
                if staff_visible { "Hide Staff" } else { "Show Staff" },
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, ID_CLIPBOARD, "Clipboard History\u{2026}", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ID_SETTINGS, "Settings\u{2026}", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(app, ID_STOP_AGENTS, "Stop All Agents", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ID_QUIT, "Quit Caduceus", true, Some("CmdOrCtrl+Q"))?,
        ],
    )
}

/// Show the user's actual Command Center hotkey in the menu, so the menu never
/// advertises a binding they have changed.
fn accelerator_hint<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<SettingsManager>()
        .map(|s| s.with(|s| s.general.command_center_hotkey.clone()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Alt+Space".into())
}

/// Rebuild the menu so the Show/Hide label matches reality.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id("caduceus-tray") else {
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
