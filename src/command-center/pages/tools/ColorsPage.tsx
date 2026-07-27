/**
 * Colours: one page instead of eight commands.
 *
 * Raycast splits this across Pick Color, Convert Color, Color Names, Color
 * Wheel, Generate Colors, Organize Colors and Extract Color from Selected
 * Image. They are all the same activity — you have a colour, or you want one,
 * and then you want to know things about it — so splitting them means knowing
 * which of eight commands holds the number you need before you can go and get
 * it.
 *
 * Here there is one colour at a time and everything true about it is on the
 * page: every notation, what it is nearest to by name, its tints and shades,
 * its harmonies, and what text is legible on it. Pick it off the screen, type
 * it, or pull a whole palette out of an image.
 *
 * Everything runs in this process. No network, and no Screen Recording grant —
 * the screen picker is macOS's own loupe, so you point at the colour rather
 * than the app reading your display.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import {
  describe,
  extractPalette,
  harmonies,
  parseColor,
  readableOn,
  rgbToHex,
  scale,
  wcag,
  type Rgb,
} from "@/shared/color";
import type { ToolPageProps } from "../ToolPage";
import { Button, cx } from "@/shared/ui";

const DEFAULT = "#6366f1";

export function ColorsPage({ onSetTitle }: ToolPageProps) {
  const [text, setText] = useState(DEFAULT);
  const [against, setAgainst] = useState("#ffffff");
  const [palette, setPalette] = useState<{ hex: string; share: number }[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => onSetTitle("Colors"), [onSetTitle]);

  const rgb = useMemo(() => parseColor(text), [text]);
  const backdrop = useMemo(() => parseColor(against), [against]);

  const copy = useCallback((value: string) => {
    navigator.clipboard
      .writeText(value)
      .then(() => setNote(`Copied ${value}`))
      .catch(() => setNote("Could not copy."));
  }, []);

  // Clear the little confirmation on its own; it is feedback, not state.
  useEffect(() => {
    if (!note) return;
    const timer = setTimeout(() => setNote(null), 2200);
    return () => clearTimeout(timer);
  }, [note]);

  const pickFromScreen = async () => {
    setPicking(true);
    try {
      const picked = await api.pickScreenColor();
      // `null` is Escape, not a failure. Saying "cancelled" at somebody who
      // just pressed Escape is noise.
      if (picked) setText(picked);
    } catch (error) {
      setNote(api.errorMessage(error));
    } finally {
      setPicking(false);
    }
  };

  const readImage = async (file: File) => {
    try {
      const bitmap = await createImageBitmap(file);
      // Downscale first: a 6000px photo is 36M pixels and none of the extra
      // precision changes which twelve colours come out.
      const side = 240;
      const ratio = Math.min(side / bitmap.width, side / bitmap.height, 1);
      const w = Math.max(1, Math.round(bitmap.width * ratio));
      const h = Math.max(1, Math.round(bitmap.height * ratio));

      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) throw new Error("This webview has no 2D canvas.");
      context.drawImage(bitmap, 0, 0, w, h);

      const found = extractPalette(context.getImageData(0, 0, w, h).data);
      setPalette(found);
      if (found[0]) setText(found[0].hex);
      setNote(`${found.length} colours in ${file.name}`);
    } catch (error) {
      setNote(error instanceof Error ? error.message : "Could not read that image.");
    }
  };

  return (
    <div className="mx-auto h-full max-w-[760px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Colors</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Pick one off the screen or type any notation. Everything below updates together —
          click any value to copy it.
        </p>
      </div>

      {/* --- the colour ---------------------------------------------------- */}
      <div className="mb-4 flex flex-wrap items-stretch gap-3">
        <div
          className="flex h-[120px] w-[180px] shrink-0 items-end justify-start rounded-cad border border-line p-3"
          style={{ background: rgb ? rgbToHex(rgb) : "transparent" }}
        >
          <span
            className="font-mono text-[13px]"
            style={{ color: rgb ? readableOn(rgb) : undefined }}
          >
            {rgb ? rgbToHex(rgb) : "—"}
          </span>
        </div>

        <div className="flex min-w-[240px] flex-1 flex-col gap-2">
          <div className="row gap-2">
            <input
              type="color"
              aria-label="Colour"
              value={rgb ? rgbToHex(rgb) : DEFAULT}
              onChange={(e) => setText(e.target.value)}
              className="h-9 w-12 shrink-0 cursor-pointer rounded-lg border border-line bg-base/40 p-1"
            />
            <input
              value={text}
              spellCheck={false}
              onChange={(e) => setText(e.target.value)}
              placeholder="#6366f1 · rgb(99 102 241) · hsl(239 84% 67%) · indigo"
              className={cx(
                "w-full rounded-lg border bg-base/40 px-3 py-2 font-mono text-[13px] text-ink",
                "placeholder:text-ink-faint focus:outline-none",
                rgb ? "border-line focus:border-accent/50" : "border-danger/50",
              )}
            />
          </div>

          <div className="row flex-wrap gap-2">
            <Button size="sm" tone="primary" onClick={() => void pickFromScreen()} disabled={picking}>
              {picking ? "Point at a colour…" : "Pick off the screen"}
            </Button>
            <Button size="sm" onClick={() => fileRef.current?.click()}>
              Colours from an image
            </Button>
            <input
              ref={fileRef}
              type="file"
              accept="image/*"
              hidden
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = "";
                if (file) void readImage(file);
              }}
            />
          </div>

          {!rgb && (
            <p className="text-2xs text-danger">
              Not a colour Caduceus recognises. Hex, rgb(), hsl() and CSS names all work.
            </p>
          )}
          {note && <p className="text-2xs text-ink-mute">{note}</p>}
        </div>
      </div>

      {rgb && (
        <>
          {/* --- every notation ------------------------------------------ */}
          <Section title="Values">
            <div className="grid gap-1 sm:grid-cols-2">
              {describe(rgb).map((row) => (
                <button
                  key={row.label}
                  type="button"
                  onClick={() => copy(row.value)}
                  title="Copy"
                  className="flex items-center gap-3 rounded-lg px-2.5 py-1.5 text-left transition-colors hover:bg-raised/60"
                >
                  <span className="w-28 shrink-0 text-2xs uppercase tracking-[0.08em] text-ink-faint">
                    {row.label}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-2xs text-ink">
                    {row.value}
                  </span>
                </button>
              ))}
            </div>
          </Section>

          {/* --- contrast -------------------------------------------------- */}
          <Section title="Contrast">
            <div className="row mb-3 flex-wrap gap-2">
              <span className="text-2xs text-ink-faint">Against</span>
              <input
                type="color"
                aria-label="Background colour"
                value={backdrop ? rgbToHex(backdrop) : "#ffffff"}
                onChange={(e) => setAgainst(e.target.value)}
                className="h-7 w-10 shrink-0 cursor-pointer rounded border border-line bg-base/40 p-0.5"
              />
              <input
                value={against}
                spellCheck={false}
                onChange={(e) => setAgainst(e.target.value)}
                className="w-40 rounded-lg border border-line bg-base/40 px-2 py-1 font-mono text-2xs text-ink focus:border-accent/50 focus:outline-none"
              />
              <Button size="sm" tone="ghost" onClick={() => setAgainst("#ffffff")}>
                White
              </Button>
              <Button size="sm" tone="ghost" onClick={() => setAgainst("#000000")}>
                Black
              </Button>
            </div>

            {backdrop ? <Contrast foreground={rgb} background={backdrop} /> : (
              <p className="text-2xs text-danger">That background is not a colour.</p>
            )}
          </Section>

          {/* --- scale and harmonies -------------------------------------- */}
          <Section title="Tints and shades">
            <Swatches items={scale(rgb)} onPick={setText} onCopy={copy} />
          </Section>

          <Section title="Goes with">
            <Swatches items={harmonies(rgb)} onPick={setText} onCopy={copy} />
          </Section>
        </>
      )}

      {palette.length > 0 && (
        <Section title="From your image">
          <Swatches
            items={palette.map((p) => ({
              label: `${Math.round(p.share * 100)}%`,
              hex: p.hex,
            }))}
            onPick={setText}
            onCopy={copy}
          />
        </Section>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mb-5">
      <p className="eyebrow mb-2">{title}</p>
      <div className="rounded-cad border border-line bg-surface/50 p-3">{children}</div>
    </section>
  );
}

function Swatches({
  items,
  onPick,
  onCopy,
}: {
  items: { label: string; hex: string }[];
  onPick: (hex: string) => void;
  onCopy: (hex: string) => void;
}) {
  return (
    <div className="flex flex-wrap gap-2">
      {items.map((item) => (
        <div key={`${item.label}-${item.hex}`} className="w-[86px]">
          <button
            type="button"
            onClick={() => onPick(item.hex)}
            onAuxClick={() => onCopy(item.hex)}
            title={`${item.hex} — click to make it the colour, middle-click to copy`}
            className="h-12 w-full rounded-lg border border-line transition-transform hover:scale-[1.03]"
            style={{ background: item.hex }}
          />
          <p className="mt-1 truncate text-center text-2xs text-ink-faint">{item.label}</p>
          <button
            type="button"
            onClick={() => onCopy(item.hex)}
            className="w-full truncate text-center font-mono text-[10px] text-ink-mute transition-colors hover:text-ink"
          >
            {item.hex}
          </button>
        </div>
      ))}
    </div>
  );
}

/** The WCAG verdict, said in words rather than as four unexplained ticks. */
function Contrast({ foreground, background }: { foreground: Rgb; background: Rgb }) {
  const verdict = wcag(foreground, background);
  const ratio = verdict.ratio.toFixed(2);

  return (
    <div className="flex flex-wrap items-center gap-4">
      <div
        className="flex h-[68px] min-w-[180px] flex-1 items-center justify-center rounded-lg border border-line"
        style={{ background: rgbToHex(background) }}
      >
        <span className="text-[15px] font-medium" style={{ color: rgbToHex(foreground) }}>
          Text at this contrast
        </span>
      </div>

      <div className="shrink-0">
        <p className="font-mono text-[19px] tabular-nums text-ink">{ratio}:1</p>
        <div className="mt-1 grid grid-cols-2 gap-x-3 gap-y-0.5">
          <Verdict ok={verdict.aaNormal} label="AA body" />
          <Verdict ok={verdict.aaLarge} label="AA large" />
          <Verdict ok={verdict.aaaNormal} label="AAA body" />
          <Verdict ok={verdict.aaaLarge} label="AAA large" />
        </div>
      </div>
    </div>
  );
}

function Verdict({ ok, label }: { ok: boolean; label: string }) {
  return (
    <span className={cx("text-2xs", ok ? "text-positive" : "text-ink-faint")}>
      {ok ? "✓" : "✕"} {label}
    </span>
  );
}
