/**
 * Which macOS privacy grants a command needs before it can run.
 *
 * Checked when a tool tab opens so the app can prompt and walk through Settings
 * instead of failing mid-action with a sentence about a pane.
 */

import type { CommandDef, ToolPageId } from "./commands";
import type { PermissionId } from "./tabs";

const PAGE_PERMISSIONS: Partial<Record<ToolPageId, PermissionId[]>> = {
  "screen-record": ["screen-recording"],
  meeting: ["screen-recording", "microphone", "speech-recognition"],
  "desktop-shapes": ["automation"],
  "desktop-sort": ["automation"],
  "sticky-notes": ["automation"],
  citations: ["automation"],
};

const ID_PREFIX: [prefix: string, permissions: PermissionId[]][] = [
  ["window.", ["accessibility"]],
  ["screen.", ["screen-recording"]],
  ["spotify.", ["automation"]],
  ["chrome.", ["automation"]],
  ["safari.", ["automation"]],
  ["arc.", ["automation"]],
  ["firefox.", ["automation"]],
  ["music.", ["automation"]],
];

const ID_EXACT: Record<string, PermissionId[]> = {
  "system.toggle_dark_mode": ["automation"],
  "system.brightness_up": ["accessibility"],
  "system.brightness_down": ["accessibility"],
  "desk.hide-others": ["accessibility"],
  "desk.quit-others": ["accessibility"],
};

/** Grants this command may need on macOS. Empty when none apply. */
export function permissionsForCommand(command: Pick<CommandDef, "id" | "page">): PermissionId[] {
  if (command.page && PAGE_PERMISSIONS[command.page]) {
    return [...PAGE_PERMISSIONS[command.page]!];
  }

  const exact = ID_EXACT[command.id];
  if (exact) return [...exact];

  for (const [prefix, perms] of ID_PREFIX) {
    if (command.id.startsWith(prefix)) return [...perms];
  }

  return [];
}
