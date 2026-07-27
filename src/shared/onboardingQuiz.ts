/**
 * First-run personalization quiz — questions, feature picks, and ranking hints.
 */

export type PrimaryFocus =
  | "launcher"
  | "clipboard"
  | "windows"
  | "system"
  | "ai"
  | "developer";

export const PRIMARY_FOCUS_OPTIONS: {
  id: PrimaryFocus;
  label: string;
  detail: string;
}[] = [
  {
    id: "launcher",
    label: "Launch apps and search fast",
    detail: "You live in the palette — open things, calculate, search the web.",
  },
  {
    id: "clipboard",
    label: "Clipboard and text",
    detail: "History, snippets, OCR, and anything you copy all day.",
  },
  {
    id: "windows",
    label: "Window layout and focus",
    detail: "Snap, resize, move displays, hide other apps.",
  },
  {
    id: "system",
    label: "System control",
    detail: "Volume, sleep, files, trash, Wi‑Fi, and housekeeping.",
  },
  {
    id: "ai",
    label: "AI and automation",
    detail: "Chat, agents, meeting notes, and driving other apps.",
  },
  {
    id: "developer",
    label: "Developer workflows",
    detail: "Hashes, JSON, git-adjacent tools, terminals, and APIs.",
  },
];

export interface OnboardingFeaturePick {
  commandId: string;
  label: string;
  group: string;
}

/** Curated list for question 3 — must be real command ids. */
export const ONBOARDING_FEATURE_PICKS: OnboardingFeaturePick[] = [
  { commandId: "page.sticky-notes", label: "Sticky notes", group: "Productivity" },
  { commandId: "page.meeting", label: "Meeting notes & transcription", group: "Productivity" },
  { commandId: "page.screen-record", label: "Record the screen (+ system audio)", group: "Productivity" },
  { commandId: "page.colors", label: "Colours & contrast", group: "Productivity" },
  { commandId: "page.convert", label: "Unit & currency conversion", group: "Productivity" },
  { commandId: "page.storage", label: "Free up disk space", group: "Productivity" },
  { commandId: "page.citations", label: "Cite this page", group: "Productivity" },
  { commandId: "window.left_half", label: "Window: left half", group: "Windows" },
  { commandId: "window.right_half", label: "Window: right half", group: "Windows" },
  { commandId: "window.maximize", label: "Window: maximize", group: "Windows" },
  { commandId: "desk.hide-others", label: "Hide every app except this one", group: "Windows" },
  { commandId: "page.desktop-shapes", label: "Desktop icon shapes", group: "Windows" },
  { commandId: "screen.ocr", label: "Copy text from the screen", group: "Clipboard & text" },
  { commandId: "tool.uuid", label: "Generate a UUID", group: "Developer" },
  { commandId: "tool.sha256", label: "SHA-256 hash", group: "Developer" },
  { commandId: "tool.json_format", label: "Format JSON", group: "Developer" },
  { commandId: "tool.jwt_decode", label: "Decode a JWT", group: "Developer" },
  { commandId: "tool.base64_encode", label: "Base64 encode", group: "Developer" },
  { commandId: "system.volume_up", label: "Volume up", group: "System" },
  { commandId: "desk.mute", label: "Mute / restore volume", group: "System" },
  { commandId: "desk.empty-trash", label: "Empty the Trash", group: "System" },
  { commandId: "files.latest-download-open", label: "Open the latest download", group: "System" },
  { commandId: "spotify.play-pause", label: "Spotify: play or pause", group: "Media" },
  { commandId: "chrome.copy-url", label: "Chrome: copy this page's address", group: "Media" },
  { commandId: "safari.copy-url", label: "Safari: copy this page's address", group: "Media" },
  { commandId: "page.processes", label: "Process manager", group: "System" },
];

export const ONBOARDING_FEATURE_GROUPS = [
  ...new Set(ONBOARDING_FEATURE_PICKS.map((p) => p.group)),
];

export const MAX_ONBOARDING_FAVORITES = 12;

/** Default focus when the user skips question 2. */
export const DEFAULT_PRIMARY_FOCUS: PrimaryFocus = "launcher";
