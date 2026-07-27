//! `ctx.files` — read and write paths the user could reach themselves.
//!
//! Allowed roots: the user's home directory and Caduceus app data (including
//! extension storage). Paths are canonicalised before the check so `../` cannot
//! escape.

use std::path::{Path, PathBuf};

const MAX_READ: usize = 10 * 1024 * 1024;
const MAX_WRITE: usize = 10 * 1024 * 1024;

pub fn read(app_data: &Path, path: &str) -> Result<String, String> {
    let path = resolve_allowed(app_data, path)?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("Could not read that path: {e}"))?;
    if meta.len() as usize > MAX_READ {
        return Err(format!(
            "That file is over {} MB. Read a smaller file or a portion of it.",
            MAX_READ / (1024 * 1024)
        ));
    }
    std::fs::read_to_string(&path).map_err(|e| format!("Could not read that file: {e}"))
}

pub fn write(app_data: &Path, path: &str, content: &str) -> Result<(), String> {
    if content.len() > MAX_WRITE {
        return Err(format!(
            "That content is over {} MB.",
            MAX_WRITE / (1024 * 1024)
        ));
    }
    let path = resolve_allowed_for_write(app_data, path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Could not create the folder: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("Could not write that file: {e}"))
}

fn resolve_allowed(app_data: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = candidate_path(raw)?;
    let canonical = std::fs::canonicalize(&candidate).map_err(|e| {
        format!("That path does not exist or cannot be opened: {e}")
    })?;
    ensure_under_allowed_roots(app_data, &canonical)
}

fn resolve_allowed_for_write(app_data: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = candidate_path(raw)?;
    if candidate.exists() {
        return resolve_allowed(app_data, raw);
    }
    let parent = candidate
        .parent()
        .ok_or("Invalid path.")?;
    let parent_canon = if parent.exists() {
        std::fs::canonicalize(parent).map_err(|e| format!("Could not resolve the folder: {e}"))?
    } else {
        resolve_allowed_for_write(app_data, parent.to_string_lossy().as_ref())?
    };
    let resolved = parent_canon.join(
        candidate
            .file_name()
            .ok_or("Invalid path.")?,
    );
    ensure_under_allowed_roots(app_data, &parent_canon)?;
    Ok(resolved)
}

fn candidate_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("A path is required.".into());
    }
    let home = std::env::home_dir().ok_or("Could not resolve the home directory.")?;
    Ok(if Path::new(trimmed).is_absolute() {
        PathBuf::from(trimmed)
    } else {
        home.join(trimmed)
    })
}

fn ensure_under_allowed_roots(app_data: &Path, canonical: &Path) -> Result<PathBuf, String> {
    let home = std::env::home_dir().ok_or("Could not resolve the home directory.")?;
    let home_canon = std::fs::canonicalize(&home).ok();
    let app_canon = std::fs::canonicalize(app_data).ok();

    let under_home = home_canon
        .as_ref()
        .is_some_and(|h| canonical.starts_with(h));
    let under_app = app_canon
        .as_ref()
        .is_some_and(|a| canonical.starts_with(a));

    if under_home || under_app {
        Ok(canonical.to_path_buf())
    } else {
        Err("That path is outside your home folder and Caduceus data. Use a path under ~.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_paths_outside_home_and_app_data() {
        let app_data = std::env::temp_dir().join("caduceus-ext-files-test");
        let _ = std::fs::remove_dir_all(&app_data);
        std::fs::create_dir_all(&app_data).unwrap();
        assert!(read(&app_data, "/etc/passwd").is_err());
    }
}
