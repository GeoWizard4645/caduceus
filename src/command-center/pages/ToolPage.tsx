/**
 * A page for one command.
 *
 * # Why every command has one
 *
 * A command that takes an argument used to be reachable only by knowing to type
 * `sha256 ` first. Pick it out of the list instead — which is how anyone finds
 * it the first time — and it ran with nothing to work on and answered "Type
 * something after the command first." The list was advertising a hundred and
 * thirty things that did not work when clicked.
 *
 * So a command is not a line you have to know the syntax of. It is a feature
 * with a page: the inputs it actually needs, the answer underneath, and the
 * buttons you were going to reach for anyway. Typing `sha256 hello` in the
 * palette still runs it in one keystroke — that path is faster and it is kept —
 * but it is now the shortcut, not the only door.
 *
 * # Where the page comes from
 *
 * Most commands declare their fields and {@link CommandFields} builds the form:
 * a direction dropdown for sorting, a length for a password, a swatch for a
 * colour. The few whose interaction *is* the feature — sampling a colour off
 * the screen, arranging a desktop — name a bespoke page instead, and are
 * dispatched below.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import {
  COMMANDS,
  formFor,
  primaryValue,
  type CommandActions,
  type CommandDef,
  type CommandOutput,
  type FieldValues,
} from "@/shared/commands";
import { permissionFromMessage } from "@/shared/permissions";
import { permissionsForCommand } from "@/shared/commandPermissions";
import { useDebounced, useEscape } from "@/shared/hooks";
import type { Tab } from "@/shared/tabs";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { Button, EmptyState, Spinner, cx } from "@/shared/ui";
import { recordUsage } from "@/shared/usage";

import { PermissionGate, usePermissionGate } from "../PermissionGate";

import { CommandFields } from "./CommandForm";
import { TOOL_PAGES } from "./tools";

export interface ToolPageProps {
  active: boolean;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  onSetTitle: (title: string | undefined) => void;
}

export function ToolPage({
  active,
  commandId,
  prefill,
  onOpenTab,
  onSetTitle,
}: ToolPageProps & { commandId: string; prefill?: string }) {
  const command = useMemo(() => COMMANDS.find((c) => c.id === commandId), [commandId]);
  const requiredPermissions = useMemo(
    () => (command ? permissionsForCommand(command) : []),
    [command],
  );

  if (!command) {
    return (
      <EmptyState
        title="That command is gone"
        hint="It was probably renamed in an update. Close this tab and search for it again."
        icon="⌂"
      />
    );
  }

  // A feature whose interaction cannot be a form gets its own page.
  const Bespoke = command.page ? TOOL_PAGES[command.page] : undefined;
  const page = Bespoke ? (
    <Bespoke active={active} onOpenTab={onOpenTab} onSetTitle={onSetTitle} />
  ) : (
    <FormTool
      key={command.id}
      active={active}
      command={command}
      prefill={prefill}
      onOpenTab={onOpenTab}
      onSetTitle={onSetTitle}
    />
  );

  return (
    <PermissionGate
      active={active}
      permissions={requiredPermissions}
      scope={command.id}
      retryCommandId={command.id}
      onOpenTab={onOpenTab}
    >
      {page}
    </PermissionGate>
  );
}

function FormTool({
  active,
  command,
  prefill,
  onOpenTab,
  onSetTitle,
}: ToolPageProps & { command: CommandDef; prefill?: string }) {
  const form = useMemo(() => formFor(command), [command]);

  const [values, setValues] = useState<FieldValues>(() => {
    const initial: FieldValues = {};
    for (const field of form.fields) {
      if ("default" in field && field.default !== undefined) initial[field.id] = field.default;
    }
    if (prefill) initial.input = prefill;
    return initial;
  });

  const [output, setOutput] = useState<CommandOutput | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const reportPermissionWall = usePermissionGate();
  const [busy, setBusy] = useState(false);
  const [armed, setArmed] = useState(false);
  /** Which required field ⌘↵ was refused for, once it has been refused. */
  const [nudge, setNudge] = useState<string | null>(null);
  /** Show the raw text instead of the pretty rows. Copy is unaffected. */
  const [raw, setRaw] = useState(false);

  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    onSetTitle(command.title);
  }, [command.title, onSetTitle]);

  const setValue = useCallback((id: string, value: string) => {
    setValues((current) => ({ ...current, [id]: value }));
    setArmed(false);
    // Already `null` on almost every keystroke, and React skips the render when
    // it is — so this costs nothing and stops the nudge outliving its reason.
    setNudge(null);
  }, []);

  const actions = useMemo<CommandActions>(
    () => ({
      notify: (message, tone) => {
        const permission = tone === "error" ? permissionFromMessage(message) : null;
        if (permission) reportPermissionWall(permission);
        setNote(permission ? null : { text: message, ok: tone !== "error" });
      },
      showOutput: (next) => {
        setNote(null);
        setOutput(next);
      },
      setInput: (value) => setValue("input", value),
      openTab: onOpenTab,
      // Deliberately nothing. A command that dismisses the palette after doing
      // its job is right about the palette and wrong about a tab you opened on
      // purpose and are still reading.
      close: () => {},
    }),
    [onOpenTab, reportPermissionWall, setValue],
  );

  const run = useCallback(
    async (current: FieldValues, counted: boolean) => {
      setBusy(true);
      try {
        await command.run({
          input: primaryValue(current).trim(),
          values: current,
          actions,
        });
        if (counted) recordUsage(`command:${command.id}`);
      } catch (error) {
        const message = api.errorMessage(error);
        const permission = permissionFromMessage(message);
        if (permission) reportPermissionWall(permission);
        else setNote({ text: message, ok: false });
      } finally {
        setBusy(false);
      }
    },
    [actions, command, reportPermissionWall],
  );

  // --- is it fillable in ------------------------------------------------
  const missing = form.fields.filter(
    (field) => field.kind === "text" && field.required && !(values[field.id] ?? "").trim(),
  );
  const ready = missing.length === 0;

  // --- live re-run --------------------------------------------------------
  const signature = JSON.stringify(values);
  const debounced = useDebounced(signature, 180);
  useEffect(() => {
    if (!form.live || !ready) {
      if (form.live && !ready) setOutput(null);
      return;
    }
    void run(JSON.parse(debounced) as FieldValues, false);
    // `run` is stable per command; including it would double-fire each change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced, form.live, ready]);

  // A command that asked "are you sure?" has to be cancellable without taking
  // the tab — and everything typed into it — down with the answer.
  useEscape(active, () => {
    if (!armed) return false;
    setArmed(false);
    return true;
  });

  const submit = () => {
    if (!ready) {
      // The Run button explains itself by being visibly disabled. ⌘↵ has
      // nothing to grey out, so refusing silently is the whole of the feedback
      // — say which field, and put the cursor in it.
      const first = missing[0];
      setNudge(`Fill in ${first.label} first.`);
      document.getElementById(`field-${first.id}`)?.focus();
      return;
    }
    if (command.confirm && !armed) {
      setArmed(true);
      return;
    }
    setArmed(false);
    void run(values, true);
  };

  const copy = (text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => setNote({ text: "Copied.", ok: true }))
      .catch(() => setNote({ text: "Could not copy.", ok: false }));
  };

  return (
    <div ref={containerRef} className="mx-auto flex h-full max-w-[760px] flex-col px-6 py-5">
      {/* --- what this is ------------------------------------------------ */}
      <div className="mb-4 flex shrink-0 items-start gap-3">
        <span
          aria-hidden="true"
          className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-line bg-raised text-[15px] text-ink-mute"
        >
          <ShortcutIcon icon={command.icon} label={command.title} className="h-5 w-5" />
        </span>
        <div className="min-w-0">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">
            {command.title}
          </h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            {command.detail}
          </p>
          {command.trigger && (
            <p className="mt-1.5 text-2xs text-ink-faint">
              In the search bar:{" "}
              <span className="font-mono text-ink-soft">
                {command.trigger}
                {command.argument ? ` ${command.argument}` : ""}
              </span>
            </p>
          )}
        </div>
      </div>

      {/* --- inputs -------------------------------------------------------- */}
      <div className="mb-3 shrink-0">
        <CommandFields
          fields={form.fields}
          values={values}
          onChange={setValue}
          onSubmit={submit}
          autoFocus={active}
        />
      </div>

      <div className="row mb-3 shrink-0 gap-2">
        <Button tone="primary" onClick={submit} disabled={busy || !ready}>
          {busy
            ? "Working…"
            : armed
              ? "Yes, do it"
              : (form.submitLabel ?? (form.fields.length ? "Run" : `Run ${command.title}`))}
        </Button>
        {form.live && (
          <span className="text-2xs text-ink-faint">Runs as you type · ⌘↵ to force it</span>
        )}
        {command.confirm && armed && (
          <span className="text-2xs text-danger">{command.confirm}</span>
        )}
        {nudge && <span className="text-2xs text-danger">{nudge}</span>}
        {busy && <Spinner className="text-accent" />}
      </div>

      {note && (
        <p className={cx("mb-3 shrink-0 text-2xs", note.ok ? "text-ink-mute" : "text-danger")}>
          {note.text}
        </p>
      )}

      {/* --- output -------------------------------------------------------- */}
      {output ? (
        <div className="flex min-h-0 flex-1 flex-col rounded-cad border border-line bg-surface/50">
          <div className="row shrink-0 justify-between gap-2 border-b border-line px-4 py-2">
            <div className="row min-w-0 gap-2">
              <span className="text-[13px] font-medium text-ink">{output.title}</span>
              {output.message && (
                <span className="truncate text-2xs text-ink-faint">{output.message}</span>
              )}
            </div>
            <div className="row shrink-0 gap-1">
              {/* Only offered when there is something to switch between. Copy
                  always copies the plain text either way, so the toggle can
                  never change what ends up on the clipboard. */}
              {output.rows?.length ? (
                <Button size="sm" tone="ghost" onClick={() => setRaw((r) => !r)}>
                  {raw ? "Formatted" : "Just text"}
                </Button>
              ) : null}
              <Button size="sm" onClick={() => copy(output.text)}>
                Copy
              </Button>
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  api
                    .addToNotes(output.text, output.title)
                    .then((result) => setNote({ text: result.message, ok: true }))
                    .catch((error) => setNote({ text: api.errorMessage(error), ok: false }));
                }}
              >
                Save to Notes
              </Button>
            </div>
          </div>

          {output.rows?.length && !raw ? (
            <div className="min-h-0 flex-1 overflow-auto p-2">
              {output.rows.map((row) => (
                <button
                  key={row.label}
                  type="button"
                  onClick={() => copy(row.value)}
                  title="Copy this line"
                  className="flex w-full items-center gap-3 rounded-lg px-2.5 py-1.5 text-left transition-colors hover:bg-raised/60"
                >
                  {row.swatch && (
                    <span
                      aria-hidden="true"
                      className="h-4 w-4 shrink-0 rounded border border-line"
                      style={{ background: row.swatch }}
                    />
                  )}
                  <span className="w-40 shrink-0 text-2xs uppercase tracking-[0.08em] text-ink-faint">
                    {row.label}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-2xs text-ink">
                    {row.value}
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-2xs leading-relaxed text-ink-soft">
              {output.text}
            </pre>
          )}
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center rounded-cad border border-dashed border-line/70">
            <p className="px-6 text-center text-2xs text-ink-faint">
              {form.fields.length === 0
                ? "Press Run — whatever it produces shows up here."
                : form.live
                  ? "Fill the fields above and the answer appears here."
                  : "Fill the fields above, then Run."}
            </p>
          </div>
      )}
    </div>
  );
}
