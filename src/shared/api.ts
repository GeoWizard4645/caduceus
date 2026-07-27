/**
 * Typed wrappers over Tauri's `invoke`.
 *
 * Every call into Rust goes through this file, so there is exactly one place to
 * look for "what can the UI actually ask the backend to do", and one place
 * where the command-name strings live.
 */

import { invoke } from "@tauri-apps/api/core";

import type {
  AgentResponse,
  CalcResult,
  HermesStatus,
  LocalAiScan,
  SystemSnapshot,
  InstalledApp,
  BackendConfig,
  BrowserInstall,
  ClipboardEntry,
  ClipboardStats,
  DesktopShape,
  DesktopShapePlan,
  DesktopShapeResult,
  DesktopSpot,
  DispatchOutcome,
  ExecOutcome,
  ChatMessage,
  ChatReply,
  Extension,
  InstallReport,
  Conversation,
  ParsedInput,
  RoutedText,
  RuntimeInfo,
  Settings,
  SettingsApplyReport,
  AudioDevice,
  BigFile,
  Container,
  FileHit,
  GitRepo,
  Leftover,
  MediaAction,
  PermissionReport,
  PortUser,
  SshHost,
  SystemAction,
  ToolId,
  ToolOutcome,
  ToolResult,
  WindowOutcome,
  WindowVerb,
} from "./types";

// --- settings --------------------------------------------------------------

export const getSettings = () => invoke<Settings>("get_settings");

export const updateSettings = (next: Settings) =>
  invoke<SettingsApplyReport>("update_settings", { next });

export const resetSettings = () => invoke<Settings>("reset_settings");

export const getRuntimeInfo = () => invoke<RuntimeInfo>("get_runtime_info");

export const validateHotkey = (accelerator: string) =>
  invoke<string>("validate_hotkey", { accelerator });

// --- secrets ---------------------------------------------------------------
// Write-only by design: there is no command to read a key back out, so a
// compromised webview cannot exfiltrate one.

export const setBackendApiKey = (backendId: string, key: string) =>
  invoke<boolean>("set_backend_api_key", { backendId, key });

export const deleteBackendApiKey = (backendId: string) =>
  invoke<void>("delete_backend_api_key", { backendId });

export const setSttApiKey = (key: string) => invoke<boolean>("set_stt_api_key", { key });

// --- shortcuts -------------------------------------------------------------

export const runShortcut = (id: string, query?: string) =>
  invoke<ExecOutcome>("run_shortcut", { id, query: query ?? null });

export const listBrowsers = () => invoke<BrowserInstall[]>("list_browsers");

export const testCommand = (command: string) => invoke<ExecOutcome>("test_command", { command });

export const openExternalUrl = (url: string) => invoke<ExecOutcome>("open_external_url", { url });

/**
 * The System Settings panes the Learn tab can send you to.
 *
 * A closed set, not a URL: `open_external_url` refuses non-http schemes on
 * purpose, so these are named on the Rust side instead of opened by string.
 */
export type SystemSettingsPane =
  | "keyboard-shortcuts"
  | "microphone"
  | "accessibility"
  | "screen-recording"
  | "speech-recognition"
  | "automation"
  | "login-items";

export const openSystemSettings = (pane: SystemSettingsPane) =>
  invoke<ExecOutcome>("open_system_settings", { pane });

// --- command center --------------------------------------------------------

export const parseInput = (input: string) => invoke<ParsedInput>("parse_input", { input });

export const dispatchInput = (input: string) =>
  invoke<DispatchOutcome>("dispatch_input", { input });

export const hideCommandCenter = () => invoke<void>("hide_command_center");

export const openCommandCenter = (mode?: string, prefill?: string, source?: string) =>
  invoke<void>("open_command_center", {
    mode: mode ?? null,
    prefill: prefill ?? null,
    // Only the first-run walkthrough reads this — it needs to tell a click on
    // the staff apart from the keyboard shortcut.
    source: source ?? null,
  });

export const openSettingsWindow = (tab?: string) =>
  invoke<void>("open_settings_window", { tab: tab ?? null });

// --- staff -------------------------------------------------------------------

export const toggleStaff = () => invoke<boolean>("toggle_staff");

/**
 * Hold the *whole* staff window clickable.
 *
 * Only for gestures that own the pointer until they end, like a resize drag.
 * Anything that is merely on screen should use {@link setStaffCaptureRect}, or
 * it makes a window-sized square of the desktop unclickable for as long as it
 * is up.
 */
export const setStaffInteractive = (interactive: boolean) =>
  invoke<void>("set_staff_interactive", { interactive });

/**
 * Let one region of the staff window capture the pointer; `null` clears it.
 *
 * Pass a `getBoundingClientRect()` straight through — the coordinates are
 * logical pixels relative to the window's top-left, which is what that returns.
 */
export const setStaffCaptureRect = (
  rect: { x: number; y: number; width: number; height: number } | null,
) => invoke<void>("set_staff_capture_rect", { rect });

export const saveStaffPosition = () => invoke<void>("save_staff_position");
export const refreshStaffLayout = () => invoke<void>("refresh_staff_layout");

export const collapseStaffPopout = () => invoke<void>("collapse_staff_popout");

export const resolveShortcutIcon = (icon: string) =>
  invoke<string | null>("resolve_shortcut_icon", { icon });

export const resolveStaffMark = (icon: string) =>
  invoke<string | null>("resolve_staff_mark", { icon });

export const importStaffMark = (sourcePath: string) =>
  invoke<string>("import_staff_mark", { sourcePath });

export const clearStaffMark = () => invoke<void>("clear_staff_mark");

/** Where the Command Center's background image lives, if one has been chosen. */
export const resolveBackdrop = (token: string) =>
  invoke<string | null>("resolve_backdrop", { token });

export const importBackdrop = (sourcePath: string) =>
  invoke<string>("import_backdrop", { sourcePath });

export const clearBackdrop = () => invoke<void>("clear_backdrop");

export const importShortcutIcon = (shortcutId: string, sourcePath: string) =>
  invoke<string>("import_shortcut_icon", { shortcutId, sourcePath });

// --- capture ---------------------------------------------------------------

export interface ScreenshotResult {
  ok: boolean;
  path: string | null;
  message: string;
}

export interface RecordingState {
  active: boolean;
  path: string | null;
  message: string;
}

export const captureScreenshot = (saveToDownloads = true) =>
  invoke<ScreenshotResult>("capture_screenshot", { saveToDownloads });

export const captureRecordStart = (mic = true, systemAudio = false) =>
  invoke<RecordingState>("capture_record_start", { mic, systemAudio });

export const captureRecordStop = () => invoke<RecordingState>("capture_record_stop");

export const captureRecordingState = () => invoke<RecordingState>("capture_recording_state");

// --- clipboard -------------------------------------------------------------

export const clipboardList = (query = "", limit = 60, pinnedOnly = false) =>
  invoke<ClipboardEntry[]>("clipboard_list", { query, limit, pinnedOnly });

export const clipboardCopy = (id: number) => invoke<void>("clipboard_copy", { id });

export const clipboardImage = (id: number) => invoke<string | null>("clipboard_image", { id });

export const clipboardPin = (id: number, pinned: boolean) =>
  invoke<void>("clipboard_pin", { id, pinned });

export const clipboardDelete = (id: number) => invoke<void>("clipboard_delete", { id });

export const clipboardClear = (keepPinned: boolean) =>
  invoke<number>("clipboard_clear", { keepPinned });

export const clipboardStats = () => invoke<ClipboardStats>("clipboard_stats");

// --- agents ----------------------------------------------------------------

export const agentChat = (prompt: string) => invoke<AgentResponse>("agent_chat", { prompt });

export const agentStartSession = (task: string) => invoke<string>("agent_start_session", { task });

export const agentStopSession = (sessionId: string) =>
  invoke<boolean>("agent_stop_session", { sessionId });

export const agentStopAll = () => invoke<void>("agent_stop_all");

export const agentApprove = (sessionId: string, approved: boolean) =>
  invoke<boolean>("agent_approve", { sessionId, approved });

export const agentActiveSessions = () => invoke<string[]>("agent_active_sessions");

export const agentTestBackend = (backendId: string) =>
  invoke<string>("agent_test_backend", { backendId });

export const agentListModels = (backendId: string) =>
  invoke<string[]>("agent_list_models", { backendId });

export const agentBackendTemplates = () => invoke<BackendConfig[]>("agent_backend_templates");

// --- Hermes Agent ----------------------------------------------------------

export const hermesStatus = () => invoke<HermesStatus>("hermes_status");

/** Probe this Mac for AI runtimes that are installed and serving. Read-only. */
export const detectLocalAi = () => invoke<LocalAiScan>("detect_local_ai");

// --- system monitor --------------------------------------------------------

export const systemSnapshot = (limit = 40, sortByMemory = false) =>
  invoke<SystemSnapshot>("system_snapshot", { limit, sortByMemory });

/** SIGTERM by default; `force` escalates to SIGKILL. */
export const systemKill = (pid: number, force = false) =>
  invoke<void>("system_kill", { pid, force });

/** Opens Terminal with the install command typed but NOT run. */
export const openHermesInstaller = () => invoke<ExecOutcome>("open_hermes_installer");

// --- launcher + calculator -------------------------------------------------

export const listInstalledApps = () => invoke<InstalledApp[]>("list_installed_apps");

export const launchApp = (path: string) => invoke<ExecOutcome>("launch_app", { path });

export const calculate = (input: string) => invoke<CalcResult | null>("calculate", { input });

// --- voice -----------------------------------------------------------------

export const voiceStart = () => invoke<void>("voice_start");

export const voiceStop = () => invoke<RoutedText | null>("voice_stop");

/** Hold the recording without ending it. Returns the new paused state. */
export const voicePause = (paused: boolean) => invoke<boolean>("voice_pause", { paused });

/**
 * End the recording and transcribe.
 *
 * Returns as soon as the request is in; the transcript arrives on
 * `EVENTS.voiceResult`. That is what keeps the HUD's Stop button responsive
 * even when the speech helper is being slow about finalising.
 */
export const voiceFinish = () => invoke<void>("voice_finish");

export const voiceCancel = () => invoke<void>("voice_cancel");

export const voiceIsRecording = () => invoke<boolean>("voice_is_recording");

export const toggleDictation = () => invoke<void>("toggle_dictation");

// --- misc ------------------------------------------------------------------

export const quitApp = () => invoke<void>("quit_app");

/**
 * Normalise an error thrown by `invoke` into a string.
 *
 * Tauri rejects with whatever the command's `Err` variant serialised to — a
 * plain string for Caduceus's commands, but an `Error` when the IPC layer itself
 * fails, and occasionally an object.
 */
export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

// --- chat --------------------------------------------------------------------

/**
 * Ask the primary backend inside a conversation.
 *
 * `conversationId` of `null` continues the most recent thread, starting one if
 * there is none — which is what a bare `/` in the palette does. The reply names
 * the thread it landed in, so the caller can keep asking into the same one.
 */
export const chatAsk = (prompt: string, conversationId: number | null = null) =>
  invoke<ChatReply>("chat_ask", { prompt, conversationId });

export const chatConversations = () => invoke<Conversation[]>("chat_conversations");

export const chatMessages = (conversationId: number) =>
  invoke<ChatMessage[]>("chat_messages", { conversationId });

export const chatNewConversation = () => invoke<number>("chat_new_conversation");

export const chatDeleteConversation = (conversationId: number) =>
  invoke<void>("chat_delete_conversation", { conversationId });

export const chatClear = () => invoke<void>("chat_clear");

/** Open the full chat window, optionally on a specific thread. */
export const openChatWindow = (conversationId: number | null = null) =>
  invoke<void>("open_chat_window", { conversationId });

// --- notes -------------------------------------------------------------------

/** Append text to Apple Notes as a new note. */
export const addToNotes = (body: string, title: string | null = null) =>
  invoke<ExecOutcome>("add_to_notes", { body, title });

// --- extensions --------------------------------------------------------------

/** Read a candidate file's header without installing or running it. */
export const inspectExtension = (path: string) =>
  invoke<Extension>("inspect_extension", { path });

export const installExtension = (path: string) =>
  invoke<InstallReport>("install_extension", { path });

export const listExtensions = () => invoke<Extension[]>("list_extensions");

export const removeExtension = (id: string) => invoke<void>("remove_extension", { id });

export const openExtensionsFolder = () => invoke<void>("open_extensions_folder");

export const extensionPermissions = () => invoke<string[]>("extension_permissions");

// --- window management -------------------------------------------------------
// Needs the Accessibility permission. `windowPermission` never prompts, so the
// UI can say what is missing instead of firing a system dialog at a keystroke.

export const windowAction = (verb: WindowVerb) =>
  invoke<WindowOutcome>("window_action", { verb });

export const windowPermission = () => invoke<boolean>("window_permission");

/** The text selected in the frontmost app, or `null`. */
export const selectedText = () => invoke<string | null>("selected_text");

// --- developer toolbox -------------------------------------------------------

/** Run one of the built-in tools. `id` is a closed set; see `ToolId`. */
export const runTool = (id: ToolId, input = "") =>
  invoke<ToolResult>("run_tool", { id, input });

// --- system controls ---------------------------------------------------------

export const systemAction = (action: SystemAction) =>
  invoke<ToolOutcome>("system_action", { action });

export const systemPermissions = () => invoke<PermissionReport>("system_permissions");

/** What `repair_permission` reports back. Mirrors `window::grants::RepairOutcome`. */
export interface RepairOutcome {
  ok: boolean;
  message: string;
  granted: boolean;
}

/**
 * Clear a stale macOS privacy grant and ask for it again.
 *
 * The fix for "Caduceus is ticked in System Settings and Caduceus says it is
 * not" — which happens because the grant is recorded against a code signature
 * that changes on every build. See `window::grants` on the Rust side.
 */
export const repairPermission = (grant: import("./tabs").PermissionId) =>
  invoke<RepairOutcome>("repair_permission", { grant });

export const machineSummary = () => invoke<ToolOutcome>("machine_summary");

export const wifiSummary = () => invoke<ToolOutcome>("wifi_summary");

export const mediaAction = (action: MediaAction) =>
  invoke<ToolOutcome>("media_action", { action });

// --- vision + audio devices --------------------------------------------------

/** Drag a region of the screen; the text inside it comes back copied. */
export const ocrScreen = () => invoke<ToolOutcome>("ocr_screen");

export const ocrImage = (path: string) => invoke<ToolOutcome>("ocr_image", { path });

/**
 * Sample a colour from anywhere on screen with macOS's own loupe.
 *
 * Resolves to `null` when the user presses Escape — a cancel, not a failure.
 * Caduceus's windows are hidden for the duration and restored afterwards.
 */
export const pickScreenColor = () => invoke<string | null>("pick_screen_color");

/** Today's rates. Mirrors `tools::rates::RateTable`. */
export interface RateTable {
  base: string;
  rates: Record<string, number>;
  /** `YYYY-MM-DD`, the day the source published these. */
  date: string;
  source: string;
  cached: boolean;
}

/**
 * Exchange rates for a base currency.
 *
 * The only call in Caduceus that needs the internet. Cached for six hours,
 * which is finer-grained than the once-a-day source publishes.
 */
export const exchangeRates = (base: string) =>
  invoke<RateTable>("exchange_rates", { base });

// --- other applications -----------------------------------------------------

/**
 * Run AppleScript and get back what it printed.
 *
 * Scripts come from Caduceus's own command registry, never from anything
 * fetched — see the Rust side for the trust boundary this depends on.
 */
export const runAppleScript = (script: string) =>
  invoke<string>("run_apple_script", { script });

/** Run a shortcut from the Shortcuts app, optionally with text as its input. */
export const runAppleShortcut = (name: string, input = "") =>
  invoke<string>("run_apple_shortcut", { name, input });

export const listAppleShortcuts = () => invoke<string[]>("list_apple_shortcuts");

// --- storage and cleaning ---------------------------------------------------

/** Mirrors `tools::cleaner::JunkGroup`. */
export interface JunkGroup {
  kind: string;
  label: string;
  detail: string;
  /** Could plausibly contain something wanted. Never pre-ticked. */
  risky: boolean;
  bytes: number;
  human: string;
  items: number;
  paths: string[];
}

export interface InstalledAppSize {
  name: string;
  path: string;
  bundleId: string | null;
  bytes: number;
  human: string;
  /** Epoch seconds, where macOS records it. */
  lastOpened: number | null;
}

/** Measure everything reclaimable. Slow on a full disk, because it measures. */
export const scanJunk = () => invoke<JunkGroup[]>("scan_junk");

/**
 * Reclaim the space in the chosen categories.
 *
 * Takes the category names rather than a list of paths, deliberately: one of
 * the categories *is* the Trash, and emptying it is a different operation from
 * moving things into it. Passing paths made "empty the Trash" a silent no-op.
 * Rust re-scans, so the removal also cannot act on a stale list.
 */
export const cleanJunk = (kinds: string[]) => invoke<ToolOutcome>("clean_junk", { kinds });

export const listInstalledAppSizes = () =>
  invoke<InstalledAppSize[]>("list_installed_app_sizes");

// --- folder sorting ---------------------------------------------------------

export type SortBy = "kind" | "extension" | "month" | "year" | "alphabetical" | "size";

export interface SortMove {
  from: string;
  to: string;
  folder: string;
  name: string;
  bytes: number;
}

export interface SortPlan {
  directory: string;
  moves: SortMove[];
  folders: Record<string, number>;
  skipped: string[];
}

export interface SortResult {
  ok: boolean;
  message: string;
  moved: SortMove[];
}

/** Work out where everything goes. Changes nothing. */
export const sortPlan = (directory: string, sortBy: SortBy) =>
  invoke<SortPlan>("sort_plan", { directory, sortBy });

export const sortApply = (moves: SortMove[]) => invoke<SortResult>("sort_apply", { moves });

export const sortRevert = (moves: SortMove[]) => invoke<SortResult>("sort_revert", { moves });

// --- desktop icon shapes ----------------------------------------------------

/** Where every Desktop icon would go. Moves nothing. */
export const desktopShapePlan = (shape: DesktopShape) =>
  invoke<DesktopShapePlan>("desktop_shape_plan", { shape });

/**
 * Arrange the Desktop.
 *
 * Takes the shape rather than the positions the preview showed: Rust re-reads
 * the Desktop, so the icons that exist *now* are the ones that get placed.
 */
export const desktopShapeApply = (shape: DesktopShape) =>
  invoke<DesktopShapeResult>("desktop_shape_apply", { shape });

export const desktopShapeRevert = (previous: DesktopSpot[]) =>
  invoke<DesktopShapeResult>("desktop_shape_revert", { previous });

// --- citations --------------------------------------------------------------

export interface CitationSource {
  title: string;
  url: string;
  author: string | null;
  site: string | null;
  published: string | null;
}

export interface Citation {
  style: string;
  label: string;
  text: string;
}

/** The page in the frontmost browser. */
export const currentPage = () => invoke<CitationSource>("current_page");

/** Fill in author and date by fetching the page. Only ever on request. */
export const enrichCitation = (source: CitationSource) =>
  invoke<CitationSource>("enrich_citation", { source });

export const formatCitations = (source: CitationSource, accessed: string) =>
  invoke<Citation[]>("format_citations", { source, accessed });

// --- recording --------------------------------------------------------------

export type RecordMode = "screen" | "audio";

export interface RecordingStatus {
  active: boolean;
  paused: boolean;
  mode: RecordMode | null;
  path: string | null;
  seconds: number;
  /** 0–1, only while the microphone is on. */
  level: number;
  error: string | null;
}

export const recordingStart = (mode: RecordMode, microphone: boolean) =>
  invoke<string>("recording_start", { mode, microphone });

export const recordingPause = (paused: boolean) =>
  invoke<boolean>("recording_pause", { paused });

export const recordingStop = () => invoke<string>("recording_stop");

export const recordingStatus = () => invoke<RecordingStatus>("recording_status");

export const audioDevices = () => invoke<AudioDevice[]>("audio_devices");

export const setAudioDevice = (uid: string, input: boolean) =>
  invoke<ToolOutcome>("set_audio_device", { uid, input });

// --- developer environment ---------------------------------------------------

export const listeningPorts = (port?: number) =>
  invoke<PortUser[]>("listening_ports", { port: port ?? null });

export const freePort = (port: number) => invoke<ToolOutcome>("free_port", { port });

export const gitRepos = (limit = 60) => invoke<GitRepo[]>("git_repos", { limit });

export const gitStatus = (path: string) => invoke<number | null>("git_status", { path });

export const sshHosts = () => invoke<SshHost[]>("ssh_hosts");

export const dockerContainers = () => invoke<Container[]>("docker_containers");

export const dockerAction = (id: string, action: "start" | "stop" | "restart") =>
  invoke<ToolOutcome>("docker_action", { id, action });

// --- files -------------------------------------------------------------------
// All of these act on the current Finder selection, so there is never a path
// string crossing IPC that the user did not select themselves.

export const compressSelection = () => invoke<ToolOutcome>("compress_selection");

export const expandSelection = () => invoke<ToolOutcome>("expand_selection");

export const trashSelection = () => invoke<ToolOutcome>("trash_selection");

export const quickLookSelection = () => invoke<ToolOutcome>("quick_look_selection");

export const openSelectionInTerminal = () =>
  invoke<ToolOutcome>("open_selection_in_terminal");

export const largestFiles = (directory?: string, limit = 40) =>
  invoke<BigFile[]>("largest_files", { directory: directory ?? null, limit });

/** Support files an app left behind. Reports only — removing is `trashPaths`. */
export const appLeftovers = (appPath: string) =>
  invoke<Leftover[]>("app_leftovers", { appPath });

export const trashPaths = (paths: string[]) => invoke<ToolOutcome>("trash_paths", { paths });

// --- network -----------------------------------------------------------------

export const networkSummary = () => invoke<ToolOutcome>("network_summary");

/** Leaves the machine. Only ever called from the row that says so. */
export const publicAddress = () => invoke<ToolOutcome>("public_address");

export const dnsLookup = (host: string) => invoke<ToolOutcome>("dns_lookup", { host });

export const pingHost = (host: string) => invoke<ToolOutcome>("ping_host", { host });

// --- the 1.1 utilities, now reachable from the palette -----------------------
// These commands shipped in 1.1.0 but had no caller: the release notes listed
// them and nothing in the UI could run them. They are wired up in 2.0.

export const changeCase = (text: string, kase: string) =>
  invoke<string>("change_case", { text, case: kase });

export const caseOptions = () => invoke<[string, string][]>("case_options");

export const copyLatestDownload = () => invoke<ToolOutcome>("copy_latest_download");

export const openLatestDownload = () => invoke<ToolOutcome>("open_latest_download");

export const copyFinderPath = () => invoke<ToolOutcome>("copy_finder_path");

export const ejectDisks = () => invoke<ToolOutcome>("eject_disks");

export const stayAwake = (on: boolean) => invoke<ToolOutcome>("stay_awake", { on });

export const stayAwakeState = () => invoke<boolean>("stay_awake_state");

export const searchFiles = (query: string, limit = 40) =>
  invoke<FileHit[]>("search_files", { query, limit });

export const defineWord = (word: string) => invoke<ToolOutcome>("define_word", { word });

export const convertImage = (path: string, width?: number, format?: string) =>
  invoke<ToolOutcome>("convert_image", {
    path,
    width: width ?? null,
    format: format ?? null,
  });

export const revealPath = (path: string) => invoke<ToolOutcome>("reveal_path", { path });

export const openPathInTerminal = (path: string) =>
  invoke<ToolOutcome>("open_path_in_terminal", { path });

export const sshConnect = (alias: string) => invoke<ToolOutcome>("ssh_connect", { alias });

// --- usage ranking -----------------------------------------------------------
// Local only. See src-tauri/src/usage.rs — nothing here is sent anywhere.

export interface UsageEntry {
  count: number;
  lastUsedMs: number;
}

export const usageCounts = () => invoke<Record<string, UsageEntry>>("usage_counts");

export const recordUsage = (id: string) => invoke<UsageEntry>("record_usage", { id });

export const clearUsage = () => invoke<void>("clear_usage");

// --- keep-awake sessions -----------------------------------------------------
// The engine behind Manage → Keep Awake. `stayAwake` above is the quick toggle
// over the same runtime, so the two can never disagree about the state.

export const awakeStart = (minutes: number | null, displayMaySleep = false) =>
  invoke<ToolOutcome>("awake_start", { minutes, displayMaySleep });

export const awakeStop = () => invoke<ToolOutcome>("awake_stop");

export const awakeStatus = () => invoke<import("./types").AwakeStatus>("awake_status");

/** Open a management tab ("awake", "sound", "ports", "docker", "machine"). */
export const openManageWindow = (page?: string) =>
  invoke<void>("open_manage_window", { page: page ?? null });

/**
 * Tell Rust whether the window is currently just the palette.
 *
 * Decides three things it cannot work out for itself: whether clicking away
 * dismisses, whether the window floats over full-screen apps, and whether
 * Caduceus appears in the Dock.
 */
export const setPaletteFloating = (floating: boolean) =>
  invoke<void>("set_palette_floating", { floating });
