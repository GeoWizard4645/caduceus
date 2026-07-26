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

/**
 * Which browser a URL opens in.
 *
 * An empty `browserId` means the OS default browser. `profile` is a Chromium
 * `--profile-directory` value and is ignored by browsers without one.
 */
export interface BrowserChoice {
  browserId: string;
  profile: string | null;
}

export type ShortcutKind =
  | "open_url"
  | "open_app"
  | "run_command"
  | "run_applescript"
  | "clipboard_view"
  | "system_monitor";

export interface Shortcut {
  id: string;
  label: string;
  /** An emoji, or any short string. Falls back to the first letter of `label`. */
  icon: string;
  kind: ShortcutKind;
  target: string;
  args: string[];
  /** Per-shortcut browser override; null uses the Command Center default. */
  browser: BrowserChoice | null;
  showInStaff: boolean;
  orderIndex: number;
  keywords: string[];
  description: string;
  hidden: boolean;
}

/** The staff draws at most this many pop-out icons. */
export const STAFF_POPOUT_LIMIT = 6;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

export type StaffEdge = "left" | "right";

export interface Point {
  x: number;
  y: number;
}

export interface GeneralSettings {
  toggleOrbHotkey: string;
  commandCenterHotkey: string;
  staffVisible: boolean;
  staffEdge: StaffEdge;
  staffPosition: Point | null;
  hoverExpandDelayMs: number;
  collapseIdleMs: number;
  launchAtLogin: boolean;
  /** False until the first-run walkthrough is finished or skipped. */
  onboardingDone: boolean;
  cursorPollMs: number;
  functionKeys: FunctionKeyBinding[];
}

export type FunctionKeyAction =
  | "none"
  | "toggle_staff"
  | "command_center"
  | "push_to_talk"
  | "start_dictation"
  | "voice_memo"
  | "screenshot"
  | "run_shortcut";

export interface FunctionKeyBinding {
  key: string;
  action: FunctionKeyAction;
  shortcutId: string;
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
  /** Per-prefix browser override; null uses the Command Center default. */
  browser: BrowserChoice | null;
  showHint: boolean;
}

export interface CommandCenterSettings {
  searchUrlTemplate: string;
  prefixes: PrefixRule[];
  browser: BrowserChoice;
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

export type BackendKind = "null" | "openai_compatible" | "hermes";

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
  extraHeaders: [string, string][];
  timeoutSecs: number;
}

export interface AgentSettings {
  backends: BackendConfig[];
  primaryBackendId: string | null;
  computerUseBackendId: string | null;
  confirmBeforeFirstAction: boolean;
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
  staffSize: number;
  popoutRadius: number;
  popoutIconSize: number;
  staffIdleOpacity: number;
  reduceTransparency: boolean;
  staffIdleAnimation: boolean;
  staffMarkIcon: string;
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

export interface BrowserProfile {
  directory: string;
  name: string;
  email: string | null;
}

export interface BrowserInstall {
  id: string;
  displayName: string;
  launchTarget: string;
  /** Whether `--profile-directory` is supported; false for Safari/Firefox. */
  chromium: boolean;
  profiles: BrowserProfile[];
}

/** One AI runtime the "Configure AI" scan probed for. */
export interface DetectedProvider {
  id: string;
  displayName: string;
  baseUrl: string;
  /** The server answered. Not-running entries are still returned, with a hint. */
  running: boolean;
  models: string[];
  detail: string;
}

export interface LocalAiScan {
  providers: DetectedProvider[];
  hermes: HermesStatus;
}

// ---------------------------------------------------------------------------
// System monitor
// ---------------------------------------------------------------------------

export interface ProcessRow {
  pid: number;
  name: string;
  /** Percent of one core, so >100 is normal for a threaded process. */
  cpu: number;
  memoryBytes: number;
  /** Ours to kill. Anything else needs privileges Caduceus does not have. */
  own: boolean;
}

export interface DiskRow {
  name: string;
  mountPoint: string;
  totalBytes: number;
  availableBytes: number;
}

export interface SystemSnapshot {
  cpuPercent: number;
  coreCount: number;
  memoryUsedBytes: number;
  memoryTotalBytes: number;
  swapUsedBytes: number;
  swapTotalBytes: number;
  netDownBytes: number;
  netUpBytes: number;
  uptimeSecs: number;
  loadAverage: [number, number, number];
  hostName: string | null;
  osVersion: string | null;
  kernelVersion: string | null;
  disks: DiskRow[];
  processes: ProcessRow[];
  /** Total before `limit` was applied, so the UI can say "40 of 612". */
  processTotal: number;
}

export interface RuntimeInfo {
  version: string;
  platform: string;
  arch: string;
  keychainAvailable: boolean;
  sttBackends: SttAvailability[];
  browsers: BrowserInstall[];
  clipboardEntries: number;
  clipboardBytes: number;
  backendsWithKeys: string[];
  computerUseNote: string;
  hermes: HermesStatus;
}

/** State of the local Hermes Agent installation. */
export interface HermesStatus {
  installed: boolean;
  path: string | null;
  version: string | null;
  model: string | null;
  provider: string | null;
  configured: boolean;
  /** One-line, actionable summary for the UI. */
  detail: string;
}

/** An installed application, for the launcher. */
export interface InstalledApp {
  name: string;
  path: string;
}

/** A successfully evaluated arithmetic expression. */
export interface CalcResult {
  expression: string;
  display: string;
  value: number;
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

export interface StaffHoverState {
  hovering: boolean;
  expanded: boolean;
}

export interface CommandCenterOpenPayload {
  prefill: string;
  mode: string;
  selectAll: boolean;
  /** "hotkey" | "staff" | "tray" | "other". */
  source: string;
}

/** Event names emitted by the Rust side. */
export const EVENTS = {
  settingsChanged: "caduceus://settings-changed",
  clipboardChanged: "caduceus://clipboard-changed",
  agentStep: "caduceus://agent-step",
  staffHover: "caduceus://staff-hover",
  commandCenterOpen: "caduceus://command-center-open",
  commandCenterShown: "caduceus://command-center-shown",
  settingsTab: "caduceus://settings-tab",
  voiceState: "caduceus://voice-state",
  voicePartial: "caduceus://voice-partial",
  voiceResult: "caduceus://voice-result",
  hotkeyProblems: "caduceus://hotkey-problems",
} as const;
