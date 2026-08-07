//! `#[tauri::command]` wrappers exposing skills to the webview frontend.
//!
//! Deliberately thin: every interesting decision lives in the Tauri-free
//! core (`discovery`, `tiers`, `manage`), the same functions
//! [`super::native`] wires into the agent tool loop — this file only
//! resolves the skills directory from the app handle and reshapes a plain
//! argument list into a core call. A human browsing skills in a future
//! Settings panel and the agent calling `skill_view` mid-conversation both
//! end up running the exact same code, so neither surface can drift from
//! the other's idea of what a skill is or what viewing one does (including
//! the usage-sidecar bump both trigger — see `tiers::view_skill`).

use tauri::{AppHandle, Manager, Runtime};

use super::discovery::SkillSummary;
use super::{manage, tiers};

type Res<T> = Result<T, String>;

fn skills_root<R: Runtime>(app: &AppHandle<R>) -> Res<std::path::PathBuf> {
    let data_dir = app.path().app_data_dir().map_err(|e| format!("could not resolve the app data directory: {e}"))?;
    Ok(data_dir.join(super::SKILLS_DIR_NAME))
}

#[tauri::command]
pub fn skills_list<R: Runtime>(app: AppHandle<R>, category: Option<String>) -> Res<Vec<SkillSummary>> {
    let root = skills_root(&app)?;
    Ok(tiers::list_skills(&root, category.as_deref()))
}

/// Tier 2 (no `file_path`) or tier 3 (`file_path` given) — see
/// `tiers::view_skill` / `tiers::view_skill_file`. Returns a bare JSON value
/// rather than a named struct because the two tiers have different shapes,
/// exactly as `skills::native::register_skill_view`'s tool handler does for
/// the same reason.
#[tauri::command]
pub fn skill_view<R: Runtime>(app: AppHandle<R>, name: String, file_path: Option<String>) -> Res<serde_json::Value> {
    let root = skills_root(&app)?;
    match file_path {
        None => {
            let view = tiers::view_skill(&root, &name)?;
            serde_json::to_value(view).map_err(|e| e.to_string())
        }
        Some(fp) => match tiers::view_skill_file(&root, &name, &fp)? {
            tiers::SkillFileContent::Text(content) => Ok(serde_json::json!({ "name": name, "file": fp, "content": content })),
            tiers::SkillFileContent::Binary { size } => {
                Ok(serde_json::json!({ "name": name, "file": fp, "isBinary": true, "sizeBytes": size }))
            }
        },
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn skill_manage<R: Runtime>(
    app: AppHandle<R>,
    action: String,
    name: String,
    content: Option<String>,
    category: Option<String>,
    file_path: Option<String>,
    file_content: Option<String>,
    old_string: Option<String>,
    new_string: Option<String>,
    replace_all: Option<bool>,
) -> Res<String> {
    let root = skills_root(&app)?;
    manage::skill_manage(
        &root,
        &action,
        &name,
        content.as_deref(),
        category.as_deref(),
        file_path.as_deref(),
        file_content.as_deref(),
        old_string.as_deref(),
        new_string.as_deref(),
        replace_all.unwrap_or(false),
    )
}
