/**
 * Wallpaper switching.
 *
 * Mirrors `src-tauri/src/tools/wallpaper.rs`, which sets every desktop's
 * picture via `osascript`/System Events.
 *
 * Uses the native file dialog (`@tauri-apps/plugin-dialog`) rather than the
 * form registry's generic "file" field: that field reads a browser
 * `<input type="file">`, which only ever hands back a filename, never a real
 * filesystem path — and a real path is exactly what `osascript` needs here.
 */

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { Button, Callout, Section, Spinner } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "heic", "tiff", "tif", "gif", "bmp"];

export function WallpaperPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Wallpaper"), [onSetTitle]);

  const [path, setPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);

  const pick = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }],
    });
    if (typeof picked === "string") {
      setPath(picked);
      setNote(null);
    }
  };

  const apply = async () => {
    if (!path) return;
    setBusy(true);
    setNote(null);
    try {
      const outcome = await api.wallpaperSet(path);
      setNote({ ok: outcome.ok, text: outcome.message });
    } catch (e) {
      setNote({ ok: false, text: api.errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[640px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Wallpaper</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            Choose an image and set it as your desktop picture — every desktop and Space, the same way
            System Settings does it.
          </p>
        </div>

        <Section title="Choose an image">
          <div className="flex items-center gap-3">
            <Button onClick={() => void pick()}>{path ? "Choose another" : "Choose an image…"}</Button>
            <span className="min-w-0 flex-1 truncate text-2xs text-ink-faint">
              {path ?? "Nothing chosen yet"}
            </span>
          </div>

          <div className="mt-4">
            <Button tone="primary" onClick={() => void apply()} disabled={!path || busy}>
              {busy ? <Spinner /> : null} Set as wallpaper
            </Button>
          </div>

          {note && (
            <div className="mt-3">
              <Callout tone={note.ok ? "positive" : "danger"}>{note.text}</Callout>
            </div>
          )}
        </Section>
      </div>
    </div>
  );
}
