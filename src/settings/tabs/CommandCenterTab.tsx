import { useEffect, useState } from "react";

import type { PrefixAction, PrefixRule, RuntimeInfo } from "@/shared/types";
import {
  Button,
  Callout,
  Field,
  IconButton,
  Section,
  Select,
  TextInput,
  Toggle,
} from "@/shared/ui";

import { COMMANDS } from "@/shared/commands";
import { clearUsage, loadUsage, usageOf } from "@/shared/usage";

import { BrowserPicker } from "../BrowserPicker";
import type { Draft } from "../useDraft";

const ACTION_LABELS: Record<PrefixAction, string> = {
  web_search: "Search the web",
  primary_ai: "Ask the primary AI backend",
  computer_use: "Start a computer-use agent",
  clipboard_search: "Search clipboard history",
  open_url_template: "Open a URL template",
  run_command: "Run a shell command",
  run_applescript: "Run AppleScript",
};

const NEEDS_TARGET: PrefixAction[] = ["open_url_template", "run_command", "run_applescript"];

const SEARCH_PRESETS = [
  { label: "Google", url: "https://www.google.com/search?q={query}" },
  { label: "DuckDuckGo", url: "https://duckduckgo.com/?q={query}" },
  { label: "Kagi", url: "https://kagi.com/search?q={query}" },
  { label: "Brave", url: "https://search.brave.com/search?q={query}" },
  { label: "Perplexity", url: "https://www.perplexity.ai/search?q={query}" },
  { label: "Startpage", url: "https://www.startpage.com/sp/search?query={query}" },
];

export function CommandCenterTab({ draft, info }: { draft: Draft; info: RuntimeInfo | null }) {
  const settings = draft.settings;
  if (!settings) return null;
  const cc = settings.commandCenter;

  const mutate = (id: string, change: (rule: PrefixRule) => void) =>
    draft.update((d) => {
      const rule = d.commandCenter.prefixes.find((p) => p.id === id);
      if (rule) change(rule);
    });


  return (
    <>
      <Section
        title="Default search"
        description="What happens when you type something with no prefix and press Enter."
      >
        <Field
          label="Search URL"
          hint="Any URL with a {query} placeholder works — a search engine, an internal wiki, a docs site."
        >
          <TextInput
            mono
            value={cc.searchUrlTemplate}
            onChange={(v) => draft.update((d) => (d.commandCenter.searchUrlTemplate = v))}
            placeholder="https://www.google.com/search?q={query}"
          />
        </Field>

        <div className="mt-3 flex flex-wrap gap-1.5">
          {SEARCH_PRESETS.map((preset) => (
            <button
              key={preset.label}
              type="button"
              onClick={() => draft.update((d) => (d.commandCenter.searchUrlTemplate = preset.url))}
              className="rounded-md border border-line bg-raised px-2 py-1 text-2xs text-ink-mute transition-colors hover:border-accent/40 hover:text-ink"
            >
              {preset.label}
            </button>
          ))}
        </div>
      </Section>

      <Section
        title="Prefixes"
        description="A prefix routes whatever follows it somewhere specific. The longest match wins, so “/c” beats “/” no matter what order they are in."
        actions={
          <Button
            tone="primary"
            onClick={() =>
              draft.update((d) => {
                d.commandCenter.prefixes.push({
                  id: `prefix-${crypto.randomUUID().slice(0, 8)}`,
                  prefix: "",
                  label: "New prefix",
                  description: "",
                  action: "open_url_template",
                  target: "",
                  browser: null,
                  showHint: true,
                });
              })
            }
          >
            Add prefix
          </Button>
        }
      >
        <div className="space-y-3">
          {cc.prefixes.map((rule) => {
            const duplicate =
              rule.prefix.trim() !== "" &&
              cc.prefixes.filter((p) => p.prefix.trim() === rule.prefix.trim()).length > 1;

            return (
              <div key={rule.id} className="rounded-lg border border-line bg-base/20 p-3">
                <div className="grid grid-cols-[100px_1fr_1fr_auto] items-end gap-3">
                  <Field label="Prefix" error={duplicate ? "Already used" : null}>
                    <TextInput
                      mono
                      value={rule.prefix}
                      placeholder="/x"
                      onChange={(v) => mutate(rule.id, (r) => (r.prefix = v))}
                    />
                  </Field>

                  <Field label="Name">
                    <TextInput
                      value={rule.label}
                      onChange={(v) => mutate(rule.id, (r) => (r.label = v))}
                    />
                  </Field>

                  <Field label="Does what">
                    <Select
                      value={rule.action}
                      onChange={(v) => mutate(rule.id, (r) => (r.action = v))}
                      options={(Object.keys(ACTION_LABELS) as PrefixAction[]).map((action) => ({
                        value: action,
                        label: ACTION_LABELS[action],
                        disabled: action === "run_applescript" && info?.platform !== "macos",
                      }))}
                    />
                  </Field>

                  <div className="pb-1">
                    <IconButton
                      label="Delete prefix"
                      tone="danger"
                      onClick={() =>
                        draft.update((d) => {
                          d.commandCenter.prefixes = d.commandCenter.prefixes.filter(
                            (p) => p.id !== rule.id,
                          );
                        })
                      }
                    >
                      ×
                    </IconButton>
                  </div>
                </div>

                {NEEDS_TARGET.includes(rule.action) && (
                  <div className="mt-3">
                    <Field
                      label="Target"
                      hint={
                        rule.action === "open_url_template"
                          ? "URL with {query}, e.g. https://github.com/search?q={query}"
                          : "{query} is inserted safely quoted; the raw text is also in $CADUCEUS_QUERY."
                      }
                    >
                      <TextInput
                        mono
                        value={rule.target}
                        onChange={(v) => mutate(rule.id, (r) => (r.target = v))}
                      />
                    </Field>
                  </div>
                )}

                <div className="mt-3">
                  <Field label="Description" hint="Shown as a hint in the empty palette.">
                    <TextInput
                      value={rule.description}
                      onChange={(v) => mutate(rule.id, (r) => (r.description = v))}
                    />
                  </Field>
                </div>

                <div className="mt-2">
                  <Toggle
                    label="Show as a hint"
                    hint="Lists this prefix in the palette before you type anything."
                    checked={rule.showHint}
                    onChange={(checked) => mutate(rule.id, (r) => (r.showHint = checked))}
                  />
                </div>
              </div>
            );
          })}
        </div>

        {cc.prefixes.length === 0 && (
          <Callout tone="info">
            With no prefixes defined, every input is treated as a web search. That is a valid setup —
            add one back whenever you want AI or clipboard routing.
          </Callout>
        )}
      </Section>

      <Section
        title="Browser"
        description="Where links open — the plain web search, prefixes, and any shortcut that does not name its own."
      >
        <div className="grid grid-cols-2 gap-5">
          <BrowserPicker
            value={cc.browser}
            onChange={(next) =>
              draft.update((d) => (d.commandCenter.browser = next ?? { browserId: "", profile: null }))
            }
            browsers={info?.browsers ?? []}
          />
        </div>
      </Section>

      <Section title="Behaviour">
        <div className="space-y-1">
          <Toggle
            label="Close the palette after running something"
            hint="Chat replies and agent sessions always keep it open, since there is something to read."
            checked={cc.closeOnAction}
            onChange={(checked) => draft.update((d) => (d.commandCenter.closeOnAction = checked))}
          />
        </div>

        <div className="mt-4 grid grid-cols-2 gap-5">
          <Field label="Results per source" hint="How many rows each result source contributes.">
            <Select
              value={String(cc.maxResultsPerSource)}
              onChange={(v) => draft.update((d) => (d.commandCenter.maxResultsPerSource = Number(v)))}
              options={["4", "6", "8", "12", "20"].map((n) => ({ value: n, label: n }))}
            />
          </Field>
        </div>
      </Section>

      <UsageRanking />
    </>
  );
}

/**
 * The palette learns your order. This says so, and offers to forget it.
 *
 * Worth its own section rather than a footnote: a list that silently reorders
 * itself is unsettling if you do not know why, and the honest answer — a count
 * in a local file, nothing sent anywhere — is short enough to just say.
 */
function UsageRanking() {
  const [total, setTotal] = useState<number | null>(null);

  const refresh = () => {
    void loadUsage().then(() => {
      // `usageOf` reads the same in-memory cache the palette ranks from.
      let runs = 0;
      let ids = 0;
      for (const key of trackedKeys()) {
        const entry = usageOf(key);
        if (entry) {
          runs += entry.count;
          ids += 1;
        }
      }
      setTotal(ids === 0 ? 0 : runs);
    });
  };

  useEffect(refresh, []);

  return (
    <Section
      title="Ranking"
      description="The Command Center puts what you use most at the top. Counts are kept in a file next to your clipboard history, are never sent anywhere, and only ever record which built-in row was run — never what you typed."
    >
      <div className="row flex-wrap gap-2">
        <span className="text-2xs text-ink-mute">
          {total === null
            ? "Reading…"
            : total === 0
              ? "Nothing recorded yet — the list is in its shipped order."
              : `${total} run${total === 1 ? "" : "s"} recorded.`}
        </span>
        <Button
          size="sm"
          className="ml-auto"
          disabled={total === 0}
          onClick={() => {
            void clearUsage().then(refresh);
          }}
        >
          Reset ranking
        </Button>
      </div>
    </Section>
  );
}

/** Every id the palette counts against, so the total is a real total. */
function trackedKeys(): string[] {
  return COMMANDS.map((command) => `command:${command.id}`);
}
