/**
 * Frontend view of `src-tauri/src/popbar.rs`'s `PopbarShowPayload`. Kept in
 * sync by hand, the same as every other Rust struct mirrored under
 * `src/shared/types.ts` — see `src/widgets/types.ts` for the identical
 * pattern this one copies.
 */

import type { TextAiAction } from "@/shared/types";

export interface PopbarShowPayload {
  requestId: string;
  /** `null` when nothing is selected, or Caduceus lacks the Accessibility
   * permission needed to read a selection at all — see `permissionGranted`. */
  text: string | null;
  permissionGranted: boolean;
}

/** One row in the PopBar's top-level menu. */
export type PopbarMenuItem =
  | { kind: "action"; action: TextAiAction; label: string }
  | { kind: "submenu"; id: "translate" | "rewrite"; label: string };

/** One row inside a submenu — a language for Translate, a style for Rewrite. */
export interface PopbarSubmenuItem {
  action: TextAiAction;
  label: string;
  /** Only Translate's items set this; it becomes `targetLanguage` in `popbarRun`. */
  targetLanguage?: string;
}

/** Every state the bar's body can be in. Exactly one is ever rendered. */
export type PopbarView =
  | { kind: "empty" }
  | { kind: "menu" }
  | { kind: "submenu"; parent: "translate" | "rewrite" }
  | { kind: "running"; label: string }
  | { kind: "done"; label: string; preview: string }
  | { kind: "error"; message: string };
