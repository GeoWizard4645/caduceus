/**
 * Extensions: drop a file in, or have an AI write one.
 *
 * The whole install flow is one drop. There is no folder to lay out and no
 * manifest to keep in sync — an extension is a single `.js` file whose header
 * comment says what it is and what it wants access to, and that header is read
 * *without executing anything*, so the permission list below is visible before
 * any of the extension's code has run.
 *
 * The prompt starter exists because a blank file is the real barrier. It carries
 * the whole contract — header format, the `ctx` API, the permission set — so the
 * only thing left to write is a sentence about what the extension should do.
 */

import { useCallback, useEffect, useState } from "react";

import { getCurrentWebview } from "@tauri-apps/api/webview";

import * as api from "@/shared/api";
import type { Extension } from "@/shared/types";
import { Button, Callout, Section, TextArea, cx } from "@/shared/ui";

export function ExtensionsTab() {
  const [installed, setInstalled] = useState<Extension[]>([]);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [task, setTask] = useState("");
  const [copied, setCopied] = useState(false);

  const reload = useCallback(async () => {
    try {
      setInstalled(await api.listExtensions());
    } catch {
      setInstalled([]);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const install = useCallback(
    async (paths: string[]) => {
      setError(null);
      for (const path of paths) {
        try {
          const report = await api.installExtension(path);
          if (report.ok) setStatus(report.message);
          // Only the first failure is worth showing; a folder of bad files
          // would otherwise flash six errors in a row.
          else setError(report.message);
        } catch (e) {
          setError(api.errorMessage(e));
        }
      }
      await reload();
    },
    [reload],
  );

  // Tauri's own drag-drop, not the DOM's: a webview drop event gives a File
  // object with no real path, and the installer needs a path to copy from.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") setDragging(true);
        else if (event.payload.type === "leave") setDragging(false);
        else if (event.payload.type === "drop") {
          setDragging(false);
          void install(event.payload.paths);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, [install]);

  const prompt = buildPrompt(task);

  return (
    <div className="flex flex-col gap-6">
      <Section
        title="Install an extension"
        description="One JavaScript file. No folder, no manifest, no build step."
      >
        <div
          className={cx(
            "flex flex-col items-center justify-center gap-2 rounded-cad border-2 border-dashed px-6 py-10 text-center transition-colors",
            dragging ? "border-accent bg-accent/5" : "border-line-strong/60 bg-raised/40",
          )}
        >
          <p className="text-[13px] font-medium text-ink">
            {dragging ? "Drop it" : "Drag a .js file here"}
          </p>
          <p className="max-w-[46ch] text-2xs leading-relaxed text-ink-faint">
            Caduceus reads the header comment to find out what it is called and what it wants
            access to. Nothing runs until you install it.
          </p>
          <div className="row mt-2 gap-2">
            <Button size="sm" onClick={() => void api.openExtensionsFolder()}>
              Open the folder
            </Button>
          </div>
        </div>

        {error && (
          <div className="mt-3">
            <Callout tone="danger">{error}</Callout>
          </div>
        )}
        {status && !error && <p className="mt-3 text-2xs text-ink-mute">{status}</p>}
      </Section>

      <Section
        title="Have an AI write it"
        description="Say what it should do. The rest of the prompt is already filled in."
      >
        <TextArea
          value={task}
          onChange={setTask}
          rows={3}
          placeholder="e.g. take the URL on my clipboard and shorten it, or list my open Safari tabs"
        />

        <div className="mt-3 max-h-64 overflow-y-auto rounded-cad border border-line bg-raised/60 p-3">
          <pre className="whitespace-pre-wrap break-words text-2xs leading-relaxed text-ink-mute">
            {prompt}
          </pre>
        </div>

        <div className="row mt-3 gap-2">
          <Button
            tone="primary"
            onClick={() => {
              navigator.clipboard
                .writeText(prompt)
                .then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 2000);
                })
                .catch(() => setError("Could not copy."));
            }}
          >
            {copied ? "Copied" : "Copy prompt"}
          </Button>
          <span className="text-2xs text-ink-faint">
            Paste it into any assistant, save the reply as a <code>.js</code> file, drop it above.
          </span>
        </div>
      </Section>

      <Section title="Installed" description={`${installed.length} extension${installed.length === 1 ? "" : "s"}.`}>
        {installed.length === 0 ? (
          <p className="text-2xs leading-relaxed text-ink-faint">
            Nothing installed yet.
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            {installed.map((e) => (
              <div
                key={e.id}
                className="row items-start justify-between gap-3 rounded-cad border border-line bg-raised/50 px-3 py-2.5"
              >
                <div className="min-w-0">
                  <p className="truncate text-[13px] font-medium text-ink">{e.name}</p>
                  {e.description && (
                    <p className="mt-0.5 text-2xs leading-relaxed text-ink-mute">{e.description}</p>
                  )}
                  <div className="row mt-1.5 flex-wrap gap-1">
                    {e.permissions.length === 0 ? (
                      <span className="rounded bg-overlay px-1.5 py-0.5 text-2xs text-ink-faint">
                        no permissions
                      </span>
                    ) : (
                      e.permissions.map((p) => (
                        <span
                          key={p}
                          className="rounded bg-accent/15 px-1.5 py-0.5 text-2xs text-accent"
                        >
                          {p}
                        </span>
                      ))
                    )}
                    {e.author && (
                      <span className="text-2xs text-ink-faint">by {e.author}</span>
                    )}
                  </div>
                </div>
                <Button
                  size="sm"
                  tone="danger"
                  onClick={() => {
                    void api
                      .removeExtension(e.id)
                      .then(() => reload())
                      .catch((err) => setError(api.errorMessage(err)));
                  }}
                >
                  Remove
                </Button>
              </div>
            ))}
          </div>
        )}
      </Section>

      <Callout>
        <strong>Extensions do not run yet.</strong> Installing, reading the header and showing what
        an extension wants all work in this version; the sandbox that executes them does not ship
        yet. The format is settled so you can write against it now.
      </Callout>
    </div>
  );
}

/**
 * The prompt starter.
 *
 * Written as one block rather than assembled from fragments so that what you
 * read in the box is exactly what gets copied — a prompt that differs from its
 * preview is a bug nobody can see.
 */
function buildPrompt(task: string): string {
  const goal = task.trim() || "<describe what the extension should do>";

  return `Write a Caduceus extension.

Caduceus is a macOS command palette. An extension is ONE JavaScript file. There
is no manifest file, no folder layout, no build step, and no npm install.

WHAT IT SHOULD DO
${goal}

REQUIRED FILE SHAPE

/**
 * @caduceus 1
 * name: <short title shown in the palette>
 * description: <one line>
 * author: <your name>
 * permissions: <comma-separated, or omit the line entirely>
 */
export default async function (input, ctx) {
  // input: string — whatever the user typed after the command name
  // return a string to show a message, or an array of rows to show a list
}

The header comment must be the FIRST comment in the file. Caduceus parses it
without running your code, so it is how the app knows what to display and what
to grant.

PERMISSIONS — ask only for what you use
  clipboard      ctx.clipboard.read() / ctx.clipboard.write(text)
  network        ctx.fetch(url, init) — only hosts you actually need
  selection      ctx.selection() — current Finder selection, array of paths
  notifications  ctx.notify(text)

THE ctx API — this is the complete list. Nothing else is available.
  ctx.clipboard.read()            -> Promise<string>
  ctx.clipboard.write(text)       -> Promise<void>
  ctx.fetch(url, init)            -> Promise<Response>
  ctx.selection()                 -> Promise<string[]>
  ctx.notify(text)                -> void
  ctx.storage.get(key)            -> Promise<any>
  ctx.storage.set(key, value)     -> Promise<void>
  ctx.open(url)                   -> Promise<void>

RULES
- Plain modern JavaScript. No TypeScript, no imports, no require, no npm.
- There is no filesystem, no shell, and no process access. Do not try.
- There is no ambient fetch — use ctx.fetch, and only with the network permission.
- Return a string for a simple result. Return an array of
  { title, subtitle?, action? } for a list of rows.
- Handle failure by returning a short human-readable string. Do not throw.
- Must work with no account, no API key and no paid service. If the task cannot
  be done for free, say so instead of writing it.

Reply with the complete file contents and nothing else.`;
}
