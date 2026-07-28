/**
 * Birthdays: add a person + birthday, see who's next.
 *
 * Mirrors `src-tauri/src/tools/birthdays.rs`. Only month/day are required —
 * the birth year is optional and only drives the "turning N" label.
 */

import { useEffect, useState } from "react";

import * as api from "@/shared/api";
import type { UpcomingBirthday } from "@/shared/api";
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

const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

function relativeLabel(b: UpcomingBirthday): string {
  if (b.daysUntil === 0) return b.turning !== null ? `Today — turning ${b.turning}` : "Today";
  if (b.daysUntil === 1) return "Tomorrow";
  if (b.daysUntil < 30) return `In ${b.daysUntil} days`;
  const weeks = Math.round(b.daysUntil / 7);
  return `In ${weeks} week${weeks === 1 ? "" : "s"}`;
}

export function BirthdaysPage({ active, onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Birthdays"), [onSetTitle]);

  const [birthdays, setBirthdays] = useState<UpcomingBirthday[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState<string | null>(null);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [isNew, setIsNew] = useState(true);
  const [name, setName] = useState("");
  const [month, setMonth] = useState("1");
  const [day, setDay] = useState(1);
  const [year, setYear] = useState("");
  const [notes, setNotes] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);

  const reload = () => {
    api
      .birthdaysList()
      .then((list) => {
        setBirthdays(list);
        setLoaded(true);
      })
      .catch((e) => setListError(api.errorMessage(e)));
  };

  useEffect(reload, []);

  const selectNew = () => {
    setEditingId(null);
    setIsNew(true);
    setName("");
    setMonth("1");
    setDay(1);
    setYear("");
    setNotes("");
    setSaveError(null);
  };

  const select = (b: UpcomingBirthday) => {
    setEditingId(b.id);
    setIsNew(false);
    setName(b.name);
    setMonth(String(b.month));
    setDay(b.day);
    setYear(b.year !== null ? String(b.year) : "");
    setNotes(b.notes);
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
      setSaveError("Give this birthday a name.");
      return;
    }
    setSaving(true);
    setSaveError(null);
    const yearNum = year.trim() ? Number(year) : null;
    try {
      if (isNew) {
        await api.birthdaysAdd(name, Number(month), day, yearNum, notes || undefined);
      } else if (editingId) {
        await api.birthdaysUpdate(editingId, name, Number(month), day, yearNum, notes || undefined);
      }
      reload();
      selectNew();
    } catch (e) {
      setSaveError(api.errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (b: UpcomingBirthday) => {
    if (deleteArmedId !== b.id) {
      setDeleteArmedId(b.id);
      return;
    }
    setDeleteArmedId(null);
    try {
      await api.birthdaysDelete(b.id);
      setBirthdays((current) => current.filter((x) => x.id !== b.id));
      if (editingId === b.id) selectNew();
    } catch (e) {
      setListError(api.errorMessage(e));
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[880px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Birthdays</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            Add a person and their birthday; the list below always sorts soonest-first.
          </p>
        </div>

        <Section
          title="Everyone"
          description="Upcoming birthdays, soonest first."
          actions={
            <Button size="sm" tone="primary" onClick={selectNew}>
              + New
            </Button>
          }
        >
          {listError && <p className="mb-3 text-2xs text-danger">{listError}</p>}

          <div className="grid grid-cols-[1fr_260px] gap-4">
            {/* --- the list --------------------------------------------- */}
            <div className="max-h-[420px] overflow-y-auto rounded-lg border border-line">
              {!loaded ? (
                <div className="flex items-center justify-center py-8 text-ink-faint">
                  <Spinner />
                </div>
              ) : birthdays.length === 0 ? (
                <EmptyState title="No birthdays yet" hint="Add one on the right." icon="🎂" />
              ) : (
                birthdays.map((b) => (
                  <div
                    key={b.id}
                    className={cx(
                      "flex items-center gap-2 border-b border-line px-3 py-2 last:border-b-0",
                      b.id === editingId ? "bg-accent/10" : "hover:bg-raised/60",
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => select(b)}
                      className="min-w-0 flex-1 text-left text-[13px]"
                    >
                      <span className="font-medium text-ink">{b.name}</span>
                      <span className="ml-2 text-2xs text-ink-faint">
                        {MONTHS[b.month - 1]} {b.day}
                        {b.year ? `, ${b.year}` : ""}
                      </span>
                    </button>
                    <span className="shrink-0 text-2xs text-ink-mute">{relativeLabel(b)}</span>
                    <IconButton
                      label={deleteArmedId === b.id ? "Confirm delete" : "Delete"}
                      tone="danger"
                      onClick={() => void remove(b)}
                    >
                      {deleteArmedId === b.id ? "!" : "×"}
                    </IconButton>
                  </div>
                ))
              )}
            </div>

            {/* --- the editor ---------------------------------------------- */}
            <div className="space-y-3">
              <Field label="Name" error={saveError}>
                <TextInput value={name} onChange={setName} placeholder="Ada Lovelace" />
              </Field>
              <div className="grid grid-cols-2 gap-2">
                <Field label="Month">
                  <Select
                    value={month}
                    onChange={setMonth}
                    options={MONTHS.map((m, i) => ({ value: String(i + 1), label: m }))}
                  />
                </Field>
                <Field label="Day">
                  <NumberInput value={day} onChange={setDay} min={1} max={31} />
                </Field>
              </div>
              <Field label="Year" hint="Optional — drives the “turning N” label.">
                <TextInput value={year} onChange={setYear} placeholder="1990" />
              </Field>
              <Field label="Notes" hint="Optional.">
                <TextInput value={notes} onChange={setNotes} placeholder="Gift ideas, reminders…" />
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
