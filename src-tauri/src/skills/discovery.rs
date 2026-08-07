//! Walk the skills directory and turn `SKILL.md` files into the metadata
//! every tier of progressive disclosure is built from.
//!
//! This is the only module that touches the filesystem to find *which*
//! skills exist — [`crate::skills::tiers`] renders what this returns,
//! [`crate::skills::manage`] mutates individual skills once
//! [`find_skill_dir`] has located one, and neither re-implements the walk.

use std::path::{Path, PathBuf};

use super::frontmatter::{self, Frontmatter};
use super::{truncate_chars, truncate_with_ellipsis, MAX_DESCRIPTION_LENGTH, MAX_NAME_LENGTH, SKILL_MD};

/// Directories a scan never treats as skill trees, wherever they appear:
/// VCS metadata, the recoverable-archive area ([`crate::skills::lifecycle`]),
/// and the Skills Hub-style install-manifest area some Hermes-compatible
/// tooling expects at `.hub/`. `.` itself is not listed because hidden dot-
/// directories are already excluded by [`is_hidden`] below.
const EXCLUDED_DIRS: &[&str] = &[".git", ".archive", ".hub"];

/// A skill's supporting-file subdirectories. When one of these sits directly
/// inside a directory that itself contains `SKILL.md`, it is progressive-
/// disclosure data for *that* skill (loaded via `skill_view(name, file_path)`,
/// see [`crate::skills::tiers`]) — never a root to scan for further skills,
/// even if some file under it happens to be named `SKILL.md` (an archived
/// copy of an old skill package, say). A directory with this same name that
/// is *not* a skill's immediate child (e.g. a category actually called
/// `scripts`) is unaffected — this check only ever looks at the current
/// directory's own children.
pub const SUPPORT_DIRS: [&str; 4] = ["references", "templates", "scripts", "assets"];

/// Recursion is bounded so a pathological symlink loop hangs or overflows
/// the stack never — no real skill tree nests anywhere close to this deep.
const MAX_WALK_DEPTH: usize = 24;

/// One discovered skill: everything [`crate::skills::tiers`] needs to render
/// any of the three disclosure tiers, plus the directory
/// [`crate::skills::manage`] and tier-2/3 reads need to actually open files.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedSkill {
    /// The frontmatter `name`, or the directory name when frontmatter omits
    /// it (frontmatter only strictly requires `name` and `description` for
    /// *writes*; discovery has to cope with whatever it finds on disk).
    pub name: String,
    /// Full description, already clamped to [`MAX_DESCRIPTION_LENGTH`].
    /// Empty when the skill has none.
    pub description: String,
    /// `Some(first-path-segment)` when the skill sits at least one directory
    /// below the skills root (`<category>/<name>/SKILL.md`); `None` for a
    /// flat `<name>/SKILL.md`.
    pub category: Option<String>,
    pub dir: PathBuf,
}

/// A minimal, model-facing view of a scanned skill — what
/// `skills_list` (tier 1) returns. Deliberately excludes `dir`: a filesystem
/// path is not something the model needs to reason about a skill, and not
/// serializing it means the skill list response can never leak the app's
/// data-directory layout.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub category: Option<String>,
}

impl From<&ScannedSkill> for SkillSummary {
    fn from(s: &ScannedSkill) -> Self {
        SkillSummary { name: s.name.clone(), description: s.description.clone(), category: s.category.clone() }
    }
}

/// Scan `root` for every valid, platform-compatible skill.
///
/// A skill whose `SKILL.md` fails to parse, or that has no readable content
/// at all, is skipped rather than failing the whole scan — one broken skill
/// (a user mid-edit, a corrupted file) must never hide every other skill
/// from the model. Results are sorted by `(category, name)`, matching
/// `tools/skills_tool.py::_sort_skills` so the tier-0/1 ordering is stable
/// run to run.
pub fn scan(root: &Path) -> Vec<ScannedSkill> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    for skill_md in walk_skill_md_files(root) {
        let Some(skill_dir) = skill_md.parent() else { continue };
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            log::debug!("skills: could not read {}", skill_md.display());
            continue;
        };
        let Ok((fm, _body)) = frontmatter::parse(&content) else {
            // Not fatal to the scan — see the doc comment above.
            continue;
        };
        if !platform_matches(&fm) {
            continue;
        }

        let dir_name = skill_dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let name = fm.name().filter(|n| !n.is_empty()).unwrap_or(dir_name);
        let name = truncate_chars(name, MAX_NAME_LENGTH);

        if !seen_names.insert(name.clone()) {
            // Two different directories resolved to the same skill name —
            // e.g. a frontmatter `name:` collides with another skill's
            // directory name. Caduceus has a single skills root (no
            // Hermes-style `external_dirs` merge to disambiguate), so unlike
            // `tools/skills_tool.py::skill_view` this does not need to refuse
            // and ask the caller to pick; keeping the first one found
            // (deterministic: sorted directory walk order) is enough, and
            // is at least visible in the log rather than silently random.
            log::debug!(
                "skills: '{name}' at {} duplicates an already-scanned skill; keeping the first one found",
                skill_dir.display()
            );
            continue;
        }

        let description = truncate_with_ellipsis(fm.description().unwrap_or_default(), MAX_DESCRIPTION_LENGTH);
        let category = category_from_path(root, &skill_md);

        found.push(ScannedSkill { name, description, category, dir: skill_dir.to_path_buf() });
    }

    found.sort_by(|a, b| (a.category.as_deref().unwrap_or(""), a.name.as_str()).cmp(&(b.category.as_deref().unwrap_or(""), b.name.as_str())));
    found
}

/// Find a scanned skill by its resolved `name` (frontmatter `name:`, or
/// directory name when absent — the same identity [`scan`] uses).
pub fn find_skill(root: &Path, name: &str) -> Option<ScannedSkill> {
    scan(root).into_iter().find(|s| s.name == name)
}

/// Just the directory, for callers (like [`crate::skills::manage`]) that
/// already know they are about to do filesystem work and do not need the
/// rest of [`ScannedSkill`].
pub fn find_skill_dir(root: &Path, name: &str) -> Option<PathBuf> {
    find_skill(root, name).map(|s| s.dir)
}

/// Whether `fm`'s `platforms:` list (if any) includes the OS Caduceus is
/// actually running on. An absent or empty list means "every platform,"
/// matching `agent/skill_utils.py::skill_matches_platform`'s default.
/// `std::env::consts::OS` already yields the friendly names skill authors
/// write (`"macos"`, `"linux"`, `"windows"`) directly, so unlike Hermes'
/// `PLATFORM_MAP` (which maps a friendly name to Python's `sys.platform`
/// value, e.g. `"macos" -> "darwin"`), no translation table is needed here.
pub fn platform_matches(fm: &Frontmatter) -> bool {
    let platforms = fm.platforms();
    if platforms.is_empty() {
        return true;
    }
    let current = std::env::consts::OS;
    platforms.iter().any(|p| p.eq_ignore_ascii_case(current))
}

/// `<category>/<name>/SKILL.md` yields `Some("category")`; a flat
/// `<name>/SKILL.md` yields `None`. Mirrors
/// `tools/skills_tool.py::_get_category_from_path`: only the first path
/// segment is ever reported, even if a skill somehow sits deeper than one
/// category level.
fn category_from_path(root: &Path, skill_md: &Path) -> Option<String> {
    let rel = skill_md.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel.components().collect();
    if parts.len() < 3 {
        return None;
    }
    parts[0].as_os_str().to_str().map(str::to_string)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Recursively collect every `SKILL.md` under `root`, pruning
/// [`EXCLUDED_DIRS`], hidden directories, and — per directory — its own
/// [`SUPPORT_DIRS`] children when that directory itself has a `SKILL.md`.
/// Sorted, so scan order (and therefore which of two colliding names wins,
/// see [`scan`]) is deterministic rather than whatever the OS's directory
/// iteration order happens to be.
pub(super) fn walk_skill_md_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_dir(root, &mut out, 0);
    out.sort();
    out
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth >= MAX_WALK_DEPTH {
        log::warn!("skills: {} exceeds the max scan depth ({MAX_WALK_DEPTH}); not descending further", dir.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    let has_own_skill_md = children.iter().any(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(SKILL_MD));

    for path in children {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if path.is_dir() {
            if is_hidden(name) || EXCLUDED_DIRS.contains(&name) {
                continue;
            }
            if has_own_skill_md && SUPPORT_DIRS.contains(&name) {
                continue;
            }
            walk_dir(&path, out, depth + 1);
        } else if name == SKILL_MD {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A private, per-test scratch directory — same pattern as
    /// `appicons.rs`'s test module, avoiding a `tempfile` dependency for
    /// tests that only need an empty directory nothing else is using.
    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn write_skill(dir: &Path, rel_path: &str, frontmatter_and_body: &str) {
        let path = dir.join(rel_path).join(SKILL_MD);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, frontmatter_and_body).unwrap();
    }

    const MINIMAL: &str = "---\nname: {NAME}\ndescription: {DESC}\n---\nBody.\n";

    fn skill_md(name: &str, desc: &str) -> String {
        MINIMAL.replace("{NAME}", name).replace("{DESC}", desc)
    }

    #[test]
    fn finds_a_flat_skill_with_no_category() {
        let root = scratch_dir("flat");
        write_skill(&root, "apple-notes", &skill_md("apple-notes", "Manage notes"));

        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "apple-notes");
        assert_eq!(found[0].description, "Manage notes");
        assert_eq!(found[0].category, None);
    }

    #[test]
    fn finds_a_categorized_skill() {
        let root = scratch_dir("categorized");
        write_skill(&root, "mlops/axolotl", &skill_md("axolotl", "Fine-tune models"));

        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "axolotl");
        assert_eq!(found[0].category, Some("mlops".to_string()));
    }

    #[test]
    fn falls_back_to_the_directory_name_when_frontmatter_omits_name() {
        let root = scratch_dir("fallback-name");
        write_skill(&root, "my-skill", "---\ndescription: something\n---\nBody\n");

        let found = scan(&root);
        assert_eq!(found[0].name, "my-skill");
    }

    #[test]
    fn a_skill_with_unparsable_frontmatter_is_skipped_not_fatal() {
        let root = scratch_dir("bad-and-good");
        write_skill(&root, "broken", "no frontmatter fence at all\n");
        write_skill(&root, "good", &skill_md("good", "fine"));

        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "good");
    }

    #[test]
    fn results_are_sorted_by_category_then_name() {
        let root = scratch_dir("sorting");
        write_skill(&root, "zzz-flat", &skill_md("zzz-flat", "d"));
        write_skill(&root, "aaa-flat", &skill_md("aaa-flat", "d"));
        write_skill(&root, "zcat/skill-a", &skill_md("skill-a", "d"));
        write_skill(&root, "acat/skill-b", &skill_md("skill-b", "d"));

        let names_with_categories: Vec<(Option<String>, String)> =
            scan(&root).into_iter().map(|s| (s.category, s.name)).collect();

        // Flat (no category, sorts as "") skills come first, then
        // categories alphabetically.
        assert_eq!(
            names_with_categories,
            vec![
                (None, "aaa-flat".to_string()),
                (None, "zzz-flat".to_string()),
                (Some("acat".to_string()), "skill-b".to_string()),
                (Some("zcat".to_string()), "skill-a".to_string()),
            ]
        );
    }

    #[test]
    fn a_skill_md_inside_a_support_directory_is_not_itself_a_skill() {
        let root = scratch_dir("support-dirs");
        write_skill(&root, "real-skill", &skill_md("real-skill", "d"));
        // An archived copy of an old skill package, preserved as reference
        // material — this must not show up as its own discoverable skill.
        write_skill(&root, "real-skill/references/old-package", &skill_md("old-package", "stale"));

        let names: Vec<String> = scan(&root).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["real-skill".to_string()]);
    }

    #[test]
    fn a_directory_literally_named_scripts_that_is_not_a_skills_own_child_is_still_scanned() {
        let root = scratch_dir("scripts-as-category");
        // "scripts" here is a *category*, not `real-skill`'s support dir —
        // there is no SKILL.md directly inside `root` for it to belong to.
        write_skill(&root, "scripts/some-skill", &skill_md("some-skill", "d"));

        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, Some("scripts".to_string()));
    }

    #[test]
    fn excluded_directories_are_never_descended_into() {
        let root = scratch_dir("excluded");
        write_skill(&root, ".archive/old-skill", &skill_md("old-skill", "d"));
        write_skill(&root, ".git/hooks-skill", &skill_md("hooks-skill", "d"));
        write_skill(&root, ".hub/hub-skill", &skill_md("hub-skill", "d"));
        write_skill(&root, "real", &skill_md("real", "d"));

        let names: Vec<String> = scan(&root).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["real".to_string()]);
    }

    #[test]
    fn hidden_directories_are_skipped() {
        let root = scratch_dir("hidden");
        write_skill(&root, ".hidden-cat/hidden-skill", &skill_md("hidden-skill", "d"));
        write_skill(&root, "visible", &skill_md("visible", "d"));

        let names: Vec<String> = scan(&root).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["visible".to_string()]);
    }

    #[test]
    fn a_missing_root_directory_scans_as_empty_not_an_error() {
        let root = scratch_dir("missing").join("does-not-exist");
        assert_eq!(scan(&root), Vec::new());
    }

    #[test]
    fn find_skill_locates_by_resolved_name() {
        let root = scratch_dir("find");
        write_skill(&root, "cat/my-skill", &skill_md("my-skill", "d"));

        let found = find_skill(&root, "my-skill").expect("should find it");
        assert_eq!(found.dir, root.join("cat/my-skill"));
        assert!(find_skill(&root, "nope").is_none());
    }

    #[test]
    fn duplicate_resolved_names_keep_only_the_first_scanned() {
        let root = scratch_dir("dup-names");
        // Both resolve to the name "dup" — one via frontmatter, one via
        // directory name. Whichever sorts first on disk wins; the point of
        // this test is only that the scan does not panic or return two
        // entries with the same name.
        write_skill(&root, "a-dir", &skill_md("dup", "first"));
        write_skill(&root, "dup", "---\ndescription: second\n---\nBody\n");

        let found = scan(&root);
        let dups: Vec<_> = found.iter().filter(|s| s.name == "dup").collect();
        assert_eq!(dups.len(), 1);
    }

    // -- platform_matches ----------------------------------------------------

    #[test]
    fn platform_matches_is_true_when_the_field_is_absent() {
        let (fm, _) = frontmatter::parse("---\nname: x\ndescription: y\n---\nBody\n").unwrap();
        assert!(platform_matches(&fm));
    }

    #[test]
    fn platform_matches_respects_an_explicit_list() {
        let (fm, _) = frontmatter::parse("---\nname: x\ndescription: y\nplatforms: [totally-not-a-real-os]\n---\nBody\n").unwrap();
        assert!(!platform_matches(&fm));
    }

    #[test]
    fn platform_matches_is_true_when_the_current_os_is_listed() {
        let content = format!("---\nname: x\ndescription: y\nplatforms: [{}]\n---\nBody\n", std::env::consts::OS);
        let (fm, _) = frontmatter::parse(&content).unwrap();
        assert!(platform_matches(&fm));
    }

    #[test]
    fn a_platform_incompatible_skill_does_not_appear_in_a_scan() {
        let root = scratch_dir("platform-filter");
        write_skill(
            &root,
            "incompatible",
            "---\nname: incompatible\ndescription: d\nplatforms: [totally-not-a-real-os]\n---\nBody\n",
        );
        assert_eq!(scan(&root), Vec::new());
    }
}
