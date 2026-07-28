/**
 * Thin wrappers over the widget commands in `src-tauri/src/widgets.rs`.
 *
 * `src/shared/api.ts` is where "every call into Rust" is supposed to live,
 * but this module owns only `src/widgets/**` — the crate owner's file is
 * theirs to wire the new command names into once the commands are registered
 * in `generate_handler!`. Until then, calling `invoke` directly here (rather
 * than reaching into `shared/api.ts`) keeps that handoff a pure addition
 * instead of a merge conflict. Once it happens, this file can be folded into
 * `shared/api.ts` or left as a thin re-export — nothing about it is special
 * beyond typing today's six commands.
 */

import { invoke } from "@tauri-apps/api/core";

import type { WidgetLayout } from "./types";

export function createWidget(
  kind: string,
  layout?: Partial<Pick<WidgetLayout, "x" | "y" | "width" | "height">>,
): Promise<WidgetLayout> {
  return invoke("widgets_create", { kind, ...layout });
}

export function destroyWidget(id: string): Promise<void> {
  return invoke("widgets_destroy", { id });
}

export function listWidgets(): Promise<WidgetLayout[]> {
  return invoke("widgets_list");
}

export function moveWidget(id: string, x: number, y: number): Promise<void> {
  return invoke("widgets_move", { id, x, y });
}

export function resizeWidget(id: string, width: number, height: number): Promise<void> {
  return invoke("widgets_resize", { id, width, height });
}

export function saveWidgetLayout(id: string): Promise<void> {
  return invoke("widgets_save_layout", { id });
}
