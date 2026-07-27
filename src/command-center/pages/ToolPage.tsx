/**
 * A page for one command.
 *
 * # Why every command has one
 *
 * A command that takes an argument used to be reachable only by knowing to type
 * `sha256 ` first. Pick it out of the list instead — which is how anyone finds
 * it the first time — and it ran with nothing to work on and answered "Type
 * something after the command first." The list was advertising a hundred and
 * twenty things that did not work when clicked.
 *
 * So a command is not a line you have to know the syntax of. It is a feature
 * with a page: a box to put the input in, the answer underneath, and the
 * buttons you were going to reach for anyway. Typing `sha256 hello` in the
 * palette still runs it in one keystroke — that path is faster and it is kept —
 * but it is now the shortcut, not the only door.
 *
 * # What is on the page
 *
 * * **Input**, when the command takes one, with the argument's own description
 *   as the placeholder. Pure local transforms re-run as you type; anything that
 *   touches the network, the disk or another app waits to be asked.
 * * **Output**, kept on screen and selectable, with Copy and Save to Notes.
 * * **A permission wall turned into a way through** — see
 *   {@link PermissionPage}. Nothing here fails with a sentence and a shrug.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import {
  COMMANDS,
  type CommandActions,
  type CommandDef,
  type CommandOutput,
} from "@/shared/commands";
import { PERMISSIONS, permissionFromMessage } from "@/shared/permissions";
import { useDebounced } from "@/shared/hooks";
import type { PermissionId, Tab } from "@/shared/tabs";
import { ShortcutIcon } from "@/shared/ShortcutIcon";
import { Button, Callout, EmptyState, Spinner, cx } from "@/shared/ui";
import { recordUsage } from "@/shared/usage";

/**
 * Groups whose commands re-run on every keystroke.
 *
 * Both are pure functions of their input that run in-process — hashing, case
 * conversion, sorting lines. Everything else is a request to the network, the
 * disk or another application, and firing one of those per keystroke is how you
 * ping a host forty times because you typed its name.
 */
const LIVE_GROUPS = new Set(["developer", "text"]);

export function ToolPage({
  active,
  commandId,
  prefill,
  onOpenTab,
  onSetTitle,
}: {
  active: boolean;
  commandId: string;
  prefill?: string;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  onSetTitle: (title: string | undefined) => void;
}) {
  const command = useMemo(() => COMMANDS.find((c) => c.id === commandId), [commandId]);

  if (!command) {
    return (
      <EmptyState
        title="That command is gone"
        hint="It was probably renamed in an update. Close this tab and search for it again."
        icon="⌂"
      />
    );
  }

  return (
    <ToolBody
      key={command.id}
      active={active}
      command={command}
      prefill={prefill}
      onOpenTab={onOpenTab}
      onSetTitle={onSetTitle}
    />
  );
}

function ToolBody({
  active,
  command,
  prefill,
  onOpenTab,
  onSetTitle,
}: {
  active: boolean;
  command: CommandDef;
  prefill?: string;
  onOpenTab: (request: Omit<Tab, "id">) => void;
  onSetTitle: (title: string | undefined) => void;
}) {
  const [input, setInput] = useState(prefill ?? "");
  const [output, setOutput] = useState<CommandOutput | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const [wall, setWall] = useState<PermissionId | null>(null);
  const [busy, setBusy] = useState(false);
  const [armed, setArmed] = useState(false);

  const inputRef = useRef<HTMLTextAreaElement>(null);
  const takesInput = Boolean(command.argument);
  const live = takesInput && LIVE_GROUPS.has(command.group);

  useEffect(() => {
    onSetTitle(command.title);
  }, [command.title, onSetTitle]);

  useEffect(() => {
    if (active && takesInput) inputRef.current?.focus();
  }, [active, takesInput]);

  const actions = useMemo<CommandActions>(
    () => ({
      notify: (message, tone) => {
        const permission = tone === "error" ? permissionFromMessage(message) : null;
        setWall(permission);
        // A permission wall says its piece in the block below; repeating it as
        // a red line underneath would be the same sentence twice.
        setNote(permission ? null : { text: message, ok: tone !== "error" });
      },
      showOutput: (next) => {
        setWall(null);
        setNote(null);
        setOutput(next);
      },
      setInput,
      openTab: onOpenTab,
      // Deliberately nothing. A command that dismisses the palette after doing
      // its job is right about the palette and wrong about a tab you opened on
      // purpose and are still reading.
      close: () => {},
    }),
    [onOpenTab],
  );

  const run = useCallback(
    async (value: string, counted: boolean) => {
      setBusy(true);
      try {
        await command.run({ input: value.trim(), actions });
        if (counted) recordUsage(`command:${command.id}`);
      } catch (error) {
        const message = api.errorMessage(error);
        const permission = permissionFromMessage(message);
        setWall(permission);
        if (!permission) setNote({ text: message, ok: false });
      } finally {
        setBusy(false);
      }
    },
    [actions, command],
  );

  // --- live re-run -----------------------------------------------------
  const debounced = useDebounced(input, 180);
  useEffect(() => {
    if (!live) return;
    if (!debounced.trim()) {
      setOutput(null);
      setNote(null);
      return;
    }
    void run(debounced, false);
    // `run` is stable per command; re-running on every identity change would
    // double-fire each keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debounced, live]);

  // An empty box is not a failure to explain, it is a button that should not be
  // pressable yet. `argumentOptional` commands mean something with nothing.
  const ready = !takesInput || command.argumentOptional || input.trim().length > 0;

  const submit = () => {
    if (!ready) {
      inputRef.current?.focus();
      return;
    }
    if (command.confirm && !armed) {
      setArmed(true);
      return;
    }
    setArmed(false);
    void run(input, true);
  };

  const paste = async () => {
    try {
      setInput(await navigator.clipboard.readText());
      inputRef.current?.focus();
    } catch {
      setNote({ text: "Could not read the clipboard.", ok: false });
    }
  };

  return (
    <div className="mx-auto flex h-full max-w-[720px] flex-col px-6 py-5">
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

      {/* --- input -------------------------------------------------------- */}
      {takesInput && (
        <div className="mb-3 shrink-0">
          <div className="mb-1.5 flex items-baseline justify-between gap-3">
            <label htmlFor="tool-input" className="eyebrow">
              {command.argument}
            </label>
            <button
              type="button"
              onClick={() => void paste()}
              className="text-2xs text-ink-faint transition-colors hover:text-ink"
            >
              Paste
            </button>
          </div>
          <textarea
            id="tool-input"
            ref={inputRef}
            value={input}
            rows={4}
            spellCheck={false}
            placeholder={command.argument}
            onChange={(event) => {
              setInput(event.target.value);
              setArmed(false);
            }}
            onKeyDown={(event) => {
              // ⌘↵ submits, the way it does in every box that has a button
              // next to it. Plain Enter has to stay a newline: half of these
              // tools work on several lines at once.
              if (event.key === "Enter" && event.metaKey) {
                event.preventDefault();
                submit();
              }
            }}
            className="w-full resize-y rounded-lg border border-line bg-base/40 px-3 py-2 font-mono text-2xs leading-relaxed text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
        </div>
      )}

      <div className="row mb-3 shrink-0 gap-2">
        <Button tone="primary" onClick={submit} disabled={busy || !ready}>
          {busy ? "Working…" : armed ? "Yes, do it" : takesInput ? "Run" : `Run ${command.title}`}
        </Button>
        {live && (
          <span className="text-2xs text-ink-faint">Runs as you type · ⌘↵ to force it</span>
        )}
        {command.confirm && armed && (
          <span className="text-2xs text-danger">{command.confirm}</span>
        )}
        {busy && <Spinner className="text-accent" />}
      </div>

      {/* --- permission wall ---------------------------------------------- */}
      {wall && (
        <div className="mb-3 shrink-0">
          <Callout tone="warn" title={`${PERMISSIONS[wall].title} is switched off`}>
            <p>{PERMISSIONS[wall].why}</p>
            <div className="row mt-2.5 gap-2">
              <Button
                size="sm"
                tone="primary"
                onClick={() => void api.openSystemSettings(PERMISSIONS[wall].pane)}
              >
                Open System Settings
              </Button>
              <Button
                size="sm"
                tone="ghost"
                onClick={() =>
                  onOpenTab({
                    kind: "permission",
                    permission: wall,
                    retryCommandId: command.id,
                  })
                }
              >
                Show me exactly what to click
              </Button>
            </div>
          </Callout>
        </div>
      )}

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
              <Button
                size="sm"
                onClick={() => {
                  navigator.clipboard
                    .writeText(output.text)
                    .then(() => setNote({ text: "Copied.", ok: true }))
                    .catch(() => setNote({ text: "Could not copy.", ok: false }));
                }}
              >
                Copy
              </Button>
              <Button
                size="sm"
                tone="ghost"
                onClick={() => {
                  api
                    .addToNotes(output.text, output.title)
                    .then((result) => setNote({ text: result.message, ok: true }))
                    .catch((error) =>
                      setNote({ text: api.errorMessage(error), ok: false }),
                    );
                }}
              >
                Save to Notes
              </Button>
            </div>
          </div>
          <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-2xs leading-relaxed text-ink-soft">
            {output.text}
          </pre>
        </div>
      ) : (
        !wall && (
          <div className="flex min-h-0 flex-1 items-center justify-center rounded-cad border border-dashed border-line/70">
            <p className="px-6 text-center text-2xs text-ink-faint">
              {takesInput
                ? live
                  ? "Type something above and the answer appears here."
                  : "Fill the box above, then Run."
                : "Press Run — whatever it produces shows up here."}
            </p>
          </div>
        )
      )}
    </div>
  );
}
