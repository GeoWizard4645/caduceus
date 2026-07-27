//! Bring Caduceus back after macOS terminates it (TCC reset, Screen Recording grant, etc.).

use std::process::{Command, Stdio};

/// Spawn a detached shell that reopens this app shortly after we exit.
#[cfg(target_os = "macos")]
pub fn schedule_relaunch() -> bool {
    let open_arg = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.ancestors()
                .find(|p| p.extension().is_some_and(|e| e == "app"))
                .map(|p| p.to_string_lossy().to_string())
        });

    let script = if let Some(app) = open_arg {
        format!("sleep 0.9; open {app:?}")
    } else {
        "sleep 0.9; open -a Caduceus".into()
    };

    Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(not(target_os = "macos"))]
pub fn schedule_relaunch() -> bool {
    false
}
