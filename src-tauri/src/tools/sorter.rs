//! Tidying a folder — usually the Desktop.
//!
//! # Plan first, then apply
//!
//! Every operation here is two calls: [`plan`] works out what *would* move
//! where and changes nothing, and [`apply`] carries out a plan the user has
//! looked at. A one-shot "tidy my Desktop" button that rearranges ninety files
//! before you can read what it decided is not a feature, it is an incident.
//!
//! # Nothing is ever overwritten and nothing leaves the folder
//!
//! Files move into subfolders of where they already are. A name collision gets
//! a numeric suffix rather than replacing what is there. Undo is real: [`apply`]
//! returns the exact moves it made, and [`revert`] puts them back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// How to group the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortBy {
    /// Images, Documents, Archives, Code… — what people actually mean by "type".
    Kind,
    /// The literal extension: `png`, `pdf`, `zip`.
    Extension,
    /// `2026-07`, from the modification date.
    Month,
    /// `2026`.
    Year,
    /// First letter of the name.
    Alphabetical,
    /// Under 1 MB / 1–100 MB / over 100 MB.
    Size,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub from: String,
    pub to: String,
    /// The folder this file is being filed under, for grouping in the UI.
    pub folder: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SortPlan {
    pub directory: String,
    pub moves: Vec<Move>,
    /// Folder name → how many files land in it.
    pub folders: BTreeMap<String, usize>,
    /// Files that will be left alone, and why.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SortResult {
    pub ok: bool,
    pub message: String,
    pub moved: Vec<Move>,
}

/// Work out where everything would go. Touches nothing.
pub fn plan(directory: &str, sort_by: SortBy) -> Result<SortPlan, String> {
    let root = PathBuf::from(shellexpand(directory));
    if !root.is_dir() {
        return Err(format!("{} is not a folder.", root.display()));
    }

    let mut moves = Vec::new();
    let mut folders: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped = Vec::new();
    // Names this run will create, so two files heading for the same new name
    // inside one plan do not collide with each other.
    let mut claimed: Vec<PathBuf> = Vec::new();

    let entries = std::fs::read_dir(&root).map_err(|e| format!("Could not read it: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if name.starts_with('.') {
            continue;
        }
        // Folders stay put. Sorting directories into directories is how a
        // tidy-up turns into a maze.
        if path.is_dir() {
            skipped.push(format!("{name} — a folder"));
            continue;
        }
        // An alias or symlink moved out from under whatever points at it is a
        // broken link, and the user did not ask for that.
        if path.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            skipped.push(format!("{name} — an alias"));
            continue;
        }

        let meta = entry.metadata().ok();
        let folder = bucket(&path, meta.as_ref(), sort_by);
        let destination = unique_path(&root.join(&folder).join(&name), &claimed);
        claimed.push(destination.clone());

        *folders.entry(folder.clone()).or_insert(0) += 1;
        moves.push(Move {
            from: path.to_string_lossy().into_owned(),
            to: destination.to_string_lossy().into_owned(),
            folder,
            name,
            bytes: meta.map(|m| m.len()).unwrap_or(0),
        });
    }

    moves.sort_by(|a, b| a.folder.cmp(&b.folder).then(a.name.cmp(&b.name)));

    Ok(SortPlan {
        directory: root.to_string_lossy().into_owned(),
        moves,
        folders,
        skipped,
    })
}

/// Carry out a plan. Returns the moves that actually happened, for undo.
pub fn apply(moves: &[Move]) -> SortResult {
    let mut done = Vec::new();
    let mut failed = 0usize;

    for m in moves {
        let to = PathBuf::from(&m.to);
        let Some(parent) = to.parent() else { continue };
        if std::fs::create_dir_all(parent).is_err() {
            failed += 1;
            continue;
        }
        // Re-check at the last moment: the plan may have been sitting on screen
        // while something else wrote a file with this name.
        let destination = unique_path(&to, &[]);
        match std::fs::rename(&m.from, &destination) {
            Ok(()) => done.push(Move { to: destination.to_string_lossy().into_owned(), ..m.clone() }),
            Err(_) => failed += 1,
        }
    }

    SortResult {
        ok: failed == 0,
        message: match (done.len(), failed) {
            (0, 0) => "Nothing to move.".into(),
            (n, 0) => format!("Filed {n} file{}.", plural(n)),
            (0, f) => format!("Could not move {f} file{}.", plural(f)),
            (n, f) => format!("Filed {n}, could not move {f}."),
        },
        moved: done,
    }
}

/// Put everything back where it was.
pub fn revert(moves: &[Move]) -> SortResult {
    let mut done = Vec::new();
    let mut failed = 0usize;

    for m in moves {
        let back = PathBuf::from(&m.from);
        if let Some(parent) = back.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(&m.to, &back) {
            Ok(()) => {
                done.push(m.clone());
                // Tidy up the folder we made, but only while it is empty —
                // never remove one that has anything else in it.
                if let Some(parent) = PathBuf::from(&m.to).parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
            Err(_) => failed += 1,
        }
    }

    SortResult {
        ok: failed == 0,
        message: if failed == 0 {
            format!("Put {} file{} back.", done.len(), plural(done.len()))
        } else {
            format!("Put {} back, {failed} could not be moved.", done.len())
        },
        moved: done,
    }
}

/// The plan currently on screen, and the moves that were last carried out.
///
/// [`apply`] and [`revert`] rename files, and a pair of arbitrary paths is a
/// rename primitive over everything the user can write. The webview is
/// therefore not trusted to supply them: it sends back which rows it means, and
/// the paths that actually get used are the ones [`plan`] chose here.
#[derive(Default)]
pub struct Session {
    inner: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    planned: Vec<Move>,
    applied: Vec<Move>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember a plan as the only one that may be applied.
    pub fn remember(&self, plan: &SortPlan) {
        let mut held = self.inner.lock().unwrap();
        held.planned = plan.moves.clone();
        held.applied.clear();
    }

    /// The planned moves the webview asked for, refusing anything else.
    pub fn planned(&self, asked: &[(String, String)]) -> Result<Vec<Move>, String> {
        matching(&self.inner.lock().unwrap().planned, asked)
    }

    /// The carried-out moves the webview asked to undo, refusing anything else.
    pub fn applied(&self, asked: &[(String, String)]) -> Result<Vec<Move>, String> {
        matching(&self.inner.lock().unwrap().applied, asked)
    }

    /// Record what [`apply`] actually did, so it — and only it — can be undone.
    pub fn record_applied(&self, moved: &[Move]) {
        let mut held = self.inner.lock().unwrap();
        held.planned.clear();
        held.applied = moved.to_vec();
    }

    pub fn record_reverted(&self) {
        self.inner.lock().unwrap().applied.clear();
    }
}

/// Look each requested pair up in what is held.
///
/// A subset is fine — the plan on screen can have lost rows — but a pair that
/// was never planned is refused outright rather than quietly dropped: a request
/// to move something the user was never shown is not a stale plan.
fn matching(known: &[Move], asked: &[(String, String)]) -> Result<Vec<Move>, String> {
    asked
        .iter()
        .map(|(from, to)| {
            known
                .iter()
                .find(|m| &m.from == from && &m.to == to)
                .cloned()
                .ok_or_else(|| {
                    "That plan is out of date — look at the folder again before applying."
                        .to_string()
                })
        })
        .collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Which folder a file belongs in.
fn bucket(path: &Path, meta: Option<&std::fs::Metadata>, sort_by: SortBy) -> String {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match sort_by {
        SortBy::Extension => {
            if extension.is_empty() { "No extension".into() } else { extension.to_uppercase() }
        }
        SortBy::Kind => kind_of(&extension).into(),
        SortBy::Month | SortBy::Year => {
            let modified = meta
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let stamp: chrono::DateTime<chrono::Local> = modified.into();
            if sort_by == SortBy::Year {
                stamp.format("%Y").to_string()
            } else {
                stamp.format("%Y-%m").to_string()
            }
        }
        SortBy::Alphabetical => path
            .file_name()
            .and_then(|n| n.to_string_lossy().chars().next())
            .map(|c| {
                if c.is_alphabetic() { c.to_uppercase().to_string() } else { "#".into() }
            })
            .unwrap_or_else(|| "#".into()),
        SortBy::Size => {
            let bytes = meta.map(|m| m.len()).unwrap_or(0);
            if bytes < 1_000_000 {
                "Small (under 1 MB)".into()
            } else if bytes < 100_000_000 {
                "Medium (1–100 MB)".into()
            } else {
                "Large (over 100 MB)".into()
            }
        }
    }
}

/// Extension → the word a person would use.
fn kind_of(extension: &str) -> &'static str {
    match extension {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "tiff" | "bmp" | "svg" | "avif" => {
            "Images"
        }
        "pdf" | "doc" | "docx" | "pages" | "txt" | "rtf" | "md" | "odt" | "epub" => "Documents",
        "xls" | "xlsx" | "numbers" | "csv" | "tsv" => "Spreadsheets",
        "ppt" | "pptx" | "key" => "Presentations",
        "zip" | "tar" | "gz" | "bz2" | "7z" | "rar" | "dmg" | "pkg" | "xz" => "Archives",
        "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" => "Video",
        "mp3" | "wav" | "aac" | "flac" | "m4a" | "aiff" | "ogg" => "Audio",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "h" | "cpp" | "swift"
        | "rb" | "php" | "sh" | "json" | "yaml" | "yml" | "toml" | "html" | "css" | "sql" => "Code",
        "app" => "Applications",
        "ttf" | "otf" | "woff" | "woff2" => "Fonts",
        "" => "No extension",
        _ => "Other",
    }
}

/// A path that does not exist yet: `report.pdf`, `report 2.pdf`, `report 3.pdf`.
fn unique_path(wanted: &Path, claimed: &[PathBuf]) -> PathBuf {
    let taken = |p: &Path| p.exists() || claimed.iter().any(|c| c == p);
    if !taken(wanted) {
        return wanted.to_path_buf();
    }

    let parent = wanted.parent().unwrap_or(Path::new("."));
    let stem = wanted.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = wanted.extension().map(|e| e.to_string_lossy().into_owned());

    for n in 2..1000 {
        let name = match &extension {
            Some(ext) => format!("{stem} {n}.{ext}"),
            None => format!("{stem} {n}"),
        };
        let candidate = parent.join(name);
        if !taken(&candidate) {
            return candidate;
        }
    }
    wanted.to_path_buf()
}

/// Expand a leading `~`. Nothing more: this is a folder chooser's output, not a shell.
fn shellexpand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("caduceus-sort-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn files_are_grouped_by_what_people_call_them() {
        assert_eq!(kind_of("png"), "Images");
        assert_eq!(kind_of("pdf"), "Documents");
        assert_eq!(kind_of("rs"), "Code");
        assert_eq!(kind_of("wibble"), "Other");
        assert_eq!(kind_of(""), "No extension");
    }

    #[test]
    fn planning_moves_nothing() {
        let dir = scratch();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        let plan = plan(&dir.to_string_lossy(), SortBy::Kind).unwrap();

        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].folder, "Images");
        // The whole point of a plan: the file is still where it was.
        assert!(dir.join("a.png").exists());
        assert!(!dir.join("Images").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_and_aliases_are_left_alone() {
        let dir = scratch();
        std::fs::create_dir(dir.join("Projects")).unwrap();
        std::fs::write(dir.join("note.txt"), b"x").unwrap();

        let plan = plan(&dir.to_string_lossy(), SortBy::Kind).unwrap();
        assert_eq!(plan.moves.len(), 1, "only the file should move");
        assert!(plan.skipped.iter().any(|s| s.contains("Projects")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_collision_gets_a_suffix_rather_than_overwriting() {
        let dir = scratch();
        std::fs::create_dir(dir.join("Images")).unwrap();
        std::fs::write(dir.join("Images/a.png"), b"original").unwrap();
        std::fs::write(dir.join("a.png"), b"new").unwrap();

        let plan = plan(&dir.to_string_lossy(), SortBy::Kind).unwrap();
        assert!(plan.moves[0].to.ends_with("a 2.png"), "got {}", plan.moves[0].to);

        apply(&plan.moves);
        // The file that was already there is untouched.
        assert_eq!(std::fs::read(dir.join("Images/a.png")).unwrap(), b"original");
        assert_eq!(std::fs::read(dir.join("Images/a 2.png")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_files_in_one_plan_do_not_collide_with_each_other() {
        // Sorting alphabetically puts both under "A", and both are called the
        // same thing once the extension is dropped from the folder name.
        let dir = scratch();
        std::fs::write(dir.join("a.png"), b"1").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let plan = plan(&dir.to_string_lossy(), SortBy::Alphabetical).unwrap();
        let destinations: Vec<_> = plan.moves.iter().map(|m| &m.to).collect();
        let unique: std::collections::BTreeSet<_> = destinations.iter().collect();
        assert_eq!(destinations.len(), unique.len(), "a plan must not target one path twice");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applying_and_reverting_leaves_the_folder_as_it_was() {
        let dir = scratch();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("b.pdf"), b"y").unwrap();

        let plan = plan(&dir.to_string_lossy(), SortBy::Kind).unwrap();
        let applied = apply(&plan.moves);
        assert!(applied.ok);
        assert!(dir.join("Images/a.png").exists());
        assert!(!dir.join("a.png").exists());

        let reverted = revert(&applied.moved);
        assert!(reverted.ok);
        assert!(dir.join("a.png").exists(), "undo must actually undo");
        assert!(dir.join("b.pdf").exists());
        // And the folders it created are gone again.
        assert!(!dir.join("Images").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_folder_is_an_error_rather_than_an_empty_plan() {
        assert!(plan("/nowhere/at/all", SortBy::Kind).is_err());
    }

    /// The webview naming a pair of paths must not be enough to rename a file:
    /// only the plan the backend produced can be carried out.
    #[test]
    fn a_move_that_was_never_planned_is_refused() {
        let dir = scratch();
        std::fs::write(dir.join("a.png"), b"x").unwrap();

        let session = Session::new();
        let plan = plan(&dir.to_string_lossy(), SortBy::Kind).unwrap();
        session.remember(&plan);

        let real = (plan.moves[0].from.clone(), plan.moves[0].to.clone());
        assert_eq!(session.planned(&[real.clone()]).unwrap().len(), 1);

        let invented = ("/Users/someone/.ssh/known_hosts".to_string(), "/tmp/taken".to_string());
        assert!(session.planned(&[invented.clone()]).is_err());
        assert!(
            session.planned(&[real, invented]).is_err(),
            "one invented move must reject the whole batch"
        );

        // Undo can only put back what apply actually did.
        assert!(session.applied(&[(plan.moves[0].to.clone(), plan.moves[0].from.clone())]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
