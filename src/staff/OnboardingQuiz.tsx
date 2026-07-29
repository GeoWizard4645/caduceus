/**
 * Three-question quiz before the staff walkthrough — the first of the three
 * phases in the first-run flow (survey → permissions → tour). The other two
 * live in `Onboarding.tsx`; this component only owns the survey, and hands
 * off by writing `onboardingQuizDone` and letting `Staff.tsx` mount
 * `Onboarding` in its place. See `Onboarding.tsx`'s doc comment for why the
 * permission step is not a fourth flag here rather than a phase over there.
 *
 * Answers are saved locally and used to seed favorites plus ranking nudges in
 * the Command Center — nothing is sent off the machine.
 */

import { useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/shared/api";
import { COMMANDS } from "@/shared/commands";
import {
  DEFAULT_PRIMARY_FOCUS,
  MAX_ONBOARDING_FAVORITES,
  ONBOARDING_FEATURE_GROUPS,
  ONBOARDING_FEATURE_PICKS,
  PRIMARY_FOCUS_OPTIONS,
  type PrimaryFocus,
} from "@/shared/onboardingQuiz";
import type { PersonalizationProfile, Settings } from "@/shared/types";
import { Button, cx } from "@/shared/ui";
import { seedUsageCache } from "@/shared/usage";

const STEPS = ["About you", "Daily habit", "What excited you"] as const;

export function OnboardingQuiz({
  // Accepted but no longer read: the card used to be parked clear of the
  // staff mark and sized off this, but it now matches the centred, full-size
  // shell the rest of the flow uses (see the return statement below). Kept in
  // the prop type because `Staff.tsx` — out of scope for this change — still
  // passes it, and dropping it here would make that call site fail to type-check.
  staffSize: _staffSize,
  settings,
  onComplete,
}: {
  staffSize: number;
  settings: Settings;
  onComplete: (next: Settings) => void;
}) {
  const [step, setStep] = useState(0);
  const [isDeveloper, setIsDeveloper] = useState<boolean | null>(null);
  const [primaryFocus, setPrimaryFocus] = useState<PrimaryFocus>(DEFAULT_PRIMARY_FOCUS);
  const [favorites, setFavorites] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  const cardRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;

    const publish = () => {
      const r = el.getBoundingClientRect();
      void api.setStaffCaptureRect({
        x: r.left,
        y: r.top,
        width: r.width,
        height: r.height,
      });
    };

    publish();
    const observer = new ResizeObserver(publish);
    observer.observe(el);
    window.addEventListener("resize", publish);

    return () => {
      observer.disconnect();
      window.removeEventListener("resize", publish);
      void api.setStaffCaptureRect(null);
    };
  }, [step]);

  const picksByGroup = useMemo(() => {
    const map = new Map<string, typeof ONBOARDING_FEATURE_PICKS>();
    for (const group of ONBOARDING_FEATURE_GROUPS) {
      map.set(
        group,
        ONBOARDING_FEATURE_PICKS.filter((p) => p.group === group),
      );
    }
    return map;
  }, []);

  const toggleFavorite = (commandId: string) => {
    setFavorites((current) => {
      const next = new Set(current);
      if (next.has(commandId)) next.delete(commandId);
      else if (next.size < MAX_ONBOARDING_FAVORITES) next.add(commandId);
      return next;
    });
  };

  const canNext =
    step === 0
      ? isDeveloper !== null
      : step === 1
        ? !!primaryFocus
        : favorites.size > 0;

  const finish = async (skipped: boolean) => {
    setBusy(true);
    try {
      const personalization: PersonalizationProfile = skipped
        ? {
            isDeveloper: false,
            primaryFocus: DEFAULT_PRIMARY_FOCUS,
            favoriteCommandIds: [],
          }
        : {
            isDeveloper: isDeveloper ?? false,
            primaryFocus,
            favoriteCommandIds: [...favorites],
          };

      const next: Settings = {
        ...settings,
        general: {
          ...settings.general,
          onboardingQuizDone: true,
          personalization,
        },
      };

      if (personalization.favoriteCommandIds.length > 0) {
        const keys = personalization.favoriteCommandIds.map((id) => `command:${id}`);
        await api.seedUsage(keys, 8);
        seedUsageCache(keys, 8);
      }

      await api.updateSettings(next);
      onComplete(next);
    } finally {
      setBusy(false);
    }
  };

  return (
    // Sized and positioned to match the permission step and tutorial that
    // follow it — `Onboarding.tsx`'s "big card" — so the survey does not read
    // as a smaller, separate screen bolted onto the front of the real
    // walkthrough. Nothing here needs the staff mark reachable underneath (no
    // step asks you to touch it), so, like the later phases, it is free to
    // sit centred at a size that does not force its content into a scrollbar.
    <div className="pointer-events-none absolute inset-0 z-50">
      <div
        ref={cardRef}
        className={cx(
          "pointer-events-auto absolute inset-x-4 top-1/2 mx-auto -translate-y-1/2",
          "w-[min(560px,calc(100%-32px))] overflow-y-auto rounded-cad-lg",
          "glass px-8 py-7 shadow-float animate-fade-rise",
        )}
        style={{ maxHeight: "calc(100% - 32px)" }}
      >
        <div className="row justify-between">
          <span className="text-2xs font-medium uppercase tracking-[0.1em] text-accent">
            {step + 1} of {STEPS.length} · {STEPS[step]}
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={() => void finish(true)}
            className="rounded px-1.5 py-0.5 text-2xs text-ink-faint transition-colors hover:bg-raised hover:text-ink disabled:opacity-50"
          >
            Skip
          </button>
        </div>

        {step === 0 && (
          <>
            <p className="mt-2 text-[14px] font-semibold leading-tight text-ink">
              Are you a developer or software engineer?
            </p>
            <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">
              This helps Caduceus float developer tools and formatting commands toward the top when
              you search — it never leaves your Mac.
            </p>
            <div className="mt-3 grid grid-cols-2 gap-2">
              {[
                { value: true, label: "Yes" },
                { value: false, label: "Not really" },
              ].map(({ value, label }) => (
                <button
                  key={label}
                  type="button"
                  onClick={() => setIsDeveloper(value)}
                  className={cx(
                    "rounded-lg border px-3 py-2.5 text-[13px] font-medium transition-colors",
                    isDeveloper === value
                      ? "border-accent/50 bg-accent/12 text-ink"
                      : "border-line bg-raised/50 text-ink-mute hover:border-line-strong hover:text-ink",
                  )}
                >
                  {label}
                </button>
              ))}
            </div>
          </>
        )}

        {step === 1 && (
          <>
            <p className="mt-2 text-[14px] font-semibold leading-tight text-ink">
              What do you reach for most on your Mac?
            </p>
            <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">
              Caduceus will bias its default command list toward that kind of work. You can change
              this any time by retaking the quiz in Settings → Help.
            </p>
            <ul className="mt-3 space-y-1.5">
              {PRIMARY_FOCUS_OPTIONS.map((option) => (
                <li key={option.id}>
                  <button
                    type="button"
                    onClick={() => setPrimaryFocus(option.id)}
                    className={cx(
                      "w-full rounded-lg border px-3 py-2 text-left transition-colors",
                      primaryFocus === option.id
                        ? "border-accent/50 bg-accent/10"
                        : "border-line bg-raised/40 hover:bg-raised",
                    )}
                  >
                    <span className="block text-[13px] font-medium text-ink">{option.label}</span>
                    <span className="mt-0.5 block text-2xs leading-relaxed text-ink-mute">
                      {option.detail}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}

        {step === 2 && (
          <>
            <p className="mt-2 text-[14px] font-semibold leading-tight text-ink">
              What made you want to try Caduceus?
            </p>
            <p className="mt-1.5 text-2xs leading-relaxed text-ink-mute">
              Pick everything that applies ({favorites.size}/{MAX_ONBOARDING_FAVORITES} max). These
              become your favorites — pinned at the top of an empty search and ranked higher
              everywhere else.
            </p>
            <div className="mt-3 space-y-3">
              {ONBOARDING_FEATURE_GROUPS.map((group) => (
                <div key={group}>
                  <p className="mb-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-ink-faint">
                    {group}
                  </p>
                  <ul className="space-y-1">
                    {(picksByGroup.get(group) ?? []).map((pick) => {
                      const on = favorites.has(pick.commandId);
                      const command = COMMANDS.find((c) => c.id === pick.commandId);
                      return (
                        <li key={pick.commandId}>
                          <label
                            className={cx(
                              "flex cursor-pointer items-start gap-2 rounded-md border px-2.5 py-1.5 transition-colors",
                              on ? "border-accent/40 bg-accent/8" : "border-transparent hover:bg-raised/60",
                            )}
                          >
                            <input
                              type="checkbox"
                              checked={on}
                              onChange={() => toggleFavorite(pick.commandId)}
                              className="mt-0.5 accent-[var(--accent)]"
                            />
                            <span className="min-w-0">
                              <span className="block text-[12px] font-medium text-ink-soft">
                                {pick.label}
                              </span>
                              {command && (
                                <span className="block truncate text-[10px] text-ink-faint">
                                  {command.detail.split(".")[0]}
                                </span>
                              )}
                            </span>
                          </label>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ))}
            </div>
          </>
        )}

        <div className="row mt-6 justify-between gap-2">
          <Button
            tone="ghost"
            size="md"
            disabled={step === 0 || busy}
            onClick={() => setStep((s) => Math.max(0, s - 1))}
          >
            Back
          </Button>
          {step < STEPS.length - 1 ? (
            <Button tone="primary" size="md" disabled={!canNext || busy} onClick={() => setStep((s) => s + 1)}>
              Next
            </Button>
          ) : (
            <Button
              tone="primary"
              size="md"
              disabled={!canNext || busy}
              onClick={() => void finish(false)}
            >
              {busy ? "Saving…" : "Continue to tour"}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
