/**
 * Thin wrappers over the PopBar commands in `src-tauri/src/popbar.rs`.
 *
 * Same reasoning as `src/widgets/widgetApi.ts`: `src/shared/api.ts` is where
 * every call into Rust is supposed to live, but this module owns only
 * `src/popbar/**` — the crate owner's file is theirs to fold these three
 * command names into once they are registered in `generate_handler!` (see
 * the doc comment on `popbar::handle_shortcut` in the Rust module for the
 * exact integration still needed). Calling `invoke`/`listen` directly here
 * keeps that a pure addition instead of a merge conflict.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { TextAiAction } from "@/shared/types";

import type { PopbarShowPayload } from "./types";

/** Mirrors `popbar::POPBAR_SHOW_EVENT`. */
export const POPBAR_SHOW_EVENT = "caduceus://popbar-show";

/**
 * Whatever the most recent hotkey press captured, read once on mount.
 *
 * Exists to close the cold-start race described on `popbar::popbar_pending`:
 * the very first time the PopBar's window is built, Rust shows it and emits
 * {@link POPBAR_SHOW_EVENT} in the same call, with no guarantee this page's
 * listener is attached yet. Every subsequent open is not at risk, but the
 * frontend cannot tell which kind of open it is about to get — so it always
 * calls this on mount *and* subscribes with {@link onPopbarShow}.
 */
export function popbarPending(): Promise<PopbarShowPayload | null> {
  return invoke("popbar_pending");
}

/**
 * Run one Highlight & Act transformation and copy the result to the
 * clipboard — a single round trip into `tools::textai::run` via
 * `popbar::popbar_run`. `targetLanguage` is only read for `"translate"`.
 */
export function popbarRun(
  action: TextAiAction,
  text: string,
  targetLanguage?: string,
): Promise<string> {
  return invoke("popbar_run", { action, text, targetLanguage: targetLanguage ?? null });
}

/** Close the PopBar. Escape and click-away are handled on the Rust side
 * (see the module docs on `popbar.rs` for why); this is for an in-app
 * "close"/"back" affordance and for auto-dismiss after an action finishes. */
export function popbarDismiss(): Promise<void> {
  return invoke("popbar_dismiss");
}

/** Subscribe to every future PopBar open. Pairs with {@link popbarPending}
 * for the one open that can race it — see that function's doc comment. */
export function onPopbarShow(handler: (payload: PopbarShowPayload) => void): Promise<UnlistenFn> {
  return listen<PopbarShowPayload>(POPBAR_SHOW_EVENT, (event) => handler(event.payload));
}
