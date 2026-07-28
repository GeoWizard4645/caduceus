/**
 * Habit tracker: create a habit, mark a day done, watch the streak.
 *
 * Mirrors `src-tauri/src/tools/habits.rs`, which persists to its own JSON
 * store rather than `Settings` — see that module's docs.
 */

import { useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import type { Habit, StreakInfo } from "@/shared/api";
import { Button, EmptyState, Field, IconButton, Section, Spinner, TextInput, cx } from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

/** How many trailing days the little calendar strip shows per habit. */
const STRIP_DAYS = 14;

function isoDate(d: Date): string {
  return d.toISOString().slice(0, 10);
}

/** The last `STRIP_DAYS` dates, oldest first, ending today. */
function recentDates(): string[] {
  const out: string[] = [];
  const today = new Date();
  for (let i = STRIP_DAYS - 1; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    out.push(isoDate(d));
  }
  return out;
}

const SWATCHES = ["#7c7cff", "#f97066", "#22c55e", "#f59e0b", "#06b6d4", "#ec4899"];

export function HabitsPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("Habit Tracker"), [onSetTitle]);

  const [habits, setHabits] = useState<Habit[]>([]);
  const [streaks, setStreaks] = useState<Record<string, StreakInfo>>({});
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [color, setColor] = useState(SWATCHES[0]);
  const [creating, setCreating] = useState(false);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);

  const days = useMemo(recentDates, []);

  const reloadStreak = async (id: string) => {
    try {
      const info = await api.habitsStreak(id);
      setStreaks((current) => ({ ...current, [id]: info }));
    } catch {
      // A missing streak badge is not worth surfacing as an error.
    }
  };

  const reload = async () => {
    try {
      const list = await api.habitsList();
      setHabits(list);
      setLoaded(true);
      await Promise.all(list.map((h) => reloadStreak(h.id)));
    } catch (e) {
      setError(api.errorMessage(e));
      setLoaded(true);
    }
  };

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const create = async () => {
    if (!name.trim()) return;
    setCreating(true);
    setError(null);
    try {
      const habit = await api.habitsCreate(name.trim(), color);
      setHabits((current) => [...current, habit]);
      setStreaks((current) => ({ ...current, [habit.id]: { current: 0, longest: 0, totalCompletions: 0 } }));
      setName("");
    } catch (e) {
      setError(api.errorMessage(e));
    } finally {
      setCreating(false);
    }
  };

  const toggleDay = async (habit: Habit, date: string) => {
    try {
      const updated = await api.habitsToggleDay(habit.id, date);
      setHabits((current) => current.map((h) => (h.id === habit.id ? updated : h)));
      await reloadStreak(habit.id);
    } catch (e) {
      setError(api.errorMessage(e));
    }
  };

  const remove = async (habit: Habit) => {
    if (deleteArmedId !== habit.id) {
      setDeleteArmedId(habit.id);
      return;
    }
    setDeleteArmedId(null);
    try {
      await api.habitsDelete(habit.id);
      setHabits((current) => current.filter((h) => h.id !== habit.id));
    } catch (e) {
      setError(api.errorMessage(e));
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[760px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">Habit tracker</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            Create a habit, tap a day to mark it done, and watch the streak. Everything here is stored
            locally on this Mac.
          </p>
        </div>

        <Section
          title="New habit"
          description="Give it a short name — you'll see this every day."
        >
          <div className="flex items-end gap-3">
            <div className="flex-1">
              <Field label="Name">
                <TextInput value={name} onChange={setName} placeholder="Drink water, Read, Stretch…" />
              </Field>
            </div>
            <div className="flex gap-1.5 pb-2">
              {SWATCHES.map((swatch) => (
                <button
                  key={swatch}
                  type="button"
                  aria-label={`Colour ${swatch}`}
                  onClick={() => setColor(swatch)}
                  className={cx(
                    "h-7 w-7 rounded-full border-2 transition-transform",
                    color === swatch ? "scale-110 border-ink" : "border-transparent",
                  )}
                  style={{ backgroundColor: swatch }}
                />
              ))}
            </div>
            <Button tone="primary" onClick={() => void create()} disabled={creating || !name.trim()}>
              {creating ? <Spinner /> : null} Add
            </Button>
          </div>
          {error && <p className="mt-3 text-2xs text-danger">{error}</p>}
        </Section>

        <Section title="Your habits">
          {!loaded ? (
            <div className="flex items-center justify-center py-8 text-ink-faint">
              <Spinner />
            </div>
          ) : habits.length === 0 ? (
            <EmptyState
              title="No habits yet"
              hint="Add one above to start tracking."
              icon="◍"
            />
          ) : (
            <div className="space-y-4">
              {habits.map((habit) => {
                const streak = streaks[habit.id];
                return (
                  <div key={habit.id} className="rounded-lg border border-line bg-base/40 p-3">
                    <div className="mb-2 flex items-center gap-2">
                      <span
                        className="h-2.5 w-2.5 shrink-0 rounded-full"
                        style={{ backgroundColor: habit.color || "#7c7cff" }}
                      />
                      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">
                        {habit.name}
                      </span>
                      {streak && (
                        <span className="shrink-0 text-2xs text-ink-faint">
                          🔥 {streak.current} current · {streak.longest} best
                        </span>
                      )}
                      <IconButton
                        label={deleteArmedId === habit.id ? "Confirm delete" : "Delete habit"}
                        tone="danger"
                        onClick={() => void remove(habit)}
                      >
                        {deleteArmedId === habit.id ? "!" : "×"}
                      </IconButton>
                    </div>

                    <div className="flex gap-1">
                      {days.map((date) => {
                        const done = habit.completions.includes(date);
                        const isToday = date === days[days.length - 1];
                        return (
                          <button
                            key={date}
                            type="button"
                            title={date}
                            onClick={() => void toggleDay(habit, date)}
                            className={cx(
                              "h-6 flex-1 rounded transition-colors",
                              done ? "" : "bg-raised hover:bg-overlay",
                              isToday && !done && "ring-1 ring-inset ring-accent/40",
                            )}
                            style={done ? { backgroundColor: habit.color || "#7c7cff" } : undefined}
                          />
                        );
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </Section>
      </div>
    </div>
  );
}
