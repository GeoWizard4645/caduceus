import { useState } from "react";

import * as api from "@/shared/api";
import type { BackendConfig, BackendKind, RuntimeInfo } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  IconButton,
  NumberInput,
  Section,
  Select,
  Spinner,
  TextArea,
  TextInput,
  Toggle,
  cx,
} from "@/shared/ui";

import type { Draft } from "../useDraft";

const KIND_LABELS: Record<BackendKind, string> = {
  null: "Not configured",
  openai_compatible: "OpenAI-compatible",
  anthropic: "Claude (Anthropic)",
};

export function BackendsTab({
  draft,
  info,
  onReloadInfo,
}: {
  draft: Draft;
  info: RuntimeInfo | null;
  onReloadInfo: () => void;
}) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const settings = draft.settings;
  if (!settings) return null;
  const agents = settings.agents;

  const computerUseCapable = agents.backends.filter(
    (b) => b.kind === "anthropic" && b.supportsComputerUse,
  );

  const addTemplate = async (kind: BackendKind) => {
    const templates = await api.agentBackendTemplates();
    const template = templates.find((t) => t.kind === kind);
    if (!template) return;
    draft.update((d) => {
      d.agents.backends.push(template);
      // A newly added real backend is almost always meant to become the
      // primary one, replacing the no-op placeholder.
      const primary = d.agents.backends.find((b) => b.id === d.agents.primaryBackendId);
      if (!primary || primary.kind === "null") d.agents.primaryBackendId = template.id;
    });
    setExpanded(template.id);
  };

  return (
    <>
      <Section
        title="How Caduceus uses AI"
        description="Everything else in Caduceus works without any of this. Add a backend when you want the “/” and “/c” prefixes, or voice routing to AI."
      >
        <div className="grid grid-cols-2 gap-5">
          <Field
            label="Primary backend"
            hint="Used by the “/” prefix and by voice routing."
          >
            <Select
              value={agents.primaryBackendId ?? ""}
              onChange={(v) => draft.update((d) => (d.agents.primaryBackendId = v || null))}
              options={agents.backends.map((b) => ({
                value: b.id,
                label: `${b.displayName || KIND_LABELS[b.kind]}${b.model ? ` · ${b.model}` : ""}`,
              }))}
            />
          </Field>

          <Field
            label="Computer-use backend"
            hint="Used by the “/c” prefix. Needs a Claude backend with computer use allowed."
          >
            <Select
              value={agents.computerUseBackendId ?? ""}
              onChange={(v) => draft.update((d) => (d.agents.computerUseBackendId = v || null))}
              options={[
                { value: "", label: "Not set up" },
                ...computerUseCapable.map((b) => ({
                  value: b.id,
                  label: `${b.displayName} · ${b.model}`,
                })),
              ]}
            />
          </Field>
        </div>

        {info && !info.keychainAvailable && (
          <div className="mt-4">
            <Callout tone="warn" title="API keys cannot be stored on this system">
              Caduceus only ever puts keys in the OS keychain, and there is not one available here.
              Local model servers that need no key will still work.
            </Callout>
          </div>
        )}
      </Section>

      <Section
        title="Backends"
        description="Add as many as you like and switch between them above."
        actions={
          <div className="row">
            <Button onClick={() => void addTemplate("openai_compatible")}>Add local model</Button>
            <Button tone="primary" onClick={() => void addTemplate("anthropic")}>
              Add Claude
            </Button>
          </div>
        }
      >
        <div className="space-y-2">
          {agents.backends.map((backend) => (
            <BackendRow
              key={backend.id}
              backend={backend}
              info={info}
              isPrimary={agents.primaryBackendId === backend.id}
              isComputerUse={agents.computerUseBackendId === backend.id}
              expanded={expanded === backend.id}
              onToggle={() => setExpanded(expanded === backend.id ? null : backend.id)}
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
      </Section>

      <Section
        title="Computer-use safety"
        description="Limits that apply to every agent session, regardless of backend."
      >
        <Toggle
          label="Ask before the first action of a session"
          hint="Caduceus never moves your mouse until you approve. Turning this off means an agent starts acting the moment you press Enter."
          checked={agents.confirmBeforeFirstAction}
          onChange={(checked) => draft.update((d) => (d.agents.confirmBeforeFirstAction = checked))}
        />

        {!agents.confirmBeforeFirstAction && (
          <div className="mt-3">
            <Callout tone="warn">
              With confirmation off, a mistyped “/c” starts controlling your machine immediately.
              The Stop button still works, but you may not reach it first.
            </Callout>
          </div>
        )}

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field
            label="Maximum steps per task"
            hint="A hard stop, so a confused agent cannot loop forever."
          >
            <NumberInput
              value={agents.maxSteps}
              min={1}
              max={200}
              onChange={(value) => draft.update((d) => (d.agents.maxSteps = value))}
            />
          </Field>

          <Field
            label="Screenshot size"
            hint="Longest edge sent to the model. Smaller is cheaper and faster; too small hurts accuracy on dense screens."
          >
            <NumberInput
              value={agents.screenshotMaxDimension}
              min={480}
              max={4096}
              step={64}
              suffix="px"
              onChange={(value) => draft.update((d) => (d.agents.screenshotMaxDimension = value))}
            />
          </Field>

          <Field
            label="Pause after each action"
            hint="Gives the app being driven time to redraw before the next screenshot."
          >
            <NumberInput
              value={agents.actionSettleMs}
              min={0}
              max={3000}
              step={50}
              suffix="ms"
              onChange={(value) => draft.update((d) => (d.agents.actionSettleMs = value))}
            />
          </Field>

          <Field label="Monitor" hint="Which display an agent sees and controls.">
            <NumberInput
              value={agents.monitorIndex}
              min={0}
              max={8}
              onChange={(value) => draft.update((d) => (d.agents.monitorIndex = value))}
            />
          </Field>
        </div>

        {info?.computerUseNote && (
          <div className="mt-4">
            <Callout tone="info" title="On this platform">
              {info.computerUseNote}
            </Callout>
          </div>
        )}
      </Section>
    </>
  );
}

function BackendRow({
  backend,
  info,
  isPrimary,
  isComputerUse,
  expanded,
  onToggle,
  onChange,
  onDelete,
  onKeySaved,
}: {
  backend: BackendConfig;
  info: RuntimeInfo | null;
  isPrimary: boolean;
  isComputerUse: boolean;
  expanded: boolean;
  onToggle: () => void;
  onChange: (change: (b: BackendConfig) => void) => void;
  onDelete: () => void;
  onKeySaved: () => void;
}) {
  const [apiKey, setApiKey] = useState("");
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [models, setModels] = useState<string[] | null>(null);

  const hasKey = info?.backendsWithKeys.includes(backend.id) ?? false;
  const isNull = backend.kind === "null";

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      setResult({ ok: true, message: await api.agentTestBackend(backend.id) });
    } catch (error) {
      setResult({ ok: false, message: api.errorMessage(error) });
    } finally {
      setTesting(false);
    }
  };

  const saveKey = async () => {
    try {
      await api.setBackendApiKey(backend.id, apiKey);
      setApiKey("");
      onKeySaved();
      setResult({ ok: true, message: apiKey ? "Key saved to your OS keychain." : "Key removed." });
    } catch (error) {
      setResult({ ok: false, message: api.errorMessage(error) });
    }
  };

  const loadModels = async () => {
    try {
      setModels(await api.agentListModels(backend.id));
    } catch {
      // Not every endpoint implements /models; free-text entry still works.
      setModels([]);
    }
  };

  return (
    <div
      className={cx(
        "rounded-lg border transition-colors",
        expanded ? "border-accent/30 bg-base/40" : "border-line bg-base/20 hover:border-line-strong",
      )}
    >
      <div className="flex items-center gap-3 px-3 py-2.5">
        <span
          aria-hidden="true"
          className={cx(
            "flex h-8 w-8 shrink-0 items-center justify-center rounded-md border text-[13px]",
            isNull ? "border-line bg-raised text-ink-faint" : "border-accent/30 bg-accent/12 text-accent",
          )}
        >
          {backend.kind === "anthropic" ? "✳" : backend.kind === "openai_compatible" ? "◍" : "○"}
        </span>

        <button type="button" onClick={onToggle} className="min-w-0 flex-1 text-left">
          <p className="row truncate text-[13px] font-medium text-ink">
            {backend.displayName || KIND_LABELS[backend.kind]}
            {isPrimary && <Badge>Primary</Badge>}
            {isComputerUse && <Badge>Computer use</Badge>}
          </p>
          <p className="truncate text-2xs text-ink-faint">
            {isNull
              ? "Placeholder — add a real backend to use AI features"
              : [KIND_LABELS[backend.kind], backend.model, hasKey ? "key stored" : "no key"]
                  .filter(Boolean)
                  .join(" · ")}
          </p>
        </button>

        {!isNull && (
          <Button size="sm" onClick={() => void test()} disabled={testing}>
            {testing ? <Spinner /> : null} Test
          </Button>
        )}

        <IconButton label="Delete backend" tone="danger" onClick={onDelete}>
          ×
        </IconButton>
      </div>

      {result && (
        <div
          className={cx(
            "mx-3 mb-3 rounded-md px-3 py-2 text-2xs leading-relaxed",
            result.ok ? "bg-positive/10 text-positive" : "bg-danger/10 text-danger",
          )}
        >
          {result.message}
        </div>
      )}

      {expanded && !isNull && (
        <div className="grid grid-cols-2 gap-4 border-t border-line px-3 pb-4 pt-4">
          <Field label="Name">
            <TextInput
              value={backend.displayName}
              onChange={(v) => onChange((b) => (b.displayName = v))}
            />
          </Field>

          <Field
            label="Model"
            hint={
              backend.kind === "anthropic"
                ? "Any Claude model id. Newer models work without updating Caduceus."
                : "Whatever your endpoint serves."
            }
          >
            <div className="row">
              <TextInput value={backend.model} onChange={(v) => onChange((b) => (b.model = v))} />
              <Button size="sm" onClick={() => void loadModels()}>
                List
              </Button>
            </div>
          </Field>

          {models && models.length > 0 && (
            <div className="col-span-2 flex flex-wrap gap-1.5">
              {models.slice(0, 24).map((model) => (
                <button
                  key={model}
                  type="button"
                  onClick={() => onChange((b) => (b.model = model))}
                  className="rounded-md border border-line bg-raised px-2 py-1 font-mono text-2xs text-ink-mute transition-colors hover:border-accent/40 hover:text-ink"
                >
                  {model}
                </button>
              ))}
            </div>
          )}
          {models && models.length === 0 && (
            <p className="col-span-2 text-2xs text-ink-faint">
              This endpoint did not return a model list — type the name yourself.
            </p>
          )}

          <Field
            label="Base URL"
            hint={
              backend.kind === "anthropic"
                ? "Leave as-is unless you route through a proxy or gateway."
                : "Ollama: http://localhost:11434/v1 · LM Studio: http://localhost:1234/v1"
            }
            wide
          >
            <TextInput mono value={backend.baseUrl} onChange={(v) => onChange((b) => (b.baseUrl = v))} />
          </Field>

          <Field
            label="API key"
            hint={
              hasKey
                ? "A key is stored in your OS keychain. Enter a new one to replace it, or save an empty field to remove it."
                : "Stored in your OS keychain — never written to a config file. Leave blank for local servers."
            }
            wide
          >
            <div className="row">
              <TextInput
                type="password"
                value={apiKey}
                onChange={setApiKey}
                placeholder={hasKey ? "•••••••••••••••••" : "sk-…"}
              />
              <Button size="sm" onClick={() => void saveKey()}>
                {apiKey ? "Save key" : hasKey ? "Remove key" : "Save"}
              </Button>
            </div>
          </Field>

          <Field label="Max tokens">
            <NumberInput
              value={backend.maxTokens}
              min={64}
              max={200000}
              step={256}
              onChange={(value) => onChange((b) => (b.maxTokens = value))}
            />
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

          <Field
            label="System prompt"
            hint="Prepended to every request. Left blank for most setups."
            wide
          >
            <TextArea
              rows={3}
              value={backend.systemPrompt}
              onChange={(v) => onChange((b) => (b.systemPrompt = v))}
              placeholder="Optional"
            />
          </Field>

          {backend.kind === "anthropic" && (
            <div className="col-span-2 rounded-lg border border-line bg-base/40 p-4">
              <p className="mb-1 text-[13px] font-semibold text-ink">Computer use</p>
              <p className="mb-4 text-2xs leading-relaxed text-ink-faint">
                These strings track Anthropic's API versions. They are plain text fields on purpose:
                when a newer tool version ships you can point Caduceus at it here, rather than waiting
                for a new release.
              </p>

              <Toggle
                label="Allow computer use with this backend"
                hint="Lets this backend take screenshots and control your mouse and keyboard when you use “/c”."
                checked={backend.supportsComputerUse}
                onChange={(checked) => onChange((b) => (b.supportsComputerUse = checked))}
              />

              {backend.supportsComputerUse && (
                <div className="mt-4 grid grid-cols-2 gap-4">
                  <Field label="Beta header" hint="Sent as anthropic-beta.">
                    <TextInput
                      mono
                      value={backend.anthropicBetaHeader}
                      onChange={(v) => onChange((b) => (b.anthropicBetaHeader = v))}
                    />
                  </Field>

                  <Field label="Tool version" hint="The computer tool's type string.">
                    <TextInput
                      mono
                      value={backend.computerToolVersion}
                      onChange={(v) => onChange((b) => (b.computerToolVersion = v))}
                    />
                  </Field>

                  <div className="col-span-2">
                    <Toggle
                      label="Allow zooming into screen regions"
                      hint="Lets the model inspect small text at full resolution. Requires computer_20251124 or newer."
                      checked={backend.enableZoom}
                      onChange={(checked) => onChange((b) => (b.enableZoom = checked))}
                    />
                  </div>
                </div>
              )}
            </div>
          )}

          <Field
            label="Extra headers"
            hint="One per line, as Name: value. For gateways and proxies that need custom auth."
            wide
          >
            <TextArea
              mono
              rows={2}
              value={backend.extraHeaders.map(([name, value]) => `${name}: ${value}`).join("\n")}
              placeholder="X-Org-Id: acme"
              onChange={(v) =>
                onChange((b) => {
                  b.extraHeaders = v
                    .split("\n")
                    .map((line) => line.split(/:(.*)/s))
                    .filter((parts) => parts.length > 1 && parts[0].trim())
                    .map((parts) => [parts[0].trim(), parts[1].trim()] as [string, string]);
                })
              }
            />
          </Field>
        </div>
      )}

      {expanded && isNull && (
        <div className="border-t border-line px-3 pb-4 pt-4">
          <Callout tone="info">
            This is the placeholder Caduceus ships with so that a fresh install has something valid
            selected. Add a real backend above; you can delete this one afterwards.
          </Callout>
        </div>
      )}
    </div>
  );
}

function Badge({ children }: { children: React.ReactNode }) {
  return (
    <span className="ml-2 rounded border border-accent/30 bg-accent/12 px-1.5 py-px text-2xs font-medium text-accent">
      {children}
    </span>
  );
}
