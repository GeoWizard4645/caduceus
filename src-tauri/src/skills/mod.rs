//! Self-evolving skills: a directory of `SKILL.md` files the agent reads to
//! learn a procedure, and writes to when it learns a new one.
//!
//! Modeled on Hermes Agent's skills system (`~/.hermes/hermes-agent/tools/
//! skills_tool.py`, `skill_manager_tool.py`, `skill_usage.py`,
//! `agent/skill_utils.py` — read in full before anything here was written,
//! and referenced throughout this module's comments as "the reference
//! implementation"). What follows is the same design, not a reinterpretation
//! of it, with two changes forced by this codebase: no YAML crate (so
//! [`frontmatter`] is hand-rolled against a bounded, documented subset —
//! see that module's doc), and no Python `fuzzy_match` port (so
//! [`manage`]'s `patch` action is an exact-substring replace — see that
//! module's doc for why that is the right simplification rather than a
//! missing feature).
//!
//! # A skill
//!
//! A directory containing `SKILL.md` (only `name` and `description` are
//! actually required in its frontmatter — see [`frontmatter`]) plus
//! optional `references/`, `templates/`, `scripts/`, and `assets/`
//! subdirectories. Those four are lazy-loaded on demand
//! ([`tiers::view_skill_file`]) and never scanned as skills themselves
//! ([`discovery::SUPPORT_DIRS`]) — this is the public `agentskills.io`
//! shape, so a skill authored for Hermes (or anything else that follows the
//! standard) drops straight into `<app data>/skills/` and just works.
//!
//! # Selection has no ranker, on purpose
//!
//! There is no embedding index, no BM25, no reranker anywhere in this
//! module, and there should not be one — the reference implementation does
//! not have one either (verified by reading it end to end, not by absence
//! of evidence). Skill *selection* is progressive disclosure, in three
//! tiers, all the way down to the model reading text and deciding what is
//! relevant, the same way it reads everything else in its context:
//!
//! 1. **Tier 0** — [`tiers::render_tier0_cached`]. Every visible skill's
//!    name plus a 60-character description, grouped by category, meant to
//!    sit in the system prompt on every turn, alongside the fixed
//!    instruction [`tiers::TIER0_INSTRUCTION`] telling the model to err on
//!    the side of loading anything even partially relevant. The `_cached`
//!    variant's snapshot file is purely a cold-start optimisation — the
//!    manifest it invalidates against is a plain (path, mtime, size) list,
//!    never anything resembling a search index.
//! 2. **Tier 1** — [`tiers::list_skills`], behind the `skills_list` tool.
//!    Name, full description, category.
//! 3. **Tier 2 / 3** — [`tiers::view_skill`] / [`tiers::view_skill_file`],
//!    behind the `skill_view` tool. Full `SKILL.md` body, then one
//!    supporting file at a time.
//!
//! # The agent authors its own skills
//!
//! [`manage::skill_manage`], behind the `skill_manage` tool
//! ([`native::register`]), dispatches `create | patch | edit | delete |
//! write_file | remove_file`. `patch` — old_string/new_string,
//! [`manage`]'s doc has the detail — is the preferred path for anything
//! short of a full rewrite: cheaper in tokens, and the tool's own
//! description (see [`native::register`]) tells the model when to reach for
//! it and when a skill is worth creating in the first place.
//!
//! # Lifecycle is deterministic, not model-judged
//!
//! [`usage`] is a sidecar (`.usage.json`, never inside `SKILL.md` itself)
//! tracking `view_count` / `use_count` / `patch_count` / `state` / `pinned`
//! / `created_by` per skill. [`lifecycle::decide`] turns a record plus the
//! current time into a state — pure code, no model call, exactly the
//! reference implementation's own design constraint. Unused 30 days →
//! `stale`; unused 90 days → moved to `.archive/`, recoverable via
//! [`lifecycle::restore_skill`], never hard-deleted; pinned skills are
//! exempt from both. [`lifecycle::apply_transitions`] also accepts a
//! `protected` name set for "referenced by a scheduled job" — Caduceus has
//! no scheduled-job subsystem today for that to read from, so every current
//! caller passes an empty set; see that module's doc for the honest gap.
//!
//! `skill_manage(action = "delete")`, by contrast, hard-deletes — a
//! deliberate difference from the automatic sweep, and from the reference
//! implementation's own foreground/background split (its foreground delete
//! is also a hard delete; only its autonomous background curator, which
//! Caduceus has no equivalent of, routes deletes through the recoverable
//! archive). An explicit, user- or agent-requested delete keeping its plain
//! meaning seemed clearer than quietly softening it, given there is no
//! separate autonomous actor here for the softer path to be *for*. See
//! [`manage`]'s doc.
//!
//! [`bundled`] is the other half of "never surprise the user": Caduceus
//! ships a `skill-authoring` seed skill (embedded via `include_str!` from
//! `bundled_skills/skill-authoring/SKILL.md`, synced into the live skills
//! directory by [`sync_bundled_skills`]) tracked by content hash in
//! `.bundled_manifest`, so a future release can improve it without ever
//! clobbering a user's edits to their own copy.
//!
//! # Integration with the tool loop
//!
//! These tools are **built-in**, not MCP — [`crate::mcp`] spawns a
//! subprocess and speaks JSON-RPC to it; nothing here does either.
//! [`native::register`] wires `skills_list` / `skill_view` / `skill_manage`
//! into [`crate::native_tools`], a small process-global registry built
//! alongside this module specifically for built-in, in-process tools (see
//! that module's doc for the full design rationale — no
//! `src-tauri/src/memory/` or equivalent registry existed anywhere in the
//! repo or in any other in-progress worktree at the time this was written;
//! searched before building it). `agent::toolloop::run_tool_loop` does not
//! yet merge [`crate::native_tools::list`] / `::call` into the tool table it
//! sends a model — that file was under active concurrent edit and out of
//! this task's assigned module, so wiring the merge in was left to whoever
//! reconciles the two efforts, per the task brief. See this task's final
//! report for exactly what that merge should look like; the shapes were
//! chosen to make it a small addition, not a redesign.
//!
//! # House rule: reject rather than guess
//!
//! [`frontmatter`] parses a real but bounded subset of YAML and refuses,
//! with a line number, anything outside it — never a best-effort
//! misinterpretation. The same instinct shows up in [`discovery`] (a skill
//! whose frontmatter fails to parse is skipped from the scan, not treated
//! as fatal to every other skill) and in [`manage`] (a `patch` that would
//! leave `SKILL.md` without valid frontmatter is refused before it is
//! written, not after).

pub mod bundled;
pub mod commands;
pub mod discovery;
pub mod frontmatter;
pub mod lifecycle;
pub mod manage;
pub mod native;
pub mod tiers;
pub mod usage;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Shared limits — the numbers named throughout this module's and its
// submodules' doc comments, defined once so they can never drift out of
// sync with what the code actually enforces.
// ---------------------------------------------------------------------------

/// Frontmatter `name`, hard-truncated (no ellipsis) if a caller ignores
/// [`manage`]'s validation and something longer slips onto disk some other
/// way.
pub const MAX_NAME_LENGTH: usize = 64;
/// Frontmatter `description`, stored — truncated further, to
/// [`TIER0_DESCRIPTION_CHARS`], only in the tier-0 index.
pub const MAX_DESCRIPTION_LENGTH: usize = 1024;
/// The tier-0 catalog's per-skill description budget — see
/// `tiers::render_tier0`.
pub const TIER0_DESCRIPTION_CHARS: usize = 60;
/// A whole `SKILL.md`'s character budget, enforced on `create`/`edit`/`patch`.
pub const MAX_SKILL_CONTENT_CHARS: usize = 100_000;
/// One supporting file's byte budget, enforced on `write_file`.
pub const MAX_SKILL_FILE_BYTES: u64 = 1_048_576;

pub const SKILL_MD: &str = "SKILL.md";
/// The subdirectory of the app data directory skills live under —
/// `commands::skills_root` and `lib.rs::setup` both need this exact name.
pub const SKILLS_DIR_NAME: &str = "skills";

/// The bundled seed skill teaching the agent how to author good skills —
/// see [`sync_bundled_skills`] and [`bundled`]'s module doc for how it
/// reaches a user's skills directory without ever clobbering their edits.
const SKILL_AUTHORING_NAME: &str = "skill-authoring";
const SKILL_AUTHORING_CONTENT: &str = include_str!("bundled_skills/skill-authoring/SKILL.md");

/// Copy every bundled skill into `skills_root`, honoring [`bundled`]'s
/// content-hash clobber protection. Idempotent and cheap to call on every
/// launch (a no-op once a skill is installed and unedited) — see
/// `lib.rs::setup` for the call site.
pub fn sync_bundled_skills(skills_root: &Path) {
    match bundled::sync_bundled_skill(skills_root, SKILL_AUTHORING_NAME, SKILL_AUTHORING_CONTENT) {
        bundled::SyncOutcome::Failed(e) => {
            log::warn!("skills: could not sync the bundled '{SKILL_AUTHORING_NAME}' skill: {e}")
        }
        outcome => log::debug!("skills: bundled '{SKILL_AUTHORING_NAME}' sync result: {outcome:?}"),
    }
}

/// Register the built-in `skills_list` / `skill_view` / `skill_manage`
/// tools against `skills_root` — see `native::register` and this module's
/// "Integration with the tool loop" doc section. Call once, after
/// `skills_root` is known.
pub fn register_native_tools(skills_root: std::path::PathBuf) {
    native::register(skills_root);
}

// ---------------------------------------------------------------------------
// Small helpers shared by more than one submodule
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max` *characters* (never splitting a multi-byte
/// UTF-8 sequence) with no ellipsis — used for `name`, which the reference
/// implementation also hard-truncates rather than shortening with "...".
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Truncate `s` to at most `max` characters, appending `...` (counted
/// within the `max` budget) when truncation actually happens — used for
/// `description`, mirroring `tools/skills_tool.py`'s
/// `description[:N-3] + "..."` pattern for both the 1024-char storage cap
/// and (with a smaller `max`) the 60-char tier-0 cap.
pub(crate) fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let head: String = s.chars().take(max - 3).collect();
    format!("{head}...")
}

/// Write `contents` to `path` via a uniquely-named temporary file in the
/// same directory, then `rename` — the rename is atomic, so a reader (or a
/// crash) never observes a partially-written skill file. Same idiom as
/// `usage.rs::write_atomically` and `appicons.rs::convert_icns_to_png`
/// elsewhere in this codebase, duplicated rather than imported because
/// those are private to unrelated modules — creates the parent directory
/// first, since (unlike those two) a fresh skill's directory may not exist
/// yet on its very first write.
pub(crate) fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}-{n}", std::process::id()));

    std::fs::write(&tmp, contents)?;
    let result = std::fs::rename(&tmp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-mod-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- truncate_chars / truncate_with_ellipsis -----------------------------

    #[test]
    fn truncate_chars_never_adds_an_ellipsis() {
        assert_eq!(truncate_chars("hello world", 5), "hello");
        assert_eq!(truncate_chars("hi", 5), "hi");
    }

    #[test]
    fn truncate_chars_counts_unicode_scalars_not_bytes() {
        // Each of these is a multi-byte UTF-8 character; splitting by byte
        // offset would panic or corrupt the string.
        let s = "\u{1F600}\u{1F600}\u{1F600}\u{1F600}\u{1F600}"; // 5 emoji
        assert_eq!(truncate_chars(s, 2).chars().count(), 2);
    }

    #[test]
    fn truncate_with_ellipsis_leaves_short_strings_untouched() {
        assert_eq!(truncate_with_ellipsis("short", 60), "short");
    }

    #[test]
    fn truncate_with_ellipsis_caps_at_max_including_the_ellipsis() {
        let long = "x".repeat(200);
        let truncated = truncate_with_ellipsis(&long, 60);
        assert_eq!(truncated.chars().count(), 60);
        assert!(truncated.ends_with("..."));
        assert_eq!(&truncated[..57], "x".repeat(57));
    }

    #[test]
    fn truncate_with_ellipsis_trims_surrounding_whitespace_first() {
        assert_eq!(truncate_with_ellipsis("  padded  ", 60), "padded");
    }

    // -- write_atomically ------------------------------------------------------

    #[test]
    fn write_atomically_creates_missing_parent_directories() {
        let root = scratch_dir("write-atomic-parents");
        let target = root.join("a/b/c/file.txt");
        write_atomically(&target, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[test]
    fn write_atomically_leaves_no_temp_file_behind() {
        let root = scratch_dir("write-atomic-cleanup");
        let target = root.join("file.txt");
        write_atomically(&target, "content").unwrap();

        let leftover: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftover.is_empty(), "temp files left behind: {leftover:?}");
    }

    #[test]
    fn write_atomically_overwrites_existing_content() {
        let root = scratch_dir("write-atomic-overwrite");
        let target = root.join("file.txt");
        write_atomically(&target, "v1").unwrap();
        write_atomically(&target, "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "v2");
    }

    // -- sync_bundled_skills / SKILL_AUTHORING_CONTENT --------------------------

    #[test]
    fn the_bundled_skill_authoring_content_is_itself_valid_frontmatter() {
        // If this ever fails to parse, the seed skill Caduceus ships would
        // silently be invisible to discovery — so the embedded text is
        // exercised through the real parser, not just eyeballed.
        let (fm, body) = frontmatter::parse(SKILL_AUTHORING_CONTENT).expect("bundled skill-authoring frontmatter must parse");
        assert_eq!(fm.name(), Some(SKILL_AUTHORING_NAME));
        assert!(fm.description().is_some_and(|d| !d.is_empty()));
        assert!(!body.trim().is_empty());
    }

    #[test]
    fn sync_bundled_skills_installs_a_skill_that_discovery_then_finds() {
        let root = scratch_dir("sync-bundled");
        sync_bundled_skills(&root);

        let found = discovery::find_skill(&root, SKILL_AUTHORING_NAME).expect("bundled skill should be discoverable after sync");
        assert!(found.description.to_lowercase().contains("skill"));
    }

    #[test]
    fn sync_bundled_skills_is_idempotent_and_never_clobbers_a_user_edit() {
        let root = scratch_dir("sync-bundled-idempotent");
        sync_bundled_skills(&root);

        let skill_md = root.join(SKILL_AUTHORING_NAME).join(SKILL_MD);
        std::fs::write(&skill_md, "---\nname: skill-authoring\ndescription: user's own version\n---\nCustomized.\n").unwrap();

        sync_bundled_skills(&root); // simulates the next app launch
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("Customized."), "a user edit must survive a re-sync");
    }

    // -- end-to-end smoke test across submodules --------------------------------

    #[test]
    fn full_lifecycle_smoke_test_create_view_patch_and_tier0_render() {
        let root = scratch_dir("smoke-test");

        manage::skill_manage(
            &root,
            "create",
            "smoke-test-skill",
            Some("---\nname: smoke-test-skill\ndescription: exercises the whole module end to end\n---\nOriginal body.\n"),
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("create should succeed");

        let tier0 = tiers::render_tier0(&root);
        assert!(tier0.contains("smoke-test-skill"));

        let tier1 = tiers::list_skills(&root, None);
        assert_eq!(tier1.len(), 1);

        let tier2 = tiers::view_skill(&root, "smoke-test-skill").unwrap();
        assert!(tier2.content.contains("Original body."));

        manage::skill_manage(
            &root,
            "patch",
            "smoke-test-skill",
            None,
            None,
            None,
            None,
            Some("Original body."),
            Some("Patched body."),
            false,
        )
        .expect("patch should succeed");

        let record = usage::get_record(&usage::path_under(&root), "smoke-test-skill");
        assert_eq!(record.view_count, 1, "the tier-2 view above should have bumped it");
        assert_eq!(record.patch_count, 1);
        assert_eq!(record.created_by, Some("agent".to_string()));
    }
}
