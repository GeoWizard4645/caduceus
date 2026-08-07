//! `skill_manage`: how the agent authors its own skills.
//!
//! Six actions — `create`, `patch`, `edit`, `delete`, `write_file`,
//! `remove_file` — dispatched from [`skill_manage`]. `patch` is the
//! preferred path for anything short of a full rewrite: it is a plain
//! find-and-replace (see the note on [`patch_skill`] for how this differs
//! from the reference implementation's fuzzy matcher), and a targeted
//! change costs far fewer tokens than resending an entire `SKILL.md`.
//!
//! Every write here is atomic ([`super::write_atomically`]) and every path
//! argument is checked against the skill's own directory before anything
//! touches disk — see [`validate_write_file_path`] and
//! [`validate_delete_target`].

use std::path::{Path, PathBuf};

use super::discovery;
use super::frontmatter;
use super::usage;
use super::{write_atomically, MAX_DESCRIPTION_LENGTH, MAX_NAME_LENGTH, MAX_SKILL_CONTENT_CHARS, MAX_SKILL_FILE_BYTES, SKILL_MD};

const ALLOWED_SUBDIRS: [&str; 4] = ["references", "templates", "scripts", "assets"];

/// Dispatch one `skill_manage` call. `root` is the skills directory;
/// everything else mirrors the tool's flat argument shape directly (see
/// `skills::commands::skill_manage` / `skills::native` for how a JSON tool
/// call's arguments become these parameters).
///
/// On success, bumps the usage sidecar the same way the reference
/// implementation's `skill_manager_tool.py::skill_manage` does at the end of
/// its dispatch: `create` marks the skill as agent-created (the provenance
/// marker `crate::skills::lifecycle`'s protected-builtin/pin logic is not
/// gated on, but which reporting can use), `patch`/`edit`/`write_file`/
/// `remove_file` bump the patch counter, and `delete` drops the usage
/// record entirely.
#[allow(clippy::too_many_arguments)]
pub fn skill_manage(
    root: &Path,
    action: &str,
    name: &str,
    content: Option<&str>,
    category: Option<&str>,
    file_path: Option<&str>,
    file_content: Option<&str>,
    old_string: Option<&str>,
    new_string: Option<&str>,
    replace_all: bool,
) -> Result<String, String> {
    let result = match action {
        "create" => {
            let content = content.ok_or("content is required for 'create'. Provide the full SKILL.md text (frontmatter + body).")?;
            create_skill(root, name, content, category)
        }
        "edit" => {
            let content = content.ok_or("content is required for 'edit'. Provide the complete updated SKILL.md text.")?;
            edit_skill(root, name, content)
        }
        "patch" => {
            let old_string = old_string.filter(|s| !s.is_empty()).ok_or("old_string is required for 'patch'. Provide the text to find.")?;
            let new_string = new_string.ok_or("new_string is required for 'patch'. Use an empty string to delete matched text.")?;
            patch_skill(root, name, old_string, new_string, file_path, replace_all)
        }
        "delete" => delete_skill(root, name),
        "write_file" => {
            let file_path = file_path.ok_or("file_path is required for 'write_file'. Example: 'references/api-guide.md'")?;
            let file_content = file_content.ok_or("file_content is required for 'write_file'.")?;
            write_file(root, name, file_path, file_content)
        }
        "remove_file" => {
            let file_path = file_path.ok_or("file_path is required for 'remove_file'.")?;
            remove_file(root, name, file_path)
        }
        other => Err(format!("Unknown action '{other}'. Use: create, patch, edit, delete, write_file, remove_file")),
    };

    if result.is_ok() {
        let usage_path = usage::path_under(root);
        match action {
            "create" => usage::mark_created_by_agent(&usage_path, name),
            "patch" | "edit" | "write_file" | "remove_file" => usage::bump_patch(&usage_path, name),
            "delete" => usage::forget(&usage_path, name),
            _ => {}
        }
    }

    result
}

fn skill_not_found(name: &str) -> String {
    format!("Skill '{name}' not found. Use skills_list to see available skills.")
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Skill name is required.".to_string());
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        return Err(format!("Skill name exceeds {MAX_NAME_LENGTH} characters."));
    }
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !first_ok || !rest_ok {
        return Err(format!(
            "Invalid skill name '{name}'. Use lowercase letters, numbers, hyphens, dots, and underscores; it must start with a letter or digit."
        ));
    }
    Ok(())
}

fn validate_category(category: Option<&str>) -> Result<(), String> {
    let Some(category) = category.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(());
    };
    if category.contains('/') || category.contains('\\') {
        return Err(format!("Invalid category '{category}'. Categories must be a single directory name."));
    }
    validate_name(category).map_err(|_| {
        format!("Invalid category '{category}'. Use lowercase letters, numbers, hyphens, dots, and underscores.")
    })
}

/// Enforces exactly what `create`/`edit` require: parseable frontmatter
/// (delegating the actual grammar to [`frontmatter::parse`]) with a
/// non-empty `name` and `description` within [`MAX_DESCRIPTION_LENGTH`],
/// and a non-empty body.
fn validate_frontmatter_for_write(content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("Content cannot be empty.".to_string());
    }
    let (fm, body) = frontmatter::parse(content)?;
    if fm.name().is_none() {
        return Err("Frontmatter must include a non-empty 'name' field.".to_string());
    }
    let Some(description) = fm.description() else {
        return Err("Frontmatter must include a non-empty 'description' field.".to_string());
    };
    if description.chars().count() > MAX_DESCRIPTION_LENGTH {
        return Err(format!("Description exceeds {MAX_DESCRIPTION_LENGTH} characters."));
    }
    if body.trim().is_empty() {
        return Err("SKILL.md must have content after the frontmatter (instructions, procedures, etc.).".to_string());
    }
    Ok(())
}

fn validate_content_size(content: &str, label: &str) -> Result<(), String> {
    let len = content.chars().count();
    if len > MAX_SKILL_CONTENT_CHARS {
        return Err(format!(
            "{label} content is {len} characters (limit: {MAX_SKILL_CONTENT_CHARS}). Consider splitting into a smaller SKILL.md with supporting files in references/ or templates/."
        ));
    }
    Ok(())
}

/// `write_file`/`remove_file` targets must sit under one of
/// [`ALLOWED_SUBDIRS`] — unlike a read ([`super::tiers::view_skill_file`]),
/// which may open anything inside the skill directory, a write is
/// restricted to the progressive-disclosure areas a skill is actually
/// supposed to grow. `SKILL.md` itself is deliberately not reachable
/// through this path — `create`/`edit` own its full-file lifecycle, `patch`
/// can target it explicitly by name (see [`patch_skill`]) — so `write_file`
/// can never bypass [`validate_frontmatter_for_write`] the way a permissive
/// reading of the reference implementation's equivalent check would allow.
fn validate_write_file_path(file_path: &str) -> Result<(), String> {
    if file_path.is_empty() {
        return Err("file_path is required.".to_string());
    }
    let candidate = Path::new(file_path);
    if candidate.is_absolute() {
        return Err("file_path must be relative, not absolute.".to_string());
    }
    if candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("Path traversal ('..') is not allowed.".to_string());
    }
    let parts: Vec<_> = candidate.components().collect();
    let first = parts.first().and_then(|c| c.as_os_str().to_str());
    if !first.is_some_and(|f| ALLOWED_SUBDIRS.contains(&f)) {
        return Err(format!("File must be under one of: {}. Got: '{file_path}'", ALLOWED_SUBDIRS.join(", ")));
    }
    if parts.len() < 2 {
        return Err(format!(
            "Provide a file path, not just a directory. Example: '{}/myfile.md'",
            first.unwrap_or("references")
        ));
    }
    Ok(())
}

/// Join `file_path` onto `skill_dir` and, when the target already exists,
/// verify it did not resolve (through a symlink) outside the skill
/// directory. Traversal via `..` is already rejected by
/// [`validate_write_file_path`] before this ever runs.
fn resolve_write_target(skill_dir: &Path, file_path: &str) -> Result<PathBuf, String> {
    let target = skill_dir.join(file_path);
    if let (Ok(canon_target), Ok(canon_dir)) = (target.canonicalize(), skill_dir.canonicalize()) {
        if !canon_target.starts_with(&canon_dir) {
            return Err("path escapes the skill directory".to_string());
        }
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn create_skill(root: &Path, name: &str, content: &str, category: Option<&str>) -> Result<String, String> {
    validate_name(name)?;
    validate_category(category)?;
    validate_frontmatter_for_write(content)?;
    validate_content_size(content, "SKILL.md")?;

    if discovery::find_skill_dir(root, name).is_some() {
        return Err(format!("A skill named '{name}' already exists."));
    }

    let dir = match category.map(str::trim).filter(|c| !c.is_empty()) {
        Some(category) => root.join(category).join(name),
        None => root.join(name),
    };
    if dir.exists() {
        return Err(format!("{} already exists.", dir.display()));
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    write_atomically(&dir.join(SKILL_MD), content).map_err(|e| format!("could not write SKILL.md: {e}"))?;

    Ok(format!("Skill '{name}' created at {}.", dir.display()))
}

// ---------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------

fn edit_skill(root: &Path, name: &str, content: &str) -> Result<String, String> {
    validate_frontmatter_for_write(content)?;
    validate_content_size(content, "SKILL.md")?;

    let dir = discovery::find_skill_dir(root, name).ok_or_else(|| skill_not_found(name))?;
    write_atomically(&dir.join(SKILL_MD), content).map_err(|e| format!("could not write SKILL.md: {e}"))?;

    Ok(format!("Skill '{name}' updated (full rewrite)."))
}

// ---------------------------------------------------------------------------
// patch
// ---------------------------------------------------------------------------

/// Plain, exact-substring find-and-replace.
///
/// The reference implementation's `patch` runs `old_string` through a
/// fuzzy matcher (`tools/fuzzy_match.py`) that tolerates whitespace and
/// indentation drift. This is deliberately not ported — it is a
/// substantial standalone algorithm, and an exact match is the correct,
/// simple starting point: it is predictable (what you pass is exactly what
/// must appear), and the failure mode (no match) already returns a preview
/// of the file so the caller can adjust `old_string` and retry, the same
/// recovery path a fuzzy match's occasional false-negative would need
/// anyway. Documented as a known gap versus the reference in this task's
/// final report.
fn patch_skill(root: &Path, name: &str, old_string: &str, new_string: &str, file_path: Option<&str>, replace_all: bool) -> Result<String, String> {
    let dir = discovery::find_skill_dir(root, name).ok_or_else(|| skill_not_found(name))?;

    let (target, label) = match file_path {
        None | Some(SKILL_MD) => (dir.join(SKILL_MD), SKILL_MD.to_string()),
        Some(fp) => {
            validate_write_file_path(fp)?;
            (resolve_write_target(&dir, fp)?, fp.to_string())
        }
    };

    if !target.is_file() {
        return Err(format!("File not found: {label}"));
    }
    let content = std::fs::read_to_string(&target).map_err(|e| format!("could not read {label}: {e}"))?;

    let count = content.matches(old_string).count();
    if count == 0 {
        let preview: String = content.chars().take(500).collect();
        let ellipsis = if content.chars().count() > 500 { "..." } else { "" };
        return Err(format!(
            "old_string did not match anything in {label}.\n\nFile preview:\n{preview}{ellipsis}"
        ));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string matches {count} times in {label}; pass replace_all=true, or include more surrounding context to make the match unique."
        ));
    }

    let new_content = if replace_all { content.replace(old_string, new_string) } else { content.replacen(old_string, new_string, 1) };

    validate_content_size(&new_content, &label)?;
    if file_path.is_none() || file_path == Some(SKILL_MD) {
        validate_frontmatter_for_write(&new_content).map_err(|e| format!("Patch would break SKILL.md structure: {e}"))?;
    }

    write_atomically(&target, &new_content).map_err(|e| format!("could not write {label}: {e}"))?;

    let n = if replace_all { count } else { 1 };
    Ok(format!("Patched {label} in skill '{name}' ({n} replacement{}).", if n == 1 { "" } else { "s" }))
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

fn delete_skill(root: &Path, name: &str) -> Result<String, String> {
    let dir = discovery::find_skill_dir(root, name).ok_or_else(|| skill_not_found(name))?;

    let usage_path = usage::path_under(root);
    if usage::get_record(&usage_path, name).pinned {
        return Err(format!("Skill '{name}' is pinned and cannot be deleted. Unpin it first if you really want to remove it."));
    }

    validate_delete_target(root, &dir)?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("could not delete {}: {e}", dir.display()))?;
    remove_if_empty(dir.parent(), root);

    Ok(format!("Skill '{name}' deleted."))
}

/// Defense-in-depth immediately before a recursive delete. `dir` was already
/// located by walking the skills tree from `root` (via [`discovery::scan`]),
/// so this should be redundant in the normal case — it exists as a second,
/// independent check for the same reason
/// `tools/skill_manager_tool.py::_validate_delete_target` does: never trust
/// a single code path to be the only thing standing between a tool call and
/// `remove_dir_all`. Refuses a symlinked skill directory, the skills root
/// itself, and anything that does not resolve strictly inside the root.
fn validate_delete_target(root: &Path, dir: &Path) -> Result<(), String> {
    if dir.is_symlink() {
        return Err(format!("Refusing to delete '{}': it is a symlink, not a real skill directory.", dir.display()));
    }
    let resolved_dir = dir.canonicalize().map_err(|e| format!("could not resolve '{}': {e}", dir.display()))?;
    let resolved_root = root.canonicalize().map_err(|e| format!("could not resolve the skills directory: {e}"))?;
    if resolved_dir == resolved_root {
        return Err("Refusing to delete: that resolves to the skills directory itself.".to_string());
    }
    if !resolved_dir.starts_with(&resolved_root) {
        return Err(format!("Refusing to delete '{}': it is outside the skills directory.", dir.display()));
    }
    Ok(())
}

/// Remove `dir` if it is empty and not `stop_at` — used after deleting a
/// skill (clean up a now-empty category) or a supporting file (clean up a
/// now-empty `references/`, etc.), never removing the skills root or a
/// skill's own top-level directory.
fn remove_if_empty(dir: Option<&Path>, stop_at: &Path) {
    let Some(dir) = dir else { return };
    if dir == stop_at {
        return;
    }
    if let Ok(mut entries) = std::fs::read_dir(dir) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

// ---------------------------------------------------------------------------
// write_file / remove_file
// ---------------------------------------------------------------------------

fn write_file(root: &Path, name: &str, file_path: &str, file_content: &str) -> Result<String, String> {
    validate_write_file_path(file_path)?;
    let byte_len = file_content.len() as u64;
    if byte_len > MAX_SKILL_FILE_BYTES {
        return Err(format!(
            "File content is {byte_len} bytes (limit: {MAX_SKILL_FILE_BYTES} bytes / 1 MiB). Consider splitting into smaller files."
        ));
    }
    validate_content_size(file_content, file_path)?;

    let dir = discovery::find_skill_dir(root, name).ok_or_else(|| format!("{} Create it first with action='create'.", skill_not_found(name)))?;
    let target = resolve_write_target(&dir, file_path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    write_atomically(&target, file_content).map_err(|e| format!("could not write {file_path}: {e}"))?;

    Ok(format!("File '{file_path}' written to skill '{name}'."))
}

fn remove_file(root: &Path, name: &str, file_path: &str) -> Result<String, String> {
    validate_write_file_path(file_path)?;
    let dir = discovery::find_skill_dir(root, name).ok_or_else(|| skill_not_found(name))?;
    let target = resolve_write_target(&dir, file_path)?;
    if !target.is_file() {
        return Err(format!("File '{file_path}' not found in skill '{name}'."));
    }
    std::fs::remove_file(&target).map_err(|e| format!("could not remove {file_path}: {e}"))?;
    remove_if_empty(target.parent(), &dir);

    Ok(format!("File '{file_path}' removed from skill '{name}'."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-manage-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn minimal_skill_md(name: &str) -> String {
        format!("---\nname: {name}\ndescription: a test skill\n---\n# {name}\n\nBody.\n")
    }

    fn create(root: &Path, name: &str, content: &str, category: Option<&str>) -> Result<String, String> {
        skill_manage(root, "create", name, Some(content), category, None, None, None, None, false)
    }

    // -- create --------------------------------------------------------------

    #[test]
    fn create_writes_a_new_skill_directory() {
        let root = scratch_dir("create-basic");
        let result = create(&root, "my-skill", &minimal_skill_md("my-skill"), None);
        assert!(result.is_ok(), "{result:?}");
        assert!(root.join("my-skill/SKILL.md").exists());
    }

    #[test]
    fn create_with_a_category_nests_the_directory() {
        let root = scratch_dir("create-category");
        create(&root, "my-skill", &minimal_skill_md("my-skill"), Some("devops")).unwrap();
        assert!(root.join("devops/my-skill/SKILL.md").exists());
    }

    #[test]
    fn create_rejects_an_invalid_name() {
        let root = scratch_dir("create-bad-name");
        let err = create(&root, "Not Valid!", &minimal_skill_md("x"), None).unwrap_err();
        assert!(err.contains("Invalid skill name"), "{err}");
    }

    #[test]
    fn create_rejects_missing_description() {
        let root = scratch_dir("create-missing-desc");
        let content = "---\nname: x\n---\nBody\n";
        let err = create(&root, "x", content, None).unwrap_err();
        assert!(err.contains("description"), "{err}");
    }

    #[test]
    fn create_rejects_a_duplicate_name() {
        let root = scratch_dir("create-duplicate");
        create(&root, "dup", &minimal_skill_md("dup"), None).unwrap();
        let err = create(&root, "dup", &minimal_skill_md("dup"), None).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn create_rejects_content_over_the_size_limit() {
        let root = scratch_dir("create-too-big");
        let huge_body = "x".repeat(MAX_SKILL_CONTENT_CHARS + 1);
        let content = format!("---\nname: big\ndescription: d\n---\n{huge_body}");
        let err = create(&root, "big", &content, None).unwrap_err();
        assert!(err.contains("limit"), "{err}");
    }

    #[test]
    fn create_marks_the_skill_as_agent_created_in_the_usage_sidecar() {
        let root = scratch_dir("create-provenance");
        create(&root, "my-skill", &minimal_skill_md("my-skill"), None).unwrap();
        let record = usage::get_record(&usage::path_under(&root), "my-skill");
        assert_eq!(record.created_by, Some("agent".to_string()));
    }

    // -- edit ------------------------------------------------------------------

    #[test]
    fn edit_replaces_the_full_content() {
        let root = scratch_dir("edit-basic");
        create(&root, "my-skill", &minimal_skill_md("my-skill"), None).unwrap();

        let new_content = "---\nname: my-skill\ndescription: updated\n---\nNew body.\n";
        let result = skill_manage(&root, "edit", "my-skill", Some(new_content), None, None, None, None, None, false);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read_to_string(root.join("my-skill/SKILL.md")).unwrap(), new_content);
    }

    #[test]
    fn edit_on_a_missing_skill_is_an_error() {
        let root = scratch_dir("edit-missing");
        let err = skill_manage(&root, "edit", "nope", Some(&minimal_skill_md("nope")), None, None, None, None, None, false).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    // -- patch -----------------------------------------------------------------

    #[test]
    fn patch_replaces_a_unique_match() {
        let root = scratch_dir("patch-unique");
        create(&root, "my-skill", &minimal_skill_md("my-skill"), None).unwrap();

        let result = skill_manage(&root, "patch", "my-skill", None, None, None, None, Some("Body."), Some("Updated body."), false);
        assert!(result.is_ok(), "{result:?}");
        assert!(std::fs::read_to_string(root.join("my-skill/SKILL.md")).unwrap().contains("Updated body."));
    }

    #[test]
    fn patch_with_no_match_reports_a_file_preview() {
        let root = scratch_dir("patch-no-match");
        create(&root, "my-skill", &minimal_skill_md("my-skill"), None).unwrap();

        let err = skill_manage(&root, "patch", "my-skill", None, None, None, None, Some("does not appear anywhere"), Some("x"), false).unwrap_err();
        assert!(err.contains("did not match"), "{err}");
        assert!(err.contains("File preview"), "{err}");
    }

    #[test]
    fn patch_refuses_an_ambiguous_match_without_replace_all() {
        let root = scratch_dir("patch-ambiguous");
        let content = "---\nname: x\ndescription: d\n---\nrepeat repeat repeat\n";
        create(&root, "x", content, None).unwrap();

        let err = skill_manage(&root, "patch", "x", None, None, None, None, Some("repeat"), Some("once"), false).unwrap_err();
        assert!(err.contains("matches 3 times"), "{err}");
    }

    #[test]
    fn patch_with_replace_all_replaces_every_occurrence() {
        let root = scratch_dir("patch-replace-all");
        let content = "---\nname: x\ndescription: d\n---\nrepeat repeat repeat\n";
        create(&root, "x", content, None).unwrap();

        skill_manage(&root, "patch", "x", None, None, None, None, Some("repeat"), Some("once"), true).unwrap();
        let updated = std::fs::read_to_string(root.join("x/SKILL.md")).unwrap();
        assert_eq!(updated, "---\nname: x\ndescription: d\n---\nonce once once\n");
    }

    #[test]
    fn patch_can_target_a_supporting_file() {
        let root = scratch_dir("patch-file-path");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();
        skill_manage(&root, "write_file", "x", None, None, Some("references/api.md"), Some("old content"), None, None, false).unwrap();

        skill_manage(&root, "patch", "x", None, None, Some("references/api.md"), None, Some("old"), Some("new"), false).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("x/references/api.md")).unwrap(), "new content");
    }

    #[test]
    fn patch_refuses_to_break_skill_md_frontmatter_structure() {
        let root = scratch_dir("patch-breaks-frontmatter");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        // Deleting the "description:" line would leave the frontmatter
        // without a required field.
        let err = skill_manage(&root, "patch", "x", None, None, None, None, Some("description: a test skill\n"), Some(""), false).unwrap_err();
        assert!(err.contains("Patch would break"), "{err}");
        // And the file on disk must be untouched.
        assert!(std::fs::read_to_string(root.join("x/SKILL.md")).unwrap().contains("description:"));
    }

    #[test]
    fn patch_can_delete_text_with_an_empty_new_string() {
        let root = scratch_dir("patch-delete");
        let content = "---\nname: x\ndescription: d\n---\nKeep this. Remove this part. Keep this too.\n";
        create(&root, "x", content, None).unwrap();

        skill_manage(&root, "patch", "x", None, None, None, None, Some("Remove this part. "), Some(""), false).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("x/SKILL.md")).unwrap(),
            "---\nname: x\ndescription: d\n---\nKeep this. Keep this too.\n"
        );
    }

    // -- delete ------------------------------------------------------------------

    #[test]
    fn delete_removes_the_skill_directory() {
        let root = scratch_dir("delete-basic");
        create(&root, "gone", &minimal_skill_md("gone"), None).unwrap();
        let result = skill_manage(&root, "delete", "gone", None, None, None, None, None, None, false);
        assert!(result.is_ok(), "{result:?}");
        assert!(!root.join("gone").exists());
    }

    #[test]
    fn delete_cleans_up_an_emptied_category_directory() {
        let root = scratch_dir("delete-category-cleanup");
        create(&root, "only-one", &minimal_skill_md("only-one"), Some("solo-cat")).unwrap();
        skill_manage(&root, "delete", "only-one", None, None, None, None, None, None, false).unwrap();
        assert!(!root.join("solo-cat").exists(), "an emptied category directory should be cleaned up");
    }

    #[test]
    fn delete_keeps_a_category_directory_that_still_has_siblings() {
        let root = scratch_dir("delete-category-kept");
        create(&root, "a", &minimal_skill_md("a"), Some("shared-cat")).unwrap();
        create(&root, "b", &minimal_skill_md("b"), Some("shared-cat")).unwrap();
        skill_manage(&root, "delete", "a", None, None, None, None, None, None, false).unwrap();
        assert!(root.join("shared-cat/b").exists());
    }

    #[test]
    fn delete_on_a_missing_skill_is_an_error() {
        let root = scratch_dir("delete-missing");
        let err = skill_manage(&root, "delete", "nope", None, None, None, None, None, None, false).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn delete_refuses_a_pinned_skill() {
        let root = scratch_dir("delete-pinned");
        create(&root, "protected", &minimal_skill_md("protected"), None).unwrap();
        usage::set_pinned(&usage::path_under(&root), "protected", true);

        let err = skill_manage(&root, "delete", "protected", None, None, None, None, None, None, false).unwrap_err();
        assert!(err.contains("pinned"), "{err}");
        assert!(root.join("protected").exists());
    }

    #[test]
    fn delete_forgets_the_usage_record() {
        let root = scratch_dir("delete-forgets-usage");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();
        assert!(usage::load(&usage::path_under(&root)).contains_key("x"));

        skill_manage(&root, "delete", "x", None, None, None, None, None, None, false).unwrap();
        assert!(!usage::load(&usage::path_under(&root)).contains_key("x"));
    }

    #[cfg(unix)]
    #[test]
    fn delete_refuses_a_symlinked_skill_directory() {
        let root = scratch_dir("delete-symlink");
        let outside = scratch_dir("delete-symlink-outside-target");
        std::fs::write(outside.join("innocent.txt"), "do not delete me").unwrap();

        std::os::unix::fs::symlink(&outside, root.join("evil-link")).unwrap();
        // `find_skill_dir` requires a real SKILL.md to resolve the name at
        // all, so point one at the symlinked-in directory to make "evil-link"
        // resolve as a skill named "evil-link" in the first place.
        std::fs::write(outside.join(SKILL_MD), minimal_skill_md("evil-link")).unwrap();

        let err = skill_manage(&root, "delete", "evil-link", None, None, None, None, None, None, false).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(outside.join("innocent.txt").exists(), "the symlink target must survive untouched");
    }

    // -- write_file / remove_file --------------------------------------------------

    #[test]
    fn write_file_adds_a_supporting_file() {
        let root = scratch_dir("write-file-basic");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let result = skill_manage(&root, "write_file", "x", None, None, Some("references/guide.md"), Some("guide content"), None, None, false);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read_to_string(root.join("x/references/guide.md")).unwrap(), "guide content");
    }

    #[test]
    fn write_file_rejects_a_path_outside_the_allowed_subdirs() {
        let root = scratch_dir("write-file-bad-subdir");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let err = skill_manage(&root, "write_file", "x", None, None, Some("not-allowed/file.md"), Some("content"), None, None, false).unwrap_err();
        assert!(err.contains("must be under one of"), "{err}");
    }

    #[test]
    fn write_file_rejects_path_traversal() {
        let root = scratch_dir("write-file-traversal");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let err = skill_manage(&root, "write_file", "x", None, None, Some("references/../../escape.md"), Some("content"), None, None, false).unwrap_err();
        assert!(err.contains("traversal"), "{err}");
    }

    #[test]
    fn write_file_rejects_content_over_one_mebibyte() {
        let root = scratch_dir("write-file-too-big");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let huge = "a".repeat(MAX_SKILL_FILE_BYTES as usize + 1);
        let err = skill_manage(&root, "write_file", "x", None, None, Some("assets/big.bin"), Some(&huge), None, None, false).unwrap_err();
        assert!(err.contains("1 MiB") || err.contains("limit"), "{err}");
    }

    #[test]
    fn write_file_cannot_target_skill_md_directly() {
        let root = scratch_dir("write-file-skill-md");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let err = skill_manage(&root, "write_file", "x", None, None, Some("SKILL.md"), Some("anything"), None, None, false).unwrap_err();
        assert!(err.contains("must be under one of"), "{err}");
    }

    #[test]
    fn remove_file_deletes_a_supporting_file_and_its_now_empty_directory() {
        let root = scratch_dir("remove-file-basic");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();
        skill_manage(&root, "write_file", "x", None, None, Some("references/only.md"), Some("content"), None, None, false).unwrap();

        let result = skill_manage(&root, "remove_file", "x", None, None, Some("references/only.md"), None, None, None, false);
        assert!(result.is_ok(), "{result:?}");
        assert!(!root.join("x/references/only.md").exists());
        assert!(!root.join("x/references").exists(), "an emptied references/ directory should be cleaned up");
    }

    #[test]
    fn remove_file_on_a_missing_file_is_an_error() {
        let root = scratch_dir("remove-file-missing");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();

        let err = skill_manage(&root, "remove_file", "x", None, None, Some("references/nope.md"), None, None, None, false).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    // -- dispatcher ------------------------------------------------------------------

    #[test]
    fn an_unknown_action_is_a_clear_error() {
        let root = scratch_dir("unknown-action");
        let err = skill_manage(&root, "explode", "x", None, None, None, None, None, None, false).unwrap_err();
        assert!(err.contains("Unknown action"), "{err}");
    }

    #[test]
    fn successful_patch_bumps_the_usage_patch_counter() {
        let root = scratch_dir("patch-bumps-usage");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();
        skill_manage(&root, "patch", "x", None, None, None, None, Some("Body."), Some("New body."), false).unwrap();

        assert_eq!(usage::get_record(&usage::path_under(&root), "x").patch_count, 1);
    }

    #[test]
    fn a_failed_action_does_not_bump_usage() {
        let root = scratch_dir("failed-action-no-bump");
        create(&root, "x", &minimal_skill_md("x"), None).unwrap();
        let _ = skill_manage(&root, "patch", "x", None, None, None, None, Some("no such text"), Some("y"), false);

        assert_eq!(usage::get_record(&usage::path_under(&root), "x").patch_count, 0);
    }
}
