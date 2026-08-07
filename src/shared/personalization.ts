/**
 * How `settings.general.personalization` nudges palette ranking — local only,
 * same file as settings.
 *
 * The data this reads used to be written by a first-run quiz (three
 * questions: developer or not, primary focus, a handful of favorite
 * commands). The quiz is gone — asking someone what kind of user they are
 * before they have used the product tested badly — but this scoring function
 * and the `favoritesProvider` result group in `providers.ts` are still very
 * much alive, and still read whatever is in `personalization` today:
 *
 * - An existing install that completed the quiz in an earlier version keeps
 *   its answers and keeps getting the same boosts; nothing here erases them.
 * - A fresh install has an all-default profile (`primaryFocus: ""`, no
 *   favorites), which resolves to the "launcher" bucket below — a small,
 *   fixed nudge toward three general-purpose commands, not a personalized
 *   one. That was already true for every install between "first launched"
 *   and "finished the quiz" before the quiz was removed; removing it just
 *   makes that the permanent state for new installs instead of a brief one.
 *
 * Nothing writes fresh values here any more. `PrimaryFocus` is defined in
 * this file rather than imported from the deleted quiz component because
 * this scoring function is the only thing left that needs it.
 */

import type { PersonalizationProfile, Settings } from "./types";

/** `settings.general.personalization.primaryFocus`, before it is serialised
 *  to a bare `string` for the Rust side. */
export type PrimaryFocus = "launcher" | "clipboard" | "windows" | "system" | "ai" | "developer";

const DEV_COMMAND_PREFIXES = ["tool.", "dev."];
const DEV_COMMAND_IDS = new Set([
  "files.terminal",
  "files.copy-path",
  "page.processes",
]);

const FOCUS_COMMANDS: Record<PrimaryFocus, string[]> = {
  launcher: ["files.latest-download-open", "desk.emoji", "utility.permissions"],
  clipboard: ["screen.ocr", "screen.ocr-selection", "utility.clipboard"],
  windows: [
    "window.left_half",
    "window.right_half",
    "window.maximize",
    "desk.hide-others",
    "page.desktop-shapes",
  ],
  system: [
    "system.volume_up",
    "system.volume_down",
    "desk.mute",
    "desk.empty-trash",
    "system.toggle_wifi",
    "page.storage",
  ],
  ai: ["page.meeting", "page.screen-record", "page.citations"],
  developer: [
    "tool.sha256",
    "tool.json_format",
    "tool.jwt_decode",
    "tool.uuid",
    "tool.base64_encode",
  ],
};

function profileOf(settings: Settings): PersonalizationProfile {
  return settings.general.personalization ?? {
    isDeveloper: false,
    primaryFocus: "",
    favoriteCommandIds: [],
  };
}

/** Extra score for browse/search ranking from `personalization`. */
export function personalizationBoost(settings: Settings, commandId: string): number {
  const p = profileOf(settings);
  let boost = 0;

  if (p.favoriteCommandIds.includes(commandId)) boost += 420;

  if (p.isDeveloper) {
    if (DEV_COMMAND_IDS.has(commandId) || DEV_COMMAND_PREFIXES.some((pre) => commandId.startsWith(pre))) {
      boost += 120;
    }
  }

  const focus = (p.primaryFocus || "launcher") as PrimaryFocus;
  const focusList = FOCUS_COMMANDS[focus] ?? [];
  if (focusList.includes(commandId)) boost += 90;

  if (p.isDeveloper && focus === "developer") boost += 40;

  return boost;
}
