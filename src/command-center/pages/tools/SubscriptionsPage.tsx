/**
 * Subscription tracker: what you pay, how often, and when it renews next.
 *
 * Mirrors `src-tauri/src/tools/subscriptions.rs`. A subscription's
 * `renewalDate` is the *next known* renewal at save time — the backend rolls
 * it forward past today by whole billing cycles on every read, so a stale
 * date is never a data-entry problem.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { BillingCycle, SubscriptionSummary, UpcomingSubscription } from "@/shared/api";
import { useEscape } from "@/shared/hooks";
import {
  Button,
  EmptyState,
  Field,
  IconButton,
  NumberInput,
  Section,
  Select,
  Spinner,
  TextInput,
  cx,
} from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

const CYCLE_OPTIONS: { value: BillingCycle; label: string }[] = [
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "quarterly", label: "Quarterly" },
  { value: "yearly", label: "Yearly" },
];

function money(n: number): string {
  return n.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

/** A styled `<input type="date">` — the only date picker used here, kept
 * local rather than promoted to `shared/ui` for one field's sake. */
function DateField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <input
      type="date"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full rounded-lg border border-line-strong/60 bg-base/60 px-3 py-2 text-[13px] text-ink focus:border-accent/70 focus:outline-none"
    />
  );
}

export function SubscriptionsPage({ active, onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Subscriptions"), [onSetTitle]);

  const [subs, setSubs] = useState<UpcomingSubscription[]>([]);
  const [summary, setSummary] = useState<SubscriptionSummary | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState<string | null>(null);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [isNew, setIsNew] = useState(true);
  const [name, setName] = useState("");
  const [cost, setCost] = useState(0);
  const [cycle, setCycle] = useState<BillingCycle>("monthly");
  const [renewalDate, setRenewalDate] = useState(todayIso());
  const [notes, setNotes] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);

  const reload = () => {
    Promise.all([api.subscriptionsList(), api.subscriptionsSummary()])
      .then(([list, sum]) => {
        setSubs(list);
        setSummary(sum);
        setLoaded(true);
      })
      .catch((e) => setListError(api.errorMessage(e)));
  };

  useEffect(reload, []);

  const selectNew = () => {
    setEditingId(null);
    setIsNew(true);
    setName("");
    setCost(0);
    setCycle("monthly");
    setRenewalDate(todayIso());
    setNotes("");
    setSaveError(null);
  };

  const select = (s: UpcomingSubscription) => {
    setEditingId(s.id);
    setIsNew(false);
    setName(s.name);
    setCost(s.cost);
    setCycle(s.cycle);
    setRenewalDate(s.renewalDate);
    setNotes(s.notes);
    setSaveError(null);
    setDeleteArmedId(null);
  };

  useEscape(active, () => {
    if (!deleteArmedId) return false;
    setDeleteArmedId(null);
    return true;
  });

  const save = async () => {
    if (!name.trim()) {
      setSaveError("Give the subscription a name.");
      return;
    }
    if (!renewalDate) {
      setSaveError("Pick a renewal date.");
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      if (isNew) {
        await api.subscriptionsAdd(name, cost, cycle, renewalDate, notes || undefined);
      } else if (editingId) {
        await api.subscriptionsUpdate(editingId, name, cost, cycle, renewalDate, notes || undefined);
      }
      reload();
      selectNew();
    } catch (e) {
      setSaveError(api.errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (s: UpcomingSubscription) => {
    if (deleteArmedId !== s.id) {
      setDeleteArmedId(s.id);
      return;
    }
    setDeleteArmedId(null);
    try {
      await api.subscriptionsDelete(s.id);
      reload();
      if (editingId === s.id) selectNew();
    } catch (e) {
      setListError(api.errorMessage(e));
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[880px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Subscriptions</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            What you pay, how often, and when it renews next — all local to this Mac.
          </p>
        </div>

        {summary && summary.count > 0 && (
          <div className="mb-5 grid grid-cols-3 gap-3">
            <div className="rounded-lg border border-line bg-base/40 p-3">
              <p className="text-2xs text-ink-faint">Subscriptions</p>
              <p className="mt-1 text-lg font-semibold tabular-nums text-ink">{summary.count}</p>
            </div>
            <div className="rounded-lg border border-line bg-base/40 p-3">
              <p className="text-2xs text-ink-faint">Per month</p>
              <p className="mt-1 text-lg font-semibold tabular-nums text-ink">{money(summary.monthlyTotal)}</p>
            </div>
            <div className="rounded-lg border border-line bg-base/40 p-3">
              <p className="text-2xs text-ink-faint">Per year</p>
              <p className="mt-1 text-lg font-semibold tabular-nums text-ink">{money(summary.yearlyTotal)}</p>
            </div>
          </div>
        )}

        <Section
          title="Everyone you pay"
          description="Sorted by next renewal."
          actions={
            <Button size="sm" tone="primary" onClick={selectNew}>
              + New
            </Button>
          }
        >
          {listError && <p className="mb-3 text-2xs text-danger">{listError}</p>}

          <div className="grid grid-cols-[1fr_280px] gap-4">
            <div className="max-h-[420px] overflow-y-auto rounded-lg border border-line">
              {!loaded ? (
                <div className="flex items-center justify-center py-8 text-ink-faint">
                  <Spinner />
                </div>
              ) : subs.length === 0 ? (
                <EmptyState title="Nothing tracked yet" hint="Add one on the right." icon="💳" />
              ) : (
                subs.map((s) => (
                  <div
                    key={s.id}
                    className={cx(
                      "flex items-center gap-2 border-b border-line px-3 py-2 last:border-b-0",
                      s.id === editingId ? "bg-accent/10" : "hover:bg-raised/60",
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => select(s)}
                      className="min-w-0 flex-1 text-left text-[13px]"
                    >
                      <span className="font-medium text-ink">{s.name}</span>
                      <span className="ml-2 text-2xs text-ink-faint">
                        {money(s.cost)} / {s.cycle}
                      </span>
                    </button>
                    <span className="shrink-0 text-2xs text-ink-mute">
                      {s.daysUntil === 0 ? "Renews today" : `In ${s.daysUntil}d`}
                    </span>
                    <IconButton
                      label={deleteArmedId === s.id ? "Confirm delete" : "Delete"}
                      tone="danger"
                      onClick={() => void remove(s)}
                    >
                      {deleteArmedId === s.id ? "!" : "×"}
                    </IconButton>
                  </div>
                ))
              )}
            </div>

            <div className="space-y-3">
              <Field label="Name" error={saveError}>
                <TextInput value={name} onChange={setName} placeholder="Streaming service" />
              </Field>
              <div className="grid grid-cols-2 gap-2">
                <Field label="Cost">
                  <NumberInput value={cost} onChange={setCost} min={0} step={0.01} />
                </Field>
                <Field label="Cycle">
                  <Select value={cycle} onChange={setCycle} options={CYCLE_OPTIONS} />
                </Field>
              </div>
              <Field label="Next renewal">
                <DateField value={renewalDate} onChange={setRenewalDate} />
              </Field>
              <Field label="Notes" hint="Optional.">
                <TextInput value={notes} onChange={setNotes} placeholder="Shared plan, cancel by…" />
              </Field>
              <Button tone="primary" onClick={() => void save()} disabled={saving}>
                {saving ? <Spinner /> : null} {isNew ? "Add" : "Save"}
              </Button>
            </div>
          </div>
        </Section>
      </div>
    </div>
  );
}
