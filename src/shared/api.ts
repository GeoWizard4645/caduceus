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
  InstalledApp,
  BackendConfig,
  BrowserInstall,
  ClipboardEntry,
  ClipboardStats,
  DispatchOutcome,
  ExecOutcome,
  ParsedInput,
  RoutedText,
  RuntimeInfo,
  Settings,
  SettingsApplyReport,
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

export const openCommandCenter = (mode?: string, prefill?: string) =>
  invoke<void>("open_command_center", { mode: mode ?? null, prefill: prefill ?? null });

export const openSettingsWindow = (tab?: string) =>
  invoke<void>("open_settings_window", { tab: tab ?? null });

// --- staff -------------------------------------------------------------------

export const toggleStaff = () => invoke<boolean>("toggle_staff");

export const saveStaffPosition = () => invoke<void>("save_staff_position");

export const collapseStaffPopout = () => invoke<void>("collapse_staff_popout");

export const resolveShortcutIcon = (icon: string) =>
  invoke<string | null>("resolve_shortcut_icon", { icon });

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
