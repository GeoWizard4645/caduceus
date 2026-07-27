/**
 * Check that palette ranking does what it claims to.
 *
 * Ranking is three numbers multiplied out across 124 commands — a shipped
 * weight, a usage boost and a fuzzy score — and the interesting properties are
 * all about how those interact. Getting the constants wrong does not break the
 * build or fail a type check; it just quietly produces a list in the wrong
 * order, which nobody notices until the app feels bad to use.
 *
 * The four properties asserted here are the ones the design actually promises:
 *
 * 1. with no history, the shipped order leads with window snapping;
 * 2. with no history, the commands that end your session are last;
 * 3. one use of anything outranks everything untouched, and more uses outrank
 *    fewer — the "most used first" the browse list claims;
 * 4. typing still wins. History breaks ties; it does not override what you
 *    asked for.
 *
 * Run with `npm run check:ranking`. It is part of `npm run build`.
 */

import { build } from "esbuild";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const scratch = await mkdtemp(join(tmpdir(), "caduceus-ranking-"));

let failures = 0;
function check(label, ok) {
  console.log(`${ok ? "ok  " : "FAIL"} ${label}`);
  if (!ok) failures += 1;
}

try {
  const stub = join(scratch, "tauri-stub.js");
  await writeFile(stub, "export const invoke = () => Promise.resolve({});\n");

  // One entry point re-exporting the three modules that decide an order.
  const entry = join(scratch, "entry.ts");
  await writeFile(
    entry,
    [
      `export { COMMANDS, commandWeight } from ${JSON.stringify(join(root, "src/shared/commands.ts"))};`,
      `export { usageBoost, recordUsage } from ${JSON.stringify(join(root, "src/shared/usage.ts"))};`,
      `export { fuzzyScore } from ${JSON.stringify(join(root, "src/shared/fuzzy.ts"))};`,
    ].join("\n"),
  );

  const bundle = join(scratch, "ranking.mjs");
  await build({
    entryPoints: [entry],
    outfile: bundle,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
    alias: { "@tauri-apps/api/core": stub },
  });

  const { COMMANDS, commandWeight, usageBoost, recordUsage, fuzzyScore } = await import(
    pathToFileURL(bundle).href
  );

  /** The browse order: what the palette shows with nothing typed. */
  const browseOrder = () =>
    [...COMMANDS].sort(
      (a, b) =>
        usageBoost(`command:${b.id}`) +
        commandWeight(b) -
        (usageBoost(`command:${a.id}`) + commandWeight(a)),
    );

  check("window snapping leads the shipped order", browseOrder()[0].id === "window.left_half");

  const sessionEnders = ["system.log_out", "system.restart_computer", "system.shut_down"];
  check(
    "commands that end your session rank last",
    browseOrder()
      .slice(-3)
      .every((command) => sessionEnders.includes(command.id)),
  );

  // The lowest-weighted command there is, so this tests the widest possible gap.
  const weakest = [...COMMANDS].sort((a, b) => commandWeight(a) - commandWeight(b))[0];
  recordUsage(`command:${weakest.id}`);
  check(
    `one use lifts "${weakest.title}" (weight ${commandWeight(weakest)}) above everything unused`,
    browseOrder()[0].id === weakest.id,
  );

  const rival = COMMANDS.find((command) => command.id === "tool.uuid");
  for (let i = 0; i < 5; i += 1) recordUsage(`command:${rival.id}`);
  const ordered = browseOrder();
  check(
    "more uses outrank fewer",
    ordered[0].id === rival.id && ordered[1].id === weakest.id,
  );

  // The search path, as `commandProvider` computes it.
  const searchScore = (command, query) => {
    const score = fuzzyScore(query, [command.title, command.detail, ...command.keywords]);
    return score === null ? null : score - 10 + usageBoost(`command:${command.id}`);
  };
  const searched = COMMANDS.map((command) => ({ command, score: searchScore(command, "shut down") }))
    .filter((row) => row.score !== null)
    .sort((a, b) => b.score - a.score);

  check(
    `typing a name still wins — "shut down" finds "${searched[0].command.title}"`,
    searched[0].command.id === "system.shut_down",
  );

  // Product-name aliases: searching the tool someone is switching *from* must
  // find the Caduceus equivalent. These live in keyword lists, which once
  // decayed to nothing after the tenth entry — hence checked here forever.
  const topFor = (query) =>
    COMMANDS.map((command) => ({
      command,
      score: fuzzyScore(query, [command.title, command.detail, ...command.keywords]),
    }))
      .filter((row) => row.score !== null)
      .sort((a, b) => b.score - a.score)[0]?.command;

  const aliases = [
    ["amphetamine", "utility.caffeinate-on"],
    // CleanShot X is a screenshot and screen-recording tool. This used to point
    // at OCR, which was the closest thing Caduceus had; now there is an actual
    // recorder, and that is the honest answer.
    ["cleanshot", "page.screen-record"],
    ["maccy", "utility.clipboard"],
    ["orbstack", "list.docker"],
    ["jettison", "utility.eject"],
    // Activity Monitor opens on a list of processes with a Quit button, which
    // is the Processes page — not the System Monitor overview it used to map
    // to. Somebody typing this name almost always wants to kill something.
    ["activity monitor", "page.processes"],
    // Named here because these are the apps people arrive already knowing.
    ["cleanmymac", "page.storage"],
    ["appcleaner", "page.storage"],
    ["stickies", "page.sticky-notes"],
    ["granola", "page.meeting"],
  ];
  for (const [query, expectedId] of aliases) {
    const found = topFor(query);
    check(
      `"${query}" finds ${expectedId} (got "${found?.title}")`,
      found?.id === expectedId,
    );
  }
  const rectangle = topFor("rectangle");
  check(
    `"rectangle" finds a window command (got "${rectangle?.title}")`,
    rectangle?.id.startsWith("window."),
  );
} finally {
  await rm(scratch, { recursive: true, force: true });
}

if (failures) {
  console.log(`\n${failures} ranking propert${failures === 1 ? "y" : "ies"} broken.`);
  process.exit(1);
}
