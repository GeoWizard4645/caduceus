/**
 * Settings → Help — remove extensions, Caduceus, or local AI stack pieces.
 */

import { useCallback, useEffect, useState, type ReactNode } from "react";

import * as api from "@/shared/api";
import type { Extension, UninstallResult, UninstallSnapshot } from "@/shared/types";
import { Button, Callout, Section, cx } from "@/shared/ui";

type Step = "idle" | "confirm" | "options" | "running" | "done";

export function UninstallSection() {
  const [step, setStep] = useState<Step>("idle");
  const [snapshot, setSnapshot] = useState<UninstallSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<UninstallResult | null>(null);

  const [extensionsOn, setExtensionsOn] = useState(true);
  const [extensionIds, setExtensionIds] = useState<Set<string>>(new Set());

  const [caduceusOn, setCaduceusOn] = useState(false);
  const [hermesOn, setHermesOn] = useState(false);
  const [ollamaOn, setOllamaOn] = useState(false);
  const [modelsOn, setModelsOn] = useState(false);
  const [modelNames, setModelNames] = useState<Set<string>>(new Set());

  const loadSnapshot = useCallback(async () => {
    setError(null);
    try {
      const snap = await api.uninstallSnapshot();
      setSnapshot(snap);
      setExtensionIds(new Set(snap.extensions.map((e) => e.id)));
      setModelNames(new Set());
    } catch (e) {
      setError(api.errorMessage(e));
      setSnapshot(null);
    }
  }, []);

  useEffect(() => {
    if (step === "options") void loadSnapshot();
  }, [step, loadSnapshot]);

  const toggleExtension = (id: string) => {
    setExtensionIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleModel = (name: string) => {
    setModelNames((current) => {
      const next = new Set(current);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const run = async () => {
    setStep("running");
    setError(null);
    try {
      const out = await api.runUninstall({
        extensionIds: extensionsOn ? [...extensionIds] : [],
        caduceus: caduceusOn,
        hermes: hermesOn,
        ollama: ollamaOn,
        ollamaModels: modelsOn ? [...modelNames] : [],
      });
      setResult(out);
      setStep("done");
      if (!out.ok) setError(out.messages.join("\n"));
    } catch (e) {
      setError(api.errorMessage(e));
      setStep("options");
    }
  };

  const reset = () => {
    setStep("idle");
    setResult(null);
    setError(null);
    setExtensionsOn(true);
    setCaduceusOn(false);
    setHermesOn(false);
    setOllamaOn(false);
    setModelsOn(false);
  };

  return (
    <Section
      title="Uninstall"
      description="Remove downloaded extensions, Caduceus, Hermes Agent, Ollama, or local model files. Apps and data go to the Trash when macOS allows."
    >
      {step === "idle" && (
        <Button tone="danger" size="sm" onClick={() => setStep("confirm")}>
          Uninstall components…
        </Button>
      )}

      {step === "confirm" && (
        <div className="space-y-3 rounded-lg border border-danger/30 bg-danger/5 px-3.5 py-3">
          <p className="text-[13px] leading-relaxed text-ink-soft">
            This can delete extensions, move Caduceus or AI tools to the Trash, and remove Ollama
            models. It cannot be undone from Caduceus afterward.
          </p>
          <p className="text-2xs text-ink-mute">Are you sure you want to continue?</p>
          <div className="row gap-2">
            <Button size="sm" onClick={reset}>
              Cancel
            </Button>
            <Button size="sm" tone="danger" onClick={() => setStep("options")}>
              Yes, choose what to remove
            </Button>
          </div>
        </div>
      )}

      {step === "options" && (
        <div className="space-y-4 rounded-lg border border-line bg-base/20 px-3.5 py-3">
          {error && !result && (
            <Callout tone="danger">{error}</Callout>
          )}

          <UninstallCheck
            checked={extensionsOn}
            onChange={setExtensionsOn}
            label="Downloaded extensions"
            hint="JavaScript extensions you dropped into Caduceus"
            disabled={!snapshot?.extensions.length}
          >
            {extensionsOn && snapshot && snapshot.extensions.length > 0 && (
              <details open className="mt-2 rounded-md border border-line/80 bg-raised/40 px-2 py-2">
                <summary className="cursor-pointer text-2xs font-medium text-ink-mute">
                  Which extensions ({extensionIds.size}/{snapshot.extensions.length})
                </summary>
                <ul className="mt-2 max-h-36 space-y-1 overflow-y-auto">
                  {snapshot.extensions.map((ext) => (
                    <ExtensionRow
                      key={ext.id}
                      ext={ext}
                      checked={extensionIds.has(ext.id)}
                      onChange={() => toggleExtension(ext.id)}
                    />
                  ))}
                </ul>
              </details>
            )}
            {extensionsOn && snapshot?.extensions.length === 0 && (
              <p className="mt-1 text-2xs text-ink-faint">No extensions installed.</p>
            )}
          </UninstallCheck>

          <UninstallCheck
            checked={caduceusOn}
            onChange={setCaduceusOn}
            label="Caduceus"
            hint="Moves the app and all Caduceus data (settings, chat, extensions folder) to the Trash, then quits"
            disabled={!snapshot?.caduceusAppInstalled && !snapshot}
          />

          <UninstallCheck
            checked={hermesOn}
            onChange={setHermesOn}
            label="Hermes Agent"
            hint="Hermes binary and its install folders"
            disabled={!snapshot?.hermesInstalled}
          />

          <UninstallCheck
            checked={ollamaOn}
            onChange={setOllamaOn}
            label="Ollama"
            hint="Stops the service and moves Ollama.app to the Trash"
            disabled={!snapshot?.ollamaInstalled}
          />

          <UninstallCheck
            checked={modelsOn}
            onChange={setModelsOn}
            label="Local AI models (Ollama)"
            hint="Runs ollama rm for each selected tag — does not remove Ollama itself unless checked above"
            disabled={!snapshot?.ollamaModels.length}
          >
            {modelsOn && snapshot && snapshot.ollamaModels.length > 0 && (
              <details open className="mt-2 rounded-md border border-line/80 bg-raised/40 px-2 py-2">
                <summary className="cursor-pointer text-2xs font-medium text-ink-mute">
                  Which models ({modelNames.size}/{snapshot.ollamaModels.length})
                </summary>
                <ul className="mt-2 max-h-36 space-y-1 overflow-y-auto">
                  {snapshot.ollamaModels.map((name) => (
                    <li key={name}>
                      <label className="flex cursor-pointer items-center gap-2 text-2xs text-ink-soft">
                        <input
                          type="checkbox"
                          checked={modelNames.has(name)}
                          onChange={() => toggleModel(name)}
                          className="accent-[var(--accent)]"
                        />
                        <span className="font-mono">{name}</span>
                      </label>
                    </li>
                  ))}
                </ul>
              </details>
            )}
            {modelsOn && snapshot?.ollamaModels.length === 0 && (
              <p className="mt-1 text-2xs text-ink-faint">No Ollama models found (is Ollama running?).</p>
            )}
          </UninstallCheck>

          <div className="row gap-2 pt-1">
            <Button size="sm" onClick={() => setStep("confirm")}>
              Back
            </Button>
            <Button
              size="sm"
              tone="danger"
              disabled={
                !(extensionsOn && extensionIds.size > 0) &&
                !caduceusOn &&
                !hermesOn &&
                !ollamaOn &&
                !(modelsOn && modelNames.size > 0)
              }
              onClick={() => void run()}
            >
              Remove selected
            </Button>
          </div>
        </div>
      )}

      {step === "running" && (
        <p className="text-2xs text-ink-mute">Removing…</p>
      )}

      {step === "done" && result && (
        <div className="space-y-2">
          <Callout tone={result.ok ? "info" : "danger"}>
            <ul className="list-disc space-y-1 pl-4 text-2xs leading-relaxed">
              {result.messages.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </Callout>
          {result.quitApp && (
            <p className="text-2xs text-ink-mute">Caduceus will quit in a moment.</p>
          )}
          {!result.quitApp && (
            <Button size="sm" onClick={reset}>
              Done
            </Button>
          )}
        </div>
      )}
    </Section>
  );
}

function UninstallCheck({
  checked,
  onChange,
  label,
  hint,
  disabled,
  children,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  hint: string;
  disabled?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className={cx(disabled && "opacity-50")}>
      <label className="flex cursor-pointer items-start gap-2.5">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
          className="mt-0.5 accent-[var(--accent)]"
        />
        <span className="min-w-0">
          <span className="block text-[13px] font-medium text-ink-soft">{label}</span>
          <span className="mt-0.5 block text-2xs leading-relaxed text-ink-faint">{hint}</span>
        </span>
      </label>
      {children}
    </div>
  );
}

function ExtensionRow({
  ext,
  checked,
  onChange,
}: {
  ext: Extension;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <li>
      <label className="flex cursor-pointer items-center gap-2 text-2xs text-ink-soft">
        <input
          type="checkbox"
          checked={checked}
          onChange={onChange}
          className="accent-[var(--accent)]"
        />
        <span>{ext.name}</span>
        <span className="text-ink-faint">({ext.id})</span>
      </label>
    </li>
  );
}
