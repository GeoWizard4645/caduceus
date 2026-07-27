/**
 * Regenerate the command half of `website/features-catalog.json`.
 *
 * The app reads the command registry directly from `src/shared/commands.ts`.
 * The website is static HTML with no bundler, so it needs the same data as
 * JSON — and a hand-maintained second copy would be wrong within a release.
 *
 * This bundles the registry with esbuild (stubbing the Tauri IPC layer, which
 * has no meaning outside a webview), reads `COMMANDS` and `COMMAND_GROUPS`, and
 * writes them into the catalogue next to the capability groups. The `run`
 * functions are simply not serialised.
 *
 * Run with `npm run catalog`. It is also run by `npm run build`, so a command
 * added to the registry cannot ship without the website knowing about it.
 */

import { build } from "esbuild";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const catalogPath = join(root, "website", "features-catalog.json");

const scratch = await mkdtemp(join(tmpdir(), "caduceus-catalog-"));

try {
  // `invoke` is never called at import time; the registry only closes over it.
  const stub = join(scratch, "tauri-stub.js");
  await writeFile(
    stub,
    "export const invoke = () => Promise.reject(new Error('not in a webview'));\n",
  );

  const bundle = join(scratch, "commands.mjs");
  await build({
    entryPoints: [join(root, "src", "shared", "commands.ts")],
    outfile: bundle,
    bundle: true,
    format: "esm",
    platform: "node",
    logLevel: "silent",
    alias: { "@tauri-apps/api/core": stub },
  });

  const { COMMANDS, COMMAND_GROUPS } = await import(pathToFileURL(bundle).href);

  const commandGroups = COMMAND_GROUPS.map((group) => ({
    id: group.id,
    title: group.title,
    blurb: group.blurb,
    items: COMMANDS.filter((command) => command.group === group.id).map((command) => ({
      name: command.trigger ? `${command.title}  ·  ${command.trigger} …` : command.title,
      detail: command.detail,
    })),
  })).filter((group) => group.items.length > 0);

  const catalog = JSON.parse(await readFile(catalogPath, "utf8"));
  catalog.commandGroups = commandGroups;

  await writeFile(catalogPath, `${JSON.stringify(catalog, null, 2)}\n`);

  const total = commandGroups.reduce((n, group) => n + group.items.length, 0);
  console.log(`features-catalog.json: ${total} commands in ${commandGroups.length} groups`);
} finally {
  await rm(scratch, { recursive: true, force: true });
}
