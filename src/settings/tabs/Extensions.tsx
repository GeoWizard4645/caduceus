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
import { buildExtensionPrompt } from "@/shared/extensionAppModel";
import { forgetExtensions } from "@/shared/providers";
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
    // The palette caches the installed list for the length of a window session,
    // so it has to be told. Otherwise an extension you just dropped in is not
    // searchable until the next launch.
    forgetExtensions();
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

  const prompt = buildExtensionPrompt(task);

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
            access to. Nothing runs until you install it and ask for it by name.
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
        <strong>How they run.</strong> Type an extension&rsquo;s name in the Command Center; anything
        after the name is passed as <code>input</code>. Extensions run in a Web Worker (no DOM). Each
        capability is declared in the file header and enforced in Rust when used — including shell,
        automation, files, settings, AI, and paid network APIs if you grant <code>network</code>.
      </Callout>
    </div>
  );
}
