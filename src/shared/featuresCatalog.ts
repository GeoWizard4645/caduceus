/**
 * The complete catalogue of what Caduceus does, assembled from two sources.
 *
 * 1. **The command registry** (`commands.ts`) — everything you can *run*. Each
 *    command's own `detail` string is what appears here, so the explanation in
 *    the catalogue and the subtitle in the palette are the same sentence and
 *    cannot drift apart.
 * 2. **`website/features-catalog.json`** — the capabilities that are not
 *    commands: the staff, the windows, the clipboard engine, the permission
 *    model. The website reads the same file.
 *
 * The old shipped/planned split is gone. A roadmap that lives in the app ages
 * badly and, worse, invites a reader to tick off features against somebody
 * else's product — which is why the "Raycast-class / Caduceus idea" labels are
 * gone too. What is here is what the app does; the last section lists official
 * first-party extensions in development.
 */

import catalog from "../../website/features-catalog.json";
import { COMMANDS, COMMAND_GROUPS, commandWeight } from "./commands";
import { usageBoost } from "./usage";

const FIRST_PARTY_EXTENSIONS_ID = "first-party-extensions";

export interface FeatureItem {
  name: string;
  detail: string;
  /**
   * The command's usage key, for items that are runnable.
   *
   * Absent on capabilities, which have nothing to count.
   */
  usageKey?: string;
  /** The shipped ranking weight, used to order items before you have run any. */
  weight?: number;
}

export interface FeatureSection {
  id: string;
  title: string;
  /** One line under the section heading. */
  blurb: string;
  items: FeatureItem[];
  /** Commands are runnable from the palette; capabilities just exist. */
  runnable: boolean;
}

interface CatalogFile {
  version: number;
  groups: { id: string; title: string; blurb: string; items: FeatureItem[] }[];
}

const file = catalog as CatalogFile;

/** Sections generated from the command registry, in registry group order. */
const COMMAND_SECTIONS: FeatureSection[] = COMMAND_GROUPS.map((group) => ({
  id: `commands-${group.id}`,
  title: group.title,
  blurb: group.blurb,
  runnable: true,
  items: COMMANDS.filter((command) => command.group === group.id).map((command) => ({
    name: command.trigger ? `${command.title}  ·  ${command.trigger} …` : command.title,
    detail: command.detail,
    usageKey: `command:${command.id}`,
    weight: commandWeight(command),
  })),
})).filter((section) => section.items.length > 0);

/** Sections describing capabilities rather than commands. */
const CAPABILITY_SECTIONS: FeatureSection[] = file.groups.map((group) => ({
  id: group.id,
  title: group.title,
  blurb: group.blurb,
  items: group.items,
  runnable: false,
}));

/**
 * Capabilities first, then commands, then first-party extensions (coming soon) last.
 *
 * That order answers the two questions a reader arrives with in the order they
 * arrive in: "what *is* this thing" before "what can I type into it".
 */
export const FEATURE_SECTIONS: FeatureSection[] = [
  ...CAPABILITY_SECTIONS.filter((section) => section.id !== FIRST_PARTY_EXTENSIONS_ID),
  ...COMMAND_SECTIONS,
  ...CAPABILITY_SECTIONS.filter((section) => section.id === FIRST_PARTY_EXTENSIONS_ID),
];

export function countFeatures(): number {
  return FEATURE_SECTIONS.filter((section) => section.id !== FIRST_PARTY_EXTENSIONS_ID).reduce(
    (total, section) => total + section.items.length,
    0,
  );
}

export function countCommands(): number {
  return COMMANDS.length;
}

/**
 * Order a section's items the way the palette orders them: most used first,
 * falling back to the shipped weight.
 *
 * Applied at render time rather than baked into `FEATURE_SECTIONS`, because the
 * counts change while the window is open and a catalogue that disagreed with the
 * palette about what comes first would be worse than one that never sorted.
 * Capability sections have nothing to count and keep their authored order.
 */
export function rankItems(section: FeatureSection): FeatureItem[] {
  if (!section.runnable) return section.items;

  return [...section.items].sort((a, b) => {
    const score = (item: FeatureItem) =>
      (item.usageKey ? usageBoost(item.usageKey) : 0) + (item.weight ?? 0);
    return score(b) - score(a);
  });
}
