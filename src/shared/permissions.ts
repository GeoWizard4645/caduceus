/**
 * The macOS grants Caduceus can ask for, and how to actually get them.
 *
 * # Why this exists as data
 *
 * "Caduceus does not have Accessibility permission yet" is a true sentence and a
 * useless one. It names a thing the reader has never heard of, does not say
 * where it lives, and leaves them to find a switch in a Settings app that has
 * reorganised itself twice in three releases.
 *
 * So every grant is written down here with the pane it lives in and the exact
 * clicks, and every place that can hit a permission wall renders the same page
 * from it — with a button that opens the pane, and a status line that notices
 * the moment the switch flips. Nothing in the app is allowed to fail with a
 * sentence and a shrug.
 */

import type { SystemSettingsPane } from "./api";
import type { PermissionId } from "./tabs";

export interface PermissionInfo {
  id: PermissionId;
  /** What the switch is called in System Settings. */
  title: string;
  /** One line: what Caduceus does with it, in the user's terms. */
  why: string;
  /** Where it lives, written the way the Settings app spells it. */
  path: string;
  pane: SystemSettingsPane;
  /** The clicks, in order. Rendered as a numbered list. */
  steps: string[];
  /**
   * Whether Caduceus can tell from inside the app that it has been granted.
   *
   * Accessibility and Screen Recording report themselves. Microphone and
   * Automation do not, so their pages say so rather than showing a status that
   * is really a guess.
   */
  detectable: boolean;
}

export const PERMISSIONS: Record<PermissionId, PermissionInfo> = {
  accessibility: {
    id: "accessibility",
    title: "Accessibility",
    why: "Moving, resizing and snapping other apps' windows, and reading the text you have selected, all go through this one switch.",
    path: "Privacy & Security → Accessibility",
    pane: "accessibility",
    steps: [
      "Click the button below — it opens straight to the right pane.",
      "Find Caduceus in the list on the right.",
      "Turn its switch on. macOS will ask for your password or Touch ID.",
      "Come back here. This page notices on its own; nothing needs restarting.",
    ],
    detectable: true,
  },
  "screen-recording": {
    id: "screen-recording",
    title: "Screen Recording",
    why: "Screenshots, screen recording and reading text off the screen. macOS files all three under this name even when nothing is being recorded.",
    path: "Privacy & Security → Screen & System Audio Recording",
    pane: "screen-recording",
    steps: [
      "Click the button below to open Screen & System Audio Recording.",
      "Find Caduceus in the list and turn its switch on.",
      "macOS asks you to quit and reopen Caduceus for this one. Use Quit in the menu-bar icon.",
    ],
    detectable: true,
  },
  microphone: {
    id: "microphone",
    title: "Microphone",
    why: "Push-to-talk dictation. Audio is transcribed and then discarded; nothing is kept.",
    path: "Privacy & Security → Microphone",
    pane: "microphone",
    steps: [
      "Click the button below to open Microphone.",
      "Find Caduceus in the list and turn its switch on.",
      "Try dictation again — the first attempt after granting it usually needs a second press.",
    ],
    detectable: false,
  },
  automation: {
    id: "automation",
    title: "Automation",
    why: "Telling another app to do something — skipping a track in Music, reading the Finder's selection. macOS asks separately for each app Caduceus talks to.",
    path: "Privacy & Security → Automation",
    pane: "automation",
    steps: [
      "Click the button below to open Automation.",
      "Find Caduceus, and unfold it — every app it has asked to control is listed underneath.",
      "Turn on the app you were trying to reach.",
    ],
    detectable: false,
  },
};

/**
 * The canonical sentence for each wall.
 *
 * Anywhere the code *knows* which permission is missing — rather than having to
 * read it out of a message — it says so with one of these, so that
 * {@link permissionFromMessage} is matching a value this file owns instead of
 * guessing at prose written somewhere else.
 */
export const PERMISSION_WALL: Record<PermissionId, string> = {
  accessibility:
    "Caduceus needs Accessibility permission for this. Grant it in System Settings → Privacy & Security → Accessibility.",
  "screen-recording":
    "Caduceus needs Screen Recording permission for this. Grant it in System Settings → Privacy & Security → Screen & System Audio Recording.",
  microphone:
    "Caduceus needs Microphone permission for this. Grant it in System Settings → Privacy & Security → Microphone.",
  automation:
    "Caduceus needs Automation permission to control that app. Grant it in System Settings → Privacy & Security → Automation.",
};

/**
 * Recognise a permission wall in a message the backend produced.
 *
 * Not general-purpose string sniffing: every sentence matched here is written
 * by Caduceus itself, in `window::accessibility::describe_error` and
 * `tools::system::osa`. The alternative — threading a typed reason through
 * every one of the ~40 outcome shapes — would be a lot of plumbing for the same
 * four answers, and this is checked by a test.
 */
export function permissionFromMessage(message: string): PermissionId | null {
  const text = message.toLowerCase();
  if (text.includes("automation")) return "automation";
  if (text.includes("accessibility")) return "accessibility";
  if (text.includes("screen recording") || text.includes("screen & system audio")) {
    return "screen-recording";
  }
  if (text.includes("microphone")) return "microphone";
  return null;
}
