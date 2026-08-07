//! Registers `skills_list`, `skill_view`, and `skill_manage` into
//! [`crate::native_tools`] — the seam described in that module's doc — so
//! the agent tool loop can call them exactly like it would call any other
//! tool, with no MCP server involved.
//!
//! Each handler closure captures its own clone of the resolved skills-root
//! path and translates the tool call's JSON arguments into a call on the
//! Tauri-free core in [`super::tiers`] / [`super::manage`] — the same
//! functions [`super::commands`]'s `#[tauri::command]` wrappers call for the
//! frontend, so the two surfaces can never drift in behavior.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::native_tools::{self, NativeTool};

use super::{manage, tiers};

/// Register all three tools against `skills_root`. Call once, from
/// `lib.rs::setup`, after the app data directory is resolved — see this
/// task's final report for the exact call site.
pub fn register(skills_root: PathBuf) {
    register_skills_list(skills_root.clone());
    register_skill_view(skills_root.clone());
    register_skill_manage(skills_root);
}

fn register_skills_list(root: PathBuf) {
    native_tools::register(NativeTool::new(
        "skills_list",
        "List available skills (name + full description + category). Use skill_view(name) to load a skill's full content.",
        json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "description": "Optional category filter to narrow results."
                }
            },
            "required": []
        }),
        move |args| {
            let category = args.get("category").and_then(Value::as_str);
            let skills = tiers::list_skills(&root, category);
            serde_json::to_value(skills).map_err(|e| e.to_string())
        },
    ));
}

fn register_skill_view(root: PathBuf) {
    native_tools::register(NativeTool::new(
        "skill_view",
        "Load a skill's full content, or one of its supporting files. Skills allow loading information about specific tasks and workflows, plus scripts and templates. First call with just `name` to get the SKILL.md body and a `linkedFiles` listing of its references/templates/scripts/assets. Call again with `file_path` (e.g. 'references/api.md') to load one of those files.",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name (use skills_list to see available skills)."
                },
                "file_path": {
                    "type": "string",
                    "description": "OPTIONAL: path to a linked file within the skill (e.g. 'references/api.md', 'templates/config.yaml', 'scripts/validate.sh'). Omit to get the main SKILL.md content."
                }
            },
            "required": ["name"]
        }),
        move |args| {
            let name = args.get("name").and_then(Value::as_str).ok_or("`name` is required")?;
            match args.get("file_path").and_then(Value::as_str) {
                None => {
                    let view = tiers::view_skill(&root, name)?;
                    serde_json::to_value(view).map_err(|e| e.to_string())
                }
                Some(file_path) => match tiers::view_skill_file(&root, name, file_path)? {
                    tiers::SkillFileContent::Text(content) => Ok(json!({ "name": name, "file": file_path, "content": content })),
                    tiers::SkillFileContent::Binary { size } => {
                        Ok(json!({ "name": name, "file": file_path, "isBinary": true, "sizeBytes": size }))
                    }
                },
            }
        },
    ));
}

fn register_skill_manage(root: PathBuf) {
    native_tools::register(NativeTool::new(
        "skill_manage",
        "Manage skills — your procedural memory: reusable, written-down approaches for recurring kinds of tasks. \
         Actions: create (full SKILL.md content + optional category), \
         patch (old_string/new_string — the preferred, token-cheap way to fix or extend an existing skill; \
         old_string must match exactly and must be unique unless replace_all is true), \
         edit (full SKILL.md rewrite — reserve for major overhauls), \
         delete, write_file, remove_file (the last two target a supporting file under references/templates/scripts/assets). \
         Consider creating a skill after a task that took 5+ tool calls to get right, after working around a \
         non-obvious pitfall, after the user corrects your approach, or when the user asks you to remember a \
         procedure. If you used an existing skill and hit something it did not cover, patch it immediately rather \
         than letting it go stale. Good skills state trigger conditions, give numbered steps with exact commands, \
         and call out pitfalls — see the bundled 'skill-authoring' skill for the full guide.",
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "patch", "edit", "delete", "write_file", "remove_file"],
                    "description": "The action to perform."
                },
                "name": {
                    "type": "string",
                    "description": "Skill name (lowercase letters, digits, '.', '_', '-'; max 64 chars). Must match an existing skill for every action except 'create'."
                },
                "content": {
                    "type": "string",
                    "description": "Full SKILL.md content (YAML frontmatter + markdown body). Required for 'create' and 'edit'."
                },
                "category": {
                    "type": "string",
                    "description": "Optional subdirectory grouping (e.g. 'devops'). Only used with 'create'."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find. Required for 'patch'. Must be unique in the file unless replace_all is true."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text. Required for 'patch'. Use an empty string to delete the matched text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "For 'patch': replace every occurrence instead of requiring a unique match. Default false."
                },
                "file_path": {
                    "type": "string",
                    "description": "Path to a supporting file under references/templates/scripts/assets. Required for write_file/remove_file; optional for patch (defaults to SKILL.md when omitted)."
                },
                "file_content": {
                    "type": "string",
                    "description": "Content for the file. Required for 'write_file'."
                }
            },
            "required": ["action", "name"]
        }),
        move |args| {
            let action = args.get("action").and_then(Value::as_str).ok_or("`action` is required")?;
            let name = args.get("name").and_then(Value::as_str).ok_or("`name` is required")?;
            let message = manage::skill_manage(
                &root,
                action,
                name,
                args.get("content").and_then(Value::as_str),
                args.get("category").and_then(Value::as_str),
                args.get("file_path").and_then(Value::as_str),
                args.get("file_content").and_then(Value::as_str),
                args.get("old_string").and_then(Value::as_str),
                args.get("new_string").and_then(Value::as_str),
                args.get("replace_all").and_then(Value::as_bool).unwrap_or(false),
            )?;
            Ok(json!({ "message": message }))
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_tools::test_support::{clear, locked};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("caduceus-skills-native-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // These tests register the crate's *real* tools into
    // `native_tools`'s process-global registry to verify the wiring
    // end-to-end — the same registry `native_tools.rs`'s own tests (and, in
    // a full crate build, `agent::toolloop`) use. `locked()` serializes
    // against every other test anywhere in the crate that touches this
    // registry; `clear()` then guarantees a clean slate regardless of what
    // ran before, so a leftover registration from another test module can
    // never make one of these assertions pass or fail for the wrong reason.

    #[test]
    fn registers_all_three_tools_under_their_hermes_compatible_names() {
        let _g = locked();
        clear();
        let root = scratch_dir("register");
        register(root);
        let names: Vec<String> = native_tools::list().into_iter().map(|t| t.name).collect();
        assert!(names.contains(&"skills_list".to_string()));
        assert!(names.contains(&"skill_view".to_string()));
        assert!(names.contains(&"skill_manage".to_string()));
    }

    #[test]
    fn skill_manage_then_skills_list_then_skill_view_round_trip_through_the_registry() {
        let _g = locked();
        clear();
        let root = scratch_dir("round-trip");
        register(root);

        let create_result = native_tools::call(
            "skill_manage",
            json!({
                "action": "create",
                "name": "native-test-skill",
                "content": "---\nname: native-test-skill\ndescription: exercised via the native tool registry\n---\nBody.\n"
            }),
        );
        assert!(create_result.is_ok(), "{create_result:?}");

        let listed = native_tools::call("skills_list", json!({})).unwrap();
        let names: Vec<&str> = listed.as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"native-test-skill"), "{listed}");

        let viewed = native_tools::call("skill_view", json!({ "name": "native-test-skill" })).unwrap();
        assert_eq!(viewed["name"], "native-test-skill");
        assert!(viewed["content"].as_str().unwrap().contains("Body."));
    }

    #[test]
    fn skill_view_file_path_reads_a_supporting_file_through_the_registry() {
        let _g = locked();
        clear();
        let root = scratch_dir("view-file");
        register(root);

        native_tools::call(
            "skill_manage",
            json!({"action": "create", "name": "with-refs", "content": "---\nname: with-refs\ndescription: d\n---\nBody\n"}),
        )
        .unwrap();
        native_tools::call(
            "skill_manage",
            json!({"action": "write_file", "name": "with-refs", "file_path": "references/api.md", "file_content": "api docs"}),
        )
        .unwrap();

        let result = native_tools::call("skill_view", json!({"name": "with-refs", "file_path": "references/api.md"})).unwrap();
        assert_eq!(result["content"], "api docs");
    }

    #[test]
    fn a_missing_required_argument_is_a_clear_error_not_a_panic() {
        let _g = locked();
        clear();
        let root = scratch_dir("missing-arg");
        register(root);
        let err = native_tools::call("skill_manage", json!({"action": "create"})).unwrap_err();
        assert!(err.contains("name"), "{err}");
    }
}
