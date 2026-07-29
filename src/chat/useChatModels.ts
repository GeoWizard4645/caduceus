/**
 * Model list for the chat composer — settings backends plus a localhost scan.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import { useSettings } from "@/shared/hooks";
import type { BackendConfig, DetectedProvider, LocalAiScan, RuntimeInfo, Settings } from "@/shared/types";

export type ChatMode = "chat" | "computer";

export interface ModelChoice {
  /** Stable value for the &lt;select&gt;. */
  value: string;
  label: string;
  backendId: string;
  /** Heading this row is filed under in the composer's grouped dropdown. */
  group: string;
  /** Set when this row comes from a detected Ollama tag not yet wired up. */
  connect?: { provider: DetectedProvider; model: string };
  /**
   * Set when this row points the Hermes backend at a specific model. Empty
   * string means "clear the override and use whatever `hermes setup` picked".
   */
  hermesModel?: string;
}

/** One `<optgroup>` in the composer dropdown, preserving first-seen order. */
export interface ModelGroup {
  label: string;
  choices: ModelChoice[];
}

const GROUP_HERMES = "Hermes Agent";
const GROUP_BACKENDS = "Backends";
const GROUP_DISCOVER = "Discover models";

const backendId = (providerId: string) => `local-${providerId}`;

/**
 * Ollama vision tags (…vl, …-vision, …) reject tool schemas with HTTP 400.
 * Hermes always attaches tools for agent / computer_use turns, so offering
 * them in the Hermes picker is a trap.
 */
function looksLikeVisionOnlyModel(tag: string): boolean {
  const t = tag.toLowerCase();
  return (
    /(^|[:/\-_.])vl([:/\-_.]|$)/.test(t) ||
    t.includes("vision") ||
    t.includes("llava") ||
    t.includes("minicpm-v")
  );
}

function backendLabel(b: BackendConfig): string {
  if (b.kind === "hermes") {
    const model = b.model?.trim();
    // A configured model is the point of this row; show it. Blank means Hermes
    // falls back to whatever `hermes setup` chose, so we just name the agent.
    return model ? `Hermes · ${model}` : "Hermes Agent";
  }
  const model = b.model?.trim();
  return model ? `${b.displayName || b.id} · ${model}` : b.displayName || b.id;
}

function choicesFromSettings(settings: Settings, mode: ChatMode): ModelChoice[] {
  const backends = settings.agents.backends.filter((b) => {
    if (mode === "computer") return b.kind === "hermes" || b.supportsComputerUse;
    return true;
  });

  return backends.map((b) => ({
    value: b.id,
    label: backendLabel(b),
    backendId: b.id,
    group: b.kind === "hermes" ? GROUP_HERMES : GROUP_BACKENDS,
  }));
}

/**
 * Alternative models to run Hermes with. Hermes routes to whatever provider it
 * was set up with (usually a local Ollama), and takes a `-m <model>` override —
 * so the models worth offering are the ones the localhost scan actually found,
 * plus whatever Hermes is currently pointed at. Selecting one writes the
 * Hermes backend's `model` field; the Rust side turns that into `-m`.
 */
function hermesModelChoices(scan: LocalAiScan | null, settings: Settings): ModelChoice[] {
  const hermes = settings.agents.backends.find((b) => b.kind === "hermes");
  if (!hermes) return [];
  const current = hermes.model?.trim() ?? "";

  const models = new Set<string>();
  if (scan) {
    for (const provider of scan.providers) {
      if (!provider.running) continue;
      for (const model of provider.models.slice(0, 12)) models.add(model);
    }
  }
  // Always offer whatever Hermes reports it is using, even if the scan missed it
  // (e.g. a cloud provider the localhost probe can't enumerate).
  const configured = scan?.hermes?.model?.trim();
  if (configured) models.add(configured);

  const rows: ModelChoice[] = [];
  // Escape hatch: hand control back to Hermes' own config. Only meaningful when
  // an override is currently set — otherwise the plain "Hermes Agent" row is it.
  if (current !== "") {
    rows.push({
      value: "hermes-model:",
      label: "Hermes · use its own default",
      backendId: hermes.id,
      group: GROUP_HERMES,
      hermesModel: "",
    });
  }
  for (const model of models) {
    if (model === current) continue; // already the plain Hermes row
    if (looksLikeVisionOnlyModel(model)) continue;
    rows.push({
      value: `hermes-model:${model}`,
      label: `Hermes · ${model}`,
      backendId: hermes.id,
      group: GROUP_HERMES,
      hermesModel: model,
    });
  }
  return rows;
}

/** Bucket flat choices into ordered `<optgroup>`s, first-seen group order. */
function groupChoices(choices: ModelChoice[]): ModelGroup[] {
  const order: string[] = [];
  const byGroup = new Map<string, ModelChoice[]>();
  for (const choice of choices) {
    let bucket = byGroup.get(choice.group);
    if (!bucket) {
      bucket = [];
      byGroup.set(choice.group, bucket);
      order.push(choice.group);
    }
    bucket.push(choice);
  }
  return order.map((label) => ({ label, choices: byGroup.get(label)! }));
}

function connectChoices(scan: LocalAiScan | null, settings: Settings, mode: ChatMode): ModelChoice[] {
  if (!scan || mode === "computer") return [];
  const rows: ModelChoice[] = [];
  for (const provider of scan.providers) {
    if (!provider.running || provider.models.length === 0) continue;
    const id = backendId(provider.id);
    if (settings.agents.backends.some((b) => b.id === id)) continue;
    for (const model of provider.models.slice(0, 12)) {
      rows.push({
        value: `connect:${provider.id}:${model}`,
        label: `Connect ${provider.displayName} · ${model}`,
        backendId: id,
        group: GROUP_DISCOVER,
        connect: { provider, model },
      });
    }
  }
  return rows;
}

export function useChatModels(mode: ChatMode) {
  const { settings, reload } = useSettings();
  const [info, setInfo] = useState<RuntimeInfo | null>(null);
  const [scan, setScan] = useState<LocalAiScan | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await api.getRuntimeInfo());
    } catch {
      setInfo(null);
    }
    try {
      setScan(await api.detectLocalAi());
    } catch {
      setScan(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const choices = useMemo(() => {
    if (!settings) return [];
    return [
      ...choicesFromSettings(settings, mode),
      ...hermesModelChoices(scan, settings),
      ...connectChoices(scan, settings, mode),
    ];
  }, [settings, scan, mode]);

  const groups = useMemo(() => groupChoices(choices), [choices]);

  const activeBackendId =
    mode === "computer"
      ? settings?.agents.computerUseBackendId ?? "hermes"
      : settings?.agents.primaryBackendId ?? "hermes";

  const selectChoice = useCallback(
    async (value: string) => {
      if (!settings) return;
      const choice = choices.find((c) => c.value === value);
      if (!choice) return;

      if (choice.hermesModel !== undefined) {
        const next = structuredClone(settings);
        const hermes = next.agents.backends.find((b) => b.id === choice.backendId);
        if (hermes) hermes.model = choice.hermesModel;
        if (mode === "chat") {
          next.agents.primaryBackendId = choice.backendId;
          // Pin so auto-routing cannot silently send micro chat to a different
          // local backend than the one the user just picked in this dropdown.
          next.agents.routingOverrideBackendId = choice.backendId;
        } else {
          next.agents.computerUseBackendId = choice.backendId;
        }
        await api.updateSettings(next);
        await reload();
        return;
      }

      if (choice.connect) {
        const { provider, model } = choice.connect;
        const id = backendId(provider.id);
        const next = structuredClone(settings);
        const existing = next.agents.backends.find((b) => b.id === id);
        const config: BackendConfig = existing
          ? { ...existing, displayName: `${provider.displayName} — ${model}`, baseUrl: provider.baseUrl, model }
          : {
              id,
              displayName: `${provider.displayName} — ${model}`,
              kind: "openai_compatible",
              baseUrl: provider.baseUrl,
              model,
              hasApiKey: false,
              maxTokens: 4096,
              temperature: null,
              systemPrompt: "",
              supportsComputerUse: false,
              extraHeaders: [],
              timeoutSecs: 600,
              reasoningEffort: null,
            };
        if (existing) Object.assign(existing, config);
        else next.agents.backends.push(config);
        if (mode === "chat") {
          next.agents.primaryBackendId = id;
          next.agents.routingOverrideBackendId = id;
        } else {
          next.agents.computerUseBackendId = id;
        }
        await api.updateSettings(next);
        await reload();
        await refresh();
        return;
      }

      const next = structuredClone(settings);
      if (mode === "chat") {
        next.agents.primaryBackendId = choice.backendId;
        next.agents.routingOverrideBackendId = choice.backendId;
      } else {
        next.agents.computerUseBackendId = choice.backendId;
      }
      await api.updateSettings(next);
      await reload();
    },
    [choices, mode, refresh, reload, settings],
  );

  const hermesModel = info?.hermes?.model;

  return {
    settings,
    info,
    scan,
    choices,
    groups,
    activeBackendId,
    hermesModel,
    selectChoice,
    refresh,
  };
}
