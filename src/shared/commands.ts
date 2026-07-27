/**
 * The built-in command registry.
 *
 * This file is the single source of truth for everything Caduceus can *do* on
 * demand. Two things read it:
 *
 * 1. {@link commandProvider} in `providers.ts`, which fuzzy-matches the registry
 *    on every keystroke, so every command sits in the same ranked list as your
 *    apps, shortcuts and clipboard rather than behind a menu of its own;
 * 2. Settings → Features and the website's feature page, which render `detail`
 *    verbatim. There is no second copy of the explanations to fall out of date.
 *
 * # Adding one
 *
 * Append an entry. That is the whole process — it appears in search, in the
 * catalogue and on the website with no further wiring.
 *
 * # Writing `detail`
 *
 * One or two sentences saying what happens and what you get back, in the same
 * register as the rest of the app: no marketing, no "simply", and a plain
 * statement of any permission or precondition the command needs.
 */

import * as api from "./api";
import type { MediaAction, SystemAction, ToolId, WindowVerb } from "./types";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A panel of text shown in the palette, with a copy button. */
export interface CommandOutput {
  title: string;
  text: string;
  message?: string;
}

/** What a command can ask the palette to do. */
export interface CommandActions {
  notify(message: string, tone?: "info" | "error"): void;
  showOutput(output: CommandOutput): void;
  setInput(value: string): void;
  close(): void;
}

export interface CommandContext {
  /** Whatever was typed after the trigger word, trimmed. */
  input: string;
  actions: CommandActions;
}

/**
 * Returning `false` keeps the palette open; anything else closes it.
 *
 * The rule the registry follows: a command that *changes something you can see*
 * closes, because the change is its own feedback. A command that *produces text*
 * stays open, because closing would throw the answer away.
 */
export type CommandResult = boolean | void;

export interface CommandDef {
  id: string;
  title: string;
  /** Rendered in the palette subtitle, in Settings → Features, and on the web. */
  detail: string;
  group: CommandGroupId;
  icon: string;
  /** Extra words this command should be findable by. */
  keywords: string[];
  /**
   * The word that, typed first, routes the rest of the line to this command —
   * `sha256 hello` hashes "hello". Commands without one take no argument.
   */
  trigger?: string;
  /** Placeholder describing what to type after the trigger. */
  argument?: string;
  /** Shown before the command runs; Enter again confirms. */
  confirm?: string;
  run(ctx: CommandContext): Promise<CommandResult> | CommandResult;
}

export type CommandGroupId =
  | "windows"
  | "system"
  | "sound"
  | "screen"
  | "developer"
  | "text"
  | "files"
  | "network"
  | "devenv"
  | "utilities";

export interface CommandGroup {
  id: CommandGroupId;
  title: string;
  /** One line for the section heading in the catalogue. */
  blurb: string;
}

export const COMMAND_GROUPS: CommandGroup[] = [
  {
    id: "windows",
    title: "Window management",
    blurb:
      "Snap, resize and move the frontmost window of any application. Needs the Accessibility permission, which Caduceus asks for once and never prompts for again.",
  },
  {
    id: "system",
    title: "System",
    blurb:
      "macOS settings, the Finder, power and the session — driven by the tools macOS already ships, so none of it needs anything installed.",
  },
  {
    id: "sound",
    title: "Sound & media",
    blurb:
      "Volume, input and output devices, and whatever is playing in Music or Spotify.",
  },
  {
    id: "screen",
    title: "Screen & text",
    blurb:
      "Capture the screen and read text off it. Recognition runs on-device through Apple's Vision framework; no image leaves the machine.",
  },
  {
    id: "developer",
    title: "Developer tools",
    blurb:
      "Encoders, hashes, identifiers, formatters and inspectors. Every one of these runs locally, which is the point — a JWT or an API payload should never be pasted into a website to be read.",
  },
  {
    id: "text",
    title: "Text",
    blurb: "Reshape whatever you type or paste: case, line order, counts and cleanup.",
  },
  {
    id: "files",
    title: "Files & Finder",
    blurb:
      "Actions on the current Finder selection. Deletion always means the Trash, never an unrecoverable remove.",
  },
  { id: "network", title: "Network", blurb: "Addresses, name resolution and reachability." },
  {
    id: "devenv",
    title: "Developer environment",
    blurb:
      "What is running on this machine: ports, repositories, SSH hosts and containers.",
  },
  {
    id: "utilities",
    title: "Utilities",
    blurb: "The small things that are faster to type than to go and find.",
  },
];

// ---------------------------------------------------------------------------
// Shared runners
// ---------------------------------------------------------------------------

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Run something that returns a `ToolOutcome`.
 *
 * Toasts are rendered *inside* the Command Center, so closing the palette
 * destroys the message along with it. The default is therefore to stay open and
 * show what happened — "Ejected Untitled, Backup" is worth reading, and losing
 * it to a window that closed half a frame earlier is not a trade worth making.
 *
 * `close: true` is for the handful of commands whose effect is unmistakable
 * without a message: arranging a window, or ending the session.
 */
async function outcome(
  actions: CommandActions,
  title: string,
  call: () => Promise<{ ok: boolean; message: string; copied: string | null }>,
  close = false,
): Promise<CommandResult> {
  try {
    const result = await call();
    if (!result.ok) {
      actions.notify(result.message, "error");
      return false;
    }
    if (result.copied) {
      const copied = await copyText(result.copied);
      actions.showOutput({
        title,
        text: result.copied,
        message: copied ? `${result.message} · copied` : result.message,
      });
      return false;
    }
    actions.notify(result.message);
    return close ? true : false;
  } catch (error) {
    actions.notify(api.errorMessage(error), "error");
    return false;
  }
}

/** Run one of the developer tools and show whatever it produced. */
async function tool(actions: CommandActions, id: ToolId, input: string): Promise<CommandResult> {
  try {
    const result = await api.runTool(id, input);
    if (!result.ok) {
      actions.notify(result.message, "error");
      return false;
    }
    const copied = result.autoCopy ? await copyText(result.output) : false;
    const suffix = copied ? (result.message ? " · copied" : "Copied") : "";
    actions.showOutput({
      title: result.title,
      text: result.output,
      message: `${result.message}${suffix}`.trim(),
    });
    return false;
  } catch (error) {
    actions.notify(api.errorMessage(error), "error");
    return false;
  }
}

/** Run a window verb, turning a missing permission into a usable sentence. */
async function windowVerb(actions: CommandActions, verb: WindowVerb): Promise<CommandResult> {
  try {
    const result = await api.windowAction(verb);
    if (result.ok) return true;
    actions.notify(result.message, "error");
    return false;
  } catch (error) {
    actions.notify(api.errorMessage(error), "error");
    return false;
  }
}

function system(
  actions: CommandActions,
  action: SystemAction,
  title: string,
  close = false,
) {
  return outcome(actions, title, () => api.systemAction(action), close);
}

function media(actions: CommandActions, action: MediaAction, title: string) {
  return outcome(actions, title, () => api.mediaAction(action));
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

interface WindowSpec {
  verb: WindowVerb;
  title: string;
  detail: string;
  keywords: string[];
}

const WINDOW_SPECS: WindowSpec[] = [
  {
    verb: "left_half",
    title: "Window: left half",
    detail:
      "Fills the left half of the display the window is currently on, stopping at the menu bar and the Dock rather than sliding under them.",
    keywords: ["snap", "tile", "left"],
  },
  {
    verb: "right_half",
    title: "Window: right half",
    detail: "Fills the right half of the current display. Pairs exactly with the left half — no gap, no overlap.",
    keywords: ["snap", "tile", "right"],
  },
  {
    verb: "top_half",
    title: "Window: top half",
    detail: "Fills the upper half of the current display.",
    keywords: ["snap", "tile", "top", "up"],
  },
  {
    verb: "bottom_half",
    title: "Window: bottom half",
    detail: "Fills the lower half of the current display.",
    keywords: ["snap", "tile", "bottom", "down"],
  },
  {
    verb: "top_left_quarter",
    title: "Window: top-left quarter",
    detail: "One of four quarters that tile the usable screen area exactly.",
    keywords: ["snap", "quarter", "corner"],
  },
  {
    verb: "top_right_quarter",
    title: "Window: top-right quarter",
    detail: "One of four quarters that tile the usable screen area exactly.",
    keywords: ["snap", "quarter", "corner"],
  },
  {
    verb: "bottom_left_quarter",
    title: "Window: bottom-left quarter",
    detail: "One of four quarters that tile the usable screen area exactly.",
    keywords: ["snap", "quarter", "corner"],
  },
  {
    verb: "bottom_right_quarter",
    title: "Window: bottom-right quarter",
    detail: "One of four quarters that tile the usable screen area exactly.",
    keywords: ["snap", "quarter", "corner"],
  },
  {
    verb: "first_third",
    title: "Window: first third",
    detail: "The left third of the display. Thirds are computed to add up exactly, so three windows leave no seam.",
    keywords: ["snap", "third", "column"],
  },
  {
    verb: "center_third",
    title: "Window: middle third",
    detail: "The middle third of the display.",
    keywords: ["snap", "third", "column", "centre"],
  },
  {
    verb: "last_third",
    title: "Window: last third",
    detail: "The right third of the display.",
    keywords: ["snap", "third", "column"],
  },
  {
    verb: "first_two_thirds",
    title: "Window: first two-thirds",
    detail: "The left two-thirds — the wide half of a two-thirds/one-third split.",
    keywords: ["snap", "two thirds", "wide"],
  },
  {
    verb: "last_two_thirds",
    title: "Window: last two-thirds",
    detail: "The right two-thirds.",
    keywords: ["snap", "two thirds", "wide"],
  },
  {
    verb: "maximize",
    title: "Window: maximize",
    detail:
      "Fills the whole usable area of the display. Not macOS full screen — the window keeps its title bar and stays in the current Space.",
    keywords: ["snap", "fill", "full", "zoom"],
  },
  {
    verb: "almost_maximize",
    title: "Window: almost maximize",
    detail: "92% of the display, centred — large without hiding what is behind it.",
    keywords: ["snap", "nearly", "big"],
  },
  {
    verb: "reasonable_size",
    title: "Window: reasonable size",
    detail: "Two-thirds of the display, centred. The size to put a window back to after it has been dragged somewhere odd.",
    keywords: ["snap", "tidy", "restore", "default"],
  },
  {
    verb: "center",
    title: "Window: center",
    detail: "Centres the window without changing its size — a move, not a resize.",
    keywords: ["centre", "middle"],
  },
  {
    verb: "larger",
    title: "Window: make bigger",
    detail: "Grows the window by 10% around its own centre, stopping at the edges of the display.",
    keywords: ["grow", "bigger", "increase", "resize"],
  },
  {
    verb: "smaller",
    title: "Window: make smaller",
    detail: "Shrinks the window by 10% around its own centre, stopping before it becomes too small to use.",
    keywords: ["shrink", "smaller", "decrease", "resize"],
  },
  {
    verb: "next_display",
    title: "Window: move to next display",
    detail:
      "Moves the window to the next display, keeping where it sat proportionally — so a window in the top-right corner arrives in the top-right corner, whatever size the other screen is.",
    keywords: ["monitor", "screen", "display", "move"],
  },
  {
    verb: "previous_display",
    title: "Window: move to previous display",
    detail: "The same, in the other direction.",
    keywords: ["monitor", "screen", "display", "move"],
  },
  {
    verb: "toggle_full_screen",
    title: "Window: toggle full screen",
    detail: "Enters or leaves macOS full screen — the green-button behaviour, with its own Space.",
    keywords: ["fullscreen", "green button", "space"],
  },
];

const WINDOW_COMMANDS: CommandDef[] = WINDOW_SPECS.map((spec) => ({
  id: `window.${spec.verb}`,
  title: spec.title,
  detail: spec.detail,
  group: "windows" as const,
  icon: "▦",
  keywords: ["window", ...spec.keywords],
  run: ({ actions }) => windowVerb(actions, spec.verb),
}));

// ---------------------------------------------------------------------------
// Developer tools
// ---------------------------------------------------------------------------

interface ToolSpec {
  id: ToolId;
  title: string;
  detail: string;
  group: CommandGroupId;
  icon: string;
  keywords: string[];
  trigger?: string;
  argument?: string;
}

const TOOL_SPECS: ToolSpec[] = [
  // --- identifiers ---
  {
    id: "uuid",
    title: "Generate a UUID",
    detail: "A random version-4 UUID, copied as soon as it appears.",
    group: "developer",
    icon: "⁘",
    keywords: ["uuid", "guid", "identifier", "random", "id"],
  },
  {
    id: "uuid_batch",
    title: "Generate ten UUIDs",
    detail: "Ten version-4 UUIDs, one per line, for seeding fixtures.",
    group: "developer",
    icon: "⁘",
    keywords: ["uuid", "batch", "many", "bulk", "guid"],
  },
  {
    id: "ulid",
    title: "Generate a ULID",
    detail:
      "A ULID: a 48-bit millisecond timestamp followed by 80 random bits, in Crockford base32. Sorts by creation time, which is why you would use one instead of a UUID as a database key.",
    group: "developer",
    icon: "⁘",
    keywords: ["ulid", "sortable", "identifier", "id"],
  },
  {
    id: "nano_id",
    title: "Generate a Nano ID",
    detail: "A 21-character URL-safe identifier — shorter than a UUID and safe to put in a path.",
    group: "developer",
    icon: "⁘",
    keywords: ["nanoid", "short", "identifier", "id", "url"],
  },
  {
    id: "password",
    title: "Generate a password",
    detail:
      "24 characters from the system's cryptographic random source, guaranteed to contain every character class. Leaves out l, I, O, 0 and 1, because a password nobody can read off a screen gets written down somewhere worse.",
    group: "developer",
    icon: "⚿",
    keywords: ["password", "passphrase", "random", "secret", "generate"],
  },

  // --- encoding ---
  {
    id: "base64_encode",
    title: "Base64 encode",
    detail: "Encodes what you type to standard base64, with padding.",
    group: "developer",
    icon: "⇄",
    keywords: ["base64", "b64", "encode"],
    trigger: "base64",
    argument: "text to encode",
  },
  {
    id: "base64_decode",
    title: "Base64 decode",
    detail:
      "Decodes base64 back to text. Tolerates missing padding, embedded newlines and either alphabet, because pasted tokens rarely arrive clean.",
    group: "developer",
    icon: "⇄",
    keywords: ["base64", "b64", "decode", "unbase64"],
    trigger: "unbase64",
    argument: "base64 to decode",
  },
  {
    id: "base64_url_encode",
    title: "Base64 encode (URL-safe)",
    detail: "The URL-safe alphabet with no padding — what JWTs and data URLs use.",
    group: "developer",
    icon: "⇄",
    keywords: ["base64url", "urlsafe", "encode", "jwt"],
  },
  {
    id: "base64_url_decode",
    title: "Base64 decode (URL-safe)",
    detail: "Decodes the URL-safe alphabet back to text.",
    group: "developer",
    icon: "⇄",
    keywords: ["base64url", "urlsafe", "decode"],
  },
  {
    id: "hex_encode",
    title: "Hex encode",
    detail: "Each byte of the input as two lowercase hex digits.",
    group: "developer",
    icon: "⇄",
    keywords: ["hex", "hexadecimal", "encode", "bytes"],
    trigger: "hex",
    argument: "text to encode",
  },
  {
    id: "hex_decode",
    title: "Hex decode",
    detail: "Hex back to text. Accepts `0x` prefixes and colon- or dash-separated bytes.",
    group: "developer",
    icon: "⇄",
    keywords: ["hex", "hexadecimal", "decode", "unhex"],
    trigger: "unhex",
    argument: "hex to decode",
  },
  {
    id: "url_encode",
    title: "URL encode",
    detail: "Percent-encodes everything outside the unreserved set, so the result is safe anywhere in a URL.",
    group: "developer",
    icon: "⇄",
    keywords: ["url", "percent", "encode", "escape", "query"],
    trigger: "urlencode",
    argument: "text to encode",
  },
  {
    id: "url_decode",
    title: "URL decode",
    detail: "Reverses percent-encoding, and reads `+` as a space the way a query string means it.",
    group: "developer",
    icon: "⇄",
    keywords: ["url", "percent", "decode", "unescape"],
    trigger: "urldecode",
    argument: "text to decode",
  },
  {
    id: "html_encode",
    title: "HTML escape",
    detail: "Escapes the five characters that change the meaning of markup.",
    group: "developer",
    icon: "⇄",
    keywords: ["html", "escape", "entities", "xml"],
    trigger: "htmlescape",
    argument: "text to escape",
  },
  {
    id: "html_decode",
    title: "HTML unescape",
    detail:
      "Turns entities back into characters, resolving `&amp;` last so `&amp;lt;` comes back as `&lt;` rather than `<`.",
    group: "developer",
    icon: "⇄",
    keywords: ["html", "unescape", "entities", "decode"],
  },

  // --- inspection ---
  {
    id: "jwt_decode",
    title: "Decode a JWT",
    detail:
      "Shows a token's header and payload, formatted, and says whether it has expired. The signature is *not* verified — that needs the key, and a tool that showed a tick without one would be worse than none. Nothing is sent anywhere.",
    group: "developer",
    icon: "⚿",
    keywords: ["jwt", "token", "bearer", "claims", "decode", "auth"],
    trigger: "jwt",
    argument: "token",
  },
  {
    id: "json_format",
    title: "Format JSON",
    detail: "Pretty-prints JSON with two-space indentation, and names the parse error if it is not valid.",
    group: "developer",
    icon: "{}",
    keywords: ["json", "pretty", "format", "indent", "beautify"],
    trigger: "json",
    argument: "JSON to format",
  },
  {
    id: "json_minify",
    title: "Minify JSON",
    detail: "Strips every byte of whitespace and reports how many that saved.",
    group: "developer",
    icon: "{}",
    keywords: ["json", "minify", "compact", "compress"],
  },
  {
    id: "json_escape",
    title: "Escape as a JSON string",
    detail: "Wraps the input in quotes and escapes it, ready to paste into a JSON document.",
    group: "developer",
    icon: "{}",
    keywords: ["json", "escape", "string", "quote"],
  },

  // --- time ---
  {
    id: "timestamp_now",
    title: "Current timestamp",
    detail: "The Unix time now, in seconds and milliseconds, with the ISO 8601 form alongside.",
    group: "developer",
    icon: "◷",
    keywords: ["timestamp", "epoch", "unix", "now", "time"],
  },
  {
    id: "timestamp_convert",
    title: "Convert a timestamp",
    detail:
      "Reads an epoch number or a date and shows the other. Ten digits are treated as seconds and thirteen as milliseconds, which stays unambiguous until the year 2286.",
    group: "developer",
    icon: "◷",
    keywords: ["timestamp", "epoch", "unix", "date", "convert"],
    trigger: "epoch",
    argument: "epoch number or date",
  },

  // --- numbers and colour ---
  {
    id: "color_convert",
    title: "Convert a colour",
    detail:
      "Reads a colour in any notation people paste and shows hex, RGB and HSL — plus the WCAG contrast against white and black, so you know which one the text can be.",
    group: "developer",
    icon: "◧",
    keywords: ["colour", "color", "hex", "rgb", "hsl", "contrast", "wcag"],
    trigger: "color",
    argument: "#3b82f6 or rgb(59, 130, 246)",
  },
  {
    id: "number_base",
    title: "Convert number bases",
    detail: "Shows one number in decimal, hex, octal and binary at once. Reads `0x`, `0b` and `0o` prefixes.",
    group: "developer",
    icon: "◑",
    keywords: ["binary", "hex", "octal", "decimal", "base", "radix", "convert"],
    trigger: "base",
    argument: "number, optionally 0x / 0b / 0o",
  },
  {
    id: "random_number",
    title: "Random number",
    detail: "A number in a range you give — `5-10`, `5 10` or a bare `100` meaning 1 to 100.",
    group: "developer",
    icon: "⚂",
    keywords: ["random", "dice", "roll", "number", "pick"],
    trigger: "random",
    argument: "range, e.g. 1-100",
  },

  // --- hashes ---
  {
    id: "sha256",
    title: "SHA-256 hash",
    detail: "Hashes the input with SHA-256 and copies the digest.",
    group: "developer",
    icon: "#",
    keywords: ["hash", "sha256", "sha", "digest", "checksum"],
    trigger: "sha256",
    argument: "text to hash",
  },
  {
    id: "sha512",
    title: "SHA-512 hash",
    detail: "Hashes the input with SHA-512.",
    group: "developer",
    icon: "#",
    keywords: ["hash", "sha512", "sha", "digest", "checksum"],
    trigger: "sha512",
    argument: "text to hash",
  },
  {
    id: "sha1",
    title: "SHA-1 hash",
    detail: "Hashes the input with SHA-1. Broken for signatures; still what git object ids use.",
    group: "developer",
    icon: "#",
    keywords: ["hash", "sha1", "digest", "checksum", "git"],
    trigger: "sha1",
    argument: "text to hash",
  },
  {
    id: "md5",
    title: "MD5 hash",
    detail: "Hashes the input with MD5. Fine for checking a download, never for a password.",
    group: "developer",
    icon: "#",
    keywords: ["hash", "md5", "digest", "checksum"],
    trigger: "md5",
    argument: "text to hash",
  },

  // --- text ---
  {
    id: "lorem",
    title: "Lorem ipsum",
    detail: "Three sentences of placeholder text.",
    group: "text",
    icon: "¶",
    keywords: ["lorem", "ipsum", "placeholder", "dummy", "filler"],
  },
  {
    id: "slugify",
    title: "Slugify",
    detail: "Lowercases and dashes the input into something safe for a URL, keeping accented letters intact.",
    group: "text",
    icon: "¶",
    keywords: ["slug", "url", "kebab", "permalink"],
    trigger: "slug",
    argument: "text to slugify",
  },
  {
    id: "text_stats",
    title: "Count words and characters",
    detail: "Words, characters, lines, paragraphs, sentences and an estimated reading time.",
    group: "text",
    icon: "¶",
    keywords: ["count", "words", "characters", "length", "statistics", "reading time"],
    trigger: "count",
    argument: "text to measure",
  },
  {
    id: "sort_lines",
    title: "Sort lines A→Z",
    detail: "Sorts the lines you paste, ignoring case.",
    group: "text",
    icon: "↓",
    keywords: ["sort", "alphabetical", "order", "lines"],
    trigger: "sort",
    argument: "lines to sort",
  },
  {
    id: "sort_lines_descending",
    title: "Sort lines Z→A",
    detail: "The same, reversed.",
    group: "text",
    icon: "↑",
    keywords: ["sort", "reverse", "descending", "lines"],
  },
  {
    id: "dedupe_lines",
    title: "Remove duplicate lines",
    detail: "Keeps the first occurrence of each line and says how many went.",
    group: "text",
    icon: "≠",
    keywords: ["dedupe", "duplicate", "unique", "distinct", "lines"],
    trigger: "dedupe",
    argument: "lines to deduplicate",
  },
  {
    id: "reverse_lines",
    title: "Reverse line order",
    detail: "Last line first.",
    group: "text",
    icon: "⇅",
    keywords: ["reverse", "flip", "order", "lines"],
  },
  {
    id: "shuffle_lines",
    title: "Shuffle lines",
    detail: "Randomises line order with a Fisher-Yates shuffle over the system random source.",
    group: "text",
    icon: "⤨",
    keywords: ["shuffle", "random", "randomise", "lines", "order"],
  },
  {
    id: "number_lines",
    title: "Number lines",
    detail: "Prefixes each line with `1.`, `2.` and so on.",
    group: "text",
    icon: "№",
    keywords: ["number", "enumerate", "list", "lines"],
  },
  {
    id: "join_lines",
    title: "Join lines",
    detail: "Collapses lines into one comma-separated line, dropping blanks.",
    group: "text",
    icon: "⇥",
    keywords: ["join", "merge", "comma", "single line", "flatten"],
  },
  {
    id: "trim_lines",
    title: "Trim whitespace",
    detail: "Removes leading and trailing whitespace from every line.",
    group: "text",
    icon: "⌫",
    keywords: ["trim", "strip", "whitespace", "clean"],
  },
  {
    id: "count_occurrences",
    title: "Count repeated lines",
    detail: "How often each line appears, most frequent first — a `sort | uniq -c` you do not have to remember.",
    group: "text",
    icon: "∑",
    keywords: ["count", "frequency", "uniq", "tally", "occurrences"],
  },
];

const TOOL_COMMANDS: CommandDef[] = TOOL_SPECS.map((spec) => ({
  id: `tool.${spec.id}`,
  title: spec.title,
  detail: spec.detail,
  group: spec.group,
  icon: spec.icon,
  keywords: spec.keywords,
  trigger: spec.trigger,
  argument: spec.argument,
  run: ({ input, actions }) => tool(actions, spec.id, input),
}));

// ---------------------------------------------------------------------------
// Case transforms
// ---------------------------------------------------------------------------

const CASES: { key: string; title: string; detail: string; keywords: string[] }[] = [
  { key: "upper", title: "UPPER CASE", detail: "Everything uppercased.", keywords: ["uppercase", "caps", "shout"] },
  { key: "lower", title: "lower case", detail: "Everything lowercased.", keywords: ["lowercase", "downcase"] },
  {
    key: "title",
    title: "Title Case",
    detail: "Each word capitalised, punctuation and spacing untouched.",
    keywords: ["titlecase", "headline", "capitalise"],
  },
  {
    key: "sentence",
    title: "Sentence case",
    detail: "Capitalises the first letter after every sentence terminator.",
    keywords: ["sentencecase", "capitalise"],
  },
  { key: "snake", title: "snake_case", detail: "Words joined with underscores.", keywords: ["snake", "underscore"] },
  { key: "kebab", title: "kebab-case", detail: "Words joined with dashes.", keywords: ["kebab", "dash", "hyphen"] },
  {
    key: "camel",
    title: "camelCase",
    detail: "First word lowercase, the rest capitalised and joined.",
    keywords: ["camel", "javascript"],
  },
  {
    key: "pascal",
    title: "PascalCase",
    detail: "Every word capitalised and joined.",
    keywords: ["pascal", "upper camel", "class name"],
  },
];

const CASE_COMMANDS: CommandDef[] = CASES.map((entry) => ({
  id: `case.${entry.key}`,
  title: `Change case: ${entry.title}`,
  detail: `${entry.detail} Handles camelCase humps, so an identifier converts the way you meant.`,
  group: "text" as const,
  icon: "Aa",
  keywords: ["case", "change case", "convert", ...entry.keywords],
  trigger: entry.key,
  argument: "text to convert",
  async run({ input, actions }) {
    const text = input.trim() || (await readClipboard());
    if (!text) {
      actions.notify("Type some text after the command, or copy some first.", "error");
      return false;
    }
    try {
      const converted = await api.changeCase(text, entry.key);
      const copied = await copyText(converted);
      actions.showOutput({
        title: entry.title,
        text: converted,
        message: copied ? "Copied" : "Could not copy",
      });
      return false;
    } catch (error) {
      actions.notify(api.errorMessage(error), "error");
      return false;
    }
  },
}));

/** The clipboard, or an empty string if the webview is not allowed to read it. */
async function readClipboard(): Promise<string> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    return "";
  }
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

interface SystemSpec {
  action: SystemAction;
  title: string;
  detail: string;
  icon: string;
  keywords: string[];
  group?: CommandGroupId;
  confirm?: string;
  /** Close the palette on success — for actions with no message worth reading. */
  close?: boolean;
}

const SYSTEM_SPECS: SystemSpec[] = [
  {
    action: "toggle_dark_mode",
    title: "Toggle dark mode",
    detail: "Switches macOS between light and dark appearance. Needs permission to control System Events, which macOS asks for the first time.",
    icon: "◐",
    keywords: ["dark", "light", "appearance", "theme", "mode"],
  },
  {
    action: "toggle_stage_manager",
    title: "Toggle Stage Manager",
    detail: "Turns Stage Manager on or off and restarts the window manager so the change takes effect immediately.",
    icon: "▤",
    keywords: ["stage", "manager", "windows"],
  },
  {
    action: "toggle_hidden_files",
    title: "Toggle hidden files in Finder",
    detail: "Shows or hides dotfiles, then restarts Finder.",
    icon: "◌",
    keywords: ["hidden", "dotfiles", "finder", "show", "invisible"],
  },
  {
    action: "toggle_desktop_icons",
    title: "Toggle desktop icons",
    detail: "Hides everything on the desktop, or brings it back. Nothing is moved or deleted — Finder just stops drawing it.",
    icon: "▢",
    keywords: ["desktop", "icons", "clean", "hide", "screen share"],
  },
  {
    action: "restart_finder",
    title: "Restart Finder",
    detail: "Relaunches Finder. The usual fix when a mounted volume will not go away.",
    icon: "↻",
    keywords: ["finder", "restart", "relaunch", "kill"],
  },
  {
    action: "restart_dock",
    title: "Restart the Dock",
    detail: "Relaunches the Dock, which also resets Mission Control and Stage Manager.",
    icon: "↻",
    keywords: ["dock", "restart", "relaunch", "mission control"],
  },
  {
    action: "restart_menu_bar",
    title: "Restart the menu bar",
    detail: "Relaunches SystemUIServer, which redraws the menu bar extras when one gets stuck.",
    icon: "↻",
    keywords: ["menu bar", "systemuiserver", "restart", "status"],
  },
  {
    action: "empty_trash",
    title: "Empty the Trash",
    detail: "Empties the Trash through Finder, so the usual warnings about locked items still apply.",
    icon: "⌧",
    keywords: ["trash", "empty", "bin", "delete"],
    group: "files",
    confirm: "Empty the Trash permanently?",
  },
  {
    action: "lock_screen",
    title: "Lock the screen",
    detail: "Locks immediately, the same as the Apple menu's Lock Screen — regardless of the password delay set for sleep.",
    icon: "⚿",
    keywords: ["lock", "screen", "secure", "away"],
    close: true,
  },
  {
    action: "sleep_display",
    title: "Turn the display off",
    detail: "Puts the display to sleep without sleeping the machine, so anything running keeps running.",
    icon: "◑",
    keywords: ["display", "screen", "sleep", "off", "monitor"],
    close: true,
  },
  {
    action: "sleep_computer",
    title: "Sleep",
    detail: "Puts the Mac to sleep.",
    icon: "☾",
    keywords: ["sleep", "suspend", "standby"],
    close: true,
  },
  {
    action: "start_screen_saver",
    title: "Start the screen saver",
    detail: "Starts the screen saver straight away.",
    icon: "◇",
    keywords: ["screensaver", "screen saver", "idle"],
    close: true,
  },
  {
    action: "log_out",
    title: "Log out",
    detail: "Ends the session. Applications are asked to save first, as they would be from the Apple menu.",
    icon: "⏻",
    keywords: ["logout", "log out", "sign out", "session"],
    confirm: "Log out of macOS?",
    close: true,
  },
  {
    action: "restart_computer",
    title: "Restart",
    detail: "Restarts the Mac.",
    icon: "⏻",
    keywords: ["restart", "reboot"],
    confirm: "Restart this Mac?",
    close: true,
  },
  {
    action: "shut_down",
    title: "Shut down",
    detail: "Shuts the Mac down.",
    icon: "⏻",
    keywords: ["shutdown", "shut down", "power off", "turn off"],
    confirm: "Shut this Mac down?",
    close: true,
  },
  {
    action: "volume_up",
    title: "Volume up",
    detail: "Raises the output volume by 10% and unmutes, the way the hardware key does.",
    icon: "▲",
    keywords: ["volume", "louder", "up", "sound"],
    group: "sound",
  },
  {
    action: "volume_down",
    title: "Volume down",
    detail: "Lowers the output volume by 10%.",
    icon: "▼",
    keywords: ["volume", "quieter", "down", "sound"],
    group: "sound",
  },
  {
    action: "toggle_mute",
    title: "Mute / unmute",
    detail: "Toggles output mute and says which state it landed in.",
    icon: "⊘",
    keywords: ["mute", "silence", "unmute", "sound"],
    group: "sound",
  },
  {
    action: "brightness_up",
    title: "Brightness up",
    detail:
      "Raises display brightness by synthesising the hardware key, which is the only route Apple has not broken. Needs the Accessibility permission.",
    icon: "☀",
    keywords: ["brightness", "brighter", "display", "screen"],
  },
  {
    action: "brightness_down",
    title: "Brightness down",
    detail: "Lowers display brightness. Needs the Accessibility permission.",
    icon: "☁",
    keywords: ["brightness", "dimmer", "darker", "display"],
  },
  {
    action: "toggle_wifi",
    title: "Toggle Wi-Fi",
    detail: "Turns Wi-Fi on or off on whichever interface is actually the Wi-Fi one — not a hardcoded en0.",
    icon: "≋",
    keywords: ["wifi", "wi-fi", "wireless", "airport", "network"],
    group: "network",
  },
];

const SYSTEM_COMMANDS: CommandDef[] = SYSTEM_SPECS.map((spec) => ({
  id: `system.${spec.action}`,
  title: spec.title,
  detail: spec.detail,
  group: spec.group ?? "system",
  icon: spec.icon,
  keywords: spec.keywords,
  confirm: spec.confirm,
  run: ({ actions }) => system(actions, spec.action, spec.title, spec.close ?? false),
}));

// ---------------------------------------------------------------------------
// Everything else
// ---------------------------------------------------------------------------

const OTHER_COMMANDS: CommandDef[] = [
  // --- screen and text ---
  {
    id: "screen.ocr",
    title: "Copy text from the screen",
    detail:
      "Drag a box over anything on screen — a screenshot, a video still, an error dialog — and the text inside it is recognised and copied. Recognition runs on-device through Apple's Vision framework, and the capture is deleted as soon as it has been read.",
    group: "screen",
    icon: "⌗",
    keywords: ["ocr", "text", "screen", "recognise", "read", "capture", "scan", "grab"],
    run: ({ actions }) => outcome(actions, "Recognised text", () => api.ocrScreen()),
  },
  {
    id: "screen.ocr-selection",
    title: "Read text from the selected image",
    detail: "The same recognition, run over the image files selected in Finder.",
    group: "screen",
    icon: "⌗",
    keywords: ["ocr", "image", "finder", "selection", "read", "text"],
    async run({ actions }) {
      try {
        const outcomeResult = await api.copyFinderPath();
        if (!outcomeResult.ok || !outcomeResult.copied) {
          actions.notify("Select an image in Finder first.", "error");
          return false;
        }
        const first = outcomeResult.copied.split("\n")[0].trim();
        return await outcome(actions, "Recognised text", () => api.ocrImage(first));
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
        return false;
      }
    },
  },

  // --- sound: media ---
  {
    id: "media.play_pause",
    title: "Play / pause",
    detail: "Plays or pauses whichever of Music or Spotify is actually running, preferring the one already playing.",
    group: "sound",
    icon: "⏯",
    keywords: ["play", "pause", "music", "spotify", "media"],
    run: ({ actions }) => media(actions, "play_pause", "Playback"),
  },
  {
    id: "media.next",
    title: "Next track",
    detail: "Skips forward and shows what started.",
    group: "sound",
    icon: "⏭",
    keywords: ["next", "skip", "track", "music", "spotify"],
    run: ({ actions }) => media(actions, "next", "Next track"),
  },
  {
    id: "media.previous",
    title: "Previous track",
    detail: "Goes back a track.",
    group: "sound",
    icon: "⏮",
    keywords: ["previous", "back", "track", "music", "spotify"],
    run: ({ actions }) => media(actions, "previous", "Previous track"),
  },
  {
    id: "media.now_playing",
    title: "What is playing",
    detail: "Copies the current track and artist.",
    group: "sound",
    icon: "♪",
    keywords: ["now playing", "current", "track", "song", "music", "spotify"],
    run: ({ actions }) => media(actions, "now_playing", "Now playing"),
  },

  // --- files ---
  {
    id: "files.compress",
    title: "Compress the Finder selection",
    detail:
      "Zips what is selected in Finder into an archive beside it, using the same tool Finder's own Compress uses — so resource forks and extended attributes survive. Never overwrites an existing archive.",
    group: "files",
    icon: "⊞",
    keywords: ["zip", "compress", "archive", "finder"],
    run: ({ actions }) => outcome(actions, "Archive", () => api.compressSelection()),
  },
  {
    id: "files.expand",
    title: "Expand the selected archive",
    detail: "Unpacks the selected archives into folders beside them.",
    group: "files",
    icon: "⊟",
    keywords: ["unzip", "expand", "extract", "archive", "decompress"],
    run: ({ actions }) => outcome(actions, "Expanded", () => api.expandSelection()),
  },
  {
    id: "files.trash",
    title: "Move the Finder selection to the Trash",
    detail: "Deletes through Finder, so it lands in the Trash and Cmd-Z puts it back.",
    group: "files",
    icon: "⌧",
    keywords: ["trash", "delete", "remove", "finder", "bin"],
    run: ({ actions }) => outcome(actions, "Trashed", () => api.trashSelection()),
  },
  {
    id: "files.quicklook",
    title: "Quick Look the Finder selection",
    detail: "Opens the macOS preview panel for what is selected, without switching to Finder.",
    group: "files",
    icon: "◉",
    keywords: ["quick look", "preview", "peek", "finder", "space"],
    run: ({ actions }) => outcome(actions, "Quick Look", () => api.quickLookSelection()),
  },
  {
    id: "files.terminal",
    title: "Open the Finder selection in Terminal",
    detail: "Opens Terminal at the selected folder, or at the folder containing the selected file.",
    group: "files",
    icon: "▸",
    keywords: ["terminal", "shell", "cd", "finder", "here"],
    run: ({ actions }) => outcome(actions, "Terminal", () => api.openSelectionInTerminal()),
  },
  {
    id: "files.copy-path",
    title: "Copy the path of the Finder selection",
    detail: "Copies the POSIX path of everything selected, one per line.",
    group: "files",
    icon: "⁄",
    keywords: ["path", "copy", "finder", "posix", "location"],
    run: ({ actions }) => outcome(actions, "Path", () => api.copyFinderPath()),
  },
  {
    id: "files.latest-download-open",
    title: "Open the latest download",
    detail: "Opens the newest file in ~/Downloads, skipping the part-files browsers leave behind mid-download.",
    group: "files",
    icon: "↓",
    keywords: ["download", "downloads", "latest", "recent", "open"],
    run: ({ actions }) => outcome(actions, "Download", () => api.openLatestDownload()),
  },
  {
    id: "files.latest-download-copy",
    title: "Copy the path of the latest download",
    detail: "The same file, as a path on the clipboard.",
    group: "files",
    icon: "↓",
    keywords: ["download", "downloads", "latest", "path", "copy"],
    run: ({ actions }) => outcome(actions, "Download path", () => api.copyLatestDownload()),
  },

  // --- utilities ---
  {
    id: "utility.eject",
    title: "Eject all disks",
    detail: "Ejects every removable volume under /Volumes, and says which ones refused.",
    group: "utilities",
    icon: "⏏",
    keywords: ["eject", "unmount", "disk", "drive", "usb", "volume"],
    run: ({ actions }) => outcome(actions, "Eject", () => api.ejectDisks()),
  },
  {
    id: "utility.caffeinate-on",
    title: "Keep this Mac awake",
    detail:
      "Blocks sleep and display dimming until you turn it off. Tied to Caduceus's own process, so quitting the app releases it — a stray assertion outliving the app that made it is how laptops cook in bags.",
    group: "utilities",
    icon: "☀",
    keywords: ["awake", "caffeine", "caffeinate", "sleep", "insomnia", "presentation"],
    run: ({ actions }) => outcome(actions, "Stay awake", () => api.stayAwake(true)),
  },
  {
    id: "utility.caffeinate-off",
    title: "Allow this Mac to sleep",
    detail: "Releases the keep-awake assertion.",
    group: "utilities",
    icon: "☾",
    keywords: ["awake", "caffeine", "sleep", "allow", "release"],
    run: ({ actions }) => outcome(actions, "Sleep", () => api.stayAwake(false)),
  },
  {
    id: "utility.define",
    title: "Define a word",
    detail: "Looks a word up in the Dictionary app that ships with macOS. No network involved.",
    group: "utilities",
    icon: "≡",
    keywords: ["define", "definition", "dictionary", "meaning", "word", "spell"],
    trigger: "define",
    argument: "word",
    run: ({ input, actions }) => outcome(actions, "Definition", () => api.defineWord(input)),
  },
  {
    id: "utility.machine",
    title: "About this Mac",
    detail: "Model, chip, cores, memory, macOS version, battery and uptime, on one screen and ready to paste into a bug report.",
    group: "utilities",
    icon: "◍",
    keywords: ["about", "mac", "system", "spec", "hardware", "model", "memory", "uptime", "battery"],
    run: ({ actions }) => outcome(actions, "This Mac", () => api.machineSummary()),
  },
  {
    id: "utility.permissions",
    title: "Check Caduceus's permissions",
    detail:
      "Shows which of Accessibility and Screen Recording Caduceus currently holds, and whether the native helper is installed. Reads the state without prompting.",
    group: "utilities",
    icon: "⚿",
    keywords: ["permission", "permissions", "accessibility", "screen recording", "privacy", "tcc", "grant"],
    async run({ actions }) {
      try {
        const report = await api.systemPermissions();
        const line = (label: string, granted: boolean) =>
          `${granted ? "granted" : "not granted"}`.padEnd(12) + label;
        actions.showOutput({
          title: "Permissions",
          text: [
            line("Accessibility — window management, brightness", report.accessibility),
            line("Screen Recording — screenshots, screen OCR", report.screenRecording),
            line("Native helper — OCR and audio switching", report.nativeHelper),
          ].join("\n"),
          message: report.accessibility
            ? "Grant anything missing in System Settings → Privacy & Security."
            : "Window management needs Accessibility: System Settings → Privacy & Security → Accessibility.",
        });
        return false;
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
        return false;
      }
    },
  },

  // --- network ---
  {
    id: "network.local",
    title: "Local IP addresses",
    detail: "Every interface that has an address, labelled the way Network settings labels it, plus the default router.",
    group: "network",
    icon: "◈",
    keywords: ["ip", "address", "local", "lan", "interface", "network"],
    run: ({ actions }) => outcome(actions, "Local addresses", () => api.networkSummary()),
  },
  {
    id: "network.public",
    title: "Public IP address",
    detail:
      "This machine's address as the internet sees it. The one command in Caduceus that deliberately leaves the machine — it runs only when you pick this row.",
    group: "network",
    icon: "◇",
    keywords: ["ip", "public", "external", "wan", "internet", "address"],
    run: ({ actions }) => outcome(actions, "Public address", () => api.publicAddress()),
  },
  {
    id: "network.wifi",
    title: "Wi-Fi details",
    detail: "The network you are on, the address it gave you, the router and the interface name.",
    group: "network",
    icon: "≋",
    keywords: ["wifi", "wi-fi", "network", "ssid", "wireless", "connection"],
    run: ({ actions }) => outcome(actions, "Wi-Fi", () => api.wifiSummary()),
  },
  {
    id: "network.dns",
    title: "Look up a hostname",
    detail: "Resolves a hostname to its addresses using the system resolver.",
    group: "network",
    icon: "◎",
    keywords: ["dns", "lookup", "resolve", "dig", "nslookup", "host"],
    trigger: "dns",
    argument: "hostname",
    run: ({ input, actions }) => outcome(actions, "DNS", () => api.dnsLookup(input)),
  },
  {
    id: "network.ping",
    title: "Ping a host",
    detail: "Five packets, summarised as loss and round-trip time.",
    group: "network",
    icon: "◌",
    keywords: ["ping", "latency", "reachable", "rtt", "network"],
    trigger: "ping",
    argument: "hostname or address",
    run: ({ input, actions }) => outcome(actions, "Ping", () => api.pingHost(input)),
  },
];


// ---------------------------------------------------------------------------
// Live lists
// ---------------------------------------------------------------------------

/**
 * Entry points for the sources that have to ask the system what exists.
 *
 * Those lists live in `liveListProvider` and are gated behind a leading keyword
 * so their cost — an `lsof`, a directory scan, a `docker ps` — is only paid when
 * asked for. That gating makes them undiscoverable on their own: nobody guesses
 * that "port" is a word the palette knows. These rows are how you find out.
 * Choosing one types the keyword for you rather than running anything.
 */
interface ListSpec {
  id: string;
  title: string;
  detail: string;
  group: CommandGroupId;
  icon: string;
  keywords: string[];
  /** What gets typed into the input when this row is chosen. */
  prefill: string;
}

const LIST_SPECS: ListSpec[] = [
  {
    id: "ports",
    title: "Listening ports",
    detail:
      "Every process holding a TCP port open, with its pid. Choosing one stops it with SIGTERM, so a dev server flushes its logs and removes its socket rather than being killed outright. Type a number after the keyword to narrow to one port.",
    group: "devenv",
    icon: "◈",
    keywords: ["port", "ports", "listening", "lsof", "3000", "8080", "kill", "free", "eaddrinuse"],
    prefill: "port ",
  },
  {
    id: "repos",
    title: "Git repositories",
    detail:
      "Repositories under the usual project folders — Developer, Projects, Code, src, dev, repos and Documents/GitHub — with the branch each one is on. Choosing one opens it in Terminal. Only two levels deep, so it stays instant.",
    group: "devenv",
    icon: "⑂",
    keywords: ["repo", "repos", "git", "project", "projects", "branch", "checkout"],
    prefill: "repo ",
  },
  {
    id: "ssh",
    title: "SSH hosts",
    detail:
      "The hosts defined in ~/.ssh/config, with their hostname and user. Choosing one opens Terminal and connects. Wildcard patterns are skipped, because they are rules rather than machines you can reach.",
    group: "devenv",
    icon: "⌁",
    keywords: ["ssh", "host", "hosts", "server", "remote", "connect"],
    prefill: "ssh ",
  },
  {
    id: "docker",
    title: "Docker containers",
    detail:
      "Every container, running or not, with its image and status. Choosing one starts or stops it. Says plainly whether Docker is missing or simply not running.",
    group: "devenv",
    icon: "◉",
    keywords: ["docker", "container", "containers", "compose", "image"],
    prefill: "docker ",
  },
  {
    id: "output",
    title: "Change the sound output",
    detail:
      "Every connected output device, with the current one marked. Switching is immediate and system-wide. Devices are tracked by their CoreAudio UID, which survives a reboot — unlike the numeric id macOS reassigns freely.",
    group: "sound",
    icon: "◐",
    keywords: ["output", "speaker", "speakers", "headphones", "audio", "sound", "device", "airpods"],
    prefill: "output ",
  },
  {
    id: "input",
    title: "Change the microphone",
    detail: "Every connected input device, with the current one marked.",
    group: "sound",
    icon: "◍",
    keywords: ["input", "mic", "microphone", "audio", "device", "recording"],
    prefill: "input ",
  },
  {
    id: "files",
    title: "Find a file",
    detail:
      "Searches file names through Spotlight, so it answers from an index that already exists rather than walking the disk. Choosing a result reveals it in Finder.",
    group: "files",
    icon: "▤",
    keywords: ["file", "files", "find", "search", "spotlight", "mdfind", "locate"],
    prefill: "file ",
  },
  {
    id: "big",
    title: "Largest files",
    detail:
      "The biggest files in your home folder, over 100 MB, newest measurement first. Also answered from the Spotlight index, so it returns immediately instead of spinning the disk. Choosing one reveals it in Finder.",
    group: "files",
    icon: "▣",
    keywords: ["large", "big", "biggest", "space", "disk", "storage", "full", "cleanup"],
    prefill: "large",
  },
];

const LIST_COMMANDS: CommandDef[] = LIST_SPECS.map((spec) => ({
  id: `list.${spec.id}`,
  title: spec.title,
  detail: spec.detail,
  group: spec.group,
  icon: spec.icon,
  keywords: spec.keywords,
  run({ actions }) {
    actions.setInput(spec.prefill);
    return false;
  },
}));

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

export const COMMANDS: CommandDef[] = [
  ...WINDOW_COMMANDS,
  ...TOOL_COMMANDS,
  ...CASE_COMMANDS,
  ...SYSTEM_COMMANDS,
  ...LIST_COMMANDS,
  ...OTHER_COMMANDS,
];

/** Commands in a group, for the catalogue. */
export function commandsInGroup(group: CommandGroupId): CommandDef[] {
  return COMMANDS.filter((command) => command.group === group);
}

/**
 * Split a query into a command and its argument.
 *
 * `sha256 hello world` is the hash command with "hello world"; `sha` on its own
 * is a search that should still surface it. Returns `null` when the first word
 * is not a trigger, which is the common case.
 */
export function matchTrigger(query: string): { command: CommandDef; input: string } | null {
  const trimmed = query.trim();
  if (!trimmed) return null;

  const space = trimmed.search(/\s/);
  const head = (space === -1 ? trimmed : trimmed.slice(0, space)).toLowerCase();
  const rest = space === -1 ? "" : trimmed.slice(space + 1).trim();

  const command = COMMANDS.find((entry) => entry.trigger === head);
  return command ? { command, input: rest } : null;
}
