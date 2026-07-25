/**
 * TypeScript mirrors of the Rust types in `src-tauri/src/settings/model.rs`.
 *
 * Rust serialises with `rename_all = "camelCase"` for structs and
 * `rename_all = "snake_case"` for enums, so field names here are camelCase and
 * enum *values* are snake_case. Keep the two files in step: there is no
 * codegen, deliberately — a generated-types build step is one more thing a
 * contributor has to run before their change compiles.
 */

// ---------------------------------------------------------------------------
// Shortcuts
// ---------------------------------------------------------------------------

export type ShortcutKind =
  | "open_url"
  | "open_app"
  | "run_command"
  | "run_applescript"
  | "clipboard_view";

export interface Shortcut {
  id: string;
  label: string;
  /** An emoji, or any short string. Falls back to the first letter of `label`. */
  icon: string;
  kind: ShortcutKind;
  target: string;
  args: string[];
  chromeProfileDirectory: string | null;
  showInOrb: boolean;
  orderIndex: number;
  keywords: string[];
  description: string;
  hidden: boolean;
}

/** The orb draws at most this many pop-out icons. */
export const ORB_POPOUT_LIMIT = 6;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

export type OrbEdge = "left" | "right";

export interface Point {
  x: number;
  y: number;
}

export interface GeneralSettings {
  toggleOrbHotkey: string;
  commandCenterHotkey: string;
  orbVisible: boolean;
  orbEdge: OrbEdge;
  orbPosition: Point | null;
  hoverExpandDelayMs: number;
  collapseIdleMs: number;
  launchAtLogin: boolean;
  cursorPollMs: number;
}

export type PrefixAction =
  | "web_search"
  | "primary_ai"
  | "computer_use"
  | "open_url_template"
  | "run_command"
  | "run_applescript"
  | "clipboard_search";

export interface PrefixRule {
  id: string;
  prefix: string;
  label: string;
  description: string;
  action: PrefixAction;
  target: string;
  chromeProfileDirectory: string | null;
  showHint: boolean;
}

export interface CommandCenterSettings {
  searchUrlTemplate: string;
  prefixes: PrefixRule[];
  defaultChromeProfile: string | null;
  preferChrome: boolean;
  historyLimit: number;
  closeOnAction: boolean;
  maxResultsPerSource: number;
}

export type SttBackendKind = "disabled" | "system_native" | "openai_compatible";
export type KeywordMatch = "leading_words" | "anywhere";
export type RouteTarget =
  | "web_search"
  | "primary_ai"
  | "computer_use"
  | "insert_only"
  | "clipboard_search";

export interface KeywordGroup {
  id: string;
  name: string;
  keywords: string[];
  route: RouteTarget;
  matchMode: KeywordMatch;
  enabled: boolean;
}

export interface VoiceSettings {
  enabled: boolean;
  pushToTalkHotkey: string;
  sttBackend: SttBackendKind;
  sttEndpoint: string;
  sttModel: string;
  sttLanguage: string;
  keywordGroups: KeywordGroup[];
  fallbackRoute: RouteTarget;
  maxRecordingSecs: number;
  autoSubmit: boolean;
}

export type BackendKind = "null" | "openai_compatible" | "anthropic";

export interface BackendConfig {
  id: string;
  displayName: string;
  kind: BackendKind;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
  maxTokens: number;
  temperature: number | null;
  systemPrompt: string;
  supportsComputerUse: boolean;
  /** Version-sensitive strings, editable so a new API release needs no rebuild. */
  anthropicBetaHeader: string;
  computerToolVersion: string;
  enableZoom: boolean;
  extraHeaders: [string, string][];
  timeoutSecs: number;
}

export interface AgentSettings {
  backends: BackendConfig[];
  primaryBackendId: string | null;
  computerUseBackendId: string | null;
  maxSteps: number;
  confirmBeforeFirstAction: boolean;
  screenshotMaxDimension: number;
  actionSettleMs: number;
  monitorIndex: number;
}

export interface ClipboardSettings {
  enabled: boolean;
  maxItems: number;
  maxAgeDays: number | null;
  pollIntervalMs: number;
  captureText: boolean;
  captureImages: boolean;
  captureFiles: boolean;
  maxEntryBytes: number;
  encryptAtRest: boolean;
  excludedApps: string[];
  respectConcealedMarker: boolean;
}

export type Theme = "dark" | "light" | "system";

export interface AppearanceSettings {
  theme: Theme;
  accent: string;
  orbSize: number;
  popoutRadius: number;
  popoutIconSize: number;
  orbIdleOpacity: number;
  reduceTransparency: boolean;
  orbIdleAnimation: boolean;
}

export interface Settings {
  version: number;
  general: GeneralSettings;
  shortcuts: Shortcut[];
  commandCenter: CommandCenterSettings;
  voice: VoiceSettings;
  agents: AgentSettings;
  clipboard: ClipboardSettings;
  appearance: AppearanceSettings;
}

// ---------------------------------------------------------------------------
// Runtime info
// ---------------------------------------------------------------------------

export interface SttAvailability {
  id: string;
  displayName: string;
  available: boolean;
  detail: string;
}

export interface ChromeProfile {
  directory: string;
  name: string;
  email: string | null;
}

export interface ChromeInstall {
  id: string;
  displayName: string;
  launchTarget: string;
  profiles: ChromeProfile[];
}

export interface RuntimeInfo {
  version: string;
  platform: string;
  arch: string;
  keychainAvailable: boolean;
  sttBackends: SttAvailability[];
  chromeInstalls: ChromeInstall[];
  clipboardEntries: number;
  clipboardBytes: number;
  backendsWithKeys: string[];
  suggestedAnthropicModels: string[];
  computerUseNote: string;
}

export interface TransitionReport {
  converted: number;
  skipped: number;
  dropped: number;
}

export interface SettingsApplyReport {
  settings: Settings;
  hotkeyProblems: string[];
  autostartError: string | null;
  encryptionReport: TransitionReport | null;
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

export type EntryKind = "text" | "image" | "files";

export interface ClipboardEntry {
  id: number;
  kind: EntryKind;
  preview: string;
  content: string | null;
  thumbnail: string | null;
  byteLen: number;
  sourceApp: string | null;
  pinned: boolean;
  /** Unix milliseconds. */
  createdAt: number;
  /** True when the row exists but cannot be decrypted. */
  unreadable: boolean;
  width: number | null;
  height: number | null;
}

export interface ClipboardStats {
  entries: number;
  bytes: number;
  encrypted: boolean;
}

// ---------------------------------------------------------------------------
// Execution + dispatch
// ---------------------------------------------------------------------------

export interface ExecOutcome {
  ok: boolean;
  message: string;
  frontendAction: string | null;
  output: string | null;
}

export interface ParsedInput {
  rule: PrefixRule | null;
  remainder: string;
  raw: string;
}

export interface DispatchOutcome {
  ok: boolean;
  message: string;
  action: PrefixAction;
  sessionId: string | null;
  clipboardQuery: string | null;
  closeWindow: boolean;
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

export interface Usage {
  inputTokens: number | null;
  outputTokens: number | null;
}

export interface AgentResponse {
  text: string;
  model: string;
  usage: Usage | null;
}

export type StopReason =
  | "completed"
  | "max_steps"
  | "user_stopped"
  | "declined"
  | "error";

export interface AgentOutcome {
  sessionId: string;
  completed: boolean;
  steps: number;
  finalMessage: string;
  stopReason: StopReason;
  usage: Usage | null;
}

export type AgentStep =
  | { type: "started"; sessionId: string; task: string; backend: string; model: string }
  | { type: "thinking"; text: string }
  | { type: "screenshot"; image: string; width: number; height: number }
  | { type: "action"; index: number; summary: string; raw: unknown }
  | { type: "actionResult"; index: number; ok: boolean; detail: string }
  | { type: "awaitingApproval"; sessionId: string; summary: string }
  | { type: "finished"; outcome: AgentOutcome }
  | { type: "error"; message: string };

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------

export type VoiceState = "idle" | "recording" | "transcribing";

export interface RoutedText {
  route: RouteTarget;
  text: string;
  matchedGroup: string | null;
  matchedKeyword: string | null;
}

export interface VoiceOutcome {
  ok: boolean;
  error: string | null;
  routed: RoutedText | null;
  autoSubmit: boolean;
}

// ---------------------------------------------------------------------------
// Window events
// ---------------------------------------------------------------------------

export interface OrbHoverState {
  hovering: boolean;
  expanded: boolean;
}

export interface CommandCenterOpenPayload {
  prefill: string;
  mode: string;
  selectAll: boolean;
}

/** Event names emitted by the Rust side. */
export const EVENTS = {
  settingsChanged: "orbit://settings-changed",
  clipboardChanged: "orbit://clipboard-changed",
  agentStep: "orbit://agent-step",
  orbHover: "orbit://orb-hover",
  commandCenterOpen: "orbit://command-center-open",
  settingsTab: "orbit://settings-tab",
  voiceState: "orbit://voice-state",
  voiceResult: "orbit://voice-result",
  hotkeyProblems: "orbit://hotkey-problems",
} as const;
