/**
 * Settings → AI.
 *
 * Caduceus does not implement model routing, tool calling or screen control —
 * Hermes Agent does all three. So this tab is mostly a status panel for the
 * local Hermes install plus a one-click way to get it, rather than the API-key
 * and tool-version form it used to be.
 *
 * The advanced section still exposes a raw OpenAI-compatible endpoint, because
 * `hermes proxy start` serves one and a plain Ollama install is a legitimate way
 * to use Caduceus without Hermes at all.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import { DOCS_CONFIGURE_AI } from "@/shared/docsUrls";
import type { BackendConfig, RuntimeInfo } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  IconButton,
  NumberInput,
  Section,
  Select,
  Spinner,
  TextInput,
  Toggle,
  cx,
} from "@/shared/ui";

import type { Draft } from "../useDraft";

const HERMES_INSTALL_COMMAND =
  "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash";

export function AiTab({
  draft,
  info,
  onReloadInfo,
}: {
  draft: Draft;
  info: RuntimeInfo | null;
  onReloadInfo: () => void;
}) {
  const [checking, setChecking] = useState(false);
  const [copied, setCopied] = useState(false);
  const settings = draft.settings;
  if (!settings) return null;

  const hermes = info?.hermes;
  const agents = settings.agents;
  const extraBackends = agents.backends.filter((b) => b.kind === "openai_compatible");

  const recheck = async () => {
    setChecking(true);
    try {
      onReloadInfo();
    } finally {
      // The probe itself is sub-second; this is only so the spinner is visible
      // long enough to read as feedback rather than a flicker.
      setTimeout(() => setChecking(false), 400);
    }
  };

  return (
    <>
      <Section
        title="Hermes Agent"
        description="Caduceus runs its AI through Hermes — an open-source agent from Nous Research that brings its own models, tools, memory and screen control."
        actions={
          <Button size="sm" onClick={() => void recheck()} disabled={checking}>
            {checking ? <Spinner /> : null} Re-check
          </Button>
        }
      >
        {!hermes ? (
          <div className="row text-[13px] text-ink-faint">
            <Spinner /> Checking for Hermes…
          </div>
        ) : hermes.installed && hermes.configured ? (
          <>
            <Callout tone="positive" title="Ready">
              {hermes.detail}
            </Callout>
            <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-[13px]">
              <dt className="text-ink-faint">Version</dt>
              <dd className="text-ink-soft">{hermes.version ?? "—"}</dd>
              <dt className="text-ink-faint">Model</dt>
              <dd className="text-ink-soft">{hermes.model ?? "—"}</dd>
              <dt className="text-ink-faint">Provider</dt>
              <dd className="text-ink-soft">{hermes.provider ?? "—"}</dd>
              <dt className="text-ink-faint">Binary</dt>
              <dd className="truncate font-mono text-2xs text-ink-faint">{hermes.path ?? "—"}</dd>
            </dl>
            <p className="mt-4 text-2xs leading-relaxed text-ink-faint">
              Change the model with <code className="text-ink-mute">hermes model</code>, or the
              whole setup with <code className="text-ink-mute">hermes setup</code>. Caduceus follows
              whatever Hermes is configured to use.
            </p>
          </>
        ) : hermes.installed ? (
          <>
            <Callout tone="warn" title="Installed, but no model connected">
              {hermes.detail}
            </Callout>
            <CommandBlock
              command="hermes setup --portal"
              copied={copied}
              onCopy={() => {
                void navigator.clipboard.writeText("hermes setup --portal");
                setCopied(true);
                setTimeout(() => setCopied(false), 1600);
              }}
            />
          </>
        ) : (
          <>
            <Callout tone="info" title="Hermes is not installed">
              Everything else in Caduceus works without it — shortcuts, the app launcher, the
              calculator, clipboard history and web search. Install Hermes when you want the{" "}
              <code>/</code> and <code>/c</code> prefixes.
            </Callout>

            <CommandBlock
              command={HERMES_INSTALL_COMMAND}
              copied={copied}
              onCopy={() => {
                void navigator.clipboard.writeText(HERMES_INSTALL_COMMAND);
                setCopied(true);
                setTimeout(() => setCopied(false), 1600);
              }}
            />

            <div className="row mt-3">
              <Button
                tone="primary"
                onClick={() => {
                  void api.openHermesInstaller();
                }}
              >
                Open in Terminal
              </Button>
              <span className="text-2xs text-ink-faint">
                Types the command into Terminal — you still press Return.
              </span>
            </div>
          </>
        )}
      </Section>

      <Section
        title="Routing"
        description="Which backend handles each prefix."
      >
        <Callout tone="info" title="Your own CLI or API">
          <p className="text-[13px] leading-relaxed text-ink-mute">
            Not using the bundled installer? Hermes, Ollama, LM Studio, and any OpenAI-compatible endpoint
            work here — step-by-step on the web:{" "}
            <button
              type="button"
              className="font-medium text-accent underline decoration-accent/40 underline-offset-2 hover:decoration-accent"
              onClick={() => void api.openExternalUrl(DOCS_CONFIGURE_AI)}
            >
              Configure AI with Caduceus
            </button>
            .
          </p>
        </Callout>

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field label="“/” — ask a question" hint="One-shot chat.">
            <Select
              value={agents.primaryBackendId ?? ""}
              onChange={(v) => draft.update((d) => (d.agents.primaryBackendId = v || null))}
              options={agents.backends.map((b) => ({
                value: b.id,
                label: b.displayName || b.kind,
              }))}
            />
          </Field>

          <Field
            label="“/c” — control this Mac"
            hint="Needs a backend that can drive the screen."
          >
            <Select
              value={agents.computerUseBackendId ?? ""}
              onChange={(v) => draft.update((d) => (d.agents.computerUseBackendId = v || null))}
              options={[
                { value: "", label: "Off" },
                ...agents.backends
                  .filter((b) => b.supportsComputerUse)
                  .map((b) => ({ value: b.id, label: b.displayName || b.kind })),
              ]}
            />
          </Field>
        </div>
      </Section>

      <Section
        title="Safety"
        description="Screen control means an agent moving your real mouse and typing on your real keyboard."
      >
        <Toggle
          label="Ask before an agent controls this Mac"
          hint="Nothing moves until you approve it. “/c” is one keystroke away from “/”, so leaving this on is strongly recommended."
          checked={agents.confirmBeforeFirstAction}
          onChange={(checked) => draft.update((d) => (d.agents.confirmBeforeFirstAction = checked))}
        />

        {!agents.confirmBeforeFirstAction && (
          <div className="mt-3">
            <Callout tone="warn">
              With this off, a mistyped “/c” starts controlling your machine immediately. Stop still
              works, but you may not reach it first.
            </Callout>
          </div>
        )}

        {info?.computerUseNote && (
          <div className="mt-4">
            <Callout tone="info" title="On this Mac">
              {info.computerUseNote}
            </Callout>
          </div>
        )}
      </Section>

      <Section
        title="Advanced: direct model endpoint"
        description="Optional. Point Caduceus straight at an OpenAI-compatible server — Ollama, LM Studio, or `hermes proxy start`. Bypasses Hermes entirely."
        actions={
          <Button
            onClick={async () => {
              const templates = await api.agentBackendTemplates();
              const template = templates.find((t) => t.kind === "openai_compatible");
              if (template) draft.update((d) => d.agents.backends.push(template));
            }}
          >
            Add endpoint
          </Button>
        }
      >
        {extraBackends.length === 0 ? (
          <p className="text-[13px] text-ink-faint">
            None configured. Most people use Hermes or the{" "}
            <button
              type="button"
              className="text-accent underline decoration-accent/40 underline-offset-2"
              onClick={() => void api.openExternalUrl(DOCS_CONFIGURE_AI)}
            >
              configure-AI guide
            </button>{" "}
            for Ollama and other CLIs.
          </p>
        ) : (
          <div className="space-y-2">
            {extraBackends.map((backend) => (
              <EndpointRow
                key={backend.id}
                backend={backend}
                hasKey={info?.backendsWithKeys.includes(backend.id) ?? false}
                onChange={(change) =>
                  draft.update((d) => {
                    const target = d.agents.backends.find((b) => b.id === backend.id);
                    if (target) change(target);
                  })
                }
                onDelete={() =>
                  draft.update((d) => {
                    d.agents.backends = d.agents.backends.filter((b) => b.id !== backend.id);
                    if (d.agents.primaryBackendId === backend.id) {
                      d.agents.primaryBackendId = d.agents.backends[0]?.id ?? null;
                    }
                    if (d.agents.computerUseBackendId === backend.id) {
                      d.agents.computerUseBackendId = null;
                    }
                  })
                }
                onKeySaved={onReloadInfo}
              />
            ))}
          </div>
        )}
      </Section>
    </>
  );
}

function CommandBlock({
  command,
  copied,
  onCopy,
}: {
  command: string;
  copied: boolean;
  onCopy: () => void;
}) {
  return (
    <div className="mt-4 flex items-center gap-2 rounded-lg border border-line bg-base/60 p-3">
      <code className="selectable min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-2xs text-ink-soft">
        {command}
      </code>
      <Button size="sm" onClick={onCopy}>
        {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

function EndpointRow({
  backend,
  hasKey,
  onChange,
  onDelete,
  onKeySaved,
}: {
  backend: BackendConfig;
  hasKey: boolean;
  onChange: (change: (b: BackendConfig) => void) => void;
  onDelete: () => void;
  onKeySaved: () => void;
}) {
  const [apiKey, setApiKey] = useState("");
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [testing, setTesting] = useState(false);

  return (
    <div className="rounded-lg border border-line bg-base/20 p-3">
      <div className="mb-3 flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">
          {backend.displayName || "Endpoint"}
        </span>
        <Button
          size="sm"
          disabled={testing}
          onClick={async () => {
            setTesting(true);
            setResult(null);
            try {
              setResult({ ok: true, message: await api.agentTestBackend(backend.id) });
            } catch (error) {
              setResult({ ok: false, message: api.errorMessage(error) });
            } finally {
              setTesting(false);
            }
          }}
        >
          {testing ? <Spinner /> : null} Test
        </Button>
        <IconButton label="Delete endpoint" tone="danger" onClick={onDelete}>
          ×
        </IconButton>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <Field label="Name">
          <TextInput
            value={backend.displayName}
            onChange={(v) => onChange((b) => (b.displayName = v))}
          />
        </Field>

        <Field label="Model">
          <TextInput value={backend.model} onChange={(v) => onChange((b) => (b.model = v))} />
        </Field>

        <Field
          label="Base URL"
          hint="Ollama: http://localhost:11434/v1 · hermes proxy: http://127.0.0.1:8765/v1"
          wide
        >
          <TextInput mono value={backend.baseUrl} onChange={(v) => onChange((b) => (b.baseUrl = v))} />
        </Field>

        <Field
          label="API key"
          hint={
            hasKey
              ? "Stored in your macOS keychain. Save an empty field to remove it."
              : "Stored in your macOS keychain, never in a config file. Blank is fine for local servers."
          }
          wide
        >
          <div className="row">
            <TextInput
              type="password"
              value={apiKey}
              onChange={setApiKey}
              placeholder={hasKey ? "•••••••••••••••••" : "Leave blank for a local server"}
            />
            <Button
              size="sm"
              onClick={async () => {
                try {
                  await api.setBackendApiKey(backend.id, apiKey);
                  setApiKey("");
                  onKeySaved();
                  setResult({ ok: true, message: apiKey ? "Key saved." : "Key removed." });
                } catch (error) {
                  setResult({ ok: false, message: api.errorMessage(error) });
                }
              }}
            >
              Save
            </Button>
          </div>
        </Field>

        <Field label="Timeout">
          <NumberInput
            value={backend.timeoutSecs}
            min={5}
            max={900}
            suffix="sec"
            onChange={(value) => onChange((b) => (b.timeoutSecs = value))}
          />
        </Field>
      </div>

      {result && (
        <div
          className={cx(
            "mt-3 rounded-md px-3 py-2 text-2xs leading-relaxed",
            result.ok ? "bg-positive/10 text-positive" : "bg-danger/10 text-danger",
          )}
        >
          {result.message}
        </div>
      )}
    </div>
  );
}
