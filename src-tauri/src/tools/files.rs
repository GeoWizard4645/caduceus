//! File actions that operate on the Finder selection or an explicit path.
//!
//! Nothing here takes a glob or a pattern. Every destructive action is given
//! exact paths that the user selected in Finder, and deletion means *Trash*,
//! never `unlink` — a command palette is a place to make fast decisions, and
//! fast decisions need an undo.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use super::ToolOutcome;

/// Zipping a folder of videos is the one thing here that can honestly take
/// minutes, so it gets its own deadline rather than the shared one.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(600);

fn run_tool(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = super::output_with_timeout(
        Command::new(program).args(args),
        timeout,
        &format!("{program} did not answer in time."),
    )?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn osa(script: &str) -> Result<String, String> {
    run_tool("osascript", &["-e", script], super::TOOL_TIMEOUT).map_err(|e| {
        if e.contains("-1743") {
            "Caduceus is not allowed to control Finder yet. Grant it in System Settings → \
             Privacy & Security → Automation."
                .to_string()
        } else {
            e
        }
    })
}

/// POSIX paths of whatever is selected in Finder.
pub fn finder_selection() -> Vec<PathBuf> {
    let script = r#"tell application "Finder"
        set sel to selection
        if (count of sel) is 0 then return ""
        set out to ""
        repeat with i in sel
            set out to out & POSIX path of (i as alias) & linefeed
        end repeat
        return out
    end tell"#;

    osa(script)
        .map(|paths| {
            paths
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Compress the Finder selection into a zip archive beside it.
///
/// `ditto` rather than `zip`: it is what Finder's own "Compress" uses, so
/// resource forks, extended attributes and symlinks survive, and the archive
/// opens identically on the other side.
pub fn compress_selection() -> ToolOutcome {
    let selection = finder_selection();
    if selection.is_empty() {
        return ToolOutcome::err("Select something in Finder first.");
    }

    let first = &selection[0];
    let Some(parent) = first.parent() else {
        return ToolOutcome::err("Could not work out where to put the archive.");
    };
    let stem = if selection.len() == 1 {
        first.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Archive".into())
    } else {
        "Archive".to_string()
    };

    let destination = unique_path(parent, &stem, "zip");

    let mut args: Vec<String> = vec!["-c".into(), "-k".into(), "--sequesterRsrc".into(), "--keepParent".into()];
    for path in &selection {
        args.push(path.to_string_lossy().to_string());
    }
    args.push(destination.to_string_lossy().to_string());

    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run_tool("ditto", &refs, ARCHIVE_TIMEOUT) {
        Ok(_) => {
            let size = std::fs::metadata(&destination).map(|m| m.len()).unwrap_or(0);
            ToolOutcome::copied(
                destination.to_string_lossy().to_string(),
                format!(
                    "Compressed {} item(s) into {} ({})",
                    selection.len(),
                    destination.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    human_size(size),
                ),
            )
        }
        Err(e) => ToolOutcome::err(format!("Could not compress: {e}")),
    }
}

/// Expand the selected archives next to themselves.
pub fn expand_selection() -> ToolOutcome {
    let selection = finder_selection();
    if selection.is_empty() {
        return ToolOutcome::err("Select an archive in Finder first.");
    }

    let mut expanded = 0;
    let mut failed = Vec::new();
    for path in &selection {
        let Some(parent) = path.parent() else { continue };
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let destination = unique_path(parent, &stem, "");

        match run_tool(
            "ditto",
            &["-x", "-k", &path.to_string_lossy(), &destination.to_string_lossy()],
            ARCHIVE_TIMEOUT,
        ) {
            Ok(_) => expanded += 1,
            Err(_) => failed.push(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        }
    }

    if expanded == 0 {
        ToolOutcome::err(format!("Could not expand {}", failed.join(", ")))
    } else {
        ToolOutcome::ok(format!("Expanded {expanded} archive(s)"))
    }
}

/// A path that does not exist yet, by appending " 2", " 3" and so on.
///
/// Overwriting an archive that happens to share a name is the kind of quiet
/// data loss that makes people stop trusting a tool.
fn unique_path(parent: &Path, stem: &str, extension: &str) -> PathBuf {
    let build = |suffix: String| {
        if extension.is_empty() {
            parent.join(format!("{stem}{suffix}"))
        } else {
            parent.join(format!("{stem}{suffix}.{extension}"))
        }
    };

    let mut candidate = build(String::new());
    let mut counter = 2;
    while candidate.exists() {
        candidate = build(format!(" {counter}"));
        counter += 1;
        if counter > 999 {
            break;
        }
    }
    candidate
}

/// Move the Finder selection to the Trash.
pub fn trash_selection() -> ToolOutcome {
    let selection = finder_selection();
    if selection.is_empty() {
        return ToolOutcome::err("Select something in Finder first.");
    }
    match osa("tell application \"Finder\" to delete (selection as alias list)") {
        Ok(_) => ToolOutcome::ok(format!("Moved {} item(s) to the Trash", selection.len())),
        Err(e) => ToolOutcome::err(e),
    }
}

/// Preview the Finder selection with Quick Look.
pub fn quick_look_selection() -> ToolOutcome {
    let selection = finder_selection();
    if selection.is_empty() {
        return ToolOutcome::err("Select something in Finder first.");
    }
    let paths: Vec<String> = selection.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let mut args: Vec<&str> = vec!["-p"];
    args.extend(paths.iter().map(String::as_str));

    // `qlmanage -p` blocks until the panel is dismissed, so it is spawned rather
    // than waited on — otherwise the palette hangs behind the preview.
    match Command::new("qlmanage")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => ToolOutcome::ok(format!("Previewing {} item(s)", selection.len())),
        Err(e) => ToolOutcome::err(format!("Could not open Quick Look: {e}")),
    }
}

/// Open the selected folder — or the parent of the selected file — in Terminal.
pub fn open_selection_in_terminal() -> ToolOutcome {
    let selection = finder_selection();
    let target = match selection.first() {
        Some(path) if path.is_dir() => path.clone(),
        Some(path) => path.parent().map(Path::to_path_buf).unwrap_or_default(),
        None => return ToolOutcome::err("Select a folder in Finder first."),
    };

    match run_tool("open", &["-a", "Terminal", &target.to_string_lossy()], super::TOOL_TIMEOUT) {
        Ok(_) => ToolOutcome::ok(format!(
            "Opened {} in Terminal",
            target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        )),
        Err(e) => ToolOutcome::err(e),
    }
}

// ---------------------------------------------------------------------------
// Disk usage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BigFile {
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub size: String,
}

/// The largest files under a directory.
///
/// Uses Spotlight rather than walking the tree: `mdfind` answers from an index
/// that already exists, so this is instant on a home directory where a
/// depth-first walk takes tens of seconds and spins up the disk.
pub fn largest_files(directory: &str, limit: usize) -> Vec<BigFile> {
    let root = if directory.trim().is_empty() {
        dirs::home_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        directory.to_string()
    };

    // 100 MB and up; below that "large file" is not a useful category.
    let Ok(output) =
        run_tool("mdfind", &["-onlyin", &root, "kMDItemFSSize > 100000000"], super::TOOL_TIMEOUT)
    else {
        return Vec::new();
    };

    let mut files: Vec<BigFile> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|path| {
            let bytes = std::fs::metadata(path).ok()?.len();
            Some(BigFile {
                name: Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string()),
                path: path.to_string(),
                bytes,
                size: human_size(bytes),
            })
        })
        .collect();

    files.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    files.truncate(limit);
    files
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// Uninstalling
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub path: String,
    pub bytes: u64,
    pub size: String,
}

/// Read an application bundle's identifier.
///
/// Read from the bundle rather than guessed from the name, because the
/// identifier is the only thing [`app_leftovers`] can safely match on.
pub fn bundle_id(app_path: &str) -> Option<String> {
    let plist = Path::new(app_path).join("Contents/Info");
    let id = run_tool(
        "defaults",
        &["read", &plist.to_string_lossy(), "CFBundleIdentifier"],
        super::TOOL_TIMEOUT,
    )
    .ok()?;
    let id = id.trim();
    (!id.is_empty() && id.contains('.')).then(|| id.to_string())
}

/// Support files an application left in `~/Library`.
///
/// Matched on the bundle identifier, never on the app's display name: an app
/// called "Notes" would otherwise match half of `~/Library/Containers`. This
/// only ever *reports*; removing is a separate, explicit step.
pub fn app_leftovers(bundle_id: &str) -> Vec<Leftover> {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() || !bundle_id.contains('.') {
        return Vec::new();
    }
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    let directories = [
        "Library/Application Support",
        "Library/Caches",
        "Library/Containers",
        "Library/Group Containers",
        "Library/Preferences",
        "Library/Saved Application State",
        "Library/HTTPStorages",
        "Library/WebKit",
        "Library/Logs",
    ];

    let mut found = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(home.join(directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // `com.acme.Widget` also owns `com.acme.Widget.plist` and
            // `com.acme.Widget.savedState`, but must not claim `com.acme.WidgetPro`.
            let matches = name == bundle_id
                || name.strip_prefix(bundle_id).is_some_and(|rest| rest.starts_with('.'));
            if !matches {
                continue;
            }
            let path = entry.path();
            let bytes = directory_size(&path);
            found.push(Leftover {
                path: path.to_string_lossy().to_string(),
                bytes,
                size: human_size(bytes),
            });
        }
    }

    found.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    found
}

/// Total size of a file or directory tree.
fn directory_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    // Never follow symlinks: a link into a large tree would be counted as if
    // deleting it would reclaim that space, which it would not.
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|entry| directory_size(&entry.path())).sum()
}

/// Move specific paths to the Trash.
///
/// Takes the exact list the user was shown and confirmed, so what is removed is
/// always what was on screen.
/// Empty the Trash for real.
///
/// Distinct from [`trash_paths`] and it has to be: that one asks Finder to
/// *move* things to the Trash, which for something already in the Trash is a
/// no-op that reports success. This is the only operation in Caduceus that
/// destroys data rather than relocating it, which is why it lives on its own
/// with its own name.
pub fn empty_trash() -> Result<(), String> {
    osa(r#"tell application "Finder" to empty trash"#).map(|_| ())
}

/// Make a path safe to sit inside an AppleScript string literal.
///
/// The backslash has to go first: escaping the quote first would turn a real
/// `\"` in a filename into `\\"`, which AppleScript reads as a literal
/// backslash followed by the end of the string — and everything after it as
/// code. Only `/` and NUL are forbidden in a macOS filename, so a downloaded
/// file can be named exactly that.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn trash_paths(paths: &[String]) -> ToolOutcome {
    if paths.is_empty() {
        return ToolOutcome::err("Nothing to remove.");
    }

    let quoted: Vec<String> = paths
        .iter()
        .filter(|path| Path::new(path).exists())
        .map(|path| format!("POSIX file \"{}\"", escape_applescript(path)))
        .collect();

    if quoted.is_empty() {
        return ToolOutcome::err("None of those paths still exist.");
    }

    let script = format!(
        "tell application \"Finder\" to delete {{{}}}",
        quoted.join(", ")
    );
    match osa(&script) {
        Ok(_) => ToolOutcome::ok(format!("Moved {} item(s) to the Trash", quoted.len())),
        Err(e) => ToolOutcome::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable_at_every_scale() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024 * 3), "3.0 GB");
    }

    #[test]
    fn unique_paths_step_around_what_already_exists() {
        let dir = std::env::temp_dir().join(format!("caduceus-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = unique_path(&dir, "Archive", "zip");
        assert_eq!(first.file_name().unwrap(), "Archive.zip");

        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir, "Archive", "zip");
        assert_eq!(second.file_name().unwrap(), "Archive 2.zip");

        std::fs::write(&second, b"x").unwrap();
        assert_eq!(unique_path(&dir, "Archive", "zip").file_name().unwrap(), "Archive 3.zip");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bundle_id_does_not_match_a_longer_one() {
        // The whole point of matching on identifier prefixes carefully.
        let id = "com.acme.Widget";
        for name in ["com.acme.Widget", "com.acme.Widget.plist", "com.acme.Widget.savedState"] {
            assert!(
                name == id || name.strip_prefix(id).is_some_and(|rest| rest.starts_with('.')),
                "{name} should match"
            );
        }
        for name in ["com.acme.WidgetPro", "com.acme.Widgets", "com.acme.Widget2"] {
            assert!(
                !(name == id || name.strip_prefix(id).is_some_and(|rest| rest.starts_with('.'))),
                "{name} should not match"
            );
        }
    }

    #[test]
    fn a_bundle_id_that_is_not_one_finds_nothing() {
        assert!(app_leftovers("").is_empty());
        assert!(app_leftovers("Safari").is_empty());
        assert!(app_leftovers("   ").is_empty());
    }

    #[test]
    fn directory_size_adds_up_a_tree_and_ignores_symlinks() {
        let dir = std::env::temp_dir().join(format!("caduceus-size-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("nested/b.bin"), vec![0u8; 500]).unwrap();

        assert_eq!(directory_size(&dir), 1500);

        std::os::unix::fs::symlink(dir.join("a.bin"), dir.join("link.bin")).unwrap();
        assert_eq!(directory_size(&dir), 1500, "a symlink must not be counted twice");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trashing_nothing_is_refused_rather_than_run() {
        assert!(!trash_paths(&[]).ok);
        assert!(!trash_paths(&["/no/such/path/anywhere".to_string()]).ok);
    }

    /// A downloaded file whose name carries a backslash and a quote would
    /// otherwise close the AppleScript string early and run the rest as code.
    #[test]
    fn a_filename_cannot_break_out_of_the_delete_script() {
        let evil = r#"/tmp/a\" & (do shell script "rm -rf /") & ""#;
        let escaped = escape_applescript(evil);

        assert_eq!(escape_applescript(r"back\slash"), r"back\\slash");
        // Every quote survives with an escape of its own, and no backslash of
        // the original is left standing in front of one.
        assert_eq!(
            escaped.replace("\\\\", "").matches("\\\"").count(),
            evil.matches('"').count()
        );
        assert!(!escaped.replace("\\\\", "").replace("\\\"", "").contains('"'));
    }
}

/// Show a path in Finder, or open it if it is a folder.
///
/// Takes a path the user was shown in a result row, and refuses anything that
/// does not exist — the webview cannot use this to probe the filesystem, because
/// it only ever passes back a path Caduceus gave it.
pub fn reveal(path: &str) -> ToolOutcome {
    let target = Path::new(path);
    if !target.exists() {
        return ToolOutcome::err("That path no longer exists.");
    }

    let args: Vec<&str> = if target.is_dir() { vec![path] } else { vec!["-R", path] };
    match run_tool("open", &args, super::TOOL_TIMEOUT) {
        Ok(_) => ToolOutcome::ok(format!(
            "Opened {}",
            target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.into())
        )),
        Err(e) => ToolOutcome::err(format!("Could not open it: {e}")),
    }
}

/// Open a folder in Terminal. Refuses anything that is not an existing folder.
pub fn open_in_terminal(path: &str) -> ToolOutcome {
    let target = Path::new(path);
    if !target.is_dir() {
        return ToolOutcome::err("That is not a folder.");
    }
    match run_tool("open", &["-a", "Terminal", path], super::TOOL_TIMEOUT) {
        Ok(_) => ToolOutcome::ok(format!(
            "Opened {} in Terminal",
            target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.into())
        )),
        Err(e) => ToolOutcome::err(e),
    }
}
