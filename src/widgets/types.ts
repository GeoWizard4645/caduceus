/**
 * A widget's identity, content selector, and on-screen geometry — the
 * frontend's view of `WidgetLayout` in `src-tauri/src/widgets.rs`. Kept in
 * sync by hand; there is no shared codegen between the two sides, the same
 * as every other Rust struct mirrored under `src/shared/types.ts`.
 */
export interface WidgetLayout {
  id: string;
  /** Which content to render — `"clock"` today. See `WidgetContent` in
   * `WidgetApp.tsx` for the mapping from kind to component. */
  kind: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

declare global {
  interface Window {
    /**
     * Set by `widgets.rs::spawn_widget_window`'s init script before any other
     * script on the page runs. A global rather than a URL query string keeps
     * `widget.html` — one entry point shared by every widget instance — free
     * of any id/kind parsing of its own.
     */
    __CADUCEUS_WIDGET__?: WidgetLayout;
  }
}
