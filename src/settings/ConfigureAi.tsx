/**
 * "Configure AI" — find what is already on this Mac and wire it up.
 *
 * The problem this solves: connecting a local model by hand means knowing that
 * Ollama speaks OpenAI's dialect on port 11434 under `/v1`, that the model name
 * has to match a pulled tag exactly, and which of `primaryBackendId` /
 * `computerUseBackendId` drives which prefix. None of that is discoverable from
 * the Settings form, and all of it is knowable by asking the machine.
 *
 * Scanning is read-only and connecting is a separate click. Adding a backend
 * and repointing `/` is a real change to how the app behaves, so it does not
 * happen as a side effect of pressing a button labelled "scan".
 */

import { useState } from "react";

import * as api from "@/shared/api";
import type { BackendConfig, DetectedProvider, LocalAiScan } from "@/shared/types";
import { Button, Callout, Section, Select, Spinner, cx } from "@/shared/ui";

import type { Draft } from "./useDraft";

/** Stable ids, so re-connecting the same runtime updates rather than duplicates. */
const backendId = (providerId: string) => `local-${providerId}`;

export function ConfigureAi({ draft, onReloadInfo }: { draft: Draft; onReloadInfo: () => void }) {
  const [scan, setScan] = useState<LocalAiScan | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Model chosen per provider before connecting; defaults to the first found. */
  const [picked, setPicked] = useState<Record<string, string>>({});

  const settings = draft.settings;
  if (!settings) return null;

  const runScan = async () => {
    setScanning(true);
    setError(null);
    try {
      const result = await api.detectLocalAi();
      setScan(result);
      setPicked((current) => {
        const next = { ...current };
        for (const p of result.providers) {
          if (!next[p.id] && p.models.length) next[p.id] = p.models[0];
        }
        return next;
      });
      onReloadInfo();
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setScanning(false);
    }
  };

  const connect = (provider: DetectedProvider, model: string) => {
    draft.update((d) => {
      const id = backendId(provider.id);
      const existing = d.agents.backends.find((b) => b.id === id);
      const config: BackendConfig = {
        ...(existing ?? blankBackend(id)),
        displayName: `${provider.displayName} — ${model}`,
        kind: "openai_compatible",
        baseUrl: provider.baseUrl,
        model,
        // Local servers ignore the key entirely, so there is nothing to put in
        // the keychain and nothing for the UI to prompt for.
        hasApiKey: false,
      };
      if (existing) Object.assign(existing, config);
      else d.agents.backends.push(config);

      // Connecting is only useful if something routes to it.
      d.agents.primaryBackendId = id;
    });
  };

  const running = scan?.providers.filter((p) => p.running) ?? [];
  const idle = scan?.providers.filter((p) => !p.running) ?? [];

  return (
    <Section
      title="Configure AI"
      description="Checks this Mac for Hermes Agent, Ollama and other local model servers, then connects one in a click. Nothing is sent anywhere — the scan only talks to localhost."
      actions={
        <Button tone="primary" onClick={runScan} disabled={scanning}>
          {scanning ? (
            <>
              <Spinner /> Scanning…
            </>
          ) : scan ? (
            "Scan again"
          ) : (
            "Scan this Mac"
          )}
        </Button>
      }
    >
      {error && (
        <div className="mb-4">
          <Callout tone="danger" title="Scan failed">
            {error}
          </Callout>
        </div>
      )}

      {!scan && !scanning && (
        <p className="text-[13px] leading-relaxed text-ink-mute">
          Press <b>Scan this Mac</b>. Caduceus checks the default ports for Ollama, LM Studio,
          llama.cpp, Jan and vLLM, and asks Hermes Agent whether it is set up. Cloud providers are
          configured further down — they need a key, so they cannot be detected.
        </p>
      )}

      {scan && (
        <div className="space-y-3">
          <HermesRow status={scan.hermes} />

          {running.map((provider) => {
            const id = backendId(provider.id);
            const connected = settings.agents.primaryBackendId === id;
            const model = picked[provider.id] ?? provider.models[0] ?? "";

            return (
              <Row
                key={provider.id}
                tone="positive"
                title={provider.displayName}
                detail={provider.detail}
                meta={provider.baseUrl}
              >
                {provider.models.length > 0 ? (
                  <div className="row">
                    <div className="w-56">
                      <Select
                        value={model}
                        onChange={(v) => setPicked((c) => ({ ...c, [provider.id]: v }))}
                        options={provider.models.map((m) => ({ value: m, label: m }))}
                      />
                    </div>
                    <Button
                      size="sm"
                      tone={connected ? "default" : "primary"}
                      onClick={() => connect(provider, model)}
                    >
                      {connected ? "Reconnect" : "Connect to /"}
                    </Button>
                  </div>
                ) : (
                  <p className="text-2xs text-ink-faint">
                    Nothing to connect until a model is pulled.
                  </p>
                )}
              </Row>
            );
          })}

          {idle.length > 0 && (
            <details className="rounded-lg border border-line bg-base/20">
              <summary className="cursor-pointer px-3.5 py-2.5 text-[13px] text-ink-mute">
                {idle.length} not running
              </summary>
              <div className="space-y-2 border-t border-line px-3.5 py-3">
                {idle.map((provider) => (
                  <div key={provider.id} className="text-2xs leading-relaxed">
                    <span className="font-medium text-ink-soft">{provider.displayName}</span>
                    <span className="text-ink-faint"> — {provider.detail}</span>
                  </div>
                ))}
              </div>
            </details>
          )}

          {running.length === 0 && (
            <Callout tone="info" title="No local model server is running">
              Start one and scan again, or use a cloud provider below. The full local stack —
              Ollama, Hermes and models, wired up for you — installs with the bundled command on
              the Caduceus site.
            </Callout>
          )}
        </div>
      )}
    </Section>
  );
}

function HermesRow({ status }: { status: LocalAiScan["hermes"] }) {
  return (
    <Row
      tone={status.installed ? (status.configured ? "positive" : "warn") : "idle"}
      title="Hermes Agent"
      detail={status.detail}
      meta={status.version ?? undefined}
    >
      {status.installed && !status.configured && (
        <p className="text-2xs text-ink-faint">
          Run <code>hermes setup --portal</code> in a terminal, then scan again.
        </p>
      )}
    </Row>
  );
}

function Row({
  tone,
  title,
  detail,
  meta,
  children,
}: {
  tone: "positive" | "warn" | "idle";
  title: string;
  detail: string;
  meta?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-line bg-base/20 px-3.5 py-3">
      <div className="row items-start justify-between gap-4">
        <div className="min-w-0">
          <p className="row text-[13px] font-medium text-ink">
            <span
              aria-hidden="true"
              className={cx(
                "h-1.5 w-1.5 shrink-0 rounded-full",
                tone === "positive" && "bg-positive",
                tone === "warn" && "bg-caution",
                tone === "idle" && "bg-ink-faint",
              )}
            />
            {title}
            {meta && <span className="text-2xs font-normal text-ink-faint">{meta}</span>}
          </p>
          <p className="mt-1 text-2xs leading-relaxed text-ink-mute">{detail}</p>
        </div>
      </div>
      {children && <div className="mt-2.5">{children}</div>}
    </div>
  );
}

/** A backend with every field at its default, for fields the scan cannot know. */
function blankBackend(id: string): BackendConfig {
  return {
    id,
    displayName: "",
    kind: "openai_compatible",
    baseUrl: "",
    model: "",
    hasApiKey: false,
    maxTokens: 4096,
    temperature: null,
    systemPrompt: "",
    supportsComputerUse: false,
    extraHeaders: [],
    timeoutSecs: 600,
  };
}
