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
  /** Set when this row comes from a detected Ollama tag not yet wired up. */
  connect?: { provider: DetectedProvider; model: string };
}

const backendId = (providerId: string) => `local-${providerId}`;

function backendLabel(b: BackendConfig): string {
  if (b.kind === "hermes") return "Hermes Agent";
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
  }));
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
    return [...choicesFromSettings(settings, mode), ...connectChoices(scan, settings, mode)];
  }, [settings, scan, mode]);

  const activeBackendId =
    mode === "computer"
      ? settings?.agents.computerUseBackendId ?? "hermes"
      : settings?.agents.primaryBackendId ?? "hermes";

  const selectChoice = useCallback(
    async (value: string) => {
      if (!settings) return;
      const choice = choices.find((c) => c.value === value);
      if (!choice) return;

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
            };
        if (existing) Object.assign(existing, config);
        else next.agents.backends.push(config);
        if (mode === "chat") next.agents.primaryBackendId = id;
        else next.agents.computerUseBackendId = id;
        await api.updateSettings(next);
        await reload();
        await refresh();
        return;
      }

      const next = structuredClone(settings);
      if (mode === "chat") next.agents.primaryBackendId = choice.backendId;
      else next.agents.computerUseBackendId = choice.backendId;
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
    activeBackendId,
    hermesModel,
    selectChoice,
    refresh,
  };
}
