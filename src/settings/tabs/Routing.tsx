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
 * # Why this tab exists before the feature is wired up
 *
 * `routing.rs` is fully built and tested but, as of this tab, is not
 * reachable from the UI at all: `classify`/`route` are plain Rust functions,
 * not `#[tauri::command]`s, and `AgentSettings` does not yet carry the two
 * fields the policy needs (`autoRoutingEnabled`, `routingOverrideBackendId`).
 * Rather than wait for that wiring to write this tab, it is built against the
 * intended shape now — the explanation, the two controls, and a live
 * "preview a decision" box — and every control that would need the missing
 * plumbing is explicit about not being real yet, instead of silently doing
 * nothing or (worse) claiming to save something that evaporates on restart.
 * See the `Callout` at the top for exactly what closes the gap.
 *
 * Concretely, that means the on/off switch and the backend pin below are
 * **local component state**, not `draft.update(...)`: `AgentSettings` has no
 * home for them yet, and round-tripping an unknown field through
 * `update_settings` would just have the backend echo back a value without
 * it, snapping the switch back a moment after you flipped it — worse than a
 * control that is honest about being a preview.
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

  // Local-only stand-ins for the two settings fields that do not exist on
  // `AgentSettings` yet — see the module doc above. Defaults match what the
  // report proposes for `Default for AgentSettings`: routing on, nothing
  // pinned.
  const [enabled, setEnabled] = useState(true);
  const [pinnedId, setPinnedId] = useState<string>("");

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
          checked={enabled}
          onChange={setEnabled}
        />
      </Section>

      <Section
        title="Pin a backend"
        description="Bypass the classifier entirely and always use one specific backend, no matter what a prompt looks like. An explicit pin always wins — it does not even need the classifier to agree."
      >
        <Field label="Always use">
          <Select
            value={pinnedId}
            onChange={setPinnedId}
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

      <Section title="What this needs to become real">
        <Callout tone="warn" title="Not wired up yet">
          This tab is a preview of the intended experience. Turning routing off or pinning a
          backend above will not survive closing Settings, and the preview box will show an
          explanatory error instead of a real decision, until two things land on the Rust side:
          a <code>autoRoutingEnabled</code> / <code>routingOverrideBackendId</code> pair of fields
          on <code>AgentSettings</code>, and a <code>routing_preview</code> command that calls the
          already-tested <code>tools::routing::route</code> with them. Nothing in this tab will
          need to change when that lands — it already calls that command and reads settings in the
          shape they'll be in.
        </Callout>
      </Section>
    </>
  );
}

function RoutingPreview() {
  const [prompt, setPrompt] = useState("");
  const [loading, setLoading] = useState(false);
  const [decision, setDecision] = useState<RoutingDecision | null>(null);
  const [notWired, setNotWired] = useState<string | null>(null);

  // No debounce-and-auto-run here: unlike the expander's placeholder preview,
  // this would be a real backend call once wired, and a preview box that
  // fires on every keystroke against a real classifier is a worse interface
  // than one with a button, not a better one.
  const run = async () => {
    if (!prompt.trim()) return;
    setLoading(true);
    setDecision(null);
    setNotWired(null);
    try {
      setDecision(await api.routingPreview(prompt));
    } catch (e) {
      // Expected today: `routing_preview` is not a registered command yet.
      // Shown verbatim rather than swallowed, so it is obvious exactly what
      // is missing rather than looking like a silent failure.
      setNotWired(api.errorMessage(e));
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

      {notWired && (
        <div className="mt-3">
          <Callout tone="warn" title="Can't run this yet">
            Caduceus doesn't have a <code>routing_preview</code> command to call, so nothing ran.
            ({notWired})
          </Callout>
        </div>
      )}
    </>
  );
}
