/**
 * How onboarding answers nudge palette ranking — local only, same file as settings.
 */

import type { PersonalizationProfile, Settings } from "./types";
import type { PrimaryFocus } from "./onboardingQuiz";

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

/** Extra score for browse/search ranking from the onboarding quiz. */
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
