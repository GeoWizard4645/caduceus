/**
 * Settings → Routing.
 *
 * Mirrors `src-tauri/src/tools/routing.rs`: a pure, deterministic classifier
 * (24 tests, no model call) that sorts a prompt into "micro" (small,
 * mechanical — safe for a fast local model) or "complex" (needs sustained
 * reasoning — goes to your primary backend), plus a policy that honours an
 * on/off switch and a user pin. See that file's module doc for the full
 * reasoning; the short version is in the Callout below.
 *
 * The on/off switch and the backend pin below are real `draft.update(...)`
 * calls against `AgentSettings.autoRoutingEnabled` /
 * `routingOverrideBackendId` (see `src-tauri/src/settings/model.rs`), same as
 * every other tab in this directory — they persist through Settings and take
 * effect on the next `/`-prefixed chat, which resolves its backend through
 * `agent::chat_with_history` → `resolve_chat_backend`, consulting
 * `tools::routing::route` exactly the way the "Preview a decision" box below
 * does.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import type { RoutingDecision } from "@/shared/api";
import { Button, Callout, Field, Section, Select, Spinner, TextArea, Toggle } from "@/shared/ui";

import type { Draft } from "../useDraft";

export function RoutingTab({ draft }: { draft: Draft }) {
  const settings = draft.settings;
  if (!settings) return null;
  const backends = settings.agents.backends;
  const { autoRoutingEnabled, routingOverrideBackendId } = settings.agents;

  return (
    <>
      <Section title="What auto-routing does">
        <p className="text-[13px] leading-relaxed text-ink-mute">
          Every prompt is sorted by a small, deterministic heuristic — word count, sentence
          shape, and a curated list of "this needs reasoning" versus "this is mechanical"
          phrases. No model is asked to make the call, which is the entire point: a classifier
          that itself needed a model round-trip would burn the exact latency this feature exists
          to skip, on the fast path where it matters most.
        </p>
        <ul className="mt-3 list-disc space-y-1.5 pl-5 text-[13px] leading-relaxed text-ink-mute">
          <li>
            <strong className="text-ink-soft">Micro</strong> — short, mechanical, single-step
            work (formatting, renaming, a regex, a commit message). Goes to the fastest local
            backend that has actually been measured to respond quickly, if one is configured.
          </li>
          <li>
            <strong className="text-ink-soft">Complex</strong> — sustained reasoning, debugging,
            design, long documents. Goes to your primary backend, same as every prompt does today.
          </li>
        </ul>
        <p className="mt-3 text-[13px] leading-relaxed text-ink-mute">
          Ties lean cheap on purpose: a short prompt with no strong signal either way is treated
          as micro, because the cost of a slightly worse first answer from a small model is low,
          and the cost of a needless round-trip to a big remote model on every short message is
          not.
        </p>
        <div className="mt-3">
          <Callout tone="info" title="Nothing measured here is stored or sent anywhere">
            Per-backend latency (used to pick "the fastest local backend") lives only in this
            process's memory and is gone the moment Caduceus quits — consistent with Caduceus not
            collecting usage data anywhere else.
          </Callout>
        </div>
      </Section>

      <Section
        title="Turn it off"
        description="With this off, every prompt goes to your primary backend, exactly like Caduceus behaves today — auto-routing is a pure opt-in change, never a silent one."
      >
        <Toggle
          label="Route short, mechanical prompts to a fast local backend"
          hint="Complex prompts always go to your primary backend regardless of this setting."
          checked={autoRoutingEnabled}
          onChange={(checked) => draft.update((d) => (d.agents.autoRoutingEnabled = checked))}
        />
      </Section>

      <Section
        title="Pin a backend"
        description="Bypass the classifier entirely and always use one specific backend, no matter what a prompt looks like. An explicit pin always wins — it does not even need the classifier to agree, and it wins even while auto-routing above is off."
      >
        <Field label="Always use">
          <Select
            value={routingOverrideBackendId ?? ""}
            onChange={(value) =>
              draft.update((d) => (d.agents.routingOverrideBackendId = value || null))
            }
            options={[
              { value: "", label: "Automatic (let auto-routing decide)" },
              ...backends.map((b) => ({ value: b.id, label: b.displayName || b.kind })),
            ]}
          />
        </Field>
      </Section>

      <Section
        title="Preview a decision"
        description="Runs the real classifier against text you type here — nothing is sent to a model, and nothing you type here is saved. This is how “why did that take eight seconds” gets an answer: the decision and the reason are always visible, never inferred after the fact."
      >
        <RoutingPreview />
      </Section>
    </>
  );
}

function RoutingPreview() {
  const [prompt, setPrompt] = useState("");
  const [loading, setLoading] = useState(false);
  const [decision, setDecision] = useState<RoutingDecision | null>(null);
  const [error, setError] = useState<string | null>(null);

  // No debounce-and-auto-run here: this is a real backend call, and a preview
  // box that fires on every keystroke against a real classifier is a worse
  // interface than one with a button, not a better one.
  const run = async () => {
    if (!prompt.trim()) return;
    setLoading(true);
    setDecision(null);
    setError(null);
    try {
      setDecision(await api.routingPreview(prompt));
    } catch (e) {
      // The only way this command fails is "no backend is configured yet" —
      // shown verbatim rather than swallowed, so it is obvious what to fix.
      setError(api.errorMessage(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <>
      <TextArea
        rows={3}
        value={prompt}
        onChange={setPrompt}
        placeholder='Try "fix the typo in this sentence" or "design a fault-tolerant architecture and analyze the trade-offs"'
      />
      <div className="row mt-2">
        <Button tone="primary" onClick={() => void run()} disabled={loading || !prompt.trim()}>
          {loading ? <Spinner /> : null} Preview routing decision
        </Button>
      </div>

      {decision && (
        <div className="mt-3 rounded-lg border border-line bg-base/40 p-3">
          <p className="text-[13px]">
            <span
              className={
                decision.class === "complex"
                  ? "font-medium text-accent"
                  : "font-medium text-positive"
              }
            >
              {decision.class === "complex" ? "Complex" : "Micro"}
            </span>
            {" → "}
            <span className="font-mono text-ink-soft">{decision.backendId}</span>
          </p>
          <p className="mt-1 text-2xs leading-relaxed text-ink-faint">{decision.reason}</p>
        </div>
      )}

      {error && (
        <div className="mt-3">
          <Callout tone="warn" title="Can't preview a decision">
            {error}
          </Callout>
        </div>
      )}
    </>
  );
}
