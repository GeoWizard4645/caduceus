/**
 * A fake Tauri IPC layer, for developing the UI without building Rust.
 *
 * `npm run ui` serves `preview.html`, which installs this before importing any
 * component. Every `invoke` is answered from in-memory fixtures, so the orb, the
 * Command Center and all seven Settings tabs render and respond exactly as they
 * do in the real app — you just cannot open a browser or drive a mouse.
 *
 * This is a **development tool**, not part of the shipped app: nothing in
 * `src/orb`, `src/command-center` or `src/settings` imports it.
 */

import type { ClipboardEntry, RuntimeInfo, Settings } from "@/shared/types";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

export const mockSettings: Settings = {
  version: 1,
  general: {
    toggleOrbHotkey: "F12",
    commandCenterHotkey: "Alt+Space",
    orbVisible: true,
    orbEdge: "right",
    orbPosition: null,
    hoverExpandDelayMs: 0,
    collapseIdleMs: 3000,
    launchAtLogin: false,
    cursorPollMs: 33,
  },
  shortcuts: [
    { id: "sc-gemini", label: "Gemini", icon: "✧", kind: "open_url", target: "https://gemini.google.com/app", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 0, keywords: ["google", "ai"], description: "Open Gemini in your browser", hidden: false },
    { id: "sc-gmail", label: "Gmail", icon: "✉", kind: "open_url", target: "https://mail.google.com", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 1, keywords: ["mail", "inbox"], description: "Open your inbox", hidden: false },
    { id: "sc-chrome", label: "Chrome", icon: "◎", kind: "open_app", target: "com.google.Chrome", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 2, keywords: ["browser"], description: "Launch the browser", hidden: false },
    { id: "sc-claude", label: "Claude", icon: "✳", kind: "open_url", target: "https://claude.ai", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 3, keywords: ["anthropic", "ai"], description: "Open Claude in your browser", hidden: false },
    { id: "sc-dictation", label: "Dictation App", icon: "◍", kind: "open_app", target: "", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 4, keywords: ["voice"], description: "Set this to your dictation app in Settings → Shortcuts", hidden: false },
    { id: "sc-clipboard", label: "Clipboard", icon: "❐", kind: "clipboard_view", target: "", args: [], chromeProfileDirectory: null, showInOrb: true, orderIndex: 5, keywords: ["history", "paste"], description: "Browse everything you have copied", hidden: false },
  ],
  commandCenter: {
    searchUrlTemplate: "https://www.google.com/search?q={query}",
    prefixes: [
      { id: "prefix-ai", prefix: "/", label: "Ask AI", description: "Send the rest of the line to your primary AI backend", action: "primary_ai", target: "", chromeProfileDirectory: null, showHint: true },
      { id: "prefix-computer", prefix: "/c", label: "Computer use", description: "Let an agent drive your screen to complete the task", action: "computer_use", target: "", chromeProfileDirectory: null, showHint: true },
      { id: "prefix-clipboard", prefix: "/v", label: "Clipboard", description: "Search your clipboard history", action: "clipboard_search", target: "", chromeProfileDirectory: null, showHint: true },
    ],
    defaultChromeProfile: null,
    preferChrome: false,
    historyLimit: 100,
    closeOnAction: true,
    maxResultsPerSource: 8,
  },
  voice: {
    enabled: false,
    pushToTalkHotkey: "CommandOrControl+Shift+Space",
    sttBackend: "system_native",
    sttEndpoint: "http://127.0.0.1:8080/v1/audio/transcriptions",
    sttModel: "whisper-1",
    sttLanguage: "",
    keywordGroups: [
      { id: "kw-search", name: "Web search", keywords: ["search", "look up", "browse"], route: "web_search", matchMode: "leading_words", enabled: true },
      { id: "kw-computer", name: "Computer use", keywords: ["computer", "jarvis", "search my mac"], route: "computer_use", matchMode: "leading_words", enabled: true },
    ],
    fallbackRoute: "primary_ai",
    maxRecordingSecs: 60,
    autoSubmit: false,
  },
  agents: {
    backends: [
      { id: "null", displayName: "Not configured", kind: "null", baseUrl: "", model: "", hasApiKey: false, maxTokens: 4096, temperature: null, systemPrompt: "", supportsComputerUse: false, anthropicBetaHeader: "computer-use-2025-11-24", computerToolVersion: "computer_20251124", enableZoom: true, extraHeaders: [], timeoutSecs: 120 },
    ],
    primaryBackendId: "null",
    computerUseBackendId: null,
    maxSteps: 25,
    confirmBeforeFirstAction: true,
    screenshotMaxDimension: 1280,
    actionSettleMs: 350,
    monitorIndex: 0,
  },
  clipboard: {
    enabled: true,
    maxItems: 500,
    maxAgeDays: 30,
    pollIntervalMs: 700,
    captureText: true,
    captureImages: true,
    captureFiles: true,
    maxEntryBytes: 8 * 1024 * 1024,
    encryptAtRest: false,
    excludedApps: ["1Password", "Bitwarden", "KeePassXC", "Proton Pass"],
    respectConcealedMarker: true,
  },
  appearance: {
    theme: "dark",
    accent: "#7c7cff",
    orbSize: 56,
    popoutRadius: 96,
    popoutIconSize: 38,
    orbIdleOpacity: 0.9,
    reduceTransparency: false,
    orbIdleAnimation: true,
  },
};

const now = Date.now();

const mockClipboard: ClipboardEntry[] = [
  { id: 5, kind: "text", preview: "https://github.com/tauri-apps/tauri/releases", content: "https://github.com/tauri-apps/tauri/releases", thumbnail: null, byteLen: 44, sourceApp: "Safari", pinned: true, createdAt: now - 40_000, unreadable: false, width: null, height: null },
  { id: 4, kind: "text", preview: "SELECT id, seq FROM entries ORDER BY seq DESC LIMIT 10;", content: "SELECT id, seq FROM entries ORDER BY seq DESC LIMIT 10;", thumbnail: null, byteLen: 55, sourceApp: "Terminal", pinned: false, createdAt: now - 400_000, unreadable: false, width: null, height: null },
  { id: 3, kind: "files", preview: "/Users/you/Documents/quarterly-report.pdf", content: "/Users/you/Documents/quarterly-report.pdf", thumbnail: null, byteLen: 41, sourceApp: "Finder", pinned: false, createdAt: now - 3_600_000, unreadable: false, width: null, height: null },
  { id: 2, kind: "text", preview: "The quick brown fox jumps over the lazy dog, and then keeps going for quite a while so that this preview needs to be truncated somewhere sensible.", content: "…", thumbnail: null, byteLen: 148, sourceApp: "Notes", pinned: false, createdAt: now - 86_400_000, unreadable: false, width: null, height: null },
  { id: 1, kind: "text", preview: "cargo test --lib", content: "cargo test --lib", thumbnail: null, byteLen: 16, sourceApp: "Terminal", pinned: false, createdAt: now - 172_800_000, unreadable: false, width: null, height: null },
];

const mockRuntimeInfo: RuntimeInfo = {
  version: "0.1.0-preview",
  platform: "macos",
  arch: "aarch64",
  keychainAvailable: true,
  sttBackends: [
    { id: "system_native", displayName: "System (on-device)", available: true, detail: "Uses Apple's Speech framework. Runs on-device when the language pack is installed, so audio never leaves your Mac." },
    { id: "openai_compatible", displayName: "HTTP endpoint (Whisper-compatible)", available: true, detail: "Posts your recording to http://127.0.0.1:8080/v1/audio/transcriptions." },
    { id: "disabled", displayName: "Off", available: true, detail: "Push-to-talk does nothing." },
  ],
  chromeInstalls: [
    { id: "chrome", displayName: "Google Chrome", launchTarget: "com.google.Chrome", profiles: [
      { directory: "Default", name: "Personal", email: "you@example.com" },
      { directory: "Profile 1", name: "Work", email: "you@work.example" },
    ] },
  ],
  clipboardEntries: mockClipboard.length,
  clipboardBytes: 2_411_520,
  backendsWithKeys: [],
  suggestedAnthropicModels: ["claude-opus-5", "claude-sonnet-5", "claude-opus-4-8", "claude-haiku-4-5"],
  computerUseNote:
    "macOS will ask for Screen Recording and Accessibility permission the first time an agent runs. Orbit never requests them at launch.",
};

// ---------------------------------------------------------------------------
// The fake IPC bridge
// ---------------------------------------------------------------------------

type Listener = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Set<Listener>>();
const callbacks = new Map<number, Listener>();
let nextCallbackId = 1;
let settings: Settings = structuredClone(mockSettings);

/** Push an event to anything listening, the way the Rust side would. */
export function emitMock(event: string, payload: unknown): void {
  for (const listener of listeners.get(event) ?? []) {
    listener({ event, id: 0, payload });
  }
}

function handle(command: string, args: Record<string, unknown>): unknown {
  switch (command) {
    case "get_settings":
      return settings;

    case "update_settings":
      settings = args.next as Settings;
      emitMock("orbit://settings-changed", settings);
      return { settings, hotkeyProblems: [], autostartError: null, encryptionReport: null };

    case "reset_settings":
      settings = structuredClone(mockSettings);
      emitMock("orbit://settings-changed", settings);
      return settings;

    case "get_runtime_info":
      return { ...mockRuntimeInfo, clipboardEntries: mockClipboard.length };

    case "clipboard_list": {
      const query = String(args.query ?? "").toLowerCase();
      const tokens = query.split(/\s+/).filter(Boolean);
      return mockClipboard.filter((e) =>
        tokens.every((t) => e.preview.toLowerCase().includes(t)),
      );
    }

    case "clipboard_stats":
      return { entries: mockClipboard.length, bytes: 2_411_520, encrypted: settings.clipboard.encryptAtRest };

    case "parse_input": {
      const raw = String(args.input ?? "");
      const trimmed = raw.trimStart();
      let best: { rule: Settings["commandCenter"]["prefixes"][number]; len: number } | null = null;
      for (const rule of settings.commandCenter.prefixes) {
        const prefix = rule.prefix.trim();
        if (!prefix || !trimmed.startsWith(prefix)) continue;
        const rest = trimmed.slice(prefix.length);
        const needsBoundary = /[a-z0-9]$/i.test(prefix);
        if (needsBoundary && rest !== "" && !/^\s/.test(rest)) continue;
        if (!best || prefix.length > best.len) best = { rule, len: prefix.length };
      }
      return best
        ? { rule: best.rule, remainder: trimmed.slice(best.len).trim(), raw }
        : { rule: null, remainder: trimmed.trim(), raw };
    }

    case "dispatch_input":
      return {
        ok: true,
        message: "Preview mode — nothing actually ran.",
        action: "web_search",
        sessionId: null,
        clipboardQuery: null,
        closeWindow: false,
      };

    case "run_shortcut":
      return { ok: true, message: "Preview mode — nothing actually ran.", frontendAction: null, output: null };

    case "agent_backend_templates":
      return [
        { ...settings.agents.backends[0], id: `b${nextCallbackId++}`, kind: "openai_compatible", displayName: "Local model", baseUrl: "http://localhost:11434/v1", model: "llama3.2", maxTokens: 2048 },
        { ...settings.agents.backends[0], id: `b${nextCallbackId++}`, kind: "anthropic", displayName: "Claude", baseUrl: "https://api.anthropic.com", model: "claude-opus-5", supportsComputerUse: true },
      ];

    case "agent_test_backend":
      throw "Preview mode — no real backend to reach.";

    case "agent_list_models":
      return ["llama3.2", "qwen2.5-coder", "mistral-nemo"];

    case "list_chrome_profiles":
      return mockRuntimeInfo.chromeInstalls;

    case "validate_hotkey":
      return String(args.accelerator ?? "");

    // Event plumbing.
    case "plugin:event|listen": {
      const event = String(args.event);
      const handler = callbacks.get(Number(args.handler));
      if (handler) {
        if (!listeners.has(event)) listeners.set(event, new Set());
        listeners.get(event)!.add(handler);
      }
      return nextCallbackId++;
    }
    case "plugin:event|unlisten":
      return null;

    default:
      // Everything else is a no-op in preview.
      return null;
  }
}

/** Install the mock. Must run before any component imports `@tauri-apps/api`. */
export function installMockTauri(): void {
  const w = window as unknown as Record<string, unknown>;

  w.__TAURI_INTERNALS__ = {
    transformCallback(callback: Listener) {
      const id = nextCallbackId++;
      callbacks.set(id, callback);
      return id;
    },
    async invoke(command: string, args: Record<string, unknown> = {}) {
      // A small delay keeps loading states honest — instant resolution hides
      // spinner bugs that would show up in the real app.
      await new Promise((resolve) => setTimeout(resolve, 12));
      return handle(command, args ?? {});
    },
    metadata: { currentWindow: { label: "preview" }, currentWebview: { label: "preview" } },
    plugins: {},
  };
}
