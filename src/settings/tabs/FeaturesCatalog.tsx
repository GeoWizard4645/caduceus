/**
 * Complete checklist of what Caduceus ships today and what is planned next.
 * Data lives in website/features-catalog.json (also used by the marketing site).
 */

import { useMemo, useState } from "react";

import * as api from "@/shared/api";
import {
  PLANNED_FEATURES,
  SHIPPED_SECTIONS,
  countShippedFeatures,
} from "@/shared/featuresCatalog";
import { DOCS_FEATURES } from "@/shared/docsUrls";
import { Button, Section, cx } from "@/shared/ui";

function matchesQuery(text: string, q: string): boolean {
  return text.toLowerCase().includes(q);
}

export function FeaturesCatalogTab() {
  const [query, setQuery] = useState("");

  const q = query.trim().toLowerCase();

  const shipped = useMemo(() => {
    if (!q) return SHIPPED_SECTIONS;
    return SHIPPED_SECTIONS.map((section) => ({
      ...section,
      items: section.items.filter(
        (item) =>
          matchesQuery(item.name, q) || (item.detail && matchesQuery(item.detail, q)),
      ),
    })).filter((section) => section.items.length > 0);
  }, [q]);

  const planned = useMemo(() => {
    if (!q) return PLANNED_FEATURES;
    return PLANNED_FEATURES.filter(
      (item) => matchesQuery(item.name, q) || matchesQuery(item.detail, q),
    );
  }, [q]);

  const shippedCount = countShippedFeatures();

  return (
    <>
      <Section
        title="Everything that exists today"
        description={`${shippedCount} built-in capabilities across ${SHIPPED_SECTIONS.length} areas. Offline features work without Hermes, API keys, or an account.`}
      >
        <div className="row flex-wrap gap-2">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter features…"
            className="min-w-[200px] flex-1 rounded-lg border border-line bg-base/40 px-3 py-2 text-[13px] text-ink placeholder:text-ink-faint focus:border-accent/50 focus:outline-none"
          />
          <Button size="sm" onClick={() => void api.openExternalUrl(DOCS_FEATURES)}>
            Open on the web ↗
          </Button>
        </div>

        <div className="mt-4 space-y-5">
          {shipped.map((section) => (
            <div key={section.id}>
              <h3 className="text-[13px] font-semibold text-ink">{section.title}</h3>
              <ul className="mt-2 space-y-1.5">
                {section.items.map((item) => (
                  <li
                    key={item.name}
                    className="rounded-lg border border-line bg-base/20 px-3 py-2 text-2xs leading-relaxed"
                  >
                    <span className="font-medium text-ink-soft">{item.name}</span>
                    {item.detail ? (
                      <span className="text-ink-mute"> — {item.detail}</span>
                    ) : null}
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {shipped.length === 0 && (
            <p className="text-2xs text-ink-faint">No shipped features match that filter.</p>
          )}
        </div>
      </Section>

      <Section
        title="Planned & inspired"
        description={`${PLANNED_FEATURES.length} ideas on the roadmap — many familiar from Raycast, plus Caduceus-only ideas. Not built yet.`}
      >
        <ul className="space-y-1.5">
          {planned.map((item) => (
            <li
              key={item.name}
              className="rounded-lg border border-dashed border-line bg-base/10 px-3 py-2 text-2xs leading-relaxed"
            >
              <span className="row flex-wrap gap-2">
                <span className="font-medium text-ink-soft">{item.name}</span>
                {item.tag && (
                  <span
                    className={cx(
                      "rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide",
                      item.tag === "raycast"
                        ? "bg-raised text-ink-faint"
                        : "bg-accent/15 text-accent",
                    )}
                  >
                    {item.tag === "raycast" ? "Raycast-class" : "Caduceus idea"}
                  </span>
                )}
              </span>
              <span className="mt-0.5 block text-ink-mute">{item.detail}</span>
            </li>
          ))}
        </ul>
        {planned.length === 0 && (
          <p className="text-2xs text-ink-faint">No planned features match that filter.</p>
        )}
      </Section>
    </>
  );
}
