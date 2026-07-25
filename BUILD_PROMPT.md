# Build Prompt: "Orbit" — Personal AI Command Center

> Placeholder name: **Orbit**. Rename freely (find/replace `orbit`/`Orbit` throughout) before or after the build — this has no effect on functionality.

Paste everything below into Claude Code as the initial instruction for a fresh repository. This is a full one-shot build spec for an ambitious, best-effort v1. Work through the Build Order at the bottom section by section, committing logically as you go, rather than trying to write everything in one pass.

---

## 1. Mission

Build a cross-platform (macOS-first, Windows/Linux best-effort) desktop utility that acts as a personal AI-assisted command center — a mix of Raycast's command palette, macOS's menu-bar utilities, and an agentic "computer use" assistant. It should feel fast, unobtrusive, and genuinely useful daily, not a tech demo. This is being open-sourced, so it must be safe, configurable, and buildable by a stranger with no access to the original author's accounts, models, or API keys.

## 2. Locked Architecture Decisions

These are decided. Do not re-litigate them:

- **Framework:** Tauri v2 (Rust backend, TypeScript/React frontend, Tailwind CSS for styling). Chosen for a small binary, low resource use, and native OS integration without bundling a separate runtime (no Python/Node sidecar requirement for end users).
- **Agent execution layer runs natively in Rust.** No external sidecar process. The agent loop, screen capture, and input simulation are implemented directly in the Rust backend using native crates. Rationale: single compiled binary, no Python environment for a stranger to install, fewest permissions/moving parts, most efficient.
  - Screen capture: `xcap` (cross-platform screenshot crate).
  - Input simulation (mouse/keyboard): `enigo`.
  - HTTP calls to AI providers: `reqwest`.
- **Frontend for the floating overlay and Command Center:** React + TypeScript + Tailwind, rendered in Tauri webviews. Use the design direction in Section 8.
- **Config/persistence:** JSON file store via `tauri-plugin-store` for settings and shortcuts. SQLite (`rusqlite` or `sqlx`) for clipboard history (structured, searchable, prunable). OS keychain (`keyring` crate) for all secrets (API keys, optional clipboard encryption key) — never store secrets in plain JSON.
- **Global hotkeys:** `tauri-plugin-global-shortcut`.
- **System tray / menu bar:** Tauri's native tray API.
- **License:** MIT.

## 3. Core Concept (translate every item below into a generic, configurable system — do not hardcode personal specifics)

### 3.1 Floating orb
- A small circular icon, always-on-top, transparent background, frameless, positioned by default on the right edge of the screen, vertically centered, draggable to reposition (position persisted).
- Visibility toggled via: (a) tray/menu-bar menu item, (b) a global hotkey (default `F12`, rebindable in Settings), (c) a Settings toggle.
- **Hover behavior:** hovering over the orb immediately expands 6 smaller icons in a radial/arc pop-out around it, animated.
- **Auto-collapse:** if the pointer is anywhere else on screen for a configurable idle duration (default 3s), the pop-out (and orb expansion) collapses. Idle duration configurable in Settings (e.g. 1–10s).
- Clicking the **center** orb opens the Command Center (3.2).

### 3.2 Command Center ("the palette")
- A large, centered, high-polish floating window — a command palette in the spirit of Raycast, but with a distinct, more refined visual identity (see Section 8). Do not clone Raycast's exact visual style; build something that looks and feels like a bespoke product.
- Single universal text input at the top. Results/suggestions list below, keyboard-navigable (arrow keys + Enter), fuzzy-searchable across: installed shortcuts, clipboard history, and any registered "sources" (extensible — design a simple provider interface so new result sources can be added later, e.g. `interface ResultProvider { search(query, prefix): ResultItem[] }`).
- **Prefix-based routing on Enter**, all prefixes and their targets fully configurable in Settings (ship sensible defaults, described below):
  - **No prefix:** treat input as a search query, open it in the configured default browser using the configured default search engine/URL template (default: Google, but this is a plain settings field — any URL template with a `{query}` token works).
  - **`/` prefix:** route the remainder to the configured "primary AI" target. Default primary AI target = open the query as a prompt to Gemini (`https://gemini.google.com/app`) in the user's default Chrome profile (see 3.4 for profile handling). Provide a Settings toggle to switch the primary AI target to a local model served through the Agent Execution Layer (Section 4) instead, with a model picker.
  - **`/c` prefix:** route the remainder as a chat message directly to the Agent Execution Layer's computer-use-enabled agent loop (Section 4), i.e. an actual agentic session that can act on the screen, not just a chat reply.
  - Users can add arbitrary custom prefixes → custom actions in Settings, using the same "Shortcut" primitive described in 3.3 (a prefix is just another shortcut trigger type).
- The Command Center window should support a "history" view (recent commands) and a lightweight "no query yet" state showing pinned shortcuts and recent clipboard items.

### 3.3 Shortcut system (generic, replaces the "6 icons" being hardcoded)
Design one generic `Shortcut` data model used both for the orb's 6 pop-out icons and for anything discoverable in the Command Center:

```ts
type ShortcutKind = "open_url" | "open_app" | "run_command" | "run_applescript" | "clipboard_view";

interface Shortcut {
  id: string;
  label: string;
  icon: string;            // icon identifier or emoji/svg reference
  kind: ShortcutKind;
  target: string;          // URL, app bundle id/path, shell command, or AppleScript source
  args?: string[];
  chromeProfileDirectory?: string; // optional, only relevant when target is a Chrome URL
  showInOrb?: boolean;      // whether this appears as one of the orb's pop-out icons
  orderIndex?: number;
}
```

- Ship exactly 6 default shortcuts with `showInOrb: true`, matching this brief's original request, but every field user-editable and deletable in Settings, with an "Add Shortcut" flow:
  1. Open Gemini in the default Chrome profile (`open_url`, chromeProfileDirectory configurable)
  2. Open Gmail (`open_url`)
  3. Open Chrome (`open_app`)
  4. Open Claude (`open_url`, `https://claude.ai`)
  5. Open a user-configured dictation app (`open_app`, target left as an empty/placeholder field the user fills in with their own app — do not assume a specific app exists; label it "Dictation App" by default)
  6. Open the built-in Clipboard History view (`clipboard_view`, opens the Command Center pre-filtered to clipboard results)
- Settings UI must let a user reorder, add, remove, and edit all shortcuts, and choose which subset (up to 6, but don't hard-cap at exactly 6 in the data model — cap the orb's pop-out display at 6 visible, allow more defined) appear on the orb.

### 3.4 Chrome profile handling
- For any `open_url` shortcut, add an optional field for a Chrome profile directory name (e.g. `Profile 1`, `Default`) that gets passed via Chrome's `--profile-directory` launch flag. Provide a Settings helper that lists detected local Chrome profiles (read `Local State` file in the Chrome user data directory, parse profile names) so the user can pick from a dropdown instead of typing a raw directory name. Handle "Chrome not found" and "no profiles detected" gracefully with a manual text-entry fallback.

### 3.5 Voice dictation and keyword routing
- A configurable hotkey (default: `Fn`, rebindable — note in docs that raw `Fn` capture is unreliable cross-platform/cross-keyboard, so allow any modifier/key combo, e.g. default to `Right Option` or `F13` as a safer fallback and let the user choose) acts as **push-to-talk**: hold to record, release to stop, transcribe, and populate the Command Center's input field with the transcribed text.
- **Do not build always-on background wake-word listening for v1.** This is push-to-talk only (holding the hotkey). Note this as an explicit, intentional scope decision.
- Speech-to-text should be pluggable: ship a default implementation using the OS's native dictation/speech APIs where available (macOS: `Speech` framework via a small Swift/Objective-C shim invoked from Rust, or a documented fallback), and a settings-configurable alternative pointing at any local/remote STT endpoint (e.g. a local Whisper server) via a simple `SttBackend` trait, mirroring the `AgentBackend` pattern in Section 4.
- After transcription, run **keyword routing** against the resulting text before treating it as a plain Command Center query:
  - Default keyword groups (all lists user-editable in Settings, plain arrays of strings, case-insensitive substring or leading-word match — pick one approach and document it clearly):
    - `["search", "look up", "browse"]` → route as a browser search (same as the no-prefix path in 3.2).
    - `["computer", "jarvis", "search my mac"]` → route to the Agent Execution Layer's computer-use loop (same as the `/c` path in 3.2).
  - If no keyword matches, fall back to the configured default target (default: primary AI target, same as `/` in 3.2).
  - Settings must expose: hotkey binding, STT backend selection/config, the keyword groups and what each group routes to, and the fallback default.

### 3.6 Clipboard manager ("rich plus" tier)
- Background clipboard watcher (poll or OS clipboard-change notification, whichever the platform supports better — prefer native change notification when Tauri/OS exposes it, poll as fallback) capturing text, images, and file references.
- Persist to SQLite: content, content type, timestamp, source app (best-effort, may be unavailable on all platforms — degrade gracefully), pinned flag.
- Full-text search over history; pin/favorite items so they survive pruning; configurable max history size (item count and/or age-based).
- **Optional local encryption at rest:** a Settings toggle that, when enabled, derives an encryption key via the OS keychain (`keyring` crate, e.g. generate and store a random key, encrypt the SQLite DB contents or individual entries with a modern AEAD cipher — e.g. `ChaCha20-Poly1305` via the `chacha20poly1305` crate). Document clearly that turning this on after history already exists requires a one-time re-encryption pass (implement it) and that losing the key (e.g. keychain reset) makes old history unrecoverable — this is expected and acceptable.
- Clipboard results appear in the Command Center's default/empty-query view and are searchable there; also reachable via the dedicated `clipboard_view` shortcut.

## 4. Agent Execution Layer ("Hermes" — the pluggable AI/agent backend)

This is the most novel and highest-risk part of the build. Keep the interface small and the default implementation solid rather than trying to support everything.

```rust
#[async_trait]
trait AgentBackend: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn supports_computer_use(&self) -> bool;
    async fn chat(&self, messages: Vec<Message>, config: &BackendConfig) -> Result<AgentResponse>;
    async fn run_agent_loop(&self, task: &str, config: &BackendConfig, on_step: impl Fn(AgentStep)) -> Result<AgentOutcome>;
}
```

- **Ship these implementations:**
  1. **AnthropicBackend** — calls the Claude Messages API. Support both plain chat and, when `supports_computer_use` is enabled in config, the computer-use tool. **Before implementing the computer-use tool integration, fetch and read the current spec at `https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool` — do not hardcode beta headers, tool version strings (e.g. `computer_20250124` vs newer), or model names from memory, because these change over time.** Make the beta header string, tool version string, and model name all plain configuration fields with sensible current defaults, so a user can update them without recompiling as Anthropic ships new versions. Implement the actual loop: send screenshot (via `xcap`) → receive tool_use action → execute via `enigo` (click, type, scroll, key, drag) → send result screenshot back → repeat until the model signals completion or a configurable max-steps safety limit is hit.
  2. **OpenAiCompatibleBackend** — generic backend for any OpenAI-compatible chat completions endpoint (covers OpenAI itself, Ollama, LM Studio, vLLM, etc. — they all speak this dialect). Config: base URL, API key (optional, blank for local servers), model name. Chat-only; `supports_computer_use` = false unless the user has separately configured Anthropic for that.
  3. A **NullBackend/no-op** shipped as the zero-config default so the app runs and is fully usable (shortcuts, clipboard, browser search) with zero AI configuration, and clearly prompts the user to add a backend in Settings before any AI-routed action is used.
- **Settings → Agent Backends tab:** add/edit/delete backend configs, choose which backend is "primary" (used by `/` and the voice AI-routing default) and which is used for `/c`/computer-use routing, model picker per backend, and a "Test connection" button per backend.
- **Permissions:** only prompt for macOS Accessibility and Screen Recording permissions the first time a computer-use action actually runs, not at app launch. Show a clear explanatory dialog before triggering the OS permission prompt. Document the equivalent for Windows/Linux (Linux: X11 vs Wayland input simulation is a known hard problem — document that `enigo` has limited/no support under Wayland and note this as a known limitation rather than silently failing).
- **Safety limits:** hard cap on agent loop steps per task (configurable, sane default e.g. 25), a visible "stop" control in the UI while an agent loop is running, and a confirmation step before the very first action of any computer-use session (so it never silently starts controlling the mouse).

## 5. Settings Window

A dedicated Tauri window (not the Command Center) with tabs:
1. **General** — hotkey for orb visibility toggle, orb position/edge, hover expand delay, auto-collapse idle duration, launch-at-login toggle.
2. **Shortcuts** — full CRUD over the `Shortcut` list from 3.3, reorderable, with "show in orb" checkboxes (max 6 enforced in UI).
3. **Command Center** — prefix-to-action mapping table (add/edit/remove prefixes), default search engine URL template.
4. **Voice** — hotkey binding, STT backend config, keyword groups editor, fallback default.
5. **Agent Backends** — as described in Section 4.
6. **Clipboard** — max history size/age, encryption toggle, per-app exclusion list (e.g. never capture from password managers).
7. **Appearance** — theme (see Section 8), orb size, accent handling.

All settings persist immediately (no separate "Save" step needed, or an explicit save with a clear success indicator — pick one pattern and apply it consistently).

## 6. Directory Structure (suggested — adjust as needed, but keep it this organized)

```
orbit/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   ├── window/          # orb window, command center window, settings window management
│   │   ├── shortcuts/       # Shortcut model, CRUD, execution (open_url/open_app/run_command/applescript)
│   │   ├── clipboard/       # watcher, SQLite store, search, encryption
│   │   ├── agent/           # AgentBackend trait + implementations, agent loop, input simulation, screen capture
│   │   ├── voice/           # hotkey capture, SttBackend trait + implementations, keyword router
│   │   ├── settings/        # config schema, tauri-plugin-store wiring, keyring wiring
│   │   ├── tray.rs
│   │   └── commands.rs      # #[tauri::command] IPC surface exposed to the frontend
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                     # React frontend
│   ├── orb/
│   ├── command-center/
│   ├── settings/
│   ├── shared/
│   └── main.tsx
├── docs/
│   ├── ARCHITECTURE.md
│   ├── PLUGIN_GUIDE.md      # how to add a new AgentBackend / SttBackend / ResultProvider
│   └── PLATFORM_SUPPORT.md  # explicit macOS/Windows/Linux capability matrix and known gaps
├── .github/workflows/ci.yml
├── README.md
├── CONTRIBUTING.md
├── LICENSE                  # MIT
└── .gitignore
```

## 7. Non-Goals for v1 (explicitly out of scope — do not attempt these)

- Always-on wake-word voice listening (push-to-talk only, see 3.5).
- Mobile companion apps.
- Cloud sync of settings/clipboard across devices.
- Auto-update infrastructure (fine to leave a TODO/doc note, not to build).
- A plugin marketplace UI. Ship the `ResultProvider`/`AgentBackend`/`SttBackend` traits and document them in `PLUGIN_GUIDE.md`; that's enough for v1 extensibility.
- Perfect Windows/Linux parity. Best-effort, with gaps explicitly documented in `PLATFORM_SUPPORT.md` rather than silently broken.

## 8. Visual Design Direction

This is a personal, slightly sci-fi "AI command center" tool for a technical/power-user audience being shared as open source — not a corporate executive deliverable. Aim for:
- Dark-first theme by default (light theme optional/secondary).
- A refined, modern "glass"/depth aesthetic: subtle blur, soft shadows, restrained motion — closer to a well-designed dev tool (think: thoughtful spacing, a real typographic hierarchy, one confident accent color) than to gamer-RGB or a literal Iron-Man-HUD pastiche. Avoid clutter, avoid neon-everything.
- The orb itself should feel alive but not busy: a calm idle state, a satisfying hover/expand animation, clear focus states in the Command Center.
- Use the `frontend-design` skill/guidance available in this environment for concrete component and styling choices.

## 9. Build Order

Work in this sequence, committing at each milestone:

1. Scaffold Tauri + React project, get an empty always-on-top transparent orb window rendering and draggable, plus tray icon with a visibility toggle.
2. Implement the `Shortcut` model, Settings persistence layer (`tauri-plugin-store` + keyring wiring), and the orb's hover pop-out with the 6 default shortcuts wired to real `open_url`/`open_app` execution.
3. Build the Command Center window: input field, static result list (shortcuts + placeholder), keyboard navigation, prefix parsing and routing to the no-prefix browser-search path first (simplest path, proves the pattern).
4. Build the clipboard watcher + SQLite store + search, wire into Command Center's empty-query view and the `clipboard_view` shortcut. Add encryption toggle.
5. Build the `AgentBackend` trait, `NullBackend`, and `OpenAiCompatibleBackend`. Wire the `/` prefix to a working chat round-trip against a configured backend.
6. Build the `AnthropicBackend` including the computer-use loop (fetch current docs first, per Section 4), wire `/c` and the computer-use routing path, add the safety limits and confirmation UX.
7. Build voice: hotkey capture, default STT implementation, keyword router, wire into Command Center input.
8. Build out the full Settings window across all tabs from Section 5.
9. Polish pass on visuals per Section 8; test the full hover → orb → shortcut and orb-center → Command Center → each routing path end to end.
10. Write `README.md` (setup, prerequisites, permissions explanation, screenshots placeholder), `ARCHITECTURE.md`, `PLUGIN_GUIDE.md`, `PLATFORM_SUPPORT.md`, `CONTRIBUTING.md`, add MIT `LICENSE`, add a basic GitHub Actions workflow that builds the app on macOS (and attempts Windows/Linux, allowed to be marked non-blocking/experimental).

## 10. Definition of Done

- [ ] App launches with zero configuration and zero API keys, and shortcuts/clipboard/browser-search all work immediately.
- [ ] Orb toggles via tray, hotkey, and Settings; hover pop-out and idle auto-collapse both work and are configurable.
- [ ] Command Center opens from the orb's center, all three default prefix behaviors work, and custom prefixes can be added in Settings.
- [ ] Clipboard history captures text/images/files, is searchable, pinnable, and encryption can be toggled on/off.
- [ ] At least one real `AgentBackend` (OpenAI-compatible) works end-to-end for chat with zero code changes required by the end user, just a Settings entry.
- [ ] The Anthropic computer-use loop runs a real simple task (e.g. "open a new browser tab and search for X") end-to-end with visible step-by-step feedback and a working stop control.
- [ ] Voice push-to-talk transcribes and keyword-routes correctly for at least the default keyword groups.
- [ ] No secrets ever written to plain JSON/disk outside the OS keychain.
- [ ] `README.md` alone is enough for a stranger to clone, build, and run the app on macOS.
