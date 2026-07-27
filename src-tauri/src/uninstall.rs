//! Optional removal of Caduceus, its extensions, and AI stack pieces the user
//! installed alongside it. Destructive operations move apps to the Trash where
//! possible rather than unlinking immediately.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::agent::hermes;
use crate::extensions;
use crate::tools::files;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallSnapshot {
    pub extensions: Vec<extensions::Extension>,
    pub ollama_models: Vec<String>,
    pub caduceus_app_installed: bool,
    pub ollama_installed: bool,
    pub hermes_installed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallRequest {
    pub extension_ids: Vec<String>,
    pub caduceus: bool,
    pub hermes: bool,
    pub ollama: bool,
    pub ollama_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    pub ok: bool,
    pub messages: Vec<String>,
    /// When true the frontend should expect the app to exit shortly.
    pub quit_app: bool,
}

pub fn snapshot<R: Runtime>(app: &AppHandle<R>) -> Result<UninstallSnapshot, String> {
    let data = app_data(app)?;
    Ok(UninstallSnapshot {
        extensions: extensions::list(&data),
        ollama_models: list_ollama_models(),
        caduceus_app_installed: caduceus_app_path().is_some(),
        ollama_installed: ollama_app_path().is_some() || command_exists("ollama"),
        hermes_installed: hermes::find_hermes().is_some(),
    })
}

pub fn run<R: Runtime>(app: &AppHandle<R>, req: UninstallRequest) -> Result<UninstallResult, String> {
    let data = app_data(app)?;
    let mut messages = Vec::new();
    let mut ok = true;

    for id in &req.extension_ids {
        match extensions::remove(id, &data) {
            Ok(()) => messages.push(format!("Removed extension “{id}”.")),
            Err(e) => {
                ok = false;
                messages.push(e);
            }
        }
    }

    for model in &req.ollama_models {
        match ollama_rm(model) {
            Ok(()) => messages.push(format!("Removed Ollama model “{model}”.")),
            Err(e) => {
                ok = false;
                messages.push(e);
            }
        }
    }

    if req.hermes {
        match remove_hermes() {
            Ok(note) => messages.push(note),
            Err(e) => {
                ok = false;
                messages.push(e);
            }
        }
    }

    if req.ollama {
        match remove_ollama() {
            Ok(note) => messages.push(note),
            Err(e) => {
                ok = false;
                messages.push(e);
            }
        }
    }

    let quit_app = req.caduceus;
    if req.caduceus {
        if let Some(path) = caduceus_app_path() {
            match trash(&[path]) {
                Ok(m) => messages.push(m),
                Err(e) => {
                    ok = false;
                    messages.push(e);
                }
            }
        }
        match trash(&[data.to_string_lossy().into_owned()]) {
            Ok(m) => messages.push(format!("Caduceus settings & data: {m}")),
            Err(e) => {
                ok = false;
                messages.push(e);
            }
        }
    }

    if quit_app && ok {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            app.exit(0);
        });
    }

    Ok(UninstallResult {
        ok,
        messages,
        quit_app: quit_app && ok,
    })
}

fn app_data<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Could not find the app data directory: {e}"))
}

fn caduceus_app_path() -> Option<String> {
    let path = Path::new("/Applications/Caduceus.app");
    path.is_dir().then(|| path.to_string_lossy().into_owned())
}

fn ollama_app_path() -> Option<String> {
    let path = Path::new("/Applications/Ollama.app");
    path.is_dir().then(|| path.to_string_lossy().into_owned())
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {name}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn list_ollama_models() -> Vec<String> {
    let output = Command::new("ollama").arg("list").output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            if name == "NAME" {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

fn ollama_rm(model: &str) -> Result<(), String> {
    if model.trim().is_empty() {
        return Ok(());
    }
    let output = Command::new("ollama")
        .args(["rm", model.trim()])
        .output()
        .map_err(|e| format!("Could not run ollama: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not remove model “{model}”: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn remove_ollama() -> Result<String, String> {
    let _ = Command::new("killall").arg("ollama").status();
    let mut paths = Vec::new();
    if let Some(p) = ollama_app_path() {
        paths.push(p);
    }
    if paths.is_empty() && !command_exists("ollama") {
        return Err("Ollama does not appear to be installed.".into());
    }
    if paths.is_empty() {
        return Ok("Stopped the Ollama service. The CLI may remain on your PATH — remove it with Homebrew if you used brew install ollama.".into());
    }
    trash(&paths)
}

fn remove_hermes() -> Result<String, String> {
    let mut paths = Vec::new();
    if let Some(bin) = hermes::find_hermes() {
        paths.push(bin.to_string_lossy().into_owned());
    }
    if let Some(home) = dirs::home_dir() {
        for rel in [".local/share/hermes", ".hermes"] {
            let p = home.join(rel);
            if p.exists() {
                paths.push(p.to_string_lossy().into_owned());
            }
        }
        let bin = home.join(".local/bin/hermes");
        if bin.is_file() {
            paths.push(bin.to_string_lossy().into_owned());
        }
    }
    if paths.is_empty() {
        return Err("Hermes Agent does not appear to be installed.".into());
    }
    trash(&paths)
}

fn trash(paths: &[String]) -> Result<String, String> {
    let outcome = files::trash_paths(paths);
    if outcome.ok {
        Ok(outcome.message)
    } else {
        Err(outcome.message)
    }
}
