//! Deterministic skill lifecycle transitions — pure code, no model call.
//!
//! A skill that nobody has viewed, used, or patched in a while becomes
//! `stale`, then `archived` (moved to `.archive/`, never deleted — see
//! [`archive_skill`]). The decision itself ([`decide`]) is a pure function
//! of a usage record and the current time; the only thing that isn't pure
//! is [`apply_transitions`] actually moving a directory and updating the
//! sidecar to match. Nothing here ever asks a model whether a skill still
//! matters — the whole point of a sidecar-driven lifecycle is that it does
//! not need to.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::discovery;
use super::usage::{self, SkillState, UsageRecord};

/// Unused longer than this becomes `stale` — still fully available, just
/// flagged as a candidate for review.
pub const STALE_AFTER_DAYS: i64 = 30;

/// Unused longer than this is archived: moved out of the active skills tree
/// into `.archive/`, recoverable via [`restore_skill`], never hard-deleted.
pub const ARCHIVE_AFTER_DAYS: i64 = 90;

const ARCHIVE_DIR_NAME: &str = ".archive";

/// What [`apply_transitions`] actually did to one skill, for a caller that
/// wants a report (a debug log today; a settings-panel list plausibly
/// later) rather than just the side effects.
#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    /// State did not change (including "stayed archived" and "pinned, so
    /// never considered").
    None,
    MarkedStale,
    Archived { destination: PathBuf },
}

/// The most recent of `last_used_at` / `last_viewed_at` / `last_patched_at`,
/// whichever is newest and parses. `None` when the record has never
/// recorded any activity at all (a freshly created skill nobody has opened
/// yet), in which case [`decide`] falls back to `created_at`.
pub fn latest_activity_at(record: &UsageRecord) -> Option<DateTime<Utc>> {
    [record.last_used_at.as_deref(), record.last_viewed_at.as_deref(), record.last_patched_at.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .max()
}

/// What `record`'s state *should* be at `now`, judged purely from its own
/// fields — no filesystem access, no model call. `now` is a parameter
/// rather than reading the clock internally so this is testable without
/// waiting 90 days or faking global time.
pub fn decide(record: &UsageRecord, now: DateTime<Utc>) -> SkillState {
    if record.pinned {
        // Pin is a hard opt-out from every automatic transition, per the
        // module doc — even a pinned skill idle for a year stays exactly
        // where its owner left it.
        return record.state;
    }
    if record.state == SkillState::Archived {
        // Archival is a one-way filesystem move performed by
        // `apply_transitions`, not something re-derived from a timestamp on
        // every sweep — a record that is already archived stays archived
        // until something explicit (`restore_skill`) says otherwise.
        return SkillState::Archived;
    }

    let anchor = latest_activity_at(record)
        .or_else(|| DateTime::parse_from_rfc3339(&record.created_at).ok().map(|dt| dt.with_timezone(&Utc)));
    let Some(anchor) = anchor else {
        // No usable timestamp anywhere on the record (a hand-edited or
        // corrupt sidecar entry) — leave it as-is rather than guess an age.
        return record.state;
    };

    let idle_days = (now - anchor).num_days();
    if idle_days > ARCHIVE_AFTER_DAYS {
        SkillState::Archived
    } else if idle_days > STALE_AFTER_DAYS {
        SkillState::Stale
    } else {
        SkillState::Active
    }
}

/// Run one lifecycle sweep: for every skill with a usage record, decide its
/// new state and act on it — flip a flag for `stale`, actually move the
/// directory for `archived`. Returns what happened to each considered skill.
///
/// `protected` names are skipped entirely, alongside pinned skills and
/// skills whose usage record has no matching directory on disk (nothing to
/// act on). Caduceus has no scheduled-job subsystem today to source
/// "referenced by a scheduled job" protection from automatically — see this
/// module's crate-level task notes — so callers that have no such source
/// should simply pass an empty set; the parameter exists so a future
/// scheduler integration has somewhere to plug in without changing this
/// function's shape.
pub fn apply_transitions(skills_root: &Path, now: DateTime<Utc>, protected: &HashSet<String>) -> Vec<(String, Transition)> {
    let usage_path = usage::path_under(skills_root);
    let data = usage::load(&usage_path);

    let mut results = Vec::with_capacity(data.len());
    for (name, record) in &data {
        if protected.contains(name) {
            results.push((name.clone(), Transition::None));
            continue;
        }
        if record.pinned {
            results.push((name.clone(), Transition::None));
            continue;
        }
        if discovery::find_skill_dir(skills_root, name).is_none() && record.state != SkillState::Archived {
            // Orphaned record: something removed the directory outside of
            // `skill_manage` (or it is already sitting in `.archive/`, which
            // `find_skill_dir` correctly does not see since `.archive` is an
            // excluded directory). Nothing to move for the former case;
            // the latter is handled by the `Archived` short-circuit in
            // `decide` and needs no directory lookup at all.
            results.push((name.clone(), Transition::None));
            continue;
        }

        let new_state = decide(record, now);
        if new_state == record.state {
            results.push((name.clone(), Transition::None));
            continue;
        }

        match new_state {
            SkillState::Stale => {
                usage::set_state(&usage_path, name, SkillState::Stale);
                results.push((name.clone(), Transition::MarkedStale));
            }
            SkillState::Archived => match archive_skill(skills_root, name) {
                Ok(destination) => {
                    usage::set_state(&usage_path, name, SkillState::Archived);
                    results.push((name.clone(), Transition::Archived { destination }));
                }
                Err(e) => {
                    log::warn!("skills: lifecycle sweep could not archive '{name}': {e}");
                    results.push((name.clone(), Transition::None));
                }
            },
            SkillState::Active => {
                // The only way to reach here is a record moving from Stale
                // back to Active — activity resumed since the last sweep.
                // (An already-Active record took the `new_state ==
                // record.state` branch above and never reaches this match.)
                usage::set_state(&usage_path, name, SkillState::Active);
                results.push((name.clone(), Transition::None));
            }
        }
    }
    results
}

fn archive_root(skills_root: &Path) -> PathBuf {
    skills_root.join(ARCHIVE_DIR_NAME)
}

/// Move `name`'s directory into `.archive/`, flattening any category
/// nesting (an archived skill's original category is not preserved — this
/// mirrors `tools/skill_usage.py::archive_skill`, which does the same, on
/// the theory that a restore does not need to reconstruct exactly where a
/// skill used to live, only that it comes back somewhere valid). A name
/// collision in the archive (the same skill archived twice) is
/// disambiguated with a UTC timestamp suffix rather than overwriting the
/// earlier copy.
pub fn archive_skill(skills_root: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = discovery::find_skill_dir(skills_root, name).ok_or_else(|| format!("skill '{name}' not found"))?;

    let archive_root = archive_root(skills_root);
    std::fs::create_dir_all(&archive_root).map_err(|e| format!("could not create archive directory: {e}"))?;

    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or(name).to_string();
    let mut dest = archive_root.join(&dir_name);
    if dest.exists() {
        let stamp = Utc::now().format("%Y%m%d%H%M%S");
        dest = archive_root.join(format!("{dir_name}-{stamp}"));
    }

    move_dir(&dir, &dest)?;
    Ok(dest)
}

/// Move an archived skill back under `skills_root`, at its top level
/// (category nesting is not reconstructed, matching [`archive_skill`]
/// discarding it on the way in). Refuses if a skill by that name already
/// exists at the destination, or if nothing by that name is in the archive.
pub fn restore_skill(skills_root: &Path, name: &str) -> Result<PathBuf, String> {
    let archive_root = archive_root(skills_root);
    if !archive_root.is_dir() {
        return Err("no archive directory".to_string());
    }

    let exact = archive_root.join(name);
    let src = if exact.is_dir() {
        exact
    } else {
        // The disambiguated form `archive_skill` writes on a name
        // collision: "<name>-" followed by exactly the 14-digit
        // `%Y%m%d%H%M%S` stamp. Only that exact shape is another copy of
        // *this* skill — a bare `starts_with` would also match an unrelated
        // sibling like an archived "git-helpers" when restoring "git".
        let prefix = format!("{name}-");
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(&archive_root)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix(&prefix))
                    .is_some_and(|suffix| suffix.len() == 14 && suffix.chars().all(|c| c.is_ascii_digit()))
            })
            .collect();
        candidates.sort(); // the timestamp suffix sorts lexicographically == chronologically
        candidates.pop().ok_or_else(|| format!("skill '{name}' not found in archive"))?
    };

    let dest = skills_root.join(name);
    if dest.exists() {
        return Err(format!("destination already exists: {}", dest.display()));
    }
    move_dir(&src, &dest)?;
    Ok(dest)
}

/// `fs::rename`, falling back to a recursive copy + remove when the rename
/// fails (typically a cross-device move — `.archive/` lives under the same
/// skills root today, so this should not trigger in practice, but costs
/// little to handle correctly).
fn move_dir(src: &Path, dest: &Path) -> Result<(), String> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    copy_dir_recursive(src, dest).map_err(|e| format!("could not copy {}: {e}", src.display()))?;
    std::fs::remove_dir_all(src).map_err(|e| format!("copied to {} but could not remove the original {}: {e}", dest.display(), src.display()))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
        // Symlinks are skipped rather than followed — a skill directory
        // should not contain one, and blindly following one during a copy
        // could walk outside the skill directory entirely.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-lifecycle-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nBody\n")).unwrap();
    }

    fn days_ago(days: i64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::days(days)
    }

    fn record_with_last_used(days_ago_count: i64) -> UsageRecord {
        let mut r = UsageRecord::default();
        r.last_used_at = Some(days_ago(days_ago_count).to_rfc3339());
        r
    }

    // -- decide() ----------------------------------------------------------

    #[test]
    fn a_freshly_created_never_used_record_stays_active() {
        let record = UsageRecord::default(); // created_at = now
        assert_eq!(decide(&record, Utc::now()), SkillState::Active);
    }

    #[test]
    fn unused_for_31_days_becomes_stale() {
        let record = record_with_last_used(31);
        assert_eq!(decide(&record, Utc::now()), SkillState::Stale);
    }

    #[test]
    fn unused_for_exactly_30_days_is_still_active() {
        let record = record_with_last_used(30);
        assert_eq!(decide(&record, Utc::now()), SkillState::Active);
    }

    #[test]
    fn unused_for_91_days_becomes_archived() {
        let record = record_with_last_used(91);
        assert_eq!(decide(&record, Utc::now()), SkillState::Archived);
    }

    #[test]
    fn recent_activity_overrides_an_old_created_at() {
        let mut record = record_with_last_used(1);
        record.created_at = days_ago(500).to_rfc3339();
        assert_eq!(decide(&record, Utc::now()), SkillState::Active);
    }

    #[test]
    fn a_pinned_skill_never_transitions_no_matter_how_idle() {
        let mut record = record_with_last_used(500);
        record.pinned = true;
        assert_eq!(decide(&record, Utc::now()), SkillState::Active);
    }

    #[test]
    fn an_already_archived_record_stays_archived_even_with_fresh_activity() {
        // Defensive case: should not occur via normal flow (a directory
        // that still exists would not normally carry an Archived record),
        // but `decide` must not "un-archive" on its own regardless.
        let mut record = record_with_last_used(0);
        record.state = SkillState::Archived;
        assert_eq!(decide(&record, Utc::now()), SkillState::Archived);
    }

    // -- apply_transitions() -------------------------------------------------

    #[test]
    fn a_stale_worthy_skill_gets_flagged_without_moving_its_directory() {
        let root = scratch_dir("stale-sweep");
        write_skill(&root, "idle-skill");
        usage::save(&usage::path_under(&root), &HashMap::from([("idle-skill".to_string(), record_with_last_used(31))]));

        let results = apply_transitions(&root, Utc::now(), &HashSet::new());
        assert_eq!(results, vec![("idle-skill".to_string(), Transition::MarkedStale)]);
        assert!(root.join("idle-skill/SKILL.md").exists(), "a stale skill's directory must not move");
        assert_eq!(usage::get_record(&usage::path_under(&root), "idle-skill").state, SkillState::Stale);
    }

    #[test]
    fn an_archive_worthy_skill_is_moved_and_recoverable() {
        let root = scratch_dir("archive-sweep");
        write_skill(&root, "ancient-skill");
        usage::save(&usage::path_under(&root), &HashMap::from([("ancient-skill".to_string(), record_with_last_used(91))]));

        let results = apply_transitions(&root, Utc::now(), &HashSet::new());
        match &results[..] {
            [(name, Transition::Archived { destination })] => {
                assert_eq!(name, "ancient-skill");
                assert!(destination.join("SKILL.md").exists());
            }
            other => panic!("expected exactly one Archived transition, got {other:?}"),
        }
        assert!(!root.join("ancient-skill").exists(), "the skill must be gone from the active tree");
        assert!(root.join(".archive/ancient-skill/SKILL.md").exists(), "and recoverable from .archive/");
        assert_eq!(usage::get_record(&usage::path_under(&root), "ancient-skill").state, SkillState::Archived);
    }

    #[test]
    fn pinned_skills_are_never_archived_by_a_sweep() {
        let root = scratch_dir("pinned-sweep");
        write_skill(&root, "pinned-skill");
        let mut record = record_with_last_used(999);
        record.pinned = true;
        usage::save(&usage::path_under(&root), &HashMap::from([("pinned-skill".to_string(), record)]));

        apply_transitions(&root, Utc::now(), &HashSet::new());
        assert!(root.join("pinned-skill/SKILL.md").exists());
        assert_eq!(usage::get_record(&usage::path_under(&root), "pinned-skill").state, SkillState::Active);
    }

    #[test]
    fn protected_names_are_skipped_by_a_sweep() {
        let root = scratch_dir("protected-sweep");
        write_skill(&root, "scheduled-skill");
        usage::save(&usage::path_under(&root), &HashMap::from([("scheduled-skill".to_string(), record_with_last_used(999))]));

        let protected = HashSet::from(["scheduled-skill".to_string()]);
        apply_transitions(&root, Utc::now(), &protected);
        assert!(root.join("scheduled-skill/SKILL.md").exists());
    }

    #[test]
    fn a_record_moving_back_to_active_is_un_flagged() {
        let root = scratch_dir("un-stale");
        write_skill(&root, "revived-skill");
        let mut record = record_with_last_used(1); // recent activity
        record.state = SkillState::Stale; // but still flagged stale from a previous sweep
        usage::save(&usage::path_under(&root), &HashMap::from([("revived-skill".to_string(), record)]));

        apply_transitions(&root, Utc::now(), &HashSet::new());
        assert_eq!(usage::get_record(&usage::path_under(&root), "revived-skill").state, SkillState::Active);
    }

    // -- archive_skill / restore_skill ---------------------------------------

    #[test]
    fn archive_then_restore_round_trips_the_directory_and_content() {
        let root = scratch_dir("round-trip");
        write_skill(&root, "roundtrip-skill");

        let archived_at = archive_skill(&root, "roundtrip-skill").unwrap();
        assert!(archived_at.join("SKILL.md").exists());
        assert!(!root.join("roundtrip-skill").exists());

        let restored_at = restore_skill(&root, "roundtrip-skill").unwrap();
        assert_eq!(restored_at, root.join("roundtrip-skill"));
        let content = std::fs::read_to_string(restored_at.join("SKILL.md")).unwrap();
        assert!(content.contains("roundtrip-skill"));
    }

    #[test]
    fn archiving_flattens_category_nesting() {
        let root = scratch_dir("flatten");
        let dir = root.join("some-category/nested-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: nested-skill\ndescription: d\n---\nBody\n").unwrap();

        let dest = archive_skill(&root, "nested-skill").unwrap();
        assert_eq!(dest, root.join(".archive/nested-skill"));
    }

    #[test]
    fn a_second_archive_of_a_same_named_skill_gets_a_disambiguating_suffix() {
        let root = scratch_dir("collision");
        write_skill(&root, "twice-skill");
        archive_skill(&root, "twice-skill").unwrap();
        // Simulate a fresh skill with the same name created after the first
        // was archived, then archived again.
        write_skill(&root, "twice-skill");
        let second = archive_skill(&root, "twice-skill").unwrap();

        assert_ne!(second, root.join(".archive/twice-skill"));
        assert!(second.file_name().unwrap().to_str().unwrap().starts_with("twice-skill-"));
        // Both copies survive — the first archive was not clobbered.
        assert!(root.join(".archive/twice-skill/SKILL.md").exists());
        assert!(second.join("SKILL.md").exists());
    }

    #[test]
    fn restoring_over_an_existing_skill_is_refused() {
        let root = scratch_dir("restore-collision");
        write_skill(&root, "conflict-skill");
        archive_skill(&root, "conflict-skill").unwrap();
        write_skill(&root, "conflict-skill"); // something now occupies the name again

        let err = restore_skill(&root, "conflict-skill").unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn restoring_a_name_never_archived_is_refused() {
        let root = scratch_dir("restore-missing");
        std::fs::create_dir_all(root.join(".archive")).unwrap();
        let err = restore_skill(&root, "never-archived").unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }
}
