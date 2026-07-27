//! Installing extensions.
//!
//! An extension is **one file**. You drop a `.js` file on the Extensions tab and
//! it is installed — there is no folder to lay out, no separate manifest to keep
//! in sync, and no build step. The metadata lives in a header comment at the top
//! of the same file, which means the thing you write and the thing that
//! describes it cannot drift apart.
//!
//! ```js
//! /**
//!  * @caduceus 1
//!  * name: Word Count
//!  * description: Count the words on your clipboard
//!  * permissions: clipboard
//!  */
//! export default async function (input, ctx) { … }
//! ```
//!
//! The header is parsed here **without executing anything**. That ordering is
//! the point: you can see an extension's name and everything it claims it wants
//! before any of its code has run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Capabilities an extension can ask for. A closed set — a permission that is
/// not on this list cannot be granted, so a typo fails loudly at install time
/// rather than silently widening what something can reach.
pub const PERMISSIONS: &[&str] = &["clipboard", "network", "selection", "notifications"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    /// Derived from the filename; also the on-disk name.
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub permissions: Vec<String>,
    pub path: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub ok: bool,
    pub message: String,
    pub extension: Option<Extension>,
}

/// Everything the header can declare, before it is checked.
#[derive(Debug, Default)]
struct Header {
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    permissions: Vec<String>,
    versioned: bool,
}

/// Pull the `@caduceus` header out of a source file.
///
/// Only the first comment block is considered. Scanning the whole file would
/// mean a `@caduceus` line inside a string or a later comment could redefine
/// what an extension claims to be, which is exactly the kind of ambiguity a
/// permission list should not have.
fn parse_header(source: &str) -> Header {
    let mut header = Header::default();

    let Some(start) = source.find("/*") else {
        return header;
    };
    let Some(end_rel) = source[start..].find("*/") else {
        return header;
    };
    let block = &source[start..start + end_rel];

    for line in block.lines() {
        let line = line.trim().trim_start_matches('*').trim();
        if line.starts_with("@caduceus") {
            header.versioned = true;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => header.name = Some(value),
            "description" => header.description = Some(value),
            "author" => header.author = Some(value),
            "permissions" => {
                header.permissions = value
                    .split(',')
                    .map(|p| p.trim().to_ascii_lowercase())
                    .filter(|p| !p.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    header
}

/// Turn a filename into a stable id: lowercase, non-alphanumerics collapsed.
fn id_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "extension".into());

    let mut out = String::new();
    let mut last_dash = true;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "extension".into()
    } else {
        trimmed
    }
}

pub fn extensions_dir(app_data: &Path) -> PathBuf {
    app_data.join("extensions")
}

/// Validate a candidate file and describe what installing it would add.
///
/// Split from [`install`] so the UI can show the name and the permission list
/// *before* anything is copied anywhere.
pub fn inspect(source_path: &Path) -> Result<Extension, String> {
    match source_path.extension().and_then(|e| e.to_str()) {
        Some("js") | Some("mjs") => {}
        Some(other) => {
            return Err(format!(
                "Extensions are JavaScript files. This one is a .{other} — rename it to .js, \
                 or ask the prompt starter for a JavaScript version."
            ))
        }
        None => return Err("That file has no extension. Extensions are .js files.".into()),
    }

    let source = std::fs::read_to_string(source_path)
        .map_err(|e| format!("Could not read that file: {e}"))?;

    if source.len() > 512 * 1024 {
        return Err("That file is over 512 KB. Extensions are single scripts, not bundles.".into());
    }

    let header = parse_header(&source);
    if !header.versioned {
        return Err(
            "That file has no `@caduceus` header, so Caduceus cannot tell what it is or what it \
             wants access to. Add the header block from the Extensions tab to the top of the file."
                .into(),
        );
    }

    let unknown: Vec<&String> = header
        .permissions
        .iter()
        .filter(|p| !PERMISSIONS.contains(&p.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "Unknown permission: {}. Valid ones are {}.",
            unknown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            PERMISSIONS.join(", ")
        ));
    }

    let id = id_from_filename(source_path);
    Ok(Extension {
        name: header.name.unwrap_or_else(|| id.clone()),
        description: header.description.unwrap_or_default(),
        author: header.author.unwrap_or_default(),
        permissions: header.permissions,
        path: source_path.to_string_lossy().to_string(),
        enabled: true,
        id,
    })
}

/// Copy a validated file into the extensions directory.
pub fn install(source_path: &Path, app_data: &Path) -> Result<Extension, String> {
    let mut ext = inspect(source_path)?;

    let dir = extensions_dir(app_data);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {dir:?}: {e}"))?;

    let dest = dir.join(format!("{}.js", ext.id));
    std::fs::copy(source_path, &dest)
        .map_err(|e| format!("Could not install it: {e}"))?;

    ext.path = dest.to_string_lossy().to_string();
    Ok(ext)
}

/// Every installed extension, newest name first.
pub fn list(app_data: &Path) -> Vec<Extension> {
    let dir = extensions_dir(app_data);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out: Vec<Extension> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("js"))
        // A file that no longer parses is skipped rather than shown broken:
        // it was validated on the way in, so this means it was edited since.
        .filter_map(|p| inspect(&p).ok())
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn remove(id: &str, app_data: &Path) -> Result<(), String> {
    // Never let an id out of the UI become a path: `../../something` would
    // otherwise delete a file well outside the extensions directory.
    let safe = id_from_filename(Path::new(id));
    let path = extensions_dir(app_data).join(format!("{safe}.js"));
    if !path.is_file() {
        return Err("That extension is not installed.".into());
    }
    std::fs::remove_file(&path).map_err(|e| format!("Could not remove it: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "caduceus-ext-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    const GOOD: &str = r#"/**
 * @caduceus 1
 * name: Word Count
 * description: Counts words
 * author: someone
 * permissions: clipboard, network
 */
export default async function (input, ctx) {}
"#;

    #[test]
    fn a_header_describes_the_extension_without_running_it() {
        let dir = tmp();
        let file = write(&dir, "word-count.js", GOOD);
        let ext = inspect(&file).unwrap();
        assert_eq!(ext.name, "Word Count");
        assert_eq!(ext.description, "Counts words");
        assert_eq!(ext.author, "someone");
        assert_eq!(ext.permissions, vec!["clipboard", "network"]);
        assert_eq!(ext.id, "word-count");
    }

    #[test]
    fn a_file_with_no_header_is_refused() {
        let dir = tmp();
        let file = write(&dir, "bare.js", "export default () => {}");
        assert!(inspect(&file).unwrap_err().contains("@caduceus"));
    }

    #[test]
    fn an_unknown_permission_fails_at_install_rather_than_being_ignored() {
        let dir = tmp();
        let file = write(
            &dir,
            "sneaky.js",
            "/**\n * @caduceus 1\n * name: X\n * permissions: clipboard, filesystem\n */\n",
        );
        let err = inspect(&file).unwrap_err();
        assert!(err.contains("filesystem"));
    }

    #[test]
    fn only_javascript_is_accepted() {
        let dir = tmp();
        let file = write(&dir, "thing.py", "# @caduceus 1\n");
        assert!(inspect(&file).unwrap_err().contains("JavaScript"));
    }

    /// A later comment must not be able to redefine the permission list.
    #[test]
    fn only_the_first_comment_block_counts() {
        let dir = tmp();
        let file = write(
            &dir,
            "two-blocks.js",
            "/**\n * @caduceus 1\n * name: Honest\n * permissions: clipboard\n */\n\
             /**\n * name: Sneaky\n * permissions: network\n */\n",
        );
        let ext = inspect(&file).unwrap();
        assert_eq!(ext.name, "Honest");
        assert_eq!(ext.permissions, vec!["clipboard"]);
    }

    #[test]
    fn filenames_become_safe_ids() {
        assert_eq!(id_from_filename(Path::new("My Cool Thing.js")), "my-cool-thing");
        assert_eq!(id_from_filename(Path::new("../../etc/passwd")), "passwd");
        assert_eq!(id_from_filename(Path::new("!!!.js")), "extension");
    }

    /// `remove` takes an id from the UI; it must not be usable as a path.
    #[test]
    fn remove_cannot_escape_the_extensions_directory() {
        let dir = tmp();
        let outside = write(&dir, "victim.js", "should survive");
        let app_data = dir.join("data");
        std::fs::create_dir_all(extensions_dir(&app_data)).unwrap();

        let _ = remove("../../victim", &app_data);
        assert!(outside.is_file(), "a traversing id must not delete anything outside");
    }

    #[test]
    fn installing_then_listing_round_trips() {
        let dir = tmp();
        let app_data = dir.join("data");
        let file = write(&dir, "word-count.js", GOOD);

        let installed = install(&file, &app_data).unwrap();
        assert!(Path::new(&installed.path).is_file());

        let listed = list(&app_data);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Word Count");

        remove("word-count", &app_data).unwrap();
        assert!(list(&app_data).is_empty());
    }
}
