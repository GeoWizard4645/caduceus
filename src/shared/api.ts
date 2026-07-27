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

export const collapseStaffPopout = () => invoke<void>("collapse_staff_popout");

export const resolveShortcutIcon = (icon: string) =>
  invoke<string | null>("resolve_shortcut_icon", { icon });

export const resolveStaffMark = (icon: string) =>
  invoke<string | null>("resolve_staff_mark", { icon });

export const importStaffMark = (sourcePath: string) =>
  invoke<string>("import_staff_mark", { sourcePath });

export const clearStaffMark = () => invoke<void>("clear_staff_mark");

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

export const machineSummary = () => invoke<ToolOutcome>("machine_summary");

export const wifiSummary = () => invoke<ToolOutcome>("wifi_summary");

export const mediaAction = (action: MediaAction) =>
  invoke<ToolOutcome>("media_action", { action });

// --- vision + audio devices --------------------------------------------------

/** Drag a region of the screen; the text inside it comes back copied. */
export const ocrScreen = () => invoke<ToolOutcome>("ocr_screen");

export const ocrImage = (path: string) => invoke<ToolOutcome>("ocr_image", { path });

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
