/**
 * The image toolkit: compress/convert, resize to a preset, strip metadata
 * before sharing, and find duplicates sitting in a folder.
 *
 * One file in, one operation at a time — not a batch pipeline. Every one of
 * these writes a *new* file beside the source (see `tools::images`'s module
 * doc on the Rust side) so there is never a "did that just overwrite my only
 * copy" moment, and each result carries a Reveal button rather than a path
 * you have to go find yourself.
 *
 * Picking a file goes through two doors, both giving a real absolute path —
 * neither goes through a plain `<input type="file">`, which in this webview
 * hands back a `File` with no path a Rust command could act on:
 *
 * 1. Tauri's own drag-drop event (`getCurrentWebview().onDragDropEvent`),
 *    for dropping a file straight from Finder.
 * 2. The system file dialog (`@tauri-apps/plugin-dialog`), for everyone else.
 *
 * There is no pixel preview here on purpose: the asset protocol that would
 * serve a local image into an `<img>` tag is scoped (see `tauri.conf.json`)
 * to this app's own generated assets, not arbitrary paths on someone's disk,
 * and widening that scope is a security decision this page does not own.
 * The filename and a Reveal-in-Finder button are the honest substitute.
 */

import { useEffect, useState } from "react";
import type { ReactNode } from "react";

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { Button, Callout, NumberInput, Select, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "heic", "heif", "webp", "tiff", "tif", "bmp", "gif"];

const FORMATS = [
  { value: "jpg", label: "JPEG" },
  { value: "png", label: "PNG" },
  { value: "heic", label: "HEIC" },
] as const;

const PRESETS = [
  { value: "square", label: "Square — 1080×1080, Instagram/LinkedIn" },
  { value: "landscape", label: "Landscape — 1920×1080, YouTube/link cards" },
  { value: "portrait", label: "Portrait — 1080×1920, Stories/Reels/TikTok" },
  { value: "custom", label: "Custom size" },
] as const;

type Format = (typeof FORMATS)[number]["value"];
type PresetKind = (typeof PRESETS)[number]["value"];

type Status = { ok: boolean; title: string; message: string; path?: string } | null;

function extOf(path: string): string {
  const dot = path.lastIndexOf(".");
  return dot === -1 ? "" : path.slice(dot + 1).toLowerCase();
}

function basename(path: string): string {
  return path.split("/").pop() || path;
}

function dirname(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx <= 0 ? path : path.slice(0, idx);
}

export function ImagesPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Images"), [onSetTitle]);

  const [path, setPath] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [status, setStatus] = useState<Status>(null);
  const [busy, setBusy] = useState<string | null>(null);

  // --- compress / convert ---------------------------------------------------
  const [format, setFormat] = useState<Format>("jpg");
  const [quality, setQuality] = useState(80);
  const [maxDimension, setMaxDimension] = useState(0); // 0 reads as "no limit"

  // --- resize to preset ------------------------------------------------------
  const [presetKind, setPresetKind] = useState<PresetKind>("square");
  const [customWidth, setCustomWidth] = useState(1080);
  const [customHeight, setCustomHeight] = useState(1080);

  // --- background removal: a real check, not a hardcoded "coming soon" ------
  const [bgAvailable, setBgAvailable] = useState<boolean | null>(null);
  useEffect(() => {
    void api
      .backgroundRemovalAvailable()
      .then(setBgAvailable)
      .catch(() => setBgAvailable(false));
  }, []);

  // --- duplicate finder --------------------------------------------------
  const [dupDir, setDupDir] = useState("");
  const [dupThreshold, setDupThreshold] = useState(0); // 0 reads as "default sensitivity"
  const [dupGroups, setDupGroups] = useState<api.DuplicateGroup[] | null>(null);
  const [dupNote, setDupNote] = useState<string | null>(null);

  // Tauri's own drag-drop, not the DOM's — see the module doc for why.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") setDragging(true);
        else if (event.payload.type === "leave") setDragging(false);
        else if (event.payload.type === "drop") {
          setDragging(false);
          const dropped = event.payload.paths;
          const first = dropped.find((p) => IMAGE_EXTENSIONS.includes(extOf(p)));
          if (first) {
            setPath(first);
            setStatus(null);
          } else if (dropped.length) {
            setStatus({ ok: false, title: "Not an image", message: "That does not look like an image file." });
          }
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, []);

  const pickFile = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }],
    });
    if (typeof picked === "string") {
      setPath(picked);
      setStatus(null);
    }
  };

  const run = async (
    name: string,
    title: string,
    call: () => Promise<{ ok: boolean; message: string; copied: string | null }>,
  ) => {
    setBusy(name);
    setStatus(null);
    try {
      const outcome = await call();
      setStatus({ ok: outcome.ok, title, message: outcome.message, path: outcome.copied ?? undefined });
    } catch (error) {
      setStatus({ ok: false, title, message: api.errorMessage(error) });
    } finally {
      setBusy(null);
    }
  };

  const compress = () => {
    if (!path) return;
    void run("compress", "Compress / convert", () =>
      api.compressImage(path, format, quality, maxDimension > 0 ? maxDimension : undefined),
    );
  };

  const resize = () => {
    if (!path) return;
    const preset: api.ImagePreset =
      presetKind === "custom" ? { kind: "custom", width: customWidth, height: customHeight } : { kind: presetKind };
    void run("resize", "Resize", () => api.resizeImageToPreset(path, preset));
  };

  const strip = () => {
    if (!path) return;
    void run("strip", "Remove identifying data", () => api.stripImageMetadata(path));
  };

  const scanDuplicates = async () => {
    if (!dupDir.trim()) return;
    setBusy("duplicates");
    setDupNote(null);
    setDupGroups(null);
    try {
      const groups = await api.findDuplicateImages(dupDir.trim(), dupThreshold > 0 ? dupThreshold : undefined);
      setDupGroups(groups);
      if (groups.length === 0) setDupNote("No duplicates found in that folder.");
    } catch (error) {
      setDupNote(api.errorMessage(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Images</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Compress, resize, or clean a photo before sharing it — all on this Mac, nothing uploaded.
          Every result is a new file next to the original.
        </p>
      </div>

      <Section title="Pick a file">
        <div
          className={cx(
            "flex flex-col items-center justify-center gap-2 rounded-cad border-2 border-dashed px-6 py-8 text-center transition-colors",
            dragging ? "border-accent bg-accent/5" : "border-line-strong/60 bg-raised/40",
          )}
        >
          <p className="text-[13px] font-medium text-ink">
            {dragging ? "Drop it" : path ? basename(path) : "Drag an image here"}
          </p>
          {path && <p className="max-w-[46ch] truncate text-2xs text-ink-faint">{path}</p>}
          <div className="row mt-1 gap-2">
            <Button size="sm" onClick={() => void pickFile()}>
              {path ? "Choose another" : "Choose a file…"}
            </Button>
            {path && (
              <Button size="sm" tone="ghost" onClick={() => void api.revealPath(path).catch(() => {})}>
                Reveal in Finder
              </Button>
            )}
          </div>
        </div>
      </Section>

      {status && (
        <div
          className={cx(
            "mb-5 rounded-cad border px-3.5 py-3 text-[13px] leading-relaxed",
            status.ok
              ? "border-positive/30 bg-positive/[0.06] text-ink-soft"
              : "border-danger/30 bg-danger/[0.06] text-ink-soft",
          )}
        >
          <p className="font-medium text-ink">{status.title}</p>
          <p className="mt-0.5">{status.message}</p>
          {status.path && (
            <Button size="sm" className="mt-2" onClick={() => void api.revealPath(status.path!).catch(() => {})}>
              Reveal the result
            </Button>
          )}
        </div>
      )}

      <Section title="Compress or convert">
        <div className="grid gap-3 sm:grid-cols-2">
          <Labeled label="Format">
            <Select
              value={format}
              onChange={setFormat}
              options={FORMATS.map((f) => ({ value: f.value, label: f.label }))}
            />
          </Labeled>
          {format !== "png" && (
            <Labeled label="Quality" hint="Only jpeg/heic use this — png is lossless.">
              <NumberInput value={quality} onChange={setQuality} min={1} max={100} suffix="%" />
            </Labeled>
          )}
          <Labeled label="Max dimension" hint="0 = no limit. Never enlarges a smaller image.">
            <NumberInput value={maxDimension} onChange={setMaxDimension} min={0} step={10} suffix="px" />
          </Labeled>
        </div>
        <p className="mt-2 text-2xs text-ink-faint">
          WebP is not offered as an output — macOS's sips can read WebP but has no WebP encoder.
        </p>
        <Button className="mt-3" tone="primary" onClick={compress} disabled={!path || busy !== null}>
          {busy === "compress" ? "Working…" : "Compress"}
        </Button>
      </Section>

      <Section title="Resize to a preset">
        <div className="grid gap-3 sm:grid-cols-2">
          <Labeled label="Target" wide={presetKind !== "custom"}>
            <Select
              value={presetKind}
              onChange={setPresetKind}
              options={PRESETS.map((p) => ({ value: p.value, label: p.label }))}
            />
          </Labeled>
          {presetKind === "custom" && (
            <>
              <Labeled label="Width">
                <NumberInput value={customWidth} onChange={setCustomWidth} min={1} suffix="px" />
              </Labeled>
              <Labeled label="Height">
                <NumberInput value={customHeight} onChange={setCustomHeight} min={1} suffix="px" />
              </Labeled>
            </>
          )}
        </div>
        <p className="mt-2 text-2xs text-ink-faint">
          Cropped to the target ratio first (centered), then resampled — never stretched.
        </p>
        <Button className="mt-3" tone="primary" onClick={resize} disabled={!path || busy !== null}>
          {busy === "resize" ? "Working…" : "Resize"}
        </Button>
      </Section>

      <Section title="Remove identifying data">
        <p className="text-[13px] leading-relaxed text-ink-mute">
          Strips GPS coordinates, camera make/model and capture timestamps by decoding the image to
          raw pixels and re-encoding it — nothing from the original container carries through, which
          is the only way to be sure it is actually gone rather than just hidden.
        </p>
        <Button className="mt-3" tone="primary" onClick={strip} disabled={!path || busy !== null}>
          {busy === "strip" ? "Working…" : "Strip metadata"}
        </Button>
      </Section>

      <Section title="Remove the background">
        {bgAvailable ? (
          <Button tone="primary" disabled>
            Remove background
          </Button>
        ) : (
          <Callout tone="info" title="Not available yet">
            This needs a small on-device Vision helper (macOS 14+) that has not been built yet — the
            capability exists in macOS itself, but Caduceus has no code calling it. Offering a button
            here would just fail every time, so it stays off until that helper ships.
          </Callout>
        )}
      </Section>

      <Section title="Find duplicate photos in a folder">
        <p className="mb-2 text-2xs text-ink-faint">
          Scans one folder — not its subfolders — comparing images by how they look, not their bytes,
          so a re-export or a resave still matches.
        </p>
        <div className="row flex-wrap gap-2">
          <input
            value={dupDir}
            spellCheck={false}
            onChange={(e) => setDupDir(e.target.value)}
            placeholder="~/Pictures"
            className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-1.5 font-mono text-2xs text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          {path && (
            <Button size="sm" tone="ghost" onClick={() => setDupDir(dirname(path))}>
              Use the picked file's folder
            </Button>
          )}
          <Button
            size="sm"
            onClick={() => {
              void open({ directory: true, multiple: false }).then((picked) => {
                if (typeof picked === "string") setDupDir(picked);
              });
            }}
          >
            Choose a folder…
          </Button>
        </div>

        <div className="mt-3 max-w-[220px]">
          <Labeled label="Sensitivity" hint="0 = default. Higher matches looser near-duplicates.">
            <NumberInput value={dupThreshold} onChange={setDupThreshold} min={0} max={64} />
          </Labeled>
        </div>

        <Button
          className="mt-3"
          tone="primary"
          onClick={() => void scanDuplicates()}
          disabled={!dupDir.trim() || busy !== null}
        >
          {busy === "duplicates" ? "Scanning…" : "Scan for duplicates"}
        </Button>

        {dupNote && <p className="mt-3 text-2xs text-ink-mute">{dupNote}</p>}

        {dupGroups && dupGroups.length > 0 && (
          <div className="mt-3 space-y-2">
            {dupGroups.map((group, i) => (
              <div key={i} className="rounded-lg border border-line bg-raised/40 p-2.5">
                <p className="mb-1.5 text-2xs font-medium text-ink">
                  {group.files.length} images that look the same
                </p>
                <ul className="space-y-1">
                  {group.files.map((file) => (
                    <li key={file} className="row justify-between gap-2">
                      <span className="min-w-0 flex-1 truncate text-2xs text-ink-mute" title={file}>
                        {basename(file)}
                      </span>
                      <button
                        type="button"
                        onClick={() => void api.revealPath(file).catch(() => {})}
                        className="shrink-0 text-2xs text-accent underline underline-offset-2"
                      >
                        Reveal
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-5">
      <p className="eyebrow mb-2">{title}</p>
      <div className="rounded-cad border border-line bg-surface/50 p-3">{children}</div>
    </section>
  );
}

function Labeled({
  label,
  hint,
  wide,
  children,
}: {
  label: string;
  hint?: string;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <label className={cx("block", wide && "sm:col-span-2")}>
      <span className="mb-1 block text-2xs uppercase tracking-[0.08em] text-ink-faint">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-2xs leading-relaxed text-ink-faint">{hint}</span>}
    </label>
  );
}
