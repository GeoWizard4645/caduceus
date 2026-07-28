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
  | "open_feature"
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

/** First-run quiz answers — stored locally, never sent anywhere. */
export interface PersonalizationProfile {
  isDeveloper: boolean;
  /** launcher | clipboard | windows | system | ai | developer */
  primaryFocus: string;
  favoriteCommandIds: string[];
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
  /** False until the three-question personalization quiz is done. */
  onboardingQuizDone: boolean;
  personalization: PersonalizationProfile;
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
  /** Master switch for smart routing; false always uses the primary backend. */
  autoRoutingEnabled: boolean;
  /** A hand-picked backend that always beats the classifier. */
  routingOverrideBackendId: string | null;
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
  /** Empty for none; `image:command-center-background.png` once chosen. */
  commandCenterBackground: string;
  /** 0–1. Low by default: decoration must not cost readability. */
  backgroundOpacity: number;
  /** Pixels, 0–40. What makes an arbitrary photograph usable behind text. */
  backgroundBlur: number;
  /** Command Center corner radius, 0–28px. */
  windowRadius: number;
  /** Command Center font scale, 0.85–1.4. */
  uiScale: number;
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

export interface ProcessGroupRow {
  name: string;
  cpu: number;
  memoryBytes: number;
  own: boolean;
  rootPid: number | null;
  processes: ProcessRow[];
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
  processGroups: ProcessGroupRow[];
  processes: ProcessRow[];
  /** Total before `limit` was applied, so the UI can say "40 of 612". */
  processTotal: number;
}

export interface UpdateCheck {
  currentVersion: string;
  updateAvailable: boolean;
  latestVersion: string | null;
  releaseUrl: string | null;
  downloadUrl: string | null;
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

export type VoiceState = "idle" | "recording" | "paused" | "transcribing";

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
  staffMarkChanged: "caduceus://staff-mark-changed",
  /** Rust asking the shell to open (or focus) a tab. */
  tabOpen: "caduceus://tab-open",
  updateAvailable: "caduceus://update-available",
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

/**
 * What "Highlight & Act" can do to a selection.
 *
 * Mirrors `tools::textai::TextAiAction` (serde `snake_case`). The frontend
 * names an action rather than sending a prompt, so the wording of every prompt
 * stays on the Rust side where it can be tested — and so a compromised webview
 * cannot ask the user's model anything it likes.
 */
export type TextAiAction =
  | "summarize"
  | "rewrite_professional"
  | "rewrite_friendly"
  | "rewrite_concise"
  | "rewrite_diplomatic"
  | "fix_grammar"
  | "explain_simply"
  | "translate"
  | "reply_politely"
  | "bullet_point"
  | "generate_title";

/**
 * The second bench of developer tools.
 *
 * Mirrors `tools::devextra::ExtraToolId`. Separate from {@link ToolId} because
 * these are a different bench, not more of the same — and because one enum of
 * fifty variants had already become the hardest thing in that file to read.
 */
export type ExtraToolId =
  | "yaml_format"
  | "yaml_validate"
  | "xml_format"
  | "xml_validate"
  | "html_entity_encode"
  | "html_entity_decode"
  | "sql_format"
  | "hosts_view";

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

export interface UninstallSnapshot {
  extensions: Extension[];
  ollamaModels: string[];
  caduceusAppInstalled: boolean;
  ollamaInstalled: boolean;
  hermesInstalled: boolean;
}

export interface UninstallRequest {
  extensionIds: string[];
  caduceus: boolean;
  hermes: boolean;
  ollama: boolean;
  ollamaModels: string[];
}

export interface UninstallResult {
  ok: boolean;
  messages: string[];
  quitApp: boolean;
}

/**
 * One `ctx.fetch` call, on its way to Rust.
 *
 * Headers are pairs rather than a record because a response may repeat one
 * (`set-cookie`), and a shape that silently drops the repeats is worse than a
 * slightly less convenient one. Mirrors `extensions::net::FetchRequest`.
 */
export interface ExtensionFetchRequest {
  url: string;
  method?: string;
  headers?: [string, string][];
  body?: string | null;
}

export interface ExtensionFetchResponse {
  ok: boolean;
  status: number;
  statusText: string;
  url: string;
  headers: [string, string][];
  body: string;
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
  | "center_half"
  | "top_left_quarter"
  | "top_right_quarter"
  | "bottom_left_quarter"
  | "bottom_right_quarter"
  | "first_third"
  | "center_third"
  | "last_third"
  | "top_third"
  | "bottom_third"
  | "first_two_thirds"
  | "last_two_thirds"
  | "top_two_thirds"
  | "bottom_two_thirds"
  | "center_two_thirds"
  | "top_left_sixth"
  | "top_center_sixth"
  | "top_right_sixth"
  | "bottom_left_sixth"
  | "bottom_center_sixth"
  | "bottom_right_sixth"
  | "first_fourth"
  | "second_fourth"
  | "third_fourth"
  | "last_fourth"
  | "first_three_fourths"
  | "last_three_fourths"
  | "top_three_fourths"
  | "bottom_three_fourths"
  | "center_three_fourths"
  | "maximize_height"
  | "maximize_width"
  | "maximize"
  | "almost_maximize"
  | "reasonable_size"
  | "center"
  | "larger"
  | "smaller"
  | "move_up"
  | "move_down"
  | "move_left"
  | "move_right"
  | "restore"
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
// Desktop icon shapes
// ---------------------------------------------------------------------------

/** Mirrors `tools::shapes::Shape`. */
export type DesktopShape = "circle" | "grid" | "heart" | "line" | "spiral";

/** One icon's centre, in points from the top-left of the main display. */
export interface DesktopSpot {
  name: string;
  x: number;
  y: number;
}

/** Mirrors `tools::shapes::Arrangement` — Finder's Sort By for the Desktop. */
export interface DesktopArrangement {
  label: string;
  /** Finder discards explicit positions in this mode, so arranging is futile. */
  blocks: boolean;
  /** Positions are honoured, then pulled onto the icon grid. */
  snaps: boolean;
  /** The menu that turns it off. Empty when there is nothing to turn off. */
  fix: string;
}

export interface DesktopShapePlan {
  shape: DesktopShape;
  /** The rectangle the shape is drawn in, for the preview to scale to. */
  area: { x: number; y: number; width: number; height: number };
  spots: DesktopSpot[];
  current: DesktopSpot[];
  arrangement: DesktopArrangement;
}

export interface DesktopShapeResult {
  ok: boolean;
  message: string;
  /** Where every icon was before this ran. Feed it back to undo. */
  previous: DesktopSpot[];
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
  | "volume_zero"
  | "volume_quarter"
  | "volume_half"
  | "volume_three_quarters"
  | "volume_full"
  | "brightness_up"
  | "brightness_down"
  | "toggle_wifi"
  | "toggle_bluetooth"
  | "show_desktop"
  | "hide_others"
  | "unhide_all"
  | "quit_all_apps"
  | "quit_others"
  | "open_camera"
  | "open_trash";

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
