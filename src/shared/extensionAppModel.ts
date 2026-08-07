/**
 * Architecture summary for the Extensions tab AI prompt — kept in one place so
 * the in-app preview matches what gets copied.
 */

export const CADUCEUS_APP_MODEL = `
CADUCEUS ARCHITECTURE (macOS desktop app, Tauri + React)

Windows
  • Staff — floating transparent orb; shortcuts on hover; opens Command Center on click.
  • Command Center — one window, browser-style tabs (palette, settings, chat, tools, …).

Command Center / palette
  • Global hotkey opens Home tab: fuzzy search over apps, shortcuts, commands, extensions.
  • Prefix router (longest match): default "/" → primary AI chat, "/c" → Hermes computer use,
    "/v" → clipboard search; user-defined prefixes in Settings → Command Center.
  • Providers merge results: calculator, apps, shortcuts, commands (~150+), live lists (ports,
    docker, …), extensions, web fallback.
  • Enter dispatches in Rust (palette.rs) so voice and keyboard share rules.

Settings (persisted JSON, auto-save)
  • general — hotkeys, staff edge/position, onboarding, function keys.
  • shortcuts — staff ring + palette; targets: URL, app, command, AppleScript, built-in views.
  • commandCenter — browser, prefixes, max results.
  • agents — backends (hermes, openai_compatible), primary + computer-use routing, API keys in keychain.
  • clipboard — history, encryption, exclusions.
  • appearance — theme, accent, staff size/mark/animation, Command Center backdrop.
  • voice — push-to-talk, keyword routing to AI/web/clipboard.
  • extensions — drop-in .js files (this system).

Built-in commands
  • Registered in src/shared/commands.ts with id, title, keywords, run/page handlers.
  • Window snaps, media (Spotify/Safari/Chrome), files, system toggles, dev tools (sha256,
    json_format, jwt_decode, …), pages (meeting, screen-record, sticky notes, …).

AI
  • "/" and AI tab: SQLite chat threads, primary backend (Hermes or OpenAI-compatible/Ollama).
  • "/c" and Cowork: Hermes Agent computer-use session with approval gate.

Extensions (this file format)
  • Single .js file, @caduceus header (parsed without executing code), Web Worker sandbox.
  • Discovered by name in palette; input = text after the name on Enter.
  • Return string → message; array of { title, subtitle?, action? } → list (action = fn or string).
  • Permissions are explicit; Rust re-checks header on every ctx call.
`.trim();

export function buildExtensionPrompt(task: string): string {
  const goal = task.trim() || "<describe what the extension should do>";

  return `Write a Caduceus extension.

Caduceus is a macOS command palette and floating staff (launcher + AI + automation).
An extension is ONE JavaScript file — no manifest folder, no build step, no npm.

WHAT IT SHOULD DO
${goal}

${CADUCEUS_APP_MODEL}

REQUIRED FILE SHAPE

/**
 * @caduceus 1
 * name: <short title shown in the palette>
 * description: <one line>
 * author: <your name>
 * permissions: <comma-separated — see below>
 */
export default async function (input, ctx) {
  // input: string — whatever the user typed after the extension name
  // return a string, a list of rows, or undefined/null for no output
}

The header comment must be the FIRST comment in the file.

PERMISSIONS — declare every capability you use (users see this before install)
  clipboard      read/write system clipboard
  network        ctx.fetch — any http(s) host (APIs, paid services, keys in ctx.storage)
  selection      Finder selection paths
  notifications  macOS notification banners
  shell          ctx.shell.run(command, input?, timeoutSecs?) — shell one-liner, stdout/stderr
  automation     ctx.automation.runAppleScript(script), runShortcut(name, input?)
  files          ctx.files.read(path), write(path, content) — under ~ or Caduceus app data
  settings       ctx.settings.get(), ctx.settings.set(fullSettingsObject)
  commands       ctx.commands.dispatch(paletteLine), ctx.commands.runTool(toolId, input)
  ai             ctx.ai.ask(prompt) — primary AI backend, one shot
  shortcuts      ctx.shortcuts.run(shortcutId, query?) — saved Caduceus shortcuts

THE ctx API (complete — no import, require, or ambient fetch)

  ctx.clipboard.read() / write(text)
  ctx.fetch(url, init?)              → Response-like (via Rust)
  ctx.selection()                     → string[] paths
  ctx.notify(text)
  ctx.storage.get(key) / set(key, value)   per-extension JSON store (2 MB cap)
  ctx.open(url)                       http(s) in browser

  ctx.shell.run(command, input?, timeoutSecs?)
  ctx.automation.runAppleScript(script)
  ctx.automation.runShortcut(name, input?)
  ctx.files.read(path) / write(path, content)
  ctx.settings.get() / set(settings)  full Settings tree — customize appearance, shortcuts, AI routes
  ctx.commands.dispatch(input)        same as typing in palette (/, /c, prefixes, …)
  ctx.commands.runTool(toolId, input)   dev tools: sha256, json_format, uuid, jwt_decode, …
  ctx.ai.ask(prompt)
  ctx.shortcuts.run(shortcutId, query?)

CUSTOMIZATION PATTERNS
  • Read settings, tweak appearance.accent / shortcuts / commandCenter.prefixes, ctx.settings.set(...)
  • Call paid APIs with ctx.fetch + network; store API keys in ctx.storage
  • Wrap ctx.commands.dispatch("/ …") or ctx.ai.ask for higher-level flows
  • Use ctx.shell or ctx.automation for macOS apps Raycast-style

RULES
  • Plain modern JavaScript in one file. No TypeScript, no imports, no require.
  • No DOM — runs in a Web Worker.
  • Ask for permissions honestly; undeclared calls fail with a clear error.
  • Prefer returning user-readable strings on failure instead of throwing.
  • Paid APIs and API keys are allowed when the user opts in to network/storage.

Reply with the complete file contents and nothing else.`;
}
