/**
 * The prompt optimiser: paste the prompt you wrote, get the one worth sending.
 *
 * # Why the scorecard is not an afterthought
 *
 * A tool that makes a prompt shorter is trivially easy to build and almost
 * impossible to trust. The failure mode is not that it does nothing — it is
 * that it quietly drops "output must be valid JSON" while reporting a
 * triumphant 68% saving, and nothing on screen tells you. So the two numbers
 * are shown together, always, with equal weight: how much smaller, and how much
 * of the original survived. Any requirement that did not make it is listed by
 * name underneath rather than folded into a percentage.
 *
 * That is also why the pass list is on the page. "It got shorter" is a claim;
 * "18 characters of courtesy, 140 of restated instructions, 31 of flattery" is
 * a receipt, and a receipt is what lets someone decide whether to paste the
 * result into something that matters.
 *
 * # Why the token count updates as you type but the optimisation does not
 *
 * Counting is arithmetic on the Rust side (`prompt_estimate`) and costs
 * microseconds, so it runs on a debounce and the "before" number is live.
 * Optimising involves several round trips to a local model and takes seconds,
 * so it is behind a button. Putting both on the same keystroke would make the
 * whole page feel broken.
 */

import { useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { Button, Callout, Field, Section, Select, Spinner, TextArea, Toggle, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

/** Grouped so the dropdown reads as families rather than as nine flat names. */
const TARGETS: { value: api.TargetModel; label: string }[] = [
  { value: "opus5", label: "Opus 5" },
  { value: "sonnet5", label: "Sonnet 5" },
  { value: "fable5", label: "Fable 5" },
  { value: "k3", label: "K3" },
  { value: "gpt56_sol", label: "GPT-5.6 Sol" },
  { value: "gpt56_luna", label: "GPT-5.6 Luna" },
  { value: "gpt53_codex", label: "GPT-5.3 Codex" },
  { value: "gemini_flash", label: "Gemini Flash" },
  { value: "qwen37", label: "Qwen3.7" },
];

const LEVELS: { value: api.OptimizeLevel; label: string; hint: string }[] = [
  {
    value: "light",
    label: "Light",
    hint: "Filler and duplication only. Your words, better organised — nothing is rephrased.",
  },
  {
    value: "balanced",
    label: "Balanced",
    hint: "Filler, duplication, restructuring for the target, and condensed context. The default.",
  },
  {
    value: "aggressive",
    label: "Aggressive",
    hint: "Also cuts background prose down to the sentences carrying a requirement, and keeps one example. Read the coverage list before you send it.",
  },
];

export function PromptOptimizerPage({ onSetTitle }: ToolPageProps) {
  const [raw, setRaw] = useState("");
  const [target, setTarget] = useState<api.TargetModel>("opus5");
  const [level, setLevel] = useState<api.OptimizeLevel>("balanced");
  const [useModel, setUseModel] = useState(true);

  // What the toggle would actually do. Resolved once when the page opens: a
  // switch labelled "use a local model" is unanswerable without knowing which
  // one, and whether there is one at all.
  const [backend, setBackend] = useState<api.OptimizerBackend | null | undefined>(undefined);
  const [estimate, setEstimate] = useState<api.TokenEstimate | null>(null);
  const [result, setResult] = useState<api.OptimizedPrompt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  // A slow optimisation that finishes after the user has already changed the
  // target would otherwise paint a result labelled with the wrong model. The
  // run counter makes a stale response identifiable, so it can be dropped.
  const runId = useRef(0);

  useEffect(() => {
    onSetTitle("Prompt optimiser");
  }, [onSetTitle]);

  useEffect(() => {
    void api
      .promptOptimizerModel()
      .then((found) => {
        setBackend(found);
        // Nothing to call means the toggle would be a switch that does
        // nothing. Turn it off rather than letting it read as enabled.
        if (!found) setUseModel(false);
      })
      .catch(() => setBackend(null));
  }, []);

  // Live "before" count. Cheap enough to run per keystroke; debounced anyway so
  // a fast typist does not queue a hundred IPC calls.
  useEffect(() => {
    if (!raw.trim()) {
      setEstimate(null);
      return;
    }
    const timer = setTimeout(() => {
      void api
        .promptEstimate(raw, target)
        .then(setEstimate)
        .catch(() => setEstimate(null));
    }, 120);
    return () => clearTimeout(timer);
  }, [raw, target]);

  const optimize = async () => {
    if (!raw.trim()) return;
    const id = ++runId.current;
    setBusy(true);
    setError(null);
    setCopied(false);
    try {
      const next = await api.promptOptimize(raw, target, level, useModel);
      if (runId.current === id) setResult(next);
    } catch (err) {
      if (runId.current === id) {
        setError(api.errorMessage(err));
        setResult(null);
      }
    } finally {
      if (runId.current === id) setBusy(false);
    }
  };

  const copy = () => {
    if (!result) return;
    navigator.clipboard
      .writeText(result.prompt)
      .then(() => setCopied(true))
      .catch(() => setCopied(false));
  };

  const levelHint = LEVELS.find((l) => l.value === level)?.hint ?? "";
  const dropped = useMemo(
    () => result?.requirements.filter((r) => !r.kept) ?? [],
    [result],
  );

  return (
    <div className="mx-auto h-full max-w-[880px] overflow-y-auto px-6 py-5">
      <div className="mb-4">
        <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Prompt optimiser</h1>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          Rewrites a long, unstructured prompt into one shaped for the model you are about to send
          it to — and tells you what it cost. The compression itself runs on this Mac with no model
          involved; a small local model is used only for the judgement passes, and only if you have
          one.
        </p>
      </div>

      <Section title="Your prompt">
        <TextArea
          value={raw}
          onChange={setRaw}
          rows={12}
          placeholder={"Paste the prompt you actually wrote — the rambling one.\n\nGreetings, flattery, the same rule stated three times, four examples that make the same point. That is the input this is built for."}
        />
        {estimate && (
          <p className="mt-2 text-2xs text-ink-faint">
            {estimate.words.toLocaleString()} words · {estimate.chars.toLocaleString()} characters ·{" "}
            <span className="font-mono text-ink-soft">
              ~{estimate.tokens.toLocaleString()} tokens
            </span>{" "}
            on {estimate.targetName}
          </p>
        )}
      </Section>

      <Section title="Target">
        <div className="grid grid-cols-2 gap-4">
          <Field
            label="Model this prompt is for"
            hint="Decides the shape of the output: tagged sections, markdown headings, or no furniture at all."
          >
            <Select value={target} onChange={setTarget} options={TARGETS} />
          </Field>
          <Field label="How hard to squeeze" hint={levelHint}>
            <Select
              value={level}
              onChange={setLevel}
              options={LEVELS.map((l) => ({ value: l.value, label: l.label }))}
            />
          </Field>
        </div>
        <div className="mt-3">
          <Toggle
            checked={useModel && backend !== null}
            onChange={setUseModel}
            disabled={backend === null || backend === undefined}
            label={
              backend
                ? `Also use ${backend.model} for the judgement passes`
                : backend === null
                  ? "No model available for the judgement passes"
                  : "Checking for a model…"
            }
            hint={
              backend === null ? (
                <>
                  The rule-based passes below still run and still do most of the work — this
                  switch only adds the two steps that need judgement. To enable it, install a
                  local runtime (Ollama, LM Studio, llama.cpp, Jan or vLLM), pull a small model,
                  and point Settings → AI at it.
                </>
              ) : backend ? (
                <>
                  {backend.detail} It shortens long background sentences and writes a task line
                  when the original never stated one — nothing else. Constraints, limits and
                  format rules are never sent to it, and every rewrite it does return is rejected
                  unless each number and identifier survived intact.
                </>
              ) : null
            }
          />
          {backend && !backend.local && (
            <div className="mt-2.5">
              <Callout tone="warn">
                This would run against <strong>{backend.displayName}</strong> rather than
                something on this Mac, so it may be billed. Optimising a prompt on a hosted
                frontier model can cost more than the prompt it saves — a 2B local model is
                enough for these two steps, which is what they were built around.
              </Callout>
            </div>
          )}
        </div>
        <div className="mt-3 row gap-2">
          <Button tone="primary" onClick={() => void optimize()} disabled={busy || !raw.trim()}>
            {busy ? "Optimising…" : "Optimise"}
          </Button>
          {busy && <Spinner className="text-accent" />}
        </div>
        {error && (
          <p className="mt-3 whitespace-pre-line text-2xs leading-relaxed text-danger">{error}</p>
        )}
      </Section>

      {result && (
        <>
          <Section title="Result">
            <div className="grid grid-cols-3 gap-3">
              <Stat
                label="Tokens"
                value={`${result.beforeTokens.toLocaleString()} → ${result.afterTokens.toLocaleString()}`}
                caption={`for ${result.targetName}`}
              />
              <Stat
                label="Smaller by"
                value={`${result.reductionPercent}%`}
                caption={result.reductionPercent > 0 ? "of the original" : "already minimal"}
                tone={result.reductionPercent >= 40 ? "positive" : undefined}
              />
              <Stat
                label="Requirements kept"
                value={`${result.coveragePercent}%`}
                caption={`${result.requirements.length - dropped.length} of ${result.requirements.length}`}
                tone={result.coveragePercent === 100 ? "positive" : "warn"}
              />
            </div>
            <p className="mt-3 text-2xs text-ink-faint">
              {result.modelUsed
                ? `Judgement passes ran on ${result.modelUsed}.`
                : "Ran entirely on rules — nothing left this Mac."}
            </p>
          </Section>

          {dropped.length > 0 && (
            <div className="mb-5">
              <Callout tone="warn" title="These did not survive">
                <ul className="mt-1 space-y-1.5">
                  {dropped.map((requirement, i) => (
                    <li key={i}>
                      {requirement.text}
                      {requirement.missing.length > 0 && (
                        <span className="text-ink-faint">
                          {" "}
                          — missing{" "}
                          <span className="font-mono">{requirement.missing.join(", ")}</span>
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
                <p className="mt-2 text-ink-faint">
                  Paste back whatever matters, or drop to a gentler setting and run it again.
                </p>
              </Callout>
            </div>
          )}

          <Section
            title="Optimised prompt"
            actions={
              <Button size="sm" onClick={copy}>
                {copied ? "Copied" : "Copy"}
              </Button>
            }
          >
            <pre className="max-h-[420px] overflow-y-auto whitespace-pre-wrap break-words font-mono text-[13px] leading-relaxed text-ink">
              {result.prompt}
            </pre>
          </Section>

          {result.notes.length > 0 && (
            <Section title="Why it looks like this">
              <ul className="space-y-2 text-[13px] leading-relaxed text-ink-soft">
                {result.notes.map((note, i) => (
                  <li key={i} className="flex gap-2">
                    <span aria-hidden="true" className="text-ink-faint">
                      ·
                    </span>
                    <span>{note}</span>
                  </li>
                ))}
              </ul>
            </Section>
          )}

          {result.passes.length > 0 && (
            <Section
              title="Where the tokens went"
              description="Every pass that removed something, and how much. Nothing here is a guess — each one is a rule you can read in the source."
            >
              <ul className="space-y-3">
                {result.passes.map((pass, i) => (
                  <PassRow key={i} pass={pass} widest={result.passes[0]?.charsBefore ?? 1} />
                ))}
              </ul>
            </Section>
          )}

          {result.requirements.length > 0 && (
            <Section
              title="Requirement checklist"
              description="Lifted out of your original before anything was touched, then checked against the finished prompt. Numbers and identifiers have to appear exactly; prose may be reworded."
            >
              <ul className="space-y-1.5">
                {result.requirements.map((requirement, i) => (
                  <li key={i} className="flex gap-2 text-[13px] leading-relaxed">
                    <span
                      aria-hidden="true"
                      className={cx(
                        "mt-px shrink-0 font-bold",
                        requirement.kept ? "text-positive" : "text-caution",
                      )}
                    >
                      {requirement.kept ? "✓" : "!"}
                    </span>
                    <span className={requirement.kept ? "text-ink-soft" : "text-ink"}>
                      {requirement.text}
                    </span>
                  </li>
                ))}
              </ul>
            </Section>
          )}
        </>
      )}
    </div>
  );
}

function Stat({
  label,
  value,
  caption,
  tone,
}: {
  label: string;
  value: string;
  caption: string;
  tone?: "positive" | "warn";
}) {
  return (
    <div className="rounded-lg border border-line bg-base/40 px-3.5 py-3">
      <p className="text-2xs uppercase tracking-wide text-ink-faint">{label}</p>
      <p
        className={cx(
          "mt-1 font-mono text-[17px] font-semibold tabular-nums",
          tone === "positive" && "text-positive",
          tone === "warn" && "text-caution",
          !tone && "text-ink",
        )}
      >
        {value}
      </p>
      <p className="mt-0.5 text-2xs text-ink-faint">{caption}</p>
    </div>
  );
}

/**
 * One pass, with a bar showing what it removed relative to the largest pass.
 *
 * Relative to the largest rather than to the whole prompt, because the passes
 * run in sequence and each one sees a smaller input than the last — scaling
 * every bar against the original length would make the later passes look like
 * they did nothing when they may have removed most of what was left.
 */
function PassRow({ pass, widest }: { pass: api.PassReport; widest: number }) {
  const saved = pass.charsBefore - pass.charsAfter;
  const share = widest > 0 ? Math.min(100, Math.round((saved / widest) * 100)) : 0;
  return (
    <li>
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-[13px] font-medium text-ink">{pass.name}</span>
        <span className="shrink-0 font-mono text-2xs tabular-nums text-ink-soft">
          −{saved.toLocaleString()} chars
        </span>
      </div>
      <div className="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-line">
        <div className="h-full rounded-full bg-accent/70" style={{ width: `${share}%` }} />
      </div>
      <p className="mt-1.5 text-2xs leading-relaxed text-ink-faint">{pass.detail}</p>
    </li>
  );
}
