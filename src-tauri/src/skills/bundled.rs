//! Content-hash tracking for skills Caduceus ships out of the box, so a
//! later release can fix or extend a bundled skill without ever clobbering
//! a user's edits to their own copy of it.
//!
//! The mechanism is the one line in the task brief: "Bundled skills must be
//! tracked by content hash so user edits are never clobbered by an update"
//! — a `.bundled_manifest` of `name:hash` lines (same format Hermes uses at
//! `~/.hermes/skills/.bundled_manifest`, though the two files are unrelated
//! — this one lives under Caduceus's own skills root). On every sync:
//!
//! * **not on disk yet** → installed fresh, hash recorded.
//! * **on disk, hash matches the manifest** (the user has not touched it
//!   since the last sync) → safe to overwrite with the newer bundled
//!   content; the manifest hash is updated to match.
//! * **on disk, hash has drifted from the manifest** (the user edited it)
//!   → left alone, unconditionally. An update that would improve the
//!   bundled default is not worth destroying someone's customization for.
//! * **on disk, no manifest entry at all** (pre-dates this tracking, or a
//!   same-named skill the user created by hand) → also left alone: with no
//!   recorded "last known bundled hash," there is no way to tell an edit
//!   from an untouched copy, so the safe default is to never overwrite.
//!
//! The hash itself ([`hash_skill_dir`]) is [`DefaultHasher`] (SipHash) over
//! every file's relative path and bytes, the same non-cryptographic,
//! collision-tolerant choice `clipboard::watcher::hash_bytes` already makes
//! for content de-duplication — nothing here is a security boundary, it
//! only needs to change when the content does.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::{write_atomically, SKILL_MD};

pub const MANIFEST_FILE: &str = ".bundled_manifest";

/// A stable digest of every file's relative path and contents under `dir`,
/// order-independent (the file list is sorted before hashing) so the same
/// directory contents always hash the same regardless of how the OS
/// happened to return directory entries.
pub fn hash_skill_dir(dir: &Path) -> std::io::Result<String> {
    let mut files = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();

    let mut hasher = DefaultHasher::new();
    for file in &files {
        let rel = file.strip_prefix(dir).unwrap_or(file);
        rel.to_string_lossy().hash(&mut hasher);
        std::fs::read(file)?.hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Read `.bundled_manifest`. Missing or malformed lines are skipped rather
/// than failing the whole read — a hand-edited or partially-written
/// manifest should degrade to "fewer known hashes," never a crash.
pub fn read_manifest(skills_root: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(skills_root.join(MANIFEST_FILE)) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, hash)) = line.split_once(':') {
            map.insert(name.trim().to_string(), hash.trim().to_string());
        }
    }
    map
}

pub fn write_manifest(skills_root: &Path, manifest: &HashMap<String, String>) {
    let mut names: Vec<&String> = manifest.keys().collect();
    names.sort();
    let mut text = String::new();
    for name in names {
        text.push_str(name);
        text.push(':');
        text.push_str(&manifest[name]);
        text.push('\n');
    }
    let path = skills_root.join(MANIFEST_FILE);
    if let Err(e) = write_atomically(&path, &text) {
        log::warn!("skills: could not write {}: {e}", path.display());
    }
}

/// What happened when syncing one bundled skill — see the module doc for
/// what each case means and why.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncOutcome {
    Installed,
    UpdatedUnedited,
    SkippedUserEdited,
    /// The skill was eligible to update (hash matched the manifest) but the
    /// new content is byte-for-byte identical to what shipped last time —
    /// distinguished from `UpdatedUnedited` purely so a caller logging sync
    /// results is not told something changed when nothing did.
    Unchanged,
    Failed(String),
}

/// Sync one bundled skill's canonical `content` (a full `SKILL.md`, already
/// including its frontmatter) into `<skills_root>/<name>/SKILL.md`, honoring
/// the clobber protection described in the module doc.
pub fn sync_bundled_skill(skills_root: &Path, name: &str, content: &str) -> SyncOutcome {
    let dir = skills_root.join(name);
    let skill_md = dir.join(SKILL_MD);
    let mut manifest = read_manifest(skills_root);

    if !skill_md.is_file() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return SyncOutcome::Failed(e.to_string());
        }
        if let Err(e) = write_atomically(&skill_md, content) {
            return SyncOutcome::Failed(e.to_string());
        }
        return match hash_skill_dir(&dir) {
            Ok(hash) => {
                manifest.insert(name.to_string(), hash);
                write_manifest(skills_root, &manifest);
                SyncOutcome::Installed
            }
            Err(e) => SyncOutcome::Failed(e.to_string()),
        };
    }

    let current_hash = match hash_skill_dir(&dir) {
        Ok(h) => h,
        Err(e) => return SyncOutcome::Failed(e.to_string()),
    };

    let matches_manifest = manifest.get(name).is_some_and(|recorded| *recorded == current_hash);
    if !matches_manifest {
        // Either never tracked, or tracked but edited since — both mean
        // "do not touch it" per the module doc.
        return SyncOutcome::SkippedUserEdited;
    }

    if let Err(e) = write_atomically(&skill_md, content) {
        return SyncOutcome::Failed(e.to_string());
    }
    let new_hash = match hash_skill_dir(&dir) {
        Ok(h) => h,
        Err(e) => return SyncOutcome::Failed(e.to_string()),
    };
    if new_hash == current_hash {
        return SyncOutcome::Unchanged;
    }
    manifest.insert(name.to_string(), new_hash);
    write_manifest(skills_root, &manifest);
    SyncOutcome::UpdatedUnedited
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-bundled-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- hash_skill_dir -------------------------------------------------------

    #[test]
    fn identical_content_hashes_the_same() {
        let a = scratch_dir("hash-a");
        let b = scratch_dir("hash-b");
        std::fs::write(a.join(SKILL_MD), "---\nname: x\ndescription: y\n---\nBody\n").unwrap();
        std::fs::write(b.join(SKILL_MD), "---\nname: x\ndescription: y\n---\nBody\n").unwrap();
        assert_eq!(hash_skill_dir(&a).unwrap(), hash_skill_dir(&b).unwrap());
    }

    #[test]
    fn different_content_hashes_differently() {
        let a = scratch_dir("hash-diff-a");
        std::fs::write(a.join(SKILL_MD), "---\nname: x\ndescription: y\n---\nBody one\n").unwrap();
        let hash1 = hash_skill_dir(&a).unwrap();
        std::fs::write(a.join(SKILL_MD), "---\nname: x\ndescription: y\n---\nBody two\n").unwrap();
        let hash2 = hash_skill_dir(&a).unwrap();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn adding_a_supporting_file_changes_the_hash() {
        let a = scratch_dir("hash-plus-file");
        std::fs::write(a.join(SKILL_MD), "---\nname: x\ndescription: y\n---\nBody\n").unwrap();
        let before = hash_skill_dir(&a).unwrap();
        std::fs::create_dir_all(a.join("references")).unwrap();
        std::fs::write(a.join("references/extra.md"), "extra").unwrap();
        let after = hash_skill_dir(&a).unwrap();
        assert_ne!(before, after);
    }

    // -- manifest read/write ---------------------------------------------------

    #[test]
    fn manifest_round_trips() {
        let root = scratch_dir("manifest-round-trip");
        let manifest = HashMap::from([("a".to_string(), "111".to_string()), ("b".to_string(), "222".to_string())]);
        write_manifest(&root, &manifest);
        assert_eq!(read_manifest(&root), manifest);
    }

    #[test]
    fn a_missing_manifest_reads_as_empty() {
        let root = scratch_dir("manifest-missing");
        assert_eq!(read_manifest(&root), HashMap::new());
    }

    // -- sync_bundled_skill: the clobber-protection contract -------------------

    #[test]
    fn a_never_installed_skill_is_installed_fresh() {
        let root = scratch_dir("sync-fresh");
        let outcome = sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nv1\n");
        assert_eq!(outcome, SyncOutcome::Installed);
        assert_eq!(std::fs::read_to_string(root.join("seed-skill/SKILL.md")).unwrap(), "---\nname: seed-skill\ndescription: d\n---\nv1\n");
        assert!(read_manifest(&root).contains_key("seed-skill"));
    }

    #[test]
    fn re_syncing_identical_content_reports_unchanged() {
        let root = scratch_dir("sync-unchanged");
        let content = "---\nname: seed-skill\ndescription: d\n---\nv1\n";
        sync_bundled_skill(&root, "seed-skill", content);
        let outcome = sync_bundled_skill(&root, "seed-skill", content);
        assert_eq!(outcome, SyncOutcome::Unchanged);
    }

    #[test]
    fn an_untouched_skill_is_updated_when_the_bundled_content_changes() {
        let root = scratch_dir("sync-update");
        sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nv1\n");

        let outcome = sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nv2\n");
        assert_eq!(outcome, SyncOutcome::UpdatedUnedited);
        assert_eq!(std::fs::read_to_string(root.join("seed-skill/SKILL.md")).unwrap(), "---\nname: seed-skill\ndescription: d\n---\nv2\n");
    }

    #[test]
    fn a_user_edited_skill_is_never_clobbered_by_a_newer_bundled_version() {
        let root = scratch_dir("sync-clobber-protection");
        sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nv1\n");

        // The user edits their local copy directly, without going through
        // this sync mechanism — its hash now no longer matches the manifest.
        std::fs::write(root.join("seed-skill/SKILL.md"), "---\nname: seed-skill\ndescription: d\n---\nMY CUSTOM EDIT\n").unwrap();

        let outcome = sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nv2 from a newer Caduceus release\n");
        assert_eq!(outcome, SyncOutcome::SkippedUserEdited);
        assert_eq!(
            std::fs::read_to_string(root.join("seed-skill/SKILL.md")).unwrap(),
            "---\nname: seed-skill\ndescription: d\n---\nMY CUSTOM EDIT\n",
            "the user's edit must survive a sync untouched"
        );
    }

    #[test]
    fn a_directory_that_exists_with_no_manifest_entry_is_left_alone() {
        let root = scratch_dir("sync-untracked");
        // A same-named skill the user (or a different mechanism) created by
        // hand, never recorded in the manifest.
        std::fs::create_dir_all(root.join("seed-skill")).unwrap();
        std::fs::write(root.join("seed-skill/SKILL.md"), "---\nname: seed-skill\ndescription: hand-authored\n---\nnot bundled content\n").unwrap();

        let outcome = sync_bundled_skill(&root, "seed-skill", "---\nname: seed-skill\ndescription: d\n---\nbundled v1\n");
        assert_eq!(outcome, SyncOutcome::SkippedUserEdited);
        assert!(std::fs::read_to_string(root.join("seed-skill/SKILL.md")).unwrap().contains("hand-authored"));
    }
}
