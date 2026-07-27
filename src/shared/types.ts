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
  /**
   * The version that last started with this settings file.
   *
   * Written by Rust at startup and round-tripped untouched. It is in this type
   * so that saving settings from the UI does not drop it — a missing value
   * reads as "new install" and would put the staff back on screen every time
   * anyone changed a checkbox.
   */
  lastLaunchedVersion: string | null;
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
  /** Set for `primary_ai`: the thread the reply was saved into. */
  conversationId: number | null;
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
  chatChanged: "caduceus://chat-changed",
  /** Rust asking the shell to open (or focus) a tab. */
  tabOpen: "caduceus://tab-open",
} as const;

// --- keep-awake sessions -----------------------------------------------------

export interface AwakeStatus {
  active: boolean;
  /** Seconds remaining, or null for an indefinite session. */
  remainingSecs: number | null;
  totalSecs: number | null;
  displayMaySleep: boolean;
}

// --- chat --------------------------------------------------------------------

export type ChatRole = "user" | "assistant";

export interface ChatMessage {
  id: number;
  role: ChatRole;
  text: string;
  /** Unix seconds. */
  createdAt: number;
}

export interface Conversation {
  id: number;
  title: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
  /** First line of the most recent turn, for the thread list. */
  preview: string;
}

export interface ChatReply {
  conversationId: number;
  text: string;
}

// --- extensions --------------------------------------------------------------

export interface Extension {
  id: string;
  name: string;
  description: string;
  author: string;
  permissions: string[];
  path: string;
  enabled: boolean;
}

export interface InstallReport {
  ok: boolean;
  message: string;
  extension: Extension | null;
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

/** Every arrangement `window_action` accepts. Mirrors `window::manage::Verb`. */
export type WindowVerb =
  | "left_half"
  | "right_half"
  | "top_half"
  | "bottom_half"
  | "top_left_quarter"
  | "top_right_quarter"
  | "bottom_left_quarter"
  | "bottom_right_quarter"
  | "first_third"
  | "center_third"
  | "last_third"
  | "first_two_thirds"
  | "last_two_thirds"
  | "maximize"
  | "almost_maximize"
  | "reasonable_size"
  | "center"
  | "larger"
  | "smaller"
  | "next_display"
  | "previous_display"
  | "toggle_full_screen";

export interface WindowOutcome {
  ok: boolean;
  message: string;
  /** Set when the only thing missing is the Accessibility grant. */
  needsPermission: boolean;
}

// ---------------------------------------------------------------------------
// Developer toolbox
// ---------------------------------------------------------------------------

/** Mirrors `tools::dev::ToolId`. */
export type ToolId =
  | "uuid"
  | "uuid_batch"
  | "ulid"
  | "nano_id"
  | "password"
  | "base64_encode"
  | "base64_decode"
  | "base64_url_encode"
  | "base64_url_decode"
  | "hex_encode"
  | "hex_decode"
  | "url_encode"
  | "url_decode"
  | "html_encode"
  | "html_decode"
  | "jwt_decode"
  | "json_format"
  | "json_minify"
  | "json_escape"
  | "timestamp_now"
  | "timestamp_convert"
  | "lorem"
  | "slugify"
  | "text_stats"
  | "sort_lines"
  | "sort_lines_descending"
  | "dedupe_lines"
  | "reverse_lines"
  | "shuffle_lines"
  | "number_lines"
  | "join_lines"
  | "trim_lines"
  | "count_occurrences"
  | "color_convert"
  | "number_base"
  | "random_number"
  | "md5"
  | "sha1"
  | "sha256"
  | "sha512";

export interface ToolResult {
  ok: boolean;
  title: string;
  output: string;
  message: string;
  /** Whether the palette should copy `output` without being asked. */
  autoCopy: boolean;
}

/** The shape every one-shot utility returns. Mirrors `tools::ToolOutcome`. */
export interface ToolOutcome {
  ok: boolean;
  message: string;
  /** Text the caller should put on the clipboard, if any. */
  copied: string | null;
}

// ---------------------------------------------------------------------------
// System controls
// ---------------------------------------------------------------------------

/** Mirrors `tools::system::SystemAction`. */
export type SystemAction =
  | "toggle_dark_mode"
  | "toggle_stage_manager"
  | "toggle_hidden_files"
  | "toggle_desktop_icons"
  | "restart_finder"
  | "restart_dock"
  | "restart_menu_bar"
  | "empty_trash"
  | "lock_screen"
  | "sleep_display"
  | "sleep_computer"
  | "start_screen_saver"
  | "log_out"
  | "restart_computer"
  | "shut_down"
  | "volume_up"
  | "volume_down"
  | "toggle_mute"
  | "brightness_up"
  | "brightness_down"
  | "toggle_wifi";

export type MediaAction = "play_pause" | "next" | "previous" | "now_playing";

export interface PermissionReport {
  accessibility: boolean;
  screenRecording: boolean;
  nativeHelper: boolean;
}

export interface AudioDevice {
  uid: string;
  name: string;
  isInput: boolean;
  isOutput: boolean;
  isDefaultInput: boolean;
  isDefaultOutput: boolean;
}

// ---------------------------------------------------------------------------
// Developer environment
// ---------------------------------------------------------------------------

export interface PortUser {
  port: number;
  pid: number;
  process: string;
}

export interface GitRepo {
  name: string;
  path: string;
  branch: string;
  dirty: number | null;
}

export interface SshHost {
  alias: string;
  hostname: string;
  user: string;
}

export interface Container {
  id: string;
  name: string;
  image: string;
  status: string;
  running: boolean;
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

export interface BigFile {
  path: string;
  name: string;
  bytes: number;
  size: string;
}

export interface Leftover {
  path: string;
  bytes: number;
  size: string;
}

export interface FileHit {
  path: string;
  name: string;
}
