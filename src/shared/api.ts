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
  CurlRequest,
  DependencyEntry,
  DependencyReport,
  DesktopShape,
  DesktopShapePlan,
  DesktopShapeResult,
  DesktopSpot,
  DispatchOutcome,
  ExecOutcome,
  ChatMessage,
  ChatReply,
  Extension,
  ExtensionFetchRequest,
  ExtraToolId,
  ExtensionFetchResponse,
  GitCommitAssist,
  GitFileChange,
  HttpPlaygroundResult,
  InstallReport,
  UninstallRequest,
  UninstallResult,
  UninstallSnapshot,
  Conversation,
  ParsedInput,
  PinKind,
  RoutedText,
  RuntimeInfo,
  UpdateCheck,
  Settings,
  SettingsApplyReport,
  AudioDevice,
  BigFile,
  Container,
  FileHit,
  GitRepo,
  Leftover,
  MediaAction,
  OptimizeLevel,
  PermissionReport,
  PortUser,
  ShortcutKind,
  SshHost,
  SystemAction,
  TargetModel,
  TextAiAction,
  ToolId,
  ToolOutcome,
  ToolResult,
  WindowOutcome,
  WindowVerb,
} from "./types";

// Re-exported so pages can write `api.HttpPlaygroundResult` etc., matching the
// convention `RegexPage`/`CronPage` already use for their own result types.
export type {
  CurlRequest,
  DependencyEntry,
  DependencyReport,
  GitCommitAssist,
  GitFileChange,
  HttpPlaygroundResult,
  OptimizeLevel,
  PinKind,
  TargetModel,
};

// --- settings --------------------------------------------------------------

export const getSettings = () => invoke<Settings>("get_settings");

export const updateSettings = (next: Settings) =>
  invoke<SettingsApplyReport>("update_settings", { next });

export const resetSettings = () => invoke<Settings>("reset_settings");

export const getRuntimeInfo = () => invoke<RuntimeInfo>("get_runtime_info");

export const checkForUpdate = () => invoke<UpdateCheck>("check_for_update");

/**
 * Update in place by running the website's installer in Terminal.
 *
 * Resolves once Terminal has the script — the update quits this process, so
 * there is nothing to await beyond the hand-off.
 */
export const runInstallerUpdate = () => invoke<void>("run_installer_update");

/** The exact command {@link runInstallerUpdate} will run, to show beforehand. */
export const installCommand = () => invoke<string>("install_command");

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

export const setTtsApiKey = (key: string) => invoke<boolean>("set_tts_api_key", { key });

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

// --- workflow imports --------------------------------------------------------
//
// Typed wrappers around the `caduceus://import/…` deep-link pipeline in
// `src-tauri/src/workflows.rs`. Read that file's module doc before touching
// anything below — it lays out the threat model these types exist to surface,
// not hide: a `run_command`/`run_applescript` action's `target` is the literal
// shell/AppleScript text, handed back unmodified specifically so a human can
// read it before deciding, and nothing about staging an import ever runs it.
// Only `workflowsCommitImport` writes anything, and only for a `token` the
// backend itself minted — the frontend never constructs one.

/** How much scrutiny an action needs before import — mirrors `ImportRisk` in workflows.rs. */
export type ImportRisk = "low" | "medium" | "high";

/**
 * One shortcut a staged workflow would add, exactly as the backend parsed and
 * validated it. `target` is the whole point for `run_command`/
 * `run_applescript` — show it verbatim, in full, never truncated or
 * "cleaned up" — see the module doc in `workflows.rs`.
 */
export interface PendingAction {
  label: string;
  description: string;
  kind: ShortcutKind;
  target: string;
  args: string[];
  keywords: string[];
  icon: string;
  risk: ImportRisk;
  /** The shortcut id this action would be written under if imported. */
  previewId: string;
}

/**
 * A workflow that has been parsed and validated but not yet applied. Exists
 * only in the backend's in-memory inbox — nothing about receiving one touches
 * disk, and an import nobody reviews simply falls off the queue or vanishes at
 * restart.
 */
export interface PendingImport {
  /** Opaque and backend-generated; only ever echoed back, never built here. */
  token: string;
  slug: string;
  label: string;
  description: string;
  actions: PendingAction[];
  maxRisk: ImportRisk;
  /** ISO-8601, from `chrono::Utc::now()` at the moment the link was staged. */
  receivedAt: string;
}

/** What committing an import actually added, for a confirmation message. */
export interface CommitOutcome {
  addedShortcutIds: string[];
}

/**
 * Emitted (no payload — listeners re-fetch with {@link workflowsListPending})
 * whenever a new import is staged, so a review UI can react without polling.
 * Mirrors `WORKFLOW_PENDING_EVENT` in workflows.rs; not in `EVENTS` because
 * this file does not own `shared/types.ts`.
 */
export const WORKFLOW_PENDING_EVENT = "caduceus://workflow-import-pending";

/**
 * Parse-and-stage a `caduceus://import/…` link directly, for a "paste a link"
 * box rather than an OS-level open. Applies the exact same validation as
 * clicking the link would — see `parse_deep_link` in workflows.rs.
 */
export const workflowsStageFromUrl = (url: string) =>
  invoke<PendingImport>("workflows_stage_from_url", { url });

/** Everything currently awaiting review. */
export const workflowsListPending = () => invoke<PendingImport[]>("workflows_list_pending");

/** Discard a pending import unreviewed. Resolves `false` if it was already gone. */
export const workflowsDismissPending = (token: string) =>
  invoke<boolean>("workflows_dismiss_pending", { token });

/**
 * Apply a staged import: append its actions as new shortcuts. `acceptHighRisk`
 * must be `true` if the import contains a `run_command`/`run_applescript`
 * action — the backend refuses otherwise (and re-queues the import rather than
 * dropping it), so this call cannot silently import a shell action even if a
 * caller forgets to ask.
 */
export const workflowsCommitImport = (token: string, acceptHighRisk: boolean) =>
  invoke<CommitOutcome>("workflows_commit_import", { token, acceptHighRisk });

// --- command center --------------------------------------------------------

export const parseInput = (input: string) => invoke<ParsedInput>("parse_input", { input });

export const dispatchInput = (input: string) =>
  invoke<DispatchOutcome>("dispatch_input", { input });

export const hideCommandCenter = () => invoke<void>("hide_command_center");

export const toggleCommandCenter = (source?: string) =>
  invoke<void>("toggle_command_center", { source: source ?? null });

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

export const relaunchApp = () => invoke<void>("relaunch_app");

export const restartApp = () => invoke<void>("restart_app");

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

/**
 * Start a tool-calling agent session against the primary backend.
 *
 * The sibling of {@link agentStartSession}: same session id / step-event /
 * stop / approve machinery (`caduceus://agent-step`, `agentStopSession`,
 * `agentApprove`), but this one calls MCP tools through the primary backend
 * rather than driving the screen through a computer-use backend — see
 * `agent::start_tool_session`'s doc for exactly how the two differ.
 */
export const agentStartToolSession = (task: string) =>
  invoke<string>("agent_start_tool_session", { task });

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

export const systemSnapshot = (limit = 40, sortByMemory = false, sortByName = false) =>
  invoke<SystemSnapshot>("system_snapshot", { limit, sortByMemory, sortByName });

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

/**
 * Type text into whatever app has keyboard focus, via simulated keystrokes.
 * The palette is a non-activating panel, so the app behind it still has the
 * caret. Needs the Accessibility permission (System Events).
 */
export const typeText = (text: string) => invoke<void>("type_text", { text });

// --- text-to-speech ----------------------------------------------------------
// Off by default (`voice.ttsEnabled`) and rejected by Rust when it is — see
// `voice::TtsRuntime::speak`. A caller offering a "preview this voice" action
// still just calls `speak` directly; there is no separate "am I allowed to"
// check to run first, the rejection itself is the answer.

/**
 * Speak `text` aloud with the configured backend.
 *
 * Resolves once playback finishes or is cut off by {@link stopSpeaking} (a
 * barge-in is reported as success, not failure — see `TtsBackend::speak`'s
 * doc). Two calls in flight at once are not safe against each other — call
 * {@link stopSpeaking} before starting a new one if a previous call might
 * still be running, so a fast-arriving second reply cannot talk over the
 * first.
 */
export const speak = (text: string) => invoke<void>("speak", { text });

/** Cut off whatever is currently being spoken. Safe to call when nothing is. */
export const stopSpeaking = () => invoke<void>("stop_speaking");

/** Installed system voices for the Settings voice picker. Empty off macOS. */
export const listSpeechVoices = () => invoke<string[]>("list_speech_voices");

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

export const uninstallSnapshot = () => invoke<UninstallSnapshot>("uninstall_snapshot");

export const runUninstall = (request: UninstallRequest) =>
  invoke<UninstallResult>("run_uninstall", { request });

export const openExtensionsFolder = () => invoke<void>("open_extensions_folder");

export const extensionPermissions = () => invoke<string[]>("extension_permissions");

// --- what a running extension is allowed to do -------------------------------
//
// The `ctx` object an extension is handed is built out of exactly these calls
// and nothing else — see `extensionRuntime.ts` for the sandbox they come from.
// Each one takes the extension's id, and Rust re-reads that extension's header
// to check the permission before it acts. A caller here cannot vouch for one.

/** The extension's own source, to be run in the sandbox. */
export const extensionSource = (id: string) => invoke<string>("extension_source", { id });

export const extensionClipboardRead = (id: string) =>
  invoke<string>("extension_clipboard_read", { id });

export const extensionClipboardWrite = (id: string, text: string) =>
  invoke<void>("extension_clipboard_write", { id, text });

export const extensionFetch = (id: string, request: ExtensionFetchRequest) =>
  invoke<ExtensionFetchResponse>("extension_fetch", { id, request });

export const extensionSelection = (id: string) =>
  invoke<string[]>("extension_selection", { id });

export const extensionNotify = (id: string, text: string) =>
  invoke<void>("extension_notify", { id, text });

export const extensionOpen = (id: string, url: string) =>
  invoke<void>("extension_open", { id, url });

export const extensionStorageGet = (id: string, key: string) =>
  invoke<unknown>("extension_storage_get", { id, key });

export const extensionStorageSet = (id: string, key: string, value: unknown) =>
  invoke<void>("extension_storage_set", { id, key, value });

export const extensionShellRun = (
  id: string,
  command: string,
  input?: string,
  timeoutSecs?: number,
) => invoke<ExecOutcome>("extension_shell_run", { id, command, input, timeoutSecs });

export const extensionAutomationScript = (id: string, script: string) =>
  invoke<string>("extension_automation_script", { id, script });

export const extensionAutomationShortcut = (id: string, name: string, input?: string) =>
  invoke<string>("extension_automation_shortcut", { id, name, input });

export const extensionFilesRead = (id: string, path: string) =>
  invoke<string>("extension_files_read", { id, path });

export const extensionFilesWrite = (id: string, path: string, content: string) =>
  invoke<void>("extension_files_write", { id, path, content });

export const extensionSettingsGet = (id: string) =>
  invoke<Settings>("extension_settings_get", { id });

export const extensionSettingsSet = (id: string, next: Settings) =>
  invoke<void>("extension_settings_set", { id, next });

export const extensionCommandsDispatch = (id: string, input: string) =>
  invoke<DispatchOutcome>("extension_commands_dispatch", { id, input });

export const extensionCommandsRunTool = (id: string, toolId: string, input: string) =>
  invoke<ToolResult>("extension_commands_run_tool", { id, toolId, input });

export const extensionAiAsk = (id: string, prompt: string) =>
  invoke<string>("extension_ai_ask", { id, prompt });

export const extensionShortcutsRun = (id: string, shortcutId: string, query?: string) =>
  invoke<ExecOutcome>("extension_shortcuts_run", { id, shortcutId, query });

// --- MCP servers ---------------------------------------------------------------
//
// Backs Settings → MCP. See `src-tauri/src/mcp.rs`'s module header for the full
// security model — the short version: an MCP server is an arbitrary program the
// user has pointed Caduceus at, nothing here ever launches one the user did not
// explicitly configure, and a server's own tool descriptions / instructions are
// untrusted text to display, never text Caduceus vouches for. These types are
// declared here rather than in `types.ts` because this file is the only thing
// that owns the MCP surface on the frontend.

/** Mirrors `mcp::ServerStatus` — a server's live connection state. */
export type McpServerStatus =
  | { state: "connecting" }
  | { state: "ready" }
  | { state: "unhealthy"; reason: string }
  | { state: "disconnected" };

/** Mirrors `mcp::ServerIdentity` — whatever the server said about itself during
 *  `initialize`. `serverName`/`instructions` are the server's own words. */
export interface McpServerIdentity {
  protocolVersion: string;
  serverName: string;
  serverVersion: string;
  instructions: string | null;
}

/** Mirrors `mcp::McpServerInfo`: a configured server's command plus its live
 *  status. Note this does NOT include the server's `env` — the backend never
 *  hands configured environment values back to the frontend, so an "edit"
 *  form cannot prefill them; re-enter any that should survive an update. */
export interface McpServerInfo {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  status: McpServerStatus;
  toolCount: number;
  identity: McpServerIdentity | null;
  /** Trailing stderr lines, kept only as inert diagnostic text. */
  recentLog: string[];
}

/** Mirrors `mcp::McpTool`. `title`/`description` are written by the server
 *  that exposes the tool — render them as quoted, attributed, untrusted text,
 *  never as if Caduceus authored or reviewed them. */
export interface McpTool {
  id: string;
  server: string;
  name: string;
  title: string | null;
  description: string;
  inputSchema: unknown;
}

/** Mirrors `mcp::McpToolCallOutcome`. `content`/`text` are the server's raw
 *  reply — untrusted data, not instructions; see the module header's point
 *  (c) in `mcp.rs`. */
export interface McpToolCallOutcome {
  server: string;
  tool: string;
  isError: boolean;
  content: unknown[];
  text: string;
}

/** Every configured server, connected or not. */
export const mcpListServers = () => invoke<McpServerInfo[]>("mcp_list_servers");

/** One server's current status — cheaper than `mcpListServers` for polling a
 *  single "connecting…" row. */
export const mcpServerStatus = (name: string) => invoke<McpServerInfo>("mcp_server_status", { name });

/**
 * Persist a new server and connect it immediately. Submitting this form *is*
 * the explicit consent to run `command args...` as a child process — there is
 * no separate "install" step and no preview-only dry run; the connection
 * attempt this call makes is the first real handshake with that program.
 */
export const mcpAddServer = (name: string, command: string, args: string[], env: Record<string, string>) =>
  invoke<McpServerInfo>("mcp_add_server", { name, command, args, env });

/**
 * Replace a server's command/args/env/enabled flag outright (not a merge —
 * omitted `env` entries are gone). Always disconnects the previous process
 * first and reconnects only if `enabled` is true.
 */
export const mcpUpdateServer = (
  name: string,
  command: string,
  args: string[],
  env: Record<string, string>,
  enabled: boolean,
) => invoke<McpServerInfo>("mcp_update_server", { name, command, args, env, enabled });

/** Stop the server if running and forget its config entirely. */
export const mcpRemoveServer = (name: string) => invoke<void>("mcp_remove_server", { name });

/** Explicitly (re)run the handshake — the "test connection" / "retry" action.
 *  Works regardless of the persisted `enabled` flag and does not change it. */
export const mcpConnectServer = (name: string) => invoke<McpServerInfo>("mcp_connect_server", { name });

/** Stop a server's process without deleting its config. */
export const mcpDisconnectServer = (name: string) => invoke<McpServerInfo>("mcp_disconnect_server", { name });

/** Every tool from every currently-Ready server, namespaced as `{server}__{tool}`. */
export const mcpListTools = () => invoke<McpTool[]>("mcp_list_tools");

/**
 * Call one namespaced tool with a fully-formed arguments object. Per
 * `mcp.rs`'s module header, this does not gate the call behind its own
 * confirmation — any caller building a "run this tool" UI is responsible for
 * showing the tool and these exact arguments to the user first, the same
 * discipline the agent loop applies to computer-use actions.
 */
export const mcpCallTool = (toolId: string, args?: unknown) =>
  invoke<McpToolCallOutcome>("mcp_call_tool", { toolId, arguments: args });

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
  willRelaunch: boolean;
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

/** Trigger the system consent sheet where macOS provides one (Accessibility, Screen Recording). */
export const requestPermission = (grant: import("./tabs").PermissionId) =>
  invoke<boolean>("request_permission", { grant });

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

/**
 * Run a "Highlight & Act" transformation over some text.
 *
 * Takes a named action, not a prompt: the wording lives in Rust so it can be
 * tested, and so this surface cannot be turned into "ask the user's model
 * anything" by whatever ends up running in the webview.
 */
export const textAiRun = (action: TextAiAction, text: string, targetLanguage?: string) =>
  invoke<string>("text_ai_run", { action, text, targetLanguage: targetLanguage ?? null });

// --- screen perception -------------------------------------------------------
// OCR happens on-device; only the extracted text reaches a model. The failure
// message is deliberately passed through unchanged so `permissionFromMessage`
// can still route a missing Screen Recording grant to its walkthrough.

export interface VisionAnswer {
  answer: string;
  /** What Apple Vision read off the screen, so the answer can be checked. */
  text: string;
}

export const visionDescribeRegion = (question: string) =>
  invoke<VisionAnswer>("vision_describe_region", { question });

export const visionDescribeActiveWindow = (question: string) =>
  invoke<VisionAnswer>("vision_describe_active_window", { question });

// --- calendar and reminders --------------------------------------------------
// Dates are parsed in Rust, offline, so these work with no model and no network.

export interface CreatedEvent {
  title: string;
  /** What was actually scheduled, echoed back so the confirmation cannot lie. */
  when: string;
}

export interface CalendarEvent {
  title: string;
  start: string;
  end: string;
  location: string | null;
}

export const createCalendarEvent = (
  title: string,
  when: string,
  durationMinutes?: number,
  location?: string,
  notes?: string,
) =>
  invoke<CreatedEvent>("create_calendar_event", {
    title,
    when,
    durationMinutes: durationMinutes ?? null,
    location: location ?? null,
    notes: notes ?? null,
  });

export const calendarEventsToday = () =>
  invoke<CalendarEvent[]>("calendar_events_today");

export const calendarEventsBetween = (start: string, end: string) =>
  invoke<CalendarEvent[]>("calendar_events_between", { start, end });

export const createReminder = (text: string, due?: string) =>
  invoke<{ text: string; due: string | null }>("create_reminder", { text, due: due ?? null });

// --- documents ---------------------------------------------------------------
// Each of these ends in a model call, so each reports "no backend configured"
// rather than failing quietly.

export const pdfSummary = (path: string) => invoke<string>("pdf_summary", { path });
export const pdfAsk = (path: string, question: string) =>
  invoke<string>("pdf_ask", { path, question });
export const articleSummary = (url: string) => invoke<string>("article_summary", { url });
export const youtubeSummary = (url: string) => invoke<string>("youtube_summary", { url });

// --- images --------------------------------------------------------------------
// Every one of these writes a *new* file beside the source and never touches
// the original — see the module doc on `tools::images` (Rust) for why.

/** A resize target: one of the social presets, or an exact pixel size. */
export type ImagePreset =
  | { kind: "square" }
  | { kind: "landscape" }
  | { kind: "portrait" }
  | { kind: "custom"; width: number; height: number };

export const compressImage = (
  path: string,
  format?: string,
  quality?: number,
  maxDimension?: number,
) =>
  invoke<ToolOutcome>("compress_image", {
    path,
    format: format ?? null,
    quality: quality ?? null,
    maxDimension: maxDimension ?? null,
  });

export const resizeImageToPreset = (path: string, preset: ImagePreset) =>
  invoke<ToolOutcome>("resize_image_to_preset", { path, preset });

/**
 * Strips GPS, camera and timestamp metadata by decoding to raw pixels and
 * re-encoding — `sips` alone cannot be trusted to actually remove it (see
 * `tools::images::strip_metadata`'s doc comment for what was tried first).
 */
export const stripImageMetadata = (path: string) =>
  invoke<ToolOutcome>("strip_image_metadata", { path });

export interface DuplicateGroup {
  files: string[];
}

/** Scans one folder (not its subfolders) for images that look the same. */
export const findDuplicateImages = (dir: string, maxDistance?: number) =>
  invoke<DuplicateGroup[]>("find_duplicate_images", { dir, maxDistance: maxDistance ?? null });

/**
 * Always resolves `false` today. A real check rather than a hardcoded UI
 * constant so the day a Vision-based remover ships, the button lights up
 * with no frontend change. See `tools::images::background_removal_available`.
 */
export const backgroundRemovalAvailable = () =>
  invoke<boolean>("background_removal_available");

// --- semantic search -------------------------------------------------------
// `tools::semantic::SemanticIndex` (BM25 + optional local-Ollama embeddings)
// is built and tested, but nothing in `commands.rs`/`lib.rs` exposes it over
// IPC yet — these wrappers name the commands that need to exist. Until they
// are registered, every call here rejects with Tauri's own "command not
// found", which `SearchPage` catches and explains rather than papering over.

export interface SemanticIndexSnapshot {
  documentCount: number;
  /** The folders a sync would walk, so the page can say what "index" means. */
  roots: string[];
}

export interface SemanticIndexStats {
  scanned: number;
  indexed: number;
  updated: number;
  removed: number;
  skippedTooLarge: number;
  skippedIndexFull: number;
  errors: number;
  embedded: number;
  /** True if this call stopped at a per-run bound rather than finishing —
   * the caller should call `semanticIndexSync` again to continue. */
  truncated: boolean;
  durationMs: number;
}

export type SemanticMatchKind = "lexical" | "semantic" | "hybrid";

export interface SemanticSearchHit {
  path: string;
  title: string;
  snippet: string;
  score: number;
  matchedVia: SemanticMatchKind;
}

/** Cheap: document count and configured roots, no directory walk. */
export const semanticIndexStats = () => invoke<SemanticIndexSnapshot>("semantic_index_stats");

/**
 * Run one bounded chunk of indexing and return its stats. `sync` in the Rust
 * module is deliberately incremental and per-run bounded, so a first index of
 * a large folder needs this called repeatedly (while `truncated` is true)
 * rather than once — see `IndexConfig`'s bounds in `tools::semantic`.
 */
export const semanticIndexSync = () => invoke<SemanticIndexStats>("semantic_index_sync");

/** Flips the `CancelFlag` the in-progress (or next) sync call checks. */
export const semanticIndexCancel = () => invoke<void>("semantic_index_cancel");

export const semanticSearch = (query: string, limit = 40) =>
  invoke<SemanticSearchHit[]>("semantic_search", { query, limit });

// --- the second tool bench ---------------------------------------------------

export const runExtraTool = (id: ExtraToolId, input = "") =>
  invoke<ToolResult>("run_extra_tool", { id, input });

/** Parse and replay a pasted `curl` command. Never honours `-k`/`--insecure`. */
export const runCurl = (command: string) =>
  invoke<HttpPlaygroundResult>("run_curl", { command });

/** Reads the repo and drafts a message. Never stages, never commits. */
export const gitCommitAssist = (repoPath: string) =>
  invoke<GitCommitAssist>("git_commit_assist", { repoPath });

export const inspectDependencies = (manifestPath: string) =>
  invoke<DependencyReport>("inspect_dependencies", { manifestPath });

/** Encode text as an SVG QR code. Generated locally; nothing is uploaded. */
export const generateQr = (text: string, ecc = "medium") =>
  invoke<string>("generate_qr", { text, ecc });

/** The frontmost browser's active-tab URL, or null if it is not a browser. */
export const frontTabUrl = () => invoke<string | null>("front_tab_url");

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

export const seedUsage = (ids: string[], count: number) =>
  invoke<void>("seed_usage", { ids, count });

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
 * Tell Rust whether the Command Center is in palette-only mode (lone Home tab).
 *
 * Used for window sizing on the frontend; macOS presentation stays consistent
 * either way. Clicking another app always hides the Command Center.
 */
export const setPaletteFloating = (floating: boolean) =>
  invoke<void>("set_palette_floating", { floating });

// --- time management ---------------------------------------------------------
// World clock, a timezone converter, countdown timers, a stopwatch and a
// pomodoro cycle. All of it — including the pomodoro's phase clock and every
// timer's deadline — is state Rust owns, not React: see the header comment on
// `tools::timekeeping` on the Rust side for why a timer that only ran while
// its tab was visible would not be worth shipping.

/** Mirrors `tools::timekeeping::ZoneClock`. */
export interface ZoneClock {
  id: string;
  label: string;
  offsetMinutes: number;
  utcOffsetLabel: string;
  /** `YYYY-MM-DDTHH:MM:SS`, this zone's wall-clock time as of the call. */
  localIso: string;
  isDst: boolean;
}

/** Every catalogued zone with its current offset — the world clock's rows and its picker's options. */
export const timeListZones = () => invoke<ZoneClock[]>("time_list_zones");

/** Mirrors `tools::timekeeping::ConvertRequest`. */
export interface TimeConvertRequest {
  zoneId: string;
  /** `YYYY-MM-DDTHH:MM`, the shape `<input type="datetime-local">` gives. */
  localDatetime: string;
}

/** Mirrors `tools::timekeeping::ConvertedTime`. */
export interface ConvertedTime {
  id: string;
  label: string;
  localIso: string;
  utcOffsetLabel: string;
  /** Days from the source zone's date to this zone's date for the same instant. */
  dayOffset: number;
}

/** Read a time in one zone and show it in a set of others — "5pm EST in Tokyo". */
export const timeConvert = (request: TimeConvertRequest, targets: string[]) =>
  invoke<ConvertedTime[]>("time_convert", { request, targets });

/** Mirrors `tools::timekeeping::TimerSnapshot`. */
export interface TimerSnapshot {
  id: number;
  name: string;
  totalSecs: number;
  remainingSecs: number;
  completed: boolean;
}

export const timeStartTimer = (name: string, seconds: number) =>
  invoke<TimerSnapshot>("time_start_timer", { name, seconds });

export const timeListTimers = () => invoke<TimerSnapshot[]>("time_list_timers");

export const timeDismissTimer = (id: number) => invoke<void>("time_dismiss_timer", { id });

/** Mirrors `tools::timekeeping::StopwatchStatus`. */
export interface StopwatchStatus {
  running: boolean;
  elapsedMs: number;
  /** Cumulative elapsed time at each lap; split times are the differences between entries. */
  lapsMs: number[];
}

export const timeStopwatchStart = () => invoke<StopwatchStatus>("time_stopwatch_start");
export const timeStopwatchStop = () => invoke<StopwatchStatus>("time_stopwatch_stop");
export const timeStopwatchLap = () => invoke<StopwatchStatus>("time_stopwatch_lap");
export const timeStopwatchReset = () => invoke<StopwatchStatus>("time_stopwatch_reset");
export const timeStopwatchStatus = () => invoke<StopwatchStatus>("time_stopwatch_status");

export type PomodoroPhase = "work" | "shortBreak" | "longBreak";

/** Mirrors `tools::timekeeping::PomodoroConfig`. */
export interface PomodoroConfig {
  workMinutes: number;
  shortBreakMinutes: number;
  longBreakMinutes: number;
  /** A long break follows every Nth work session instead of a short one; `0` means never. */
  cyclesBeforeLongBreak: number;
  /** Total work sessions for the run; `0` means until stopped by hand. */
  totalCycles: number;
}

/** Mirrors `tools::timekeeping::PomodoroStatus`. */
export interface PomodoroStatus {
  running: boolean;
  phase: PomodoroPhase | null;
  cycle: number;
  totalCycles: number;
  remainingSecs: number;
  totalSecs: number;
}

export const timePomodoroStart = (config: PomodoroConfig) =>
  invoke<PomodoroStatus>("time_pomodoro_start", { config });

export const timePomodoroStop = () => invoke<PomodoroStatus>("time_pomodoro_stop");

export const timePomodoroStatus = () => invoke<PomodoroStatus>("time_pomodoro_status");

// --- regex tester --------------------------------------------------------------
// Runs entirely on the Rust side via the `regex` crate — nothing here is sent
// anywhere, which matters for a tool people paste API tokens and log lines into.

/** One capture group within a match. `text` is `null` when the group did not
 * participate in this particular match — an alternative inside it was not the
 * one taken, which is a normal outcome and not the same thing as "". */
export interface CaptureGroup {
  index: number;
  name: string | null;
  text: string | null;
  start: number | null;
  end: number | null;
}

export interface RegexMatch {
  text: string;
  start: number;
  end: number;
  groups: CaptureGroup[];
}

export interface ExplainToken {
  token: string;
  description: string;
}

/** Run a pattern against sample text. `flags` is any of `i`, `m`, `s`, `x`. */
export const regexTest = (pattern: string, flags: string, text: string) =>
  invoke<RegexMatch[]>("regex_test", { pattern, flags, text });

/** A plain-English, token-by-token explanation of a pattern. */
export const regexExplain = (pattern: string) =>
  invoke<ExplainToken[]>("regex_explain", { pattern });

// --- prompt optimiser ------------------------------------------------------
// Rewrites a bloated prompt into one shaped for a specific target model. The
// compression itself is deterministic Rust; a small local model is used only
// for the bounded judgement passes, and only if one is configured. See
// `tools::promptopt` for why it is split that way.

/** One deterministic pass, and what it actually removed. */
export interface PassReport {
  name: string;
  detail: string;
  charsBefore: number;
  charsAfter: number;
}

/**
 * One requirement lifted out of the original, and whether it survived.
 *
 * `missing` lists the tokens that had to appear verbatim and did not — a
 * number, a backticked identifier, a quoted literal. Empty when `kept`.
 */
export interface RequirementCheck {
  text: string;
  kept: boolean;
  missing: string[];
}

/**
 * What one turn actually costs, both halves of it.
 *
 * The prompt is the cheap half twice over: output bills at roughly 4× input,
 * and an unbounded answer runs several times the length of the prompt asking
 * for it. `totalReductionPercent` is the headline for that reason —
 * `reductionPercent` (input only) reads as a saving it is not.
 */
export interface TokenEconomics {
  inputBefore: number;
  inputAfter: number;
  outputBefore: number;
  outputAfter: number;
  /** False means `outputBefore` is an assumption, not read from the prompt. */
  boundedBefore: boolean;
  boundedAfter: boolean;
  boundSource: string | null;
  totalBefore: number;
  totalAfter: number;
  totalReductionPercent: number;
  outputCostRatio: number;
}

export interface OptimizedPrompt {
  prompt: string;
  target: TargetModel;
  targetName: string;
  economics: TokenEconomics;
  beforeTokens: number;
  afterTokens: number;
  reductionPercent: number;
  coveragePercent: number;
  requirements: RequirementCheck[];
  passes: PassReport[];
  notes: string[];
  /** The model that did the judgement passes, or null when the whole run was
   * deterministic — which is a supported outcome, not a failure. */
  modelUsed: string | null;
}

export interface TokenEstimate {
  tokens: number;
  chars: number;
  words: number;
  targetName: string;
}

/**
 * Optimise a prompt. Slow by nature — several bounded model round trips.
 *
 * `outputCapWords` is the only argument that changes what the prompt *asks
 * for*, so it is never inferred, and it is ignored when the prompt already
 * states a length bound of its own.
 */
export const promptOptimize = (
  raw: string,
  target: TargetModel,
  level: OptimizeLevel,
  useModel: boolean,
  outputCapWords: number | null,
) =>
  invoke<OptimizedPrompt>("prompt_optimize", {
    raw,
    target,
    level,
    useModel,
    outputCapWords,
  });

/** What a prompt costs on one target. Pure arithmetic, safe to call per
 * keystroke — which is exactly why it is not part of `promptOptimize`. */
export const promptEstimate = (raw: string, target: TargetModel) =>
  invoke<TokenEstimate>("prompt_estimate", { raw, target });

/** Which model the judgement passes would use. */
export interface OptimizerBackend {
  displayName: string;
  model: string;
  /** Served from this machine — free, private, and what the feature is tuned for. */
  local: boolean;
  detail: string;
}

/** What the "use a model" toggle would actually do, or null if nothing usable
 * is configured. Settings only, no network — safe to call when the page opens. */
export const promptOptimizerModel = () =>
  invoke<OptimizerBackend | null>("prompt_optimizer_model");

// --- cron parser -----------------------------------------------------------------

export interface CronAnalysis {
  description: string;
  /** ISO 8601, in this Mac's local time zone — cron itself has no time zone
   * of its own, so that is the only reading "next run" can mean here. */
  nextRuns: string[];
}

/** Parse a 5-field cron expression and list its next occurrences. */
export const parseCron = (expression: string, count = 10) =>
  invoke<CronAnalysis>("parse_cron", { expression, count });

// --- text expander, markdown paste, emoji search, proofreader --------------
//
// Mirrors `src-tauri/src/tools/expander.rs`. That module's commands are all
// written and tested (55 tests) but were never added to `generate_handler!`,
// so every wrapper below calls a command name that is real and stable —
// nothing here is speculative the way `routingPreview` further down is.

/** A saved shortcut and the body it expands to. Placeholders inside `body`
 * (`{date}`, `{time}`, `{date+7d}`, `{clipboard}`, `{cursor}`) are substituted
 * at expansion time, not save time — see `SnippetsPage` for the full list. */
export interface Snippet {
  id: string;
  shortcut: string;
  body: string;
}

/** Where the caret should land after typing, if the snippet used `{cursor}`. */
export interface ExpansionOutcome {
  text: string;
  cursorOffset: number | null;
}

export const expanderListSnippets = () => invoke<Snippet[]>("expander_list_snippets");

/** Create a snippet (`id: null`) or update one in place (`id` of an existing one). */
export const expanderSaveSnippet = (id: string | null, shortcut: string, body: string) =>
  invoke<Snippet>("expander_save_snippet", { id, shortcut, body });

export const expanderDeleteSnippet = (id: string) =>
  invoke<void>("expander_delete_snippet", { id });

/** Expand arbitrary body text against the live clock and clipboard, without
 * it being a saved snippet — what a live preview calls on every keystroke. */
export const expanderPreview = (body: string) =>
  invoke<ExpansionOutcome>("expander_preview", { body });

/** Look up a snippet by shortcut and type its expansion into whatever app
 * currently has focus (macOS only — see the Rust module for why). */
export const expanderExpandAndInsert = (shortcut: string) =>
  invoke<ExpansionOutcome>("expander_expand_and_insert", { shortcut });

/** Render Markdown to the same HTML that would be copied as rich text, for a
 * live preview pane. */
export const expanderMarkdownPreview = (markdown: string) =>
  invoke<string>("expander_markdown_preview", { markdown });

/** Convert Markdown to HTML and place it on the clipboard as styled text
 * (`public.html`, with a plain-text fallback), ready to paste into Mail,
 * Notes or Word. */
export const expanderCopyMarkdownAsRichText = (markdown: string) =>
  invoke<ToolOutcome>("expander_copy_markdown_as_rich_text", { markdown });

export interface EmojiHit {
  emoji: string;
  /** Which keyword actually matched — shown so a result is not a mystery. */
  keyword: string;
  score: number;
}

/** Concept search over a curated keyword table ("celebrate" -> 🎉🥳🥂), not a
 * Unicode name lookup. */
export const expanderSearchEmoji = (query: string, limit = 24) =>
  invoke<EmojiHit[]>("expander_search_emoji", { query, limit });

export interface ProofreadIssue {
  original: string;
  suggestion: string;
  reason: string;
}

export interface ProofreadResult {
  corrected: string;
  issues: ProofreadIssue[];
}

/** Proofread for the class of mistake a spellchecker cannot see (homophones,
 * subject-verb agreement, a wrong date, a dropped "not") through whichever
 * backend is configured for primary chat. Can take a few seconds — it is a
 * real model call, unlike everything else in this section. */
export const expanderProofread = (text: string) =>
  invoke<ProofreadResult>("expander_proofread", { text });

// --- smart model routing -----------------------------------------------------
//
// Mirrors `src-tauri/src/tools/routing.rs`: a pure, deterministic classifier
// (24 tests) that decides whether a prompt is "micro" (fast local model) or
// "complex" (the configured primary backend), plus a policy that honours an
// on/off switch and a user pin. `routing_preview` is a registered
// `#[tauri::command]` (see `src-tauri/src/commands.rs`) that runs the same
// `tools::routing::route` used for real chat traffic — through
// `agent::chat_with_history` → `resolve_chat_backend` — against a prompt you
// type, without sending anything to a model. `AgentSettings` (see
// `src/shared/types.ts`) carries the two fields the policy needs,
// `autoRoutingEnabled` and `routingOverrideBackendId`, which the Routing
// settings tab reads and writes through `draft.update(...)` like any other
// setting.
export interface RoutingDecision {
  backendId: string;
  class: "micro" | "complex";
  /** One sentence, safe to show directly in the UI — the whole point of
   * exposing this at all is that invisible routing is untrustworthy routing. */
  reason: string;
}

export const routingPreview = (prompt: string) =>
  invoke<RoutingDecision>("routing_preview", { prompt });

// --- CSV / table cleaner ----------------------------------------------------
//
// Mirrors `src-tauri/src/tools/csv_clean.rs`: a hand-written RFC 4180-style
// parser/writer, not a new `csv` crate dependency.

export interface CsvCleanOptions {
  /** A single character to split fields on; omit to auto-detect. */
  delimiter?: string;
  trim?: boolean;
  dedupe?: boolean;
  hasHeader?: boolean;
}

export interface CsvCleanResult {
  csv: string;
  rows: number;
  columns: number;
  duplicatesRemoved: number;
  raggedRowsFixed: number;
  detectedDelimiter: string;
}

export const csvClean = (input: string, options?: CsvCleanOptions) =>
  invoke<CsvCleanResult>("csv_clean", { input, options });

// --- Redactor ----------------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/redactor.rs`.

export type PiiKind = "email" | "phone" | "ssn" | "credit_card" | "ip_address";

export interface RedactMatch {
  kind: PiiKind;
  text: string;
  start: number;
  end: number;
}

export interface RedactResult {
  text: string;
  matches: RedactMatch[];
}

/** Redact PII in `text`. `kinds` empty means "check everything". `replacement`
 * may contain `{KIND}`, substituted with the matched kind's label. */
export const redactText = (text: string, kinds: PiiKind[], replacement?: string) =>
  invoke<RedactResult>("redact_text", { text, kinds, replacement });

// --- Habit tracker -----------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/habits.rs`. Its own JSON store, not
// `Settings` — see that module's docs.

export interface Habit {
  id: string;
  name: string;
  /** YYYY-MM-DD, the day the habit was created. */
  createdAt: string;
  /** `#rrggbb`, or empty for "no colour chosen". */
  color: string;
  /** Every day this habit was marked done, as YYYY-MM-DD. */
  completions: string[];
}

export interface StreakInfo {
  current: number;
  longest: number;
  totalCompletions: number;
}

export const habitsList = () => invoke<Habit[]>("habits_list");

export const habitsCreate = (name: string, color?: string) =>
  invoke<Habit>("habits_create", { name, color });

export const habitsDelete = (id: string) => invoke<void>("habits_delete", { id });

/** Flip whether `date` (YYYY-MM-DD) is marked done for this habit. */
export const habitsToggleDay = (id: string, date: string) =>
  invoke<Habit>("habits_toggle_day", { id, date });

export const habitsStreak = (id: string) => invoke<StreakInfo>("habits_streak", { id });

// --- Birthdays -----------------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/birthdays.rs`.

export interface Birthday {
  id: string;
  name: string;
  month: number;
  day: number;
  /** The birth year, if known — drives "turning N". */
  year: number | null;
  notes: string;
}

/** A birthday with its next occurrence resolved — what `birthdaysList` returns. */
export interface UpcomingBirthday extends Birthday {
  /** ISO date of the next time this birthday occurs, today included. */
  nextOccurrence: string;
  daysUntil: number;
  /** The age they turn on `nextOccurrence`, if `year` is known. */
  turning: number | null;
}

export const birthdaysList = () => invoke<UpcomingBirthday[]>("birthdays_list");

export const birthdaysAdd = (
  name: string,
  month: number,
  day: number,
  year?: number | null,
  notes?: string,
) => invoke<Birthday>("birthdays_add", { name, month, day, year: year ?? null, notes });

export const birthdaysUpdate = (
  id: string,
  name: string,
  month: number,
  day: number,
  year?: number | null,
  notes?: string,
) => invoke<Birthday>("birthdays_update", { id, name, month, day, year: year ?? null, notes });

export const birthdaysDelete = (id: string) => invoke<void>("birthdays_delete", { id });

// --- Subscription tracker -------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/subscriptions.rs`.

export type BillingCycle = "weekly" | "monthly" | "quarterly" | "yearly";

export interface Subscription {
  id: string;
  name: string;
  cost: number;
  cycle: BillingCycle;
  /** YYYY-MM-DD — the next *known* renewal at the time this was saved; see
   * the Rust module for why a stale date here is not a data-entry error. */
  renewalDate: string;
  notes: string;
}

export interface UpcomingSubscription extends Subscription {
  /** `renewalDate` rolled forward past today. */
  nextRenewal: string;
  daysUntil: number;
  /** `cost` converted to a monthly rate, for comparing across cycles. */
  monthlyEquivalent: number;
}

export interface SubscriptionSummary {
  count: number;
  monthlyTotal: number;
  yearlyTotal: number;
}

export const subscriptionsList = () => invoke<UpcomingSubscription[]>("subscriptions_list");

export const subscriptionsSummary = () => invoke<SubscriptionSummary>("subscriptions_summary");

export const subscriptionsAdd = (
  name: string,
  cost: number,
  cycle: BillingCycle,
  renewalDate: string,
  notes?: string,
) => invoke<Subscription>("subscriptions_add", { name, cost, cycle, renewalDate, notes });

export const subscriptionsUpdate = (
  id: string,
  name: string,
  cost: number,
  cycle: BillingCycle,
  renewalDate: string,
  notes?: string,
) => invoke<Subscription>("subscriptions_update", { id, name, cost, cycle, renewalDate, notes });

export const subscriptionsDelete = (id: string) => invoke<void>("subscriptions_delete", { id });

// --- 2FA code picker -------------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/totp.rs`. The secret itself never round-trips
// through these calls after `totpAddAccount` — it lives only in the OS
// keychain from then on; see that module's docs.

export interface TotpAccount {
  id: string;
  label: string;
  issuer: string;
  digits: number;
  period: number;
}

export interface TotpCurrentCode {
  code: string;
  secondsRemaining: number;
  period: number;
}

export const totpListAccounts = () => invoke<TotpAccount[]>("totp_list_accounts");

export const totpAddAccount = (
  label: string,
  issuer: string | undefined,
  secret: string,
  digits?: number,
  period?: number,
) => invoke<TotpAccount>("totp_add_account", { label, issuer, secret, digits, period });

export const totpDeleteAccount = (id: string) => invoke<void>("totp_delete_account", { id });

export const totpCurrentCode = (id: string) => invoke<TotpCurrentCode>("totp_current_code", { id });

// --- Wallpaper switching ---------------------------------------------------------
//
// Mirrors `src-tauri/src/tools/wallpaper.rs`. Sets every desktop/Space's
// picture via `osascript` — nothing installed, no private API.

export const wallpaperSet = (path: string) => invoke<ToolOutcome>("wallpaper_set", { path });

// --- Own-window opacity -----------------------------------------------------------
//
// Mirrors `window::set_window_opacity` in `src-tauri/src/window/mod.rs`.
// Scoped to Caduceus's own windows — there is no macOS API for dimming a
// window that belongs to another process, and this deliberately does not
// reach for the private one. macOS only.

export type OpacityTarget = "staff" | "command_center";

export const windowSetOpacity = (target: OpacityTarget, opacity: number) =>
  invoke<void>("window_set_opacity", { target, opacity });
