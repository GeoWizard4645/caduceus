/**
 * Settings → Features: everything Caduceus does, one collapsible section at a
 * time.
 *
 * # Why accordions
 *
 * The catalogue is long enough that a flat list is a wall — you scroll past what
 * you wanted before you have read the heading. Collapsed sections turn it into a
 * table of contents you can read in one screen, and open exactly the part you
 * came for.
 *
 * Filtering overrides that: while there is a query, every section with a match
 * is open, because a hit hidden inside a collapsed section is the same as no hit
 * at all.
 */

import { useEffect, useMemo, useState } from "react";

import * as api from "@/shared/api";
import {
  FEATURE_SECTIONS,
  countCommands,
  countFeatures,
  rankItems,
  type FeatureSection,
} from "@/shared/featuresCatalog";
import { loadUsage } from "@/shared/usage";
import { DOCS_FEATURES } from "@/shared/docsUrls";
import { Button, Section, cx } from "@/shared/ui";

export function FeaturesCatalogTab() {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [usageReady, setUsageReady] = useState(false);

  // Command sections are ordered by how often you run each one, so this window
  // agrees with the palette about what comes first.
  useEffect(() => {
    void loadUsage().finally(() => setUsageReady(true));
  }, []);

  const q = query.trim().toLowerCase();

  const sections = useMemo(
    () => filterSections(FEATURE_SECTIONS, q).map((section) => ({
      ...section,
      items: rankItems(section),
    })),
    // `usageReady` is a dependency because the counts it loads are read
    // synchronously by `rankItems` rather than passed in.
    [q, usageReady],
  );

  const total = countFeatures();
  const commands = countCommands();

  const allOpen = sections.length > 0 && sections.every((section) => open.has(section.id));

  return (
    <Section
      title="Everything Caduceus does"
      description={`${total} capabilities, of which ${commands} are commands you can run straight from the Command Center. Everything below works offline, with no account and no API key, unless its description says otherwise.`}
    >
      <div className="row flex-wrap gap-2">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter everything…"
          className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-2 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
        />
        <Button
          size="sm"
          onClick={() =>
            setOpen(allOpen ? new Set() : new Set(sections.map((section) => section.id)))
          }
        >
          {allOpen ? "Collapse all" : "Expand all"}
        </Button>
        <Button size="sm" onClick={() => void api.openExternalUrl(DOCS_FEATURES)}>
          Open on the web ↗
        </Button>
      </div>

      <div className="mt-4 space-y-2">
        {sections.map((section) => (
          <FeatureAccordion
            key={section.id}
            section={section}
            // A query forces every matching section open: a result you cannot
            // see is not a result.
            open={q.length > 0 || open.has(section.id)}
            locked={q.length > 0}
            onToggle={() =>
              setOpen((current) => {
                const next = new Set(current);
                if (!next.delete(section.id)) next.add(section.id);
                return next;
              })
            }
          />
        ))}

        {sections.length === 0 && (
          <p className="rounded-lg border border-dashed border-line px-3 py-6 text-center text-2xs text-ink-faint">
            Nothing matches “{query}”.
          </p>
        )}
      </div>
    </Section>
  );
}

function FeatureAccordion({
  section,
  open,
  locked,
  onToggle,
}: {
  section: FeatureSection;
  open: boolean;
  locked: boolean;
  onToggle: () => void;
}) {
  const isUpcoming = section.id === "first-party-extensions";

  return (
    <div className="overflow-hidden rounded-xl border border-line bg-base/20 transition-colors">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        disabled={locked}
        className={cx(
          "flex w-full items-center gap-3 px-3.5 py-3 text-left transition-colors",
          !locked && "hover:bg-raised/50",
        )}
      >
        <span
          aria-hidden="true"
          className={cx(
            "shrink-0 text-ink-faint transition-transform duration-150",
            open && "rotate-90",
          )}
        >
          ›
        </span>

        <span className="min-w-0 flex-1">
          <span className="row gap-2">
            <span className="text-[13px] font-semibold text-ink">{section.title}</span>
            {section.runnable && (
              <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-accent">
                Commands
              </span>
            )}
            {isUpcoming && (
              <span className="rounded bg-caution/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-caution">
                Coming soon
              </span>
            )}
          </span>
          {section.blurb && (
            <span className="mt-0.5 block text-2xs leading-relaxed text-ink-mute">
              {section.blurb}
            </span>
          )}
        </span>

        <span className="shrink-0 text-2xs tabular-nums text-ink-faint">
          {section.items.length}
        </span>
      </button>

      {open && (
        <ul className="border-t border-line/60 px-3.5 py-2">
          {section.items.map((item) => (
            <li key={item.name} className="border-b border-line/40 py-2 last:border-b-0">
              <p className="text-2xs font-medium text-ink-soft">{item.name}</p>
              {item.detail && (
                <p className="mt-0.5 text-2xs leading-relaxed text-ink-mute">{item.detail}</p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Keep sections that have a matching item, narrowed to the items that match. */
function filterSections(sections: FeatureSection[], q: string): FeatureSection[] {
  if (!q) return sections;

  return sections
    .map((section) => {
      // A section whose own title matches keeps all of its items — searching
      // "window" should show the whole window-management section, not the two
      // entries that happen to repeat the word.
      if (section.title.toLowerCase().includes(q)) return section;

      return {
        ...section,
        items: section.items.filter(
          (item) =>
            item.name.toLowerCase().includes(q) || item.detail.toLowerCase().includes(q),
        ),
      };
    })
    .filter((section) => section.items.length > 0);
}
