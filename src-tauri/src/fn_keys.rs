//! Actions bound to function keys in Settings.

use tauri::{AppHandle, Runtime};

use crate::capture;
use crate::settings::{FunctionKeyAction, SettingsManager};
use crate::shortcuts;
use crate::window;

#[cfg(target_os = "macos")]
pub async fn start_voice_memo() -> Result<(), String> {
    let opened = shortcuts::exec::open_app("com.apple.VoiceMemos", &[]).await;
    if !opened.ok {
        return Err(opened.message);
    }

    // Voice Memos has no stable public “record” API; try the menu shortcut after launch.
    let script = r#"
tell application "Voice Memos" to activate
delay 0.4
tell application "System Events" to keystroke "n" using {command down}
"#;
    match shortcuts::exec::run_applescript(script).await {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!("Voice Memos opened but could not start recording automatically: {e}");
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub async fn start_voice_memo() -> Result<(), String> {
    Err("Voice Memos is only available on macOS.".into())
}

pub fn dispatch_press<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
    action: FunctionKeyAction,
    shortcut_id: &str,
) {
    match action {
        FunctionKeyAction::None => {}
        FunctionKeyAction::ToggleStaff => {
            if let Err(e) = window::toggle_staff(app, settings) {
                log::error!("function key could not toggle the staff: {e}");
            }
            crate::tray::refresh(app);
        }
        FunctionKeyAction::CommandCenter => {
            if let Err(e) = window::toggle_command_center(app) {
                log::error!("function key could not open the Command Center: {e}");
            }
        }
        FunctionKeyAction::PushToTalk => {
            crate::hotkeys::start_push_to_talk(app, settings);
        }
        FunctionKeyAction::StartDictation => {
            crate::hotkeys::toggle_dictation(app, settings);
        }
        FunctionKeyAction::VoiceMemo => {
            tauri::async_runtime::spawn(async move {
                if let Err(e) = start_voice_memo().await {
                    log::error!("voice memo: {e}");
                }
            });
        }
        FunctionKeyAction::Screenshot => {
            match capture::screenshot_full(true) {
                Ok(r) => log::info!("screenshot: {}", r.message),
                Err(e) => log::error!("screenshot: {e}"),
            }
        }
        FunctionKeyAction::RunShortcut => {
            if shortcut_id.trim().is_empty() {
                log::warn!("function key shortcut binding has no shortcut selected");
                return;
            }
            let settings = settings.clone();
            let id = shortcut_id.to_string();
            tauri::async_runtime::spawn(async move {
                let cfg = settings.get();
                let Some(shortcut) = cfg.shortcuts.iter().find(|s| s.id == id) else {
                    log::error!("function key shortcut id {id} not found");
                    return;
                };
                let outcome = shortcuts::execute_shortcut(
                    shortcut,
                    "",
                    &cfg.command_center.browser,
                )
                .await;
                if !outcome.ok {
                    log::error!("function key shortcut failed: {}", outcome.message);
                }
            });
        }
    }
}

pub fn dispatch_release<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsManager,
    action: FunctionKeyAction,
) {
    if action == FunctionKeyAction::PushToTalk {
        crate::hotkeys::stop_push_to_talk(app, settings);
    }
}
