---
name: skill-authoring
description: How to write, structure, and validate a good SKILL.md for Caduceus's own skills system.
version: 1.0.0
license: MIT
metadata:
  hermes:
    tags: [skills, authoring, meta]
---

# Authoring Caduceus Skills

## Overview

A skill is a directory containing a `SKILL.md` file, plus optional `references/`, `templates/`, `scripts/`, and `assets/` subdirectories for material that does not need to be loaded every time. Skills are how you turn a hard-won approach into something a future session can reuse without re-deriving it. Use `skill_manage` to create and edit them — never write into the skills directory with a generic file tool.

## When to consider writing a skill

- A task just took 5 or more tool calls to get right.
- You worked around a non-obvious error or pitfall that will recur.
- The user corrected your approach, and the corrected approach is worth remembering exactly.
- You discover a repeatable, multi-step workflow — a build, a deployment, a cleanup routine.
- The user explicitly asks you to remember how to do something.

Skip it for simple one-off tasks: a skill nobody will ever load again is just clutter competing for attention with the skills that matter. On a nontrivial task, offer to save it rather than deciding unilaterally.

If you use an existing skill and hit a gap it did not cover, patch it immediately — a skill that quietly goes stale is worse than no skill at all, because it is trusted without being checked.

## Required frontmatter

Only two fields are required:

```yaml
---
name: my-skill-name
description: One sentence describing what this does and when to use it.
---
```

- `name` — lowercase letters, digits, `.`, `_`, `-`; must start with a letter or digit; 64 characters max.
- `description` — 1024 characters max. Only its first ~60 characters appear in the always-visible skill index shown on every turn, so front-load the trigger condition: "Use when converting between currencies" beats "Currency conversion helper" — the model deciding whether to load the skill sees the truncated form first.

Everything else is optional but commonly useful:

```yaml
version: 1.0.0
license: MIT
platforms: [macos]              # restrict to macos / linux / windows; omit for all platforms
metadata:
  hermes:
    tags: [tag-one, tag-two]
    related_skills: [other-skill-name]
```

The frontmatter parser understands plain scalars, quoted scalars (`"..."` / `'...'`), inline `[a, b, c]` lists, block `- item` lists, and nested mappings. It does **not** understand YAML anchors/aliases (`&`/`*`), tags (`!`), block scalars (`|`/`>`), flow mappings (`{a: b}`), or lists of mappings — a frontmatter block using any of those is rejected outright with a line number, not silently misread. Keep frontmatter to the shapes above and it will always parse.

## Structure

```markdown
# Title

One paragraph: what this does and why it exists.

## When to use
- Bulleted trigger conditions
- What this is easily confused with, and why that's different

## The actual how-to
Numbered steps, exact commands, concrete examples — this is the bulk of the skill.

## Pitfalls
Mistakes made before, and how to avoid them this time.

## Verification
How to check the result actually worked.
```

Not every section is required — a short skill can be a few paragraphs — but a skill with no concrete steps, only generic advice ("be careful," "use best practices," "double-check your work"), is not worth creating: it would not change what you actually do differently. Every sentence should earn its place by changing behavior; if it wouldn't, cut it rather than polish it.

## Supporting files

Keep `SKILL.md` itself focused on what is needed on every use. Push bulky or branch-specific material into:

- `references/` — documentation only sometimes needed (a full API reference, an option table)
- `templates/` — boilerplate to copy and fill in
- `scripts/` — small helper scripts the steps call out to
- `assets/` — anything else (sample data, images, fixtures)

Link to them from `SKILL.md` by relative path (for example, "see `references/api.md`"). They are not loaded automatically — the agent reading this skill fetches them on demand with `skill_view(name, file_path)` only when the step that needs them is reached.

## Using skill_manage

- **`create`** — full `SKILL.md` content (frontmatter + body), with an optional `category` for a subdirectory grouping (e.g. `devops`, `data-cleanup`). Fails if the name is already taken.
- **`patch`** — the preferred way to fix or extend an existing skill, and the cheapest in tokens. Give `old_string` (must match the file's exact text, including whitespace) and `new_string`; it must match exactly once unless `replace_all: true` is passed. If the match fails, widen `old_string` with more surrounding context rather than guessing at reformatting — this is an exact-match patch, not a fuzzy one, so copy the text precisely from what `skill_view` returned rather than paraphrasing it. An empty `new_string` deletes the matched text.
- **`edit`** — a full-content rewrite. Reserve this for a genuine overhaul; anything smaller should be a `patch`.
- **`write_file` / `remove_file`** — add, replace, or delete a file under `references/`, `templates/`, `scripts/`, or `assets/`. `SKILL.md` itself is not reachable this way — use `create`/`edit`/`patch` for it.
- **`delete`** — removes a skill outright. Refuses if the skill is pinned.

## Limits

- Description: 1024 characters.
- Full `SKILL.md`: 100,000 characters. A peer skill in the low thousands of characters is typical; if a draft is pushing past 15–20k, that is usually a sign some of it belongs in `references/` instead.
- Any one supporting file: 1 MiB.

## Lifecycle

Skills are tracked by actual use, not by judgment calls: unused for 30 days, a skill is marked `stale` (still fully available, nothing changes for the agent); unused for 90 days, it is moved to an archive — recoverable, never deleted outright. A skill that matters regardless of how often it gets touched can be pinned, which blocks both automatic archival and `skill_manage(delete)`; patches and edits still go through normally on a pinned skill, so it can keep improving even while protected from being swept away.

## After finishing a skill

Re-read what was written once, as if encountering it fresh with no memory of this conversation: does each step carry enough detail to actually follow it? Would completion be obvious, or does it depend on judgment the skill never spelled out? If a step only works because of something true just in this one instance — a file path that happens to exist right now, a value pulled from this conversation — generalize it, or mark it clearly as an example rather than a hard requirement.
