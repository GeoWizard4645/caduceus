# Caduceus → JARVIS: the actionable build plan

Derived from the two videos you sent, then **verified against the real Hermes Agent source**, which is
already installed on this machine at `~/.hermes/hermes-agent/` (Nous Research, **MIT**, v0.18.2, pure
Python — so every algorithm is readable). Everything below is free. Nothing needs to be bought.

> The guy in video 2 (Julian Goldie) sells a Skool wrapper called "Agent OS / Hermes Jarvis". The
> capabilities he demos are all in the free open-source Hermes. His good *ideas* (JARVIS voice persona,
> memory galaxy, real-time vs agent mode) are worth stealing. His product is not worth buying.

---

## Part 0 — What I found that changes the plan

Three findings that matter before any code gets written.

### 0.1 The Dock problem may already be fixed

`src-tauri/Info.plist:7` already sets `LSUIElement = true`, and `src-tauri/src/lib.rs:541` already calls
`app.set_activation_policy(ActivationPolicy::Accessory)`. The **shipped** `/Applications/Caduceus.app`
also has `LSUIElement = true`, and macOS currently reports the running process as background-only.

There is also a stale comment at `lib.rs:539` claiming *"the Settings window temporarily switches this
back (see window::open_settings)"* — **no such code exists**. `window::open_settings`
(`src-tauri/src/window/mod.rs:946`) never touches the activation policy, and `set_palette_floating`
(`mod.rs:981`) carries a comment saying switching to `Regular` was tried and deliberately reverted
because it caused exactly the Dock icon you're describing.

**So: I can't reproduce it, and the fix is already in the build you're running.** Most likely cause is a
*second copy* — see 0.2. If you're seeing the Dock icon in `npm run tauri dev`, that's expected and not
a real bug: `tauri dev` runs a bare binary, not a bundle, so `Info.plist` isn't applied and only the
runtime `Accessory` call takes effect (a few hundred ms after launch).

### 0.2 The hotkey bug is a two-copies bug, and you were mid-fix

`hotkeys.rs` uses `tauri-plugin-global-shortcut`, which registers through Carbon `RegisterEventHotKey` —
genuinely system-wide. It does not need the app to be frontmost, so "only works while the app is up"
isn't a scoping bug.

Your own uncommitted `lib.rs` diff diagnoses it exactly:

> *"A global hotkey can be held by exactly one process. Caduceus is a resident menu-bar app with no Dock
> icon and no Force Quit entry, so a stray second copy is invisible — and `hotkeys::register_all` reacts
> to the OS refusing an already-taken accelerator by moving that action to a fallback and *saving* the
> move… whichever loses the race quietly rewrites the user's Command Center key from Alt+Space to
> Control+Space in their settings file."*

You added `tauri_plugin_single_instance` to fix it. That work is **uncommitted and not in the build you
are running** — which is consistent with the symptom. There are four `Caduceus.app` bundles on this
machine (`/Applications` plus three under `src-tauri/target/`).

**Actions:** finish and ship the single-instance work; make `register_all` *never* silently rewrite a
user-chosen accelerator (surface a conflict instead); add a "hotkey health" line in Settings showing
which accelerator is actually held.

### 0.3 Computer use: don't reimplement it — drive `cua-driver`

The "Takeover Engine" in video 2 is Hermes driving [`trycua/cua`](https://github.com/trycua/cua). It is
**already installed here** as a native Rust universal binary (`/Applications/CuaDriver.app`,
`cua-driver 0.12.3`), the daemon is running, and Accessibility + Screen Recording are already granted.

I verified the actual macOS mechanism from its tool schemas:

| Claim from the video | Real mechanism |
|---|---|
| "clicks without moving your cursor" | `click` with `element_index` → **AX action path**, no cursor move, no focus steal; works on backgrounded/minimized/off-Space windows |
| "types in the background" | `type_text` → **`AXSetAttribute(kAXSelectedText)`**, CGEvent character synthesis as fallback |
| key presses | `press_key` → **`CGEventPostToPid`** (posts to one pid, not the global event stream) |
| "windows don't jump to front" | `launch_app` launches background-only; `delivery_mode: background` is the default on every action |
| "you see its cursor, not yours" | `start_session` creates a color-coded **agent cursor overlay**; the real OS cursor never moves |

Reimplementing this in Rust means private SkyLight APIs and an AX-tree walker — weeks of fragile work.
Caduceus already has 72KB of MCP client code (`src-tauri/src/mcp.rs`). **Drive `cua-driver` over MCP.**

---

## Part 1 — Raycast-like behavior (your first ask)

| # | Item | Notes |
|---|---|---|
| 1.1 | Ship the single-instance fix | Already written, uncommitted. Root cause of the hotkey bug. |
| 1.2 | Stop `register_all` rewriting user accelerators | Fail loudly on conflict instead of silently remapping and saving. |
| 1.3 | Delete the stale comment at `lib.rs:539` | It describes behavior that doesn't exist and will mislead the next reader. |
| 1.4 | Dismiss on focus loss + Escape | Confirm the blur handler covers every window, not just the palette. |
| 1.5 | Never quit on last window close | Verify `ExitRequested` is prevented; the app must stay resident like Raycast. |
| 1.6 | Autostart at login | `autostart.rs` has +170 uncommitted lines — finish it. Resident-at-login is what makes the hotkey always work. |
| 1.7 | Fixed-width, auto-height palette | Raycast doesn't let you resize the main window at all — it grows to fit results. This removes the resize annoyance instead of fixing it. Keep resize only for Command Center/Settings. |
| 1.8 | Idle footprint | Currently ~137 MB RSS. Audit boot-time `thread::spawn` / `tokio::spawn` / timers and make them lazy. |
| 1.9 | Rebuild onboarding | Reduce to three steps: permissions → hotkey → model. Skippable, never reappears. `src/staff/Onboarding.tsx`. |

## Part 2 — Memory (Hermes' core differentiator)

> **Corrected after reading the source.** Hermes' memory is **not a knowledge graph and uses no
> embeddings by default** — the "memory galaxy" in video 2 is a *runtime-derived visualisation*
> (`agent/learning_graph.py`), not a persisted graph. `state.db` contains no memory tables at all.
> The real design is much simpler, and better. Build this, not a graph.

The whole store is two flat markdown files, `MEMORY.md` and `USER.md`, entries joined by a literal
`\n§\n` delimiter, written atomically (temp + rename), and **hard-capped by character count**
(defaults 2,200 and 1,375).

The clever part is what happens at the cap: **an over-budget write is rejected**, forcing the model to
consolidate via `replace`/`remove`. There is no scoring, no decay, no pruning heuristic — the budget
*is* the pruning mechanism. Because the file is bounded, it is simply injected whole every session; no
retrieval step exists.

| # | Item |
|---|---|
| 2.1 | `MEMORY.md` + `USER.md` under app-data, `§`-delimited, atomic writes, human-editable |
| 2.2 | Hard char budget; **reject over-budget writes** rather than auto-pruning |
| 2.3 | Inject whole, with a live budget banner (`[67% — 1,474/2,200 chars]`) |
| 2.4 | Background "nudge": every ~10 turns, fork the agent with tools restricted to `{memory, skills}` and ask it to review the conversation for durable facts |
| 2.5 | Prompt guidance that earns its keep: *"Save durable facts… If a fact will be stale in a week, it does not belong in memory. Write declarative facts, not instructions to yourself."* |
| 2.6 | Recall over *past sessions* = **SQLite FTS5 + BM25**, not vectors. `tools/semantic.rs` already has BM25 — reuse it. |
| 2.7 | Per-profile memory isolation |

Embeddings stay optional. I pulled `nomic-embed-text` and `semantic.rs` already auto-detects an Ollama
embedder, so a vector layer can be added later — but it is explicitly *not* how Hermes works, and
shipping without it is the faithful implementation.

## Part 3 — Skills (self-evolving)

> **Corrected after reading the source.** Skill selection uses **no embeddings, no BM25, no reranker**
> (verified by exhaustive grep). Do not build a ranker.

A skill is a **directory** containing `SKILL.md` — only `name` and `description` are actually required —
plus optional `references/`, `templates/`, `scripts/`, `assets/` that are lazy-loaded on demand. The
format matches the public [agentskills.io](https://agentskills.io) standard, so Hermes' own skills are
drop-in compatible.

Selection is **three-tier progressive disclosure**, and the model does the choosing:

- **Tier 0** — every visible skill's name + a **60-char** truncated description is always in the system
  prompt, grouped by category, with the instruction *"If a skill matches or is even partially relevant,
  you MUST load it with `skill_view(name)`. Err on the side of loading."*
- **Tier 1** — `skills_list()` returns name + full description + category.
- **Tier 2** — `skill_view(name)` returns the full body.
- **Tier 3** — `skill_view(name, file_path=…)` returns one supporting file.

Lifecycle is **deterministic, not LLM-driven**: a usage sidecar tracks view/use/patch counts; unused
>30 days → `stale`; >90 days → moved to `.archive/` (recoverable, never hard-deleted) unless pinned or
referenced by a cron job. An LLM consolidation pass exists but is **opt-in and off by default**, and may
only touch skills the agent itself authored.

| # | Item |
|---|---|
| 3.1 | Skill directory format + `SKILL.md` frontmatter, matching Hermes exactly |
| 3.2 | Tier-0 catalog rendering into the system prompt (cached, invalidated on mtime/size change) |
| 3.3 | `skills_list` / `skill_view` tools for tiers 1–3 |
| 3.4 | `skill_manage` dispatcher (`create`/`patch`/`edit`/`delete`) so the agent authors its own; `patch` is the token-cheap default |
| 3.5 | Deterministic curator: stale at 30d, archive at 90d, never delete |
| 3.6 | Feedback → skill revision loop (the daily-briefing pattern from video 1) |
| 3.7 | Skills browser UI with enable/disable |

## Part 4 — Computer use via cua-driver

| # | Item |
|---|---|
| 4.1 | MCP client wiring to `cua-driver mcp` |
| 4.2 | Auto-install + permission-grant flow (`cua-driver permissions grant`) so you never touch a terminal |
| 4.3 | Capture → element-index → act → verify loop (**always prefer `element_index` over pixel coords**) |
| 4.4 | Approval gate on destructive actions + a always-visible stop button |
| 4.5 | Real-time mode (fast, single action) vs agent mode (background, multi-step) |
| 4.6 | Session agent-cursor overlay so you can see what it's doing |

## Part 5 — Models

| # | Item |
|---|---|
| 5.1 | Ollama provider (already running here, 10 models installed) |
| 5.2 | **64k context guard** — the trap from video 1, confirmed in source as `MINIMUM_CONTEXT_LENGTH = 64_000` for any tool-calling model. Probe the live endpoint rather than trusting a table, and cache as `"<model>@<base_url>": <int>` (Hermes keeps this in `context_length_cache.yaml`). Re-validate cached values against the floor on read. You already have `gemma4-64k`, `qwen3vl-64k`, `qwen-vl-64k`. |
| 5.3 | Vision model routing for computer use (`qwen3-vl:8b` / `qwen2.5vl:7b` installed) |
| 5.4 | Profiles: separate config, skills, memory, working dir, default model |

Three non-obvious Ollama workarounds worth copying verbatim — each exists because of a real upstream bug:

- **`default_max_tokens = 65536`** as a floor, because Ollama otherwise truncates at `num_predict=128`.
- **`ollama_num_ctx` → `extra_body.options.num_ctx`**, which is how you actually raise the context.
- To disable reasoning you must set **both** top-level `reasoning_effort: "none"` **and**
  `extra_body.think: false` — Ollama's `/v1/chat/completions` ignores `think=`, only `/api/chat` honours it.

## Part 5b — Approval & safety

Worth copying because Hermes' own `SECURITY.md` is unusually honest about what this is and isn't:

> *"The only security boundary against an adversarial LLM is the operating system. Nothing inside the
> agent process constitutes containment — not the approval gate, not output redaction, not any pattern
> scanner, not any tool allowlist."*

Treat the gate as accident-prevention, not a security boundary. Two details are genuinely load-bearing:

- A **hardline floor** that fires even in yolo mode (`rm -rf /`, `mkfs`, `dd` to a raw device, fork bombs,
  headless `sudo`).
- The yolo flag is **frozen at process start**, specifically so a mid-session prompt injection or a
  malicious skill cannot flip it at runtime. Do the same.

Cron runs default to **deny** on approval (fail closed), since no human is present.

## Part 6 — Automations & reach

| # | Item |
|---|---|
| 6.1 | Cron jobs: name, schedule, prompt, attached skills, delivery target, trigger-now |
| 6.2 | Each cron run = fresh session (so it must pull from memory/skills, not chat history) |
| 6.3 | Telegram gateway: BotFather token + allow-listed user ID, long-polling (no server needed) |
| 6.4 | Daily-briefing template wired to the feedback→skill loop |

## Part 7 — JARVIS voice

| # | Item |
|---|---|
| 7.1 | Local STT (Parakeet helper already in the tree) |
| 7.2 | TTS replies with a butler persona — *"Quite well, sir."* |
| 7.3 | Wake word / push-to-talk → spoken answer, fully hands-free |
| 7.4 | `SOUL.md`-equivalent persona file, per profile |

---

## Suggested order

1. **Part 1** — small, bounded, fixes what annoys you daily.
2. **Part 4 + 5** — biggest capability jump per unit of work, because cua-driver and Ollama are already
   installed and permissioned. This is what makes it feel like JARVIS.
3. **Part 2 + 3** — the self-evolving core. Most net-new code.
4. **Part 6 + 7** — reach and personality.
