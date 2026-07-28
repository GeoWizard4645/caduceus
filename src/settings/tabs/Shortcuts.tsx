import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import * as api from "@/shared/api";
import { COMMANDS } from "@/shared/commands";
import { GLYPH_NAMES, GLYPH_PREFIX } from "@/shared/glyphs";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import type { RuntimeInfo, Shortcut, ShortcutKind } from "@/shared/types";
import { STAFF_POPOUT_LIMIT } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  IconButton,
  Section,
  Select,
  TextArea,
  TextInput,
  Toggle,
  cx,
} from "@/shared/ui";

import { BrowserPicker } from "../BrowserPicker";
import type { Draft } from "../useDraft";

const KIND_LABELS: Record<ShortcutKind, string> = {
  open_url: "Open a URL",
  open_app: "Launch an app",
  run_command: "Run a shell command",
  run_applescript: "Run AppleScript",
  open_feature: "Open a Caduceus feature",
  clipboard_view: "Open clipboard history",
  system_monitor: "Open system status",
};

const TARGET_HINTS: Record<ShortcutKind, string> = {
  open_url: "Use {query} to accept text from the Command Center, e.g. https://github.com/search?q={query}",
  open_app:
    "A macOS bundle id (com.apple.Safari), an app path (/Applications/Notes.app), or an executable name.",
  run_command:
    "Runs in your login shell. {query} is inserted safely quoted; the raw text is also in $CADUCEUS_QUERY.",
  run_applescript: "AppleScript source. {query} is substituted verbatim.",
  open_feature: "Which built-in tool, command or page this opens.",
  clipboard_view: "No target needed.",
  system_monitor: "No target needed.",
};

/**
 * Every built-in feature a shortcut can point at, grouped and alphabetised.
 *
 * Derived from the command registry rather than written out, so a feature added
 * there is bindable here with no second list to update.
 */
const FEATURE_OPTIONS = COMMANDS.map((command) => ({
  value: command.id,
  label: `${command.title} · ${command.group}`,
}))
  .slice()
  .sort((a, b) => a.label.localeCompare(b.label));

export function ShortcutsTab({ draft, info }: { draft: Draft; info: RuntimeInfo | null }) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const settings = draft.settings;
  if (!settings) return null;

  const shortcuts = [...settings.shortcuts].sort((a, b) => a.orderIndex - b.orderIndex);
  const orbCount = shortcuts.filter((s) => s.showInStaff).length;

  const mutate = (id: string, change: (s: Shortcut) => void) =>
    draft.update((d) => {
      const target = d.shortcuts.find((s) => s.id === id);
      if (target) change(target);
    });

  const move = (id: string, direction: -1 | 1) =>
    draft.update((d) => {
      const ordered = [...d.shortcuts].sort((a, b) => a.orderIndex - b.orderIndex);
      const index = ordered.findIndex((s) => s.id === id);
      const target = index + direction;
      if (index < 0 || target < 0 || target >= ordered.length) return;
      [ordered[index], ordered[target]] = [ordered[target], ordered[index]];
      // Re-index from scratch so ordering stays dense even after deletions.
      ordered.forEach((s, i) => {
        const item = d.shortcuts.find((x) => x.id === s.id);
        if (item) item.orderIndex = i;
      });
    });

  const add = () => {
    const id = `sc-${crypto.randomUUID().slice(0, 8)}`;
    draft.update((d) => {
      d.shortcuts.push({
        id,
        label: "New shortcut",
        icon: "✦",
        kind: "open_url",
        target: "",
        args: [],
        browser: null,
        showInStaff: false,
        orderIndex: d.shortcuts.length,
        keywords: [],
        description: "",
        hidden: false,
      });
    });
    setExpanded(id);
  };

  return (
    <>
      <Section
        title="Shortcuts"
        description="One list powers both the staff's pop-out icons and the Command Center's results. Everything here is editable, including the six Caduceus ships with."
        actions={<Button tone="primary" onClick={add}>Add shortcut</Button>}
      >
        {orbCount > STAFF_POPOUT_LIMIT && (
          <div className="mb-4">
            <Callout tone="warn">
              {orbCount} shortcuts are marked for the staff, but it draws at most {STAFF_POPOUT_LIMIT}.
              The extras are still searchable in the Command Center — untick some, or reorder so the
              ones you want come first.
            </Callout>
          </div>
        )}

        <div className="space-y-2">
          {shortcuts.map((shortcut, index) => (
            <ShortcutRow
              key={shortcut.id}
              shortcut={shortcut}
              info={info}
              expanded={expanded === shortcut.id}
              isFirst={index === 0}
              isLast={index === shortcuts.length - 1}
              onToggle={() => setExpanded(expanded === shortcut.id ? null : shortcut.id)}
              onChange={(change) => mutate(shortcut.id, change)}
              onMove={(direction) => move(shortcut.id, direction)}
              onDelete={() =>
                draft.update((d) => {
                  d.shortcuts = d.shortcuts.filter((s) => s.id !== shortcut.id);
                })
              }
            />
          ))}
        </div>

        {shortcuts.length === 0 && (
          <p className="py-8 text-center text-2xs text-ink-faint">
            No shortcuts. Add one, or reset settings to get the defaults back.
          </p>
        )}
      </Section>
    </>
  );
}

function ShortcutRow({
  shortcut,
  info,
  expanded,
  isFirst,
  isLast,
  onToggle,
  onChange,
  onMove,
  onDelete,
}: {
  shortcut: Shortcut;
  info: RuntimeInfo | null;
  expanded: boolean;
  isFirst: boolean;
  isLast: boolean;
  onToggle: () => void;
  onChange: (change: (s: Shortcut) => void) => void;
  onMove: (direction: -1 | 1) => void;
  onDelete: () => void;
}) {
  const [testResult, setTestResult] = useState<string | null>(null);
  // The frontend-handled kinds have nothing to point at.
  const needsTarget =
    shortcut.kind !== "clipboard_view" && shortcut.kind !== "system_monitor";
  const incomplete = needsTarget && !shortcut.target.trim();


  return (
    <div
      className={cx(
        "rounded-lg border transition-colors",
        expanded ? "border-accent/30 bg-base/40" : "border-line bg-base/20 hover:border-line-strong",
      )}
    >
      {/* --- summary row --------------------------------------------- */}
      <div className="flex items-center gap-3 px-3 py-2.5">
        <span
          aria-hidden="true"
          className="flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-md border border-line bg-raised text-[13px]"
        >
          <ShortcutIcon icon={shortcut.icon} label={shortcut.label} className="h-6 w-6" />
        </span>

        <button type="button" onClick={onToggle} className="min-w-0 flex-1 text-left">
          <p className="truncate text-[13px] font-medium text-ink">
            {shortcut.label || "Untitled"}
            {incomplete && <span className="ml-2 text-2xs font-normal text-caution">needs setup</span>}
          </p>
          <p className="truncate text-2xs text-ink-faint">
            {KIND_LABELS[shortcut.kind]}
            {shortcut.target ? ` · ${shortcut.target}` : ""}
          </p>
        </button>

        <label className="row shrink-0 cursor-pointer text-2xs text-ink-mute">
          <input
            type="checkbox"
            checked={shortcut.showInStaff}
            onChange={(e) => onChange((s) => (s.showInStaff = e.target.checked))}
            className="h-3.5 w-3.5 accent-[rgb(var(--c-accent))]"
          />
          Staff
        </label>

        <div className="row shrink-0">
          <IconButton label="Move up" disabled={isFirst} onClick={() => onMove(-1)}>
            ↑
          </IconButton>
          <IconButton label="Move down" disabled={isLast} onClick={() => onMove(1)}>
            ↓
          </IconButton>
          <IconButton label="Delete shortcut" tone="danger" onClick={onDelete}>
            ×
          </IconButton>
        </div>
      </div>

      {/* --- editor --------------------------------------------------- */}
      {expanded && (
        <div className="grid grid-cols-2 gap-4 border-t border-line px-3 pb-4 pt-4">
          <Field label="Name">
            <TextInput value={shortcut.label} onChange={(v) => onChange((s) => (s.label = v))} />
          </Field>

          <Field
            label="Icon"
            hint="Pick a glyph, upload your own image, or type any emoji. Glyphs follow your accent colour; uploads and emoji are drawn as-is."
          >
            <div className="row">
              <TextInput
                value={shortcut.icon}
                onChange={(v) => onChange((s) => (s.icon = v))}
                placeholder="glyph:sparkle or ✦"
              />
              <Button
                size="sm"
                onClick={async () => {
                  const path = await open({
                    multiple: false,
                    filters: [
                      {
                        name: "Images",
                        extensions: ["png", "jpg", "jpeg", "webp", "gif"],
                      },
                    ],
                  });
                  if (!path || typeof path !== "string") return;
                  try {
                    const token = await api.importShortcutIcon(shortcut.id, path);
                    onChange((s) => (s.icon = token));
                    setTestResult("Icon saved");
                  } catch (error) {
                    setTestResult(api.errorMessage(error));
                  }
                }}
              >
                Upload…
              </Button>
            </div>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {GLYPH_NAMES.map((name) => {
                const token = `${GLYPH_PREFIX}${name}`;
                const active = shortcut.icon === token;
                return (
                  <button
                    key={name}
                    type="button"
                    title={name}
                    aria-pressed={active}
                    onClick={() => onChange((s) => (s.icon = token))}
                    className={cx(
                      "flex h-8 w-8 items-center justify-center rounded-md border transition-colors",
                      active
                        ? "border-accent/60 bg-accent/12 text-accent"
                        : "border-line bg-raised text-ink-mute hover:border-accent/40 hover:text-ink",
                    )}
                  >
                    <ShortcutIcon icon={token} label={name} className="h-[18px] w-[18px]" />
                  </button>
                );
              })}
            </div>
          </Field>

          <Field label="Type">
            <Select
              value={shortcut.kind}
              onChange={(v) => onChange((s) => (s.kind = v))}
              options={(Object.keys(KIND_LABELS) as ShortcutKind[]).map((kind) => ({
                value: kind,
                label: KIND_LABELS[kind],
                disabled: kind === "run_applescript" && info?.platform !== "macos",
              }))}
            />
          </Field>

          <Field label="Description" hint="Shown under the name in the Command Center.">
            <TextInput
              value={shortcut.description}
              onChange={(v) => onChange((s) => (s.description = v))}
            />
          </Field>

          {needsTarget && (
            <Field label="Target" hint={TARGET_HINTS[shortcut.kind]} wide>
              {shortcut.kind === "open_feature" ? (
                // A closed list rather than a text box: the valid values are
                // exactly the registry's command ids, and typing one by hand is
                // a way to save a shortcut that silently opens nothing.
                <Select
                  value={shortcut.target}
                  onChange={(v) => onChange((s) => (s.target = v))}
                  options={[
                    { value: "", label: "Choose a feature…", disabled: true },
                    ...FEATURE_OPTIONS,
                  ]}
                />
              ) : shortcut.kind === "run_applescript" ? (
                <TextArea
                  mono
                  rows={5}
                  value={shortcut.target}
                  onChange={(v) => onChange((s) => (s.target = v))}
                  placeholder={'tell application "Finder" to activate'}
                />
              ) : (
                <TextInput
                  mono={shortcut.kind === "run_command"}
                  value={shortcut.target}
                  onChange={(v) => onChange((s) => (s.target = v))}
                  placeholder={
                    shortcut.kind === "open_url"
                      ? "https://example.com"
                      : shortcut.kind === "open_app"
                        ? "com.apple.Safari"
                        : "echo hello"
                  }
                />
              )}
            </Field>
          )}

          {shortcut.kind === "open_url" && (
            <BrowserPicker
              value={shortcut.browser}
              onChange={(next) => onChange((sc) => (sc.browser = next))}
              browsers={info?.browsers ?? []}
              label="Open in"
              inheritLabel="Use the Command Center default"
            />
          )}

          <Field
            label="Search keywords"
            hint="Extra words that should find this in the Command Center. Comma-separated."
            wide
          >
            <TextInput
              value={shortcut.keywords.join(", ")}
              onChange={(v) =>
                onChange(
                  (s) =>
                    (s.keywords = v
                      .split(",")
                      .map((k) => k.trim())
                      .filter(Boolean)),
                )
              }
            />
          </Field>

          <div className="col-span-2 flex items-center justify-between border-t border-line pt-3">
            <Toggle
              label="Hide from search"
              hint="Still works from the staff."
              checked={shortcut.hidden}
              onChange={(checked) => onChange((s) => (s.hidden = checked))}
            />

            <div className="row">
              {testResult && (
                <span className="max-w-[280px] truncate text-2xs text-ink-faint" title={testResult}>
                  {testResult}
                </span>
              )}
              <Button
                size="sm"
                disabled={incomplete}
                onClick={async () => {
                  setTestResult("Running…");
                  try {
                    const outcome = await api.runShortcut(shortcut.id);
                    setTestResult(outcome.message);
                  } catch (error) {
                    setTestResult(api.errorMessage(error));
                  }
                }}
              >
                Test
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
