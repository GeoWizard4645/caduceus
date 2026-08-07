//! The three-tier progressive disclosure surface: the always-visible catalog
//! (tier 0), the fuller listing behind `skills_list` (tier 1), and a
//! skill's full content behind `skill_view` (tiers 2 and 3).
//!
//! # Why tier 0 is a plain render, not an index
//!
//! There is no ranking, embedding, or search anywhere in this module —
//! confirmed by reading the reference implementation, which does none of
//! that either. Tier 0 exists purely so a model sees *something* about
//! every skill without spending a tool call, and its accompanying
//! instruction ([`TIER0_INSTRUCTION`]) tells the model to err on the side of
//! loading anything that looks even partially relevant. The model does the
//! actual matching by reading names and descriptions, the same way it reads
//! everything else in its context. [`render_tier0_cached`]'s snapshot file
//! is a cold-start optimisation for that render — never a retrieval
//! shortcut — invalidated the moment the skill tree's contents change.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::discovery::{self, ScannedSkill, SkillSummary};
use super::frontmatter;
use super::usage;
use super::{truncate_with_ellipsis, write_atomically, SKILL_MD, TIER0_DESCRIPTION_CHARS};

pub const SNAPSHOT_FILE: &str = ".skills_prompt_snapshot.json";

/// Accompanies the tier-0 catalog verbatim, per the task brief: skill
/// *selection* is the model reading this list and deciding to load
/// something, never a scored search result, so the instruction has to do
/// the work a ranker would otherwise do — bias toward loading.
pub const TIER0_INSTRUCTION: &str =
    "If a skill matches or is even partially relevant, you MUST load it with skill_view(name). Err on the side of loading.";

// ---------------------------------------------------------------------------
// Tier 0 — always-visible catalog
// ---------------------------------------------------------------------------

/// Render the tier-0 catalog fresh from disk: every visible skill's name and
/// a 60-character description, grouped by category. Empty string when there
/// are no skills at all — nothing worth adding to a system prompt.
pub fn render_tier0(root: &Path) -> String {
    render_tier0_from(&discovery::scan(root))
}

fn render_tier0_from(skills: &[ScannedSkill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("## Available skills\n\n");
    out.push_str(TIER0_INSTRUCTION);
    out.push('\n');

    // `skills` is already sorted by (category, name) — see `discovery::scan`
    // — so grouping only has to notice when the category changes, never
    // re-sort or bucket anything itself.
    let mut current_category: Option<&str> = None;
    let mut seen_first_group = false;
    for skill in skills {
        let category = skill.category.as_deref();
        if !seen_first_group || category != current_category {
            out.push('\n');
            out.push_str(&format!("### {}\n", category.unwrap_or("General")));
            current_category = category;
            seen_first_group = true;
        }
        let description = truncate_with_ellipsis(&skill.description, TIER0_DESCRIPTION_CHARS);
        out.push_str(&format!("- {}: {description}\n", skill.name));
    }
    out
}

/// One `SKILL.md`'s identity for cache-invalidation purposes: its path
/// (relative to the skills root, so the cache is portable across an app
/// data directory move) plus mtime and size. Content itself is never
/// hashed here — mtime/size is the cheap manifest signal the task brief
/// asks for; a real content hash is what [`crate::skills::bundled`] uses
/// where correctness (not cache-freshness) is actually on the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ManifestEntry {
    path: String,
    mtime_secs: i64,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    manifest: Vec<ManifestEntry>,
    rendered: String,
}

fn build_manifest(root: &Path) -> Vec<ManifestEntry> {
    // Reuses the exact walk `discovery::scan` itself uses, so the cache's
    // notion of "what's on disk" can never drift from what a real scan
    // would see — a skill `scan` would find but this manifest missed (or
    // vice versa) would mean stale tier-0 output surviving a real change.
    discovery::walk_skill_md_files(root)
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let mtime_secs = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
            let rel = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
            Some(ManifestEntry { path: rel, mtime_secs, size: meta.len() })
        })
        .collect()
}

/// [`render_tier0`], but reusing a cached render from [`SNAPSHOT_FILE`] when
/// the skill tree has not changed since it was written — the win is
/// skipping re-reading and re-parsing every `SKILL.md` on a cold start when
/// nothing has actually changed since the last one.
pub fn render_tier0_cached(root: &Path) -> String {
    let manifest = build_manifest(root);
    let snapshot_path = root.join(SNAPSHOT_FILE);

    if let Some(snapshot) = read_snapshot(&snapshot_path) {
        if snapshot.manifest == manifest {
            return snapshot.rendered;
        }
    }

    let rendered = render_tier0(root);
    write_snapshot(&snapshot_path, &Snapshot { manifest, rendered: rendered.clone() });
    rendered
}

fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_snapshot(path: &Path, snapshot: &Snapshot) {
    if let Ok(text) = serde_json::to_string(snapshot) {
        if let Err(e) = write_atomically(path, &text) {
            log::debug!("skills: could not write {}: {e} (non-fatal — just loses the cold-start cache)", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 1 — skills_list
// ---------------------------------------------------------------------------

/// Every visible skill's name, full (untruncated) description, and
/// category — what `skills_list` hands back.
pub fn list_skills(root: &Path, category_filter: Option<&str>) -> Vec<SkillSummary> {
    discovery::scan(root)
        .iter()
        .filter(|s| category_filter.is_none_or(|c| s.category.as_deref() == Some(c)))
        .map(SkillSummary::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Tiers 2 & 3 — skill_view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedFiles {
    pub references: Vec<String>,
    pub templates: Vec<String>,
    pub scripts: Vec<String>,
    pub assets: Vec<String>,
}

impl LinkedFiles {
    fn is_empty(&self) -> bool {
        self.references.is_empty() && self.templates.is_empty() && self.scripts.is_empty() && self.assets.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub name: String,
    pub description: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub related_skills: Vec<String>,
    /// The skill body — everything after the closing `---` fence, verbatim.
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_files: Option<LinkedFiles>,
}

/// Tier 2: a skill's full `SKILL.md` content plus what supporting files it
/// has (not their content — see [`view_skill_file`] for that). Bumps
/// `view_count` and `use_count` on success: loading a skill's content is
/// both "looked at it" and "is about to act on it," matching
/// `tools/skills_tool.py::_skill_view_with_bump`'s reasoning for bumping
/// both counters from one call.
pub fn view_skill(root: &Path, name: &str) -> Result<SkillView, String> {
    let scanned = discovery::find_skill(root, name)
        .ok_or_else(|| format!("Skill '{name}' not found. Use skills_list to see available skills."))?;

    let skill_md = scanned.dir.join(SKILL_MD);
    let content = std::fs::read_to_string(&skill_md).map_err(|e| format!("could not read {}: {e}", skill_md.display()))?;
    let (fm, body) = frontmatter::parse(&content).map_err(|e| format!("skill '{name}' has invalid frontmatter: {e}"))?;

    let linked = linked_files(&scanned.dir);

    bump_view_and_use(root, &scanned.name);

    Ok(SkillView {
        name: scanned.name,
        description: scanned.description,
        category: scanned.category,
        tags: fm.tags(),
        related_skills: fm.related_skills(),
        content: body,
        linked_files: if linked.is_empty() { None } else { Some(linked) },
    })
}

/// Every file under `skill_dir`'s four support directories, recursively, as
/// paths relative to `skill_dir` (e.g. `"references/sub/deep.md"`) — ready
/// to hand straight back as a `file_path` argument for
/// [`view_skill_file`]. Unlike the reference implementation (whose
/// equivalent listing is non-recursive for some support dirs and recursive
/// for others, for no principled reason this module could find), all four
/// behave identically here: fully recursive, so nested reference material
/// is always addressable.
fn linked_files(skill_dir: &Path) -> LinkedFiles {
    LinkedFiles {
        references: list_support_files(skill_dir, "references"),
        templates: list_support_files(skill_dir, "templates"),
        scripts: list_support_files(skill_dir, "scripts"),
        assets: list_support_files(skill_dir, "assets"),
    }
}

fn list_support_files(skill_dir: &Path, subdir: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_relative_files(&skill_dir.join(subdir), skill_dir, &mut out);
    out.sort();
    out
}

fn collect_relative_files(dir: &Path, skill_dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_relative_files(&path, skill_dir, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(skill_dir) {
                // Forward slashes on every platform, so a `file_path` this
                // returns is always valid to pass straight back in,
                // regardless of the OS the skill directory lives on.
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

#[derive(Debug)]
pub enum SkillFileContent {
    Text(String),
    /// Not UTF-8 — reported rather than read, mirroring
    /// `tools/skills_tool.py::skill_view`'s "[Binary file: ...]" fallback,
    /// so a stray image under `assets/` does not turn into a decode error.
    Binary { size: u64 },
}

/// Tier 3: one supporting file's content. `file_path` is relative to the
/// skill's own directory (e.g. `"references/api.md"`) and is not restricted
/// to `references/templates/scripts/assets` — unlike
/// [`crate::skills::manage`]'s write paths, a *read* is allowed anywhere
/// inside the skill directory, matching the reference implementation's own
/// asymmetry between viewing and writing. Also bumps `view_count`/
/// `use_count`, for the same reason as [`view_skill`].
pub fn view_skill_file(root: &Path, name: &str, file_path: &str) -> Result<SkillFileContent, String> {
    let scanned = discovery::find_skill(root, name)
        .ok_or_else(|| format!("Skill '{name}' not found. Use skills_list to see available skills."))?;

    let target = resolve_within(&scanned.dir, file_path)?;
    if !target.is_file() {
        return Err(format!("File '{file_path}' not found in skill '{name}'."));
    }

    let bytes = std::fs::read(&target).map_err(|e| format!("could not read '{file_path}': {e}"))?;
    let result = match String::from_utf8(bytes) {
        Ok(text) => SkillFileContent::Text(text),
        Err(e) => SkillFileContent::Binary { size: e.into_bytes().len() as u64 },
    };

    bump_view_and_use(root, &scanned.name);
    Ok(result)
}

fn bump_view_and_use(root: &Path, name: &str) {
    let usage_path = usage::path_under(root);
    usage::bump_view(&usage_path, name);
    usage::bump_use(&usage_path, name);
}

/// Join `file_path` onto `skill_dir`, refusing anything that could reach
/// outside it: an absolute path, a `..` component, or — for a path that
/// already exists — a symlink whose resolved target escapes the directory.
fn resolve_within(skill_dir: &Path, file_path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(file_path);
    if candidate.is_absolute() {
        return Err("file_path must be relative, not absolute".to_string());
    }
    if candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("path traversal ('..') is not allowed".to_string());
    }

    let target = skill_dir.join(candidate);
    // Canonicalize-and-compare only when the target exists — a path that
    // does not exist yet cannot have escaped anywhere, and canonicalizing a
    // missing path would only fail and complicate the "file not found"
    // error the caller already produces.
    if let (Ok(canon_target), Ok(canon_dir)) = (target.canonicalize(), skill_dir.canonicalize()) {
        if !canon_target.starts_with(&canon_dir) {
            return Err("path escapes the skill directory".to_string());
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-tiers-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(root: &Path, rel: &str, name: &str, description: &str, body: &str) {
        let dir = root.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_MD), format!("---\nname: {name}\ndescription: {description}\n---\n{body}")).unwrap();
    }

    // -- render_tier0 --------------------------------------------------------

    #[test]
    fn an_empty_skills_tree_renders_to_an_empty_string() {
        let root = scratch_dir("tier0-empty");
        assert_eq!(render_tier0(&root), "");
    }

    #[test]
    fn tier0_includes_the_required_instruction_verbatim() {
        let root = scratch_dir("tier0-instruction");
        write_skill(&root, "a", "a", "does a thing", "Body\n");
        let rendered = render_tier0(&root);
        assert!(rendered.contains(TIER0_INSTRUCTION));
    }

    #[test]
    fn tier0_groups_by_category_with_uncategorized_first() {
        let root = scratch_dir("tier0-groups");
        write_skill(&root, "flat-skill", "flat-skill", "d", "Body\n");
        write_skill(&root, "cat-a/skill-a", "skill-a", "d", "Body\n");
        write_skill(&root, "cat-b/skill-b", "skill-b", "d", "Body\n");

        let rendered = render_tier0(&root);
        let general_pos = rendered.find("### General").unwrap();
        let cat_a_pos = rendered.find("### cat-a").unwrap();
        let cat_b_pos = rendered.find("### cat-b").unwrap();
        assert!(general_pos < cat_a_pos && cat_a_pos < cat_b_pos, "{rendered}");
        assert!(rendered.contains("- flat-skill: d"));
        assert!(rendered.contains("- skill-a: d"));
    }

    #[test]
    fn tier0_truncates_descriptions_to_sixty_characters() {
        let root = scratch_dir("tier0-truncate");
        let long_desc = "x".repeat(200);
        write_skill(&root, "long", "long", &long_desc, "Body\n");

        let rendered = render_tier0(&root);
        let line = rendered.lines().find(|l| l.starts_with("- long:")).unwrap();
        let shown = line.trim_start_matches("- long: ");
        assert_eq!(shown.chars().count(), 60);
        assert!(shown.ends_with("..."));
    }

    #[test]
    fn tier0_does_not_truncate_a_description_already_under_sixty_chars() {
        let root = scratch_dir("tier0-no-truncate");
        write_skill(&root, "short", "short", "brief", "Body\n");
        let rendered = render_tier0(&root);
        assert!(rendered.contains("- short: brief\n"));
    }

    // -- render_tier0_cached ---------------------------------------------------

    #[test]
    fn the_cache_reuses_a_render_when_nothing_changed() {
        let root = scratch_dir("cache-hit");
        write_skill(&root, "a", "a", "d", "Body\n");

        let first = render_tier0_cached(&root);
        assert!(root.join(SNAPSHOT_FILE).exists());

        // Corrupt what a fresh render would produce by hand-editing the
        // cached "rendered" field, then confirm a second call — with the
        // skill tree unchanged — returns the (now-wrong) cached text rather
        // than re-rendering. This is how we know it is actually reusing the
        // cache rather than coincidentally recomputing the same string.
        let snapshot_path = root.join(SNAPSHOT_FILE);
        let mut snapshot: Snapshot = serde_json::from_str(&std::fs::read_to_string(&snapshot_path).unwrap()).unwrap();
        snapshot.rendered = "SENTINEL-FROM-CACHE".to_string();
        std::fs::write(&snapshot_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

        let second = render_tier0_cached(&root);
        assert_eq!(second, "SENTINEL-FROM-CACHE");
        assert_ne!(first, second, "sanity: the sentinel really did replace the original render");
    }

    #[test]
    fn the_cache_invalidates_when_a_skill_is_added() {
        let root = scratch_dir("cache-invalidate-add");
        write_skill(&root, "a", "a", "d", "Body\n");
        render_tier0_cached(&root);

        write_skill(&root, "b", "b", "d", "Body\n");
        let rendered = render_tier0_cached(&root);
        assert!(rendered.contains("- b: d"), "a newly added skill must appear after the cache invalidates: {rendered}");
    }

    #[test]
    fn the_cache_invalidates_when_a_skill_md_is_edited() {
        let root = scratch_dir("cache-invalidate-edit");
        write_skill(&root, "a", "a", "original description", "Body\n");
        let first = render_tier0_cached(&root);
        assert!(first.contains("original description"));

        // Sleep briefly so the rewritten file's mtime is observably
        // different — some filesystems have 1-second mtime resolution.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_skill(&root, "a", "a", "updated description", "Body\n");

        let second = render_tier0_cached(&root);
        assert!(second.contains("updated description"), "{second}");
    }

    // -- list_skills (tier 1) ---------------------------------------------------

    #[test]
    fn list_skills_returns_full_untruncated_descriptions() {
        let root = scratch_dir("tier1-full-desc");
        let long_desc = "y".repeat(200);
        write_skill(&root, "a", "a", &long_desc, "Body\n");

        let summaries = list_skills(&root, None);
        assert_eq!(summaries[0].description.chars().count(), 200);
    }

    #[test]
    fn list_skills_filters_by_category() {
        let root = scratch_dir("tier1-filter");
        write_skill(&root, "cat-a/x", "x", "d", "Body\n");
        write_skill(&root, "cat-b/y", "y", "d", "Body\n");

        let filtered = list_skills(&root, Some("cat-a"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "x");
    }

    // -- view_skill (tier 2) -----------------------------------------------------

    #[test]
    fn view_skill_returns_the_body_tags_and_related_skills() {
        let root = scratch_dir("tier2-basic");
        write_skill(
            &root,
            "a",
            "a",
            "d",
            "# Title\n\nBody text here.\n",
        );
        // Add tags/related_skills via a second write with fuller frontmatter.
        std::fs::write(
            root.join("a/SKILL.md"),
            "---\nname: a\ndescription: d\nmetadata:\n  hermes:\n    tags: [t1, t2]\n    related_skills: [other]\n---\n# Title\n\nBody text here.\n",
        )
        .unwrap();

        let view = view_skill(&root, "a").unwrap();
        assert_eq!(view.name, "a");
        assert_eq!(view.tags, vec!["t1", "t2"]);
        assert_eq!(view.related_skills, vec!["other"]);
        assert!(view.content.contains("Body text here."));
        assert!(view.linked_files.is_none());
    }

    #[test]
    fn view_skill_bumps_view_and_use_counts() {
        let root = scratch_dir("tier2-bump");
        write_skill(&root, "a", "a", "d", "Body\n");

        view_skill(&root, "a").unwrap();
        view_skill(&root, "a").unwrap();

        let record = usage::get_record(&usage::path_under(&root), "a");
        assert_eq!(record.view_count, 2);
        assert_eq!(record.use_count, 2);
    }

    #[test]
    fn view_skill_lists_supporting_files_without_their_content() {
        let root = scratch_dir("tier2-linked-files");
        write_skill(&root, "a", "a", "d", "Body\n");
        std::fs::create_dir_all(root.join("a/references")).unwrap();
        std::fs::write(root.join("a/references/api.md"), "reference content").unwrap();
        std::fs::create_dir_all(root.join("a/scripts")).unwrap();
        std::fs::write(root.join("a/scripts/run.sh"), "#!/bin/sh").unwrap();

        let view = view_skill(&root, "a").unwrap();
        let linked = view.linked_files.unwrap();
        assert_eq!(linked.references, vec!["references/api.md".to_string()]);
        assert_eq!(linked.scripts, vec!["scripts/run.sh".to_string()]);
        assert!(linked.templates.is_empty());
        assert!(linked.assets.is_empty());
    }

    #[test]
    fn view_skill_on_an_unknown_name_is_a_clear_error() {
        let root = scratch_dir("tier2-not-found");
        let err = view_skill(&root, "nope").unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    // -- view_skill_file (tier 3) -------------------------------------------------

    #[test]
    fn view_skill_file_reads_a_reference_file() {
        let root = scratch_dir("tier3-basic");
        write_skill(&root, "a", "a", "d", "Body\n");
        std::fs::create_dir_all(root.join("a/references")).unwrap();
        std::fs::write(root.join("a/references/api.md"), "reference content").unwrap();

        match view_skill_file(&root, "a", "references/api.md").unwrap() {
            SkillFileContent::Text(text) => assert_eq!(text, "reference content"),
            SkillFileContent::Binary { .. } => panic!("expected text"),
        }
    }

    #[test]
    fn view_skill_file_reports_binary_content_instead_of_erroring() {
        let root = scratch_dir("tier3-binary");
        write_skill(&root, "a", "a", "d", "Body\n");
        std::fs::create_dir_all(root.join("a/assets")).unwrap();
        std::fs::write(root.join("a/assets/logo.png"), [0xFFu8, 0xD8, 0xFF, 0x00, 0x01, 0x02]).unwrap();

        match view_skill_file(&root, "a", "assets/logo.png").unwrap() {
            SkillFileContent::Binary { size } => assert_eq!(size, 6),
            SkillFileContent::Text(_) => panic!("expected binary"),
        }
    }

    #[test]
    fn view_skill_file_rejects_path_traversal() {
        let root = scratch_dir("tier3-traversal");
        write_skill(&root, "a", "a", "d", "Body\n");
        let err = view_skill_file(&root, "a", "../../../etc/passwd").unwrap_err();
        assert!(err.contains("traversal"), "{err}");
    }

    #[test]
    fn view_skill_file_rejects_an_absolute_path() {
        let root = scratch_dir("tier3-absolute");
        write_skill(&root, "a", "a", "d", "Body\n");
        let err = view_skill_file(&root, "a", "/etc/passwd").unwrap_err();
        assert!(err.contains("absolute"), "{err}");
    }

    #[test]
    fn view_skill_file_on_a_missing_file_names_it() {
        let root = scratch_dir("tier3-missing");
        write_skill(&root, "a", "a", "d", "Body\n");
        let err = view_skill_file(&root, "a", "references/nope.md").unwrap_err();
        assert!(err.contains("nope.md"), "{err}");
    }

    #[test]
    fn view_skill_file_bumps_view_and_use_counts() {
        let root = scratch_dir("tier3-bump");
        write_skill(&root, "a", "a", "d", "Body\n");
        std::fs::create_dir_all(root.join("a/references")).unwrap();
        std::fs::write(root.join("a/references/api.md"), "content").unwrap();

        view_skill_file(&root, "a", "references/api.md").unwrap();
        let record = usage::get_record(&usage::path_under(&root), "a");
        assert_eq!(record.view_count, 1);
        assert_eq!(record.use_count, 1);
    }
}
