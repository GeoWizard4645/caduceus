/**
 * Renders a command's declared fields as a real form.
 *
 * # Why this is one component and not a page per command
 *
 * Every feature is supposed to have an interface built for that feature. Doing
 * that with a hand-written React page per command would mean a hundred and
 * thirty files, each a slightly different opinion about where the label goes —
 * and the hundred-and-thirty-first command would get the generic textarea
 * anyway, because nobody writes the page for the boring one.
 *
 * So commands describe their inputs (see `Field` in `shared/commands.ts`) and
 * this builds the page. A direction dropdown, a length spinner, a colour swatch
 * and a "remove duplicates" tick are all one line in the registry. The handful
 * of features whose interaction genuinely *is* the feature — sampling a colour
 * off the screen, arranging files on a desktop — get a real page instead, named
 * by `CommandDef.page`.
 *
 * # Every text field takes a file
 *
 * Anything that works on a block of text works just as well on a file full of
 * it, and making somebody open the file, select all and copy first is a chore
 * the app can simply do. Files are read in the webview and passed as text, so
 * nothing needs disk access it did not already have.
 */

import { useRef, useState } from "react";

import type { Field, FieldValues } from "@/shared/commands";
import { Select, Toggle, cx } from "@/shared/ui";

export function CommandFields({
  fields,
  values,
  onChange,
  onSubmit,
  autoFocus,
}: {
  fields: Field[];
  values: FieldValues;
  onChange: (id: string, value: string) => void;
  /** ⌘↵ anywhere in the form. */
  onSubmit: () => void;
  autoFocus?: boolean;
}) {
  if (fields.length === 0) return null;

  return (
    <div className="space-y-3">
      {fields.map((field, index) => (
        <FieldRow
          key={field.id}
          field={field}
          value={values[field.id] ?? ""}
          onChange={(next) => onChange(field.id, next)}
          onSubmit={onSubmit}
          autoFocus={autoFocus && index === 0}
        />
      ))}
    </div>
  );
}

function FieldRow({
  field,
  value,
  onChange,
  onSubmit,
  autoFocus,
}: {
  field: Field;
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  autoFocus?: boolean;
}) {
  // ⌘↵ submits from anywhere in the form. Plain Enter cannot: half of these
  // fields are multi-line and the newline is the point.
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Enter" && event.metaKey) {
      event.preventDefault();
      onSubmit();
    }
  };

  return (
    <div>
      <div className="mb-1.5 flex items-baseline justify-between gap-3">
        <label htmlFor={`field-${field.id}`} className="eyebrow">
          {field.label}
        </label>
        {field.kind === "text" && field.file !== false && (
          <FileButton
            types={field.fileTypes}
            onText={onChange}
            label="Use a file"
          />
        )}
      </div>

      <FieldControl
        field={field}
        value={value}
        onChange={onChange}
        onKeyDown={onKeyDown}
        autoFocus={autoFocus}
      />

      {field.hint && <p className="mt-1 text-2xs text-ink-faint">{field.hint}</p>}
    </div>
  );
}

function FieldControl({
  field,
  value,
  onChange,
  onKeyDown,
  autoFocus,
}: {
  field: Field;
  value: string;
  onChange: (value: string) => void;
  onKeyDown: (event: React.KeyboardEvent) => void;
  autoFocus?: boolean;
}) {
  const box =
    "w-full rounded-lg border border-line bg-base/40 px-3 py-2 text-[13px] text-ink " +
    "placeholder:text-ink-faint focus:border-accent/50 focus:outline-none";

  switch (field.kind) {
    case "text":
      return field.multiline ? (
        <textarea
          id={`field-${field.id}`}
          value={value}
          rows={5}
          spellCheck={false}
          autoFocus={autoFocus}
          placeholder={field.placeholder ?? field.label}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          className={cx(box, "resize-y leading-relaxed", field.mono && "font-mono text-2xs")}
        />
      ) : (
        <input
          id={`field-${field.id}`}
          value={value}
          spellCheck={false}
          autoComplete="off"
          autoFocus={autoFocus}
          placeholder={field.placeholder ?? field.label}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          className={cx(box, field.mono && "font-mono")}
        />
      );

    case "number":
      return (
        <input
          id={`field-${field.id}`}
          type="number"
          value={value}
          min={field.min}
          max={field.max}
          step={field.step}
          autoFocus={autoFocus}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          className={cx(box, "tabular-nums")}
        />
      );

    case "select":
      return (
        <Select
          value={value || field.default || field.options[0]?.value || ""}
          onChange={onChange}
          options={field.options}
        />
      );

    case "toggle":
      return (
        <Toggle
          checked={value === "true"}
          onChange={(next) => onChange(next ? "true" : "false")}
          label={field.label}
        />
      );

    case "color":
      return (
        <div className="row gap-2">
          <input
            type="color"
            aria-label={field.label}
            value={normaliseHex(value) || "#3b82f6"}
            onChange={(e) => onChange(e.target.value)}
            className="h-9 w-12 shrink-0 cursor-pointer rounded-lg border border-line bg-base/40 p-1"
          />
          <input
            id={`field-${field.id}`}
            value={value}
            spellCheck={false}
            placeholder="#3b82f6, rgb(59 130 246), hsl(217 91% 60%)"
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={onKeyDown}
            className={cx(box, "font-mono")}
          />
        </div>
      );

    case "file":
      return (
        <div className="row gap-2">
          <FileButton
            types={field.fileTypes}
            onText={onChange}
            onPath={field.readAs === "path" ? onChange : undefined}
            label={value ? "Choose another" : "Choose a file"}
          />
          <span className="truncate text-2xs text-ink-faint">
            {value ? `${summarise(value)}` : "Nothing chosen yet"}
          </span>
        </div>
      );
  }
}

/**
 * "Use a file" — reads it in the webview and hands over the text.
 *
 * A plain `<input type="file">` rather than Tauri's dialog plugin, deliberately:
 * the browser picker gives the webview the *contents* without the app needing
 * filesystem access, so a feature that only wants the text of a file cannot
 * accidentally become a feature that can read the disk.
 */
function FileButton({
  types,
  onText,
  onPath,
  label,
}: {
  types?: string[];
  onText: (text: string) => void;
  onPath?: (path: string) => void;
  label: string;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);

  return (
    <>
      <button
        type="button"
        onClick={() => inputRef.current?.click()}
        className="shrink-0 text-2xs text-ink-faint transition-colors hover:text-ink"
      >
        {busy ? "Reading…" : label}
      </button>
      <input
        ref={inputRef}
        type="file"
        hidden
        accept={types?.map((t) => (t.startsWith(".") ? t : `.${t}`)).join(",")}
        onChange={async (event) => {
          const file = event.target.files?.[0];
          // Reset first: choosing the same file twice must fire again.
          event.target.value = "";
          if (!file) return;
          if (onPath) {
            onPath(file.name);
            return;
          }
          setBusy(true);
          try {
            onText(await file.text());
          } catch {
            onText("");
          } finally {
            setBusy(false);
          }
        }}
      />
    </>
  );
}

/** `<input type="color">` only understands `#rrggbb`. */
function normaliseHex(value: string): string | null {
  const match = /^#?([0-9a-f]{6})$/i.exec(value.trim());
  if (match) return `#${match[1]}`;
  const short = /^#?([0-9a-f]{3})$/i.exec(value.trim());
  if (short) {
    const [r, g, b] = short[1].split("");
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return null;
}

function summarise(text: string): string {
  const lines = text.split("\n").length;
  const chars = text.length;
  return `${chars.toLocaleString()} characters · ${lines.toLocaleString()} line${lines === 1 ? "" : "s"}`;
}
