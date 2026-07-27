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
import { PERMISSION_WALL } from "./permissions";
import type { Tab } from "./tabs";
import type { MediaAction, SystemAction, ToolId, WindowVerb } from "./types";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A panel of text shown in the palette, with a copy button. */
export interface CommandOutput {
  title: string;
  text: string;
  message?: string;
  /**
   * Structured rows, when the answer is a table rather than a paragraph.
   *
   * Rendered as a definition list on the command's page; `text` is still what
   * Copy puts on the clipboard, so nothing depends on the pretty version.
   */
  rows?: { label: string; value: string; swatch?: string }[];
}

// ---------------------------------------------------------------------------
// Forms
// ---------------------------------------------------------------------------

/**
 * What a command's page asks for.
 *
 * # Why commands describe their own inputs
 *
 * Every command used to get the same page: one textarea, one Run button. That
 * is right for "SHA-256 this" and wrong for almost everything else. Sorting
 * lines wants a direction and a "remove duplicates" tick. Converting a colour
 * wants a swatch you can pick from. Generating a password wants a length. Given
 * one textarea, all of those either grew a syntax you had to know, or silently
 * did the only thing they could.
 *
 * So a command declares its fields and the page builds itself. That keeps the
 * promise — *every feature has an interface made for that feature* — without a
 * hand-written React page per command, and it means adding a field is one line
 * in the registry rather than a new file.
 *
 * Commands that need something genuinely bespoke — a colour picker that samples
 * the screen, a sticky-notes board — name a {@link CommandDef.page} instead.
 */
export type Field =
  | {
      kind: "text";
      id: string;
      label: string;
      hint?: string;
      placeholder?: string;
      /** A box rather than a line, for anything that can be several lines. */
      multiline?: boolean;
      mono?: boolean;
      required?: boolean;
      default?: string;
      /**
       * Offer "choose a file" beside this field.
       *
       * On by default for multiline text: anything that operates on a block of
       * text operates just as well on a file full of it, and having to open the
       * file and copy it out first is a chore the app can simply do.
       */
      file?: boolean;
      /** Extensions the file picker should suggest, e.g. `["txt", "csv"]`. */
      fileTypes?: string[];
    }
  | {
      kind: "select";
      id: string;
      label: string;
      hint?: string;
      options: { value: string; label: string }[];
      default?: string;
    }
  | {
      kind: "number";
      id: string;
      label: string;
      hint?: string;
      min?: number;
      max?: number;
      step?: number;
      default?: string;
    }
  | { kind: "toggle"; id: string; label: string; hint?: string; default?: string }
  | { kind: "color"; id: string; label: string; hint?: string; default?: string }
  | {
      kind: "file";
      id: string;
      label: string;
      hint?: string;
      fileTypes?: string[];
      /** Hand the command the file's text, rather than its path. */
      readAs?: "text" | "path";
    };

export interface CommandForm {
  fields: Field[];
  /**
   * Re-run on every change.
   *
   * Only for commands that are pure functions of their inputs and run in this
   * process. Anything that touches the network, the disk or another app waits
   * to be asked — otherwise typing a hostname pings it once per keystroke.
   */
  live?: boolean;
  /** Label for the button. Defaults to "Run". */
  submitLabel?: string;
}

/** Values collected from a command's form, keyed by field id. */
export type FieldValues = Record<string, string>;

/** What a command can ask the palette to do. */
export interface CommandActions {
  notify(message: string, tone?: "info" | "error"): void;
  showOutput(output: CommandOutput): void;
  setInput(value: string): void;
  /** Open (or focus) a tab. */
  openTab(request: Omit<Tab, "id">): void;
  close(): void;
}

export interface CommandContext {
  /** Whatever was typed after the trigger word, trimmed. */
  input: string;
  actions: CommandActions;
  /**
   * The command's form values, keyed by field id.
   *
   * Empty when the command was run straight from the palette (`sha256 hello`),
   * which is why every command has to keep working from `input` alone. A field
   * whose id is `input` is fed from — and back into — that same string, so the
   * common single-box case needs no special handling at either end.
   */
  values: FieldValues;
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
  /**
   * Set when running with an empty argument is a meaningful thing to do.
   *
   * A command with a required argument, chosen from the list with nothing typed
   * after it, opens its own page instead of running on an empty string — see
   * `runCommand` in `providers.ts`. A handful already do something sensible
   * with nothing (`awake` on its own opens the Keep Awake page) and opt out of
   * that redirection here.
   */
  argumentOptional?: boolean;
  /** Shown before the command runs; Enter again confirms. */
  confirm?: string;
  /**
   * The fields this command's page asks for.
   *
   * Omitted means "one box", derived from {@link argument} — see
   * {@link formFor}. Commands with nothing to ask for get a page with a
   * button and their output.
   */
  form?: CommandForm;
  /**
   * A page of its own, for the handful that cannot be a form.
   *
   * Names a component in `src/command-center/pages/tools/`; see `ToolPage`.
   * Reach for this only when the interaction *is* the feature — sampling a
   * colour off the screen, arranging files on a desktop — not merely because a
   * command has several inputs.
   */
  page?: ToolPageId;
  /**
   * Roughly how many people will ever want this, 0–100.
   *
   * Drives the order of the browse list and breaks ties in search. Sticky notes
   * and "empty the Trash" are near the top; decoding a JWT is near the bottom.
   * Not a judgement about which is more useful — a judgement about how many
   * people know what a JWT is. See `commandWeight`.
   */
  reach?: number;
  run(ctx: CommandContext): Promise<CommandResult> | CommandResult;
}

/** Bespoke pages, named rather than imported so the registry stays data. */
export type ToolPageId =
  | "colors"
  | "sticky-notes"
  | "convert"
  | "processes"
  | "storage"
  | "desktop-sort"
  | "desktop-shapes"
  | "citations"
  | "meeting"
  | "screen-record";

/**
 * The form a command's page should render.
 *
 * A command with no declared form still gets a real one: a single field built
 * from its `argument`, with file upload attached, because "paste the text" and
 * "point at a file containing the text" are the same request.
 */
export function formFor(command: CommandDef): CommandForm {
  if (command.form) return command.form;
  if (!command.argument) return { fields: [] };
  return {
    fields: [
      {
        kind: "text",
        id: "input",
        label: command.argument,
        multiline: true,
        mono: true,
        required: true,
        file: true,
      },
    ],
    live: LIVE_GROUPS.has(command.group),
  };
}

/**
 * Groups whose commands re-run on every keystroke.
 *
 * Both are pure functions of their input, computed in this process — hashing,
 * case conversion, sorting lines. Everything else is a request to the network,
 * the disk or another application, and firing one of those per keystroke is how
 * you ping a host forty times because you typed its name.
 */
const LIVE_GROUPS = new Set<CommandGroupId>(["developer", "text"]);

/**
 * Reduce a command's form values to the single string its `run` expects.
 *
 * The bridge between the two ways a command can be invoked. `sha256 hello` from
 * the palette has no form at all; the same command opened as a page has an
 * `input` field. Both end up calling `run` with the same `input`.
 */
export function primaryValue(values: FieldValues): string {
  return values.input ?? "";
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

/** Run a window verb, turning a missing permission into a way to grant it. */
async function windowVerb(actions: CommandActions, verb: WindowVerb): Promise<CommandResult> {
  try {
    const result = await api.windowAction(verb);
    if (result.ok) return true;
    // `needsPermission` is the structured answer; `PERMISSION_WALL` is the
    // sentence every caller of `notify` knows how to recognise. Sending the
    // canonical one makes the routing a contract rather than a lucky substring
    // match on whatever the AX layer happened to say.
    actions.notify(
      result.needsPermission ? PERMISSION_WALL.accessibility : result.message,
      "error",
    );
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
  // The product names carry the search: someone who has used Rectangle for
  // years types "rectangle", not "snap".
  keywords: [
    "window",
    ...spec.keywords,
    "rectangle", "magnet", "moom", "spectacle", "amethyst", "yabai",
  ],
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
    keywords: ["colour", "color", "hex", "rgb", "hsl", "contrast", "wcag", "colorslurp", "sip"],
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
    keywords: ["dark", "light", "appearance", "theme", "mode", "night owl", "nightowl"],
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
    keywords: ["hidden", "dotfiles", "finder", "show", "invisible", "funter"],
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

/**
 * "45" → 45 minutes; "2h", "1h30m", "2:30" as expected. `null` for nonsense.
 *
 * Mirrors `tools::awake::parse_duration` on the Rust side, in minutes because
 * that is what the `awake_start` command takes.
 */
function parseAwakeMinutes(input: string): number | null {
  const text = input.trim().toLowerCase();
  if (!text) return null;

  // "2:30" = hours:minutes.
  const clock = text.match(/^(\d+):([0-5]?\d)$/);
  if (clock) {
    const minutes = Number(clock[1]) * 60 + Number(clock[2]);
    return minutes >= 1 && minutes <= 7 * 24 * 60 ? minutes : null;
  }

  // Unit-tagged: "2h", "45m", "1h30m", "1h30".
  if (/[hms]/.test(text)) {
    const match = text.match(/^(?:(\d+)h)?\s*(?:(\d+)m?)?$/);
    if (!match || (!match[1] && !match[2])) return null;
    const minutes = Number(match[1] ?? 0) * 60 + Number(match[2] ?? 0);
    return minutes >= 1 && minutes <= 7 * 24 * 60 ? minutes : null;
  }

  const minutes = Number.parseInt(text, 10);
  if (!Number.isFinite(minutes) || String(minutes) !== text) return null;
  return minutes >= 1 && minutes <= 7 * 24 * 60 ? minutes : null;
}

const OTHER_COMMANDS: CommandDef[] = [
  // --- screen and text ---
  {
    id: "screen.ocr",
    title: "Copy text from the screen",
    detail:
      "Drag a box over anything on screen — a screenshot, a video still, an error dialog — and the text inside it is recognised and copied. Recognition runs on-device through Apple's Vision framework, and the capture is deleted as soon as it has been read.",
    group: "screen",
    icon: "⌗",
    keywords: [
      "ocr", "text", "screen", "recognise", "read", "capture", "scan", "grab",
      "cleanshot", "textsniper", "shottr", "live text",
    ],
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
    keywords: ["eject", "unmount", "disk", "drive", "usb", "volume", "jettison", "ejectify"],
    run: ({ actions }) => outcome(actions, "Eject", () => api.ejectDisks()),
  },
  {
    id: "utility.caffeinate-on",
    title: "Keep this Mac awake",
    detail:
      "Opens the Keep Awake page: sessions that run indefinitely, for a duration, or until a time, with a live countdown — everything Amphetamine's core does, on the caffeinate macOS already ships. Or skip the page: `awake 45` and `awake 2h` start a timed session straight from here. Sessions are tied to Caduceus's process, so quitting the app always re-enables sleep.",
    group: "utilities",
    icon: "☀",
    keywords: [
      "awake", "caffeine", "caffeinate", "sleep", "insomnia", "presentation", "session",
      // What the app people are switching from is called.
      "amphetamine", "keepingyouawake", "lungo", "theine", "owly",
    ],
    trigger: "awake",
    argument: "duration (45, 2h, 1h30m) — or empty for the page",
    argumentOptional: true,
    async run({ input, actions }) {
      // `awake 45` starts a session without opening anything; a bare `awake`
      // opens the management page, which is where the options live.
      const trimmed = input.trim();
      if (trimmed) {
        const minutes = parseAwakeMinutes(trimmed);
        if (minutes === null) {
          actions.notify("Try a duration like 45, 2h or 1h30m.", "error");
          return false;
        }
        return outcome(actions, "Stay awake", () => api.awakeStart(minutes));
      }
      actions.openTab({ kind: "awake" });
      return false;
    },
  },
  {
    id: "utility.caffeinate-off",
    title: "Allow this Mac to sleep",
    detail: "Ends the running keep-awake session immediately.",
    group: "utilities",
    icon: "☾",
    keywords: ["awake", "caffeine", "sleep", "allow", "release", "amphetamine", "end session"],
    async run({ actions }) {
      const status = await api.awakeStatus();
      // Nothing to stop is not an error and it is not news either — it is
      // someone reaching for the sleep controls. Give them the sleep controls.
      if (!status.active) {
        actions.openTab({ kind: "awake" });
        return false;
      }
      return outcome(actions, "Sleep", () => api.awakeStop());
    },
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

  {
    id: "utility.clipboard",
    title: "Clipboard history",
    detail:
      "Everything you have copied — text, images, file paths — searchable and pinnable, kept in a local database that never leaves this Mac. The /v prefix filters it directly.",
    group: "utilities",
    icon: "❐",
    keywords: [
      "clipboard", "history", "copied", "paste", "pasteboard",
      "maccy", "jumpcut", "flycut", "copyclip", "pastebot",
    ],
    run({ actions }) {
      actions.openTab({ kind: "clipboard" });
      return false;
    },
  },
  {
    id: "utility.system-monitor",
    title: "System monitor",
    detail: "Live CPU, memory, disk and network, with the processes using them — and a way to end one that has stopped behaving.",
    group: "utilities",
    icon: "◔",
    keywords: [
      "system", "monitor", "cpu", "memory", "ram", "processes", "kill",
      "activity monitor", "istat", "stats", "htop", "top",
    ],
    run({ actions }) {
      actions.openTab({ kind: "system" });
      return false;
    },
  },
  {
    id: "utility.manage",
    title: "Open the Manage window",
    detail:
      "The tabbed window for everything with live state: keep-awake sessions, sound devices, listening ports, Docker containers and this Mac's details. Tabs stay open while you use another, like a browser — ⌘1–⌘5 switch, ⌘W closes.",
    group: "utilities",
    icon: "▤",
    keywords: [
      "manage", "management", "tabs", "window", "control", "panel", "dashboard",
      "awake", "sound", "ports", "docker", "amphetamine",
    ],
    trigger: "manage",
    run({ actions }) {
      actions.openTab({ kind: "awake" });
      return false;
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
    keywords: ["docker", "container", "containers", "compose", "image", "orbstack", "colima"],
    prefill: "docker ",
  },
  {
    id: "output",
    title: "Change the sound output",
    detail:
      "Every connected output device, with the current one marked. Switching is immediate and system-wide. Devices are tracked by their CoreAudio UID, which survives a reboot — unlike the numeric id macOS reassigns freely.",
    group: "sound",
    icon: "◐",
    keywords: [
      "output", "speaker", "speakers", "headphones", "audio", "sound", "device", "airpods",
      "soundsource",
    ],
    prefill: "output ",
  },
  {
    id: "input",
    title: "Change the microphone",
    detail: "Every connected input device, with the current one marked.",
    group: "sound",
    icon: "◍",
    keywords: ["input", "mic", "microphone", "audio", "device", "recording", "soundsource"],
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
// Ranking
// ---------------------------------------------------------------------------

/**
 * What Caduceus guesses you want, before it knows anything about you.
 *
 * These are the *starting* order only. Your own use overrules them — see
 * `usageBoost` in `usage.ts`, whose scale is deliberately larger than the 0–100
 * this spans, so anything you have actually run outranks anything you have not,
 * however confidently it was ranked here.
 *
 * A command with no entry falls back to its group's baseline. That keeps this
 * table to the commands worth holding an opinion about, rather than 124 guesses.
 */
const GROUP_BASELINE: Record<CommandGroupId, number> = {
  windows: 55,
  screen: 50,
  developer: 40,
  text: 38,
  system: 36,
  files: 34,
  sound: 30,
  network: 28,
  devenv: 28,
  utilities: 26,
};

const WEIGHTS: Record<string, number> = {
  // Window snapping is the reason most people install something like this, and
  // the two halves are most of the use between them.
  "window.left_half": 100,
  "window.right_half": 99,
  "window.maximize": 96,
  "window.center": 88,
  "window.next_display": 80,
  "window.top_half": 72,
  "window.bottom_half": 70,
  "window.almost_maximize": 68,
  "window.toggle_full_screen": 66,
  "window.first_two_thirds": 62,
  "window.last_two_thirds": 60,
  "window.top_left_quarter": 58,
  "window.top_right_quarter": 58,
  "window.bottom_left_quarter": 56,
  "window.bottom_right_quarter": 56,
  "window.larger": 52,
  "window.smaller": 52,
  "window.reasonable_size": 50,
  "window.first_third": 48,
  "window.last_third": 48,
  "window.center_third": 44,
  "window.previous_display": 40,

  // Reading text off the screen has no alternative that is not a chore.
  "screen.ocr": 94,
  "screen.ocr-selection": 44,

  // The everyday half of the developer toolbox.
  "tool.uuid": 82,
  "tool.json_format": 80,
  "tool.base64_decode": 76,
  "tool.base64_encode": 74,
  "tool.jwt_decode": 72,
  "tool.sha256": 66,
  "tool.password": 64,
  "tool.timestamp_convert": 60,
  "tool.url_encode": 54,
  "tool.url_decode": 54,
  "tool.color_convert": 52,
  "tool.timestamp_now": 50,
  "tool.json_minify": 46,
  "tool.number_base": 42,
  "tool.random_number": 36,
  "tool.md5": 34,
  "tool.hex_encode": 30,
  "tool.hex_decode": 30,
  "tool.sha1": 28,
  "tool.sha512": 26,
  "tool.ulid": 26,
  "tool.uuid_batch": 24,
  "tool.nano_id": 20,
  "tool.base64_url_encode": 20,
  "tool.base64_url_decode": 20,
  "tool.html_encode": 20,
  "tool.html_decode": 20,
  "tool.json_escape": 18,

  // Case conversion is the single most-reached-for text tool.
  "case.title": 68,
  "case.lower": 64,
  "case.upper": 62,
  "case.sentence": 54,
  "case.kebab": 46,
  "case.snake": 46,
  "case.camel": 42,
  "case.pascal": 40,
  "tool.text_stats": 56,
  "tool.slugify": 50,
  "tool.sort_lines": 48,
  "tool.dedupe_lines": 46,
  "tool.trim_lines": 36,
  "tool.join_lines": 32,
  "tool.count_occurrences": 28,
  "tool.number_lines": 24,
  "tool.reverse_lines": 22,
  "tool.sort_lines_descending": 22,
  "tool.lorem": 20,
  "tool.shuffle_lines": 16,

  // System: the toggles people actually toggle.
  "system.toggle_dark_mode": 78,
  "system.lock_screen": 64,
  "system.toggle_mute": 58,
  "system.sleep_display": 52,
  "system.toggle_hidden_files": 48,
  "system.volume_up": 44,
  "system.volume_down": 44,
  "system.empty_trash": 42,
  "system.restart_finder": 40,
  "system.toggle_desktop_icons": 36,
  "system.brightness_up": 32,
  "system.brightness_down": 32,
  "system.restart_dock": 30,
  "system.toggle_stage_manager": 26,
  "system.restart_menu_bar": 24,
  "system.start_screen_saver": 22,
  "system.sleep_computer": 22,
  // Bottom of the list on purpose. These sit one keystroke from "Sleep" in a
  // fuzzy list, and ranking them low is a second line of defence behind the
  // confirmation step.
  "system.log_out": 10,
  "system.restart_computer": 8,
  "system.shut_down": 6,

  // Files.
  "list.files": 74,
  "files.copy-path": 56,
  "files.quicklook": 50,
  "files.compress": 48,
  "files.latest-download-open": 46,
  "files.terminal": 44,
  "files.expand": 40,
  "files.trash": 34,
  "files.latest-download-copy": 32,
  "list.big": 30,

  // Sound and media.
  "list.output": 72,
  "media.play_pause": 56,
  "list.input": 50,
  "media.now_playing": 40,
  "media.next": 36,
  "media.previous": 28,

  // Network and environment.
  "list.ports": 70,
  "network.local": 54,
  "list.repos": 52,
  "network.wifi": 40,
  "list.ssh": 36,
  "network.public": 34,
  "network.dns": 30,
  "list.docker": 30,
  "network.ping": 28,
  "system.toggle_wifi": 38,

  // Utilities.
  "utility.define": 48,
  "utility.caffeinate-on": 44,
  "utility.manage": 40,
  "utility.clipboard": 76,
  "utility.system-monitor": 46,
  "utility.machine": 34,
  "utility.eject": 32,
  "utility.permissions": 28,
  "utility.caffeinate-off": 24,
};

/**
 * The shipped ranking weight for a command.
 *
 * # How the browse list is ordered
 *
 * By how many people would ever want the thing. Sticky notes, "free up disk
 * space" and "force quit something" are near the top; decoding a JWT, minting a
 * Nano ID and Base64 are near the bottom. That is not a judgement about which
 * is more *useful* — it is a judgement about how many people know what a JWT
 * is, and the empty palette is the first thing a new user sees.
 *
 * Two sources, plus one adjustment:
 *
 * 1. `command.reach`, set on the command itself, is taken as final — it is a
 *    direct statement about that command and nothing should second-guess it.
 * 2. Otherwise `WEIGHTS` below, or the group's baseline, **scaled down for the
 *    groups that are specialist as a whole**.
 *
 * The scaling applies to the hand-tuned weights as well as the baseline, and
 * that is the point. Those weights order the developer tools sensibly *against
 * each other*, which they still do — but they were set when the registry was
 * mostly developer tools, so "Generate a UUID" ended up above "free up disk
 * space". Scaling the whole group keeps the tuning and fixes the altitude.
 *
 * Scaled rather than subtracted, and floored, for a reason worth keeping: a
 * flat subtraction pushed the cheapest developer commands to *negative* weights
 * and therefore below "Shut down" and "Log out". Those three have to stay at
 * the bottom — that is a safety property, not a taste one, because the browse
 * list is a place people arrow through.
 */
export function commandWeight(command: CommandDef): number {
  if (command.reach !== undefined) return command.reach;
  const base = WEIGHTS[command.id] ?? GROUP_BASELINE[command.group];
  const scaled = base * SPECIALIST_SCALE[command.group];
  return Math.round(Math.max(scaled, base > 0 ? SPECIALIST_FLOOR : base));
}

/**
 * How much of its weight a group keeps.
 *
 * Only the groups that are specialist in every member. Everything else contains
 * something a person with no technical background might plausibly want, so
 * pushing the group down would bury the wrong things.
 */
const SPECIALIST_SCALE: Record<CommandGroupId, number> = {
  windows: 1,
  system: 1,
  sound: 1,
  screen: 1,
  // Encoders, hashes, JWTs, identifiers. Every one is something you have to
  // already know the name of before you can want it.
  developer: 0.55,
  text: 0.9,
  files: 1,
  network: 0.85,
  // Ports, containers, SSH hosts, git repositories.
  devenv: 0.6,
  utilities: 1,
};

/**
 * The lowest a *scaled* command may land.
 *
 * Above the session-ending commands, which sit at 6–10 and are deliberately
 * last. Nothing that merely happens to be niche should share a floor with
 * "Shut down".
 */
const SPECIALIST_FLOOR = 14;

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/**
 * The features that are a page rather than an action.
 *
 * Every one of these is something you *stay in* for a minute or an hour — a
 * board of notes, a colour you are working out, a call you are recording. They
 * have no meaningful one-shot form, so `run` opens the tab and that is the
 * whole command.
 *
 * They carry the highest `reach` values in the registry on purpose: these are
 * the things a person who has never used a launcher wants, and the browse list
 * should lead with them rather than with base64.
 */
const PAGE_COMMANDS: CommandDef[] = [
  {
    id: "page.sticky-notes",
    title: "Sticky notes",
    detail:
      "Somewhere to put four words before you lose them. Saves as you type, survives a restart, and never asks you to name a file.",
    group: "utilities",
    icon: "▤",
    keywords: [
      "note", "notes", "sticky", "scratch", "jot", "memo", "reminder", "todo", "list",
      "stickies", "postit", "post-it", "write",
    ],
    page: "sticky-notes",
    reach: 96,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.sticky-notes", title: "Sticky Notes" });
      return false;
    },
  },
  {
    id: "page.colors",
    title: "Colors",
    detail:
      "Pick a colour from anywhere on screen, or type any notation, and get every other notation, its name, its tints and shades, what goes with it, and whether text on it passes WCAG. Pull a palette out of an image too.",
    group: "utilities",
    icon: "◍",
    keywords: [
      "color", "colour", "hex", "rgb", "hsl", "cmyk", "picker", "eyedropper", "dropper",
      "palette", "contrast", "wcag", "accessibility", "swatch", "tint", "shade", "convert",
      "wheel", "harmony", "complementary", "extract",
    ],
    page: "colors",
    reach: 84,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.colors", title: "Colors" });
      return false;
    },
  },
  {
    id: "page.convert",
    title: "Convert units",
    detail:
      "Length, weight, temperature, volume, area, speed, time, data, pressure, energy and angle — all offline arithmetic on definitions. Currency too, which is the one thing here that needs the internet and says so.",
    group: "utilities",
    icon: "⇄",
    keywords: [
      "convert", "conversion", "unit", "units", "metric", "imperial", "temperature",
      "celsius", "fahrenheit", "kelvin", "km", "miles", "kg", "pounds", "currency",
      "exchange", "rate", "usd", "eur", "gbp", "money", "calculator",
    ],
    page: "convert",
    reach: 88,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.convert", title: "Convert" });
      return false;
    },
  },
  {
    id: "page.meeting",
    title: "Meeting notes",
    detail:
      "Records both sides of a call — the room through system audio, you through the microphone — transcribes on-device as it goes, and keeps your notes beside the transcript. Nothing is uploaded and no bot joins the meeting.",
    group: "utilities",
    icon: "◉",
    keywords: [
      "meeting", "notetaker", "note taker", "minutes", "transcript", "transcribe",
      "record", "call", "zoom", "teams", "google meet", "meet", "webex", "facetime",
      "slack huddle", "huddle", "interview", "lecture", "otter", "granola", "fathom",
      "fireflies", "notes",
    ],
    page: "meeting",
    reach: 90,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.meeting", title: "Meeting notes" });
      return false;
    },
  },
  {
    id: "page.screen-record",
    title: "Record the screen",
    detail:
      "Screen video with the audio your Mac is playing — the thing ⇧⌘5 cannot do without an audio driver. Your microphone goes on its own track. Needs macOS 13 and the Screen Recording permission.",
    group: "screen",
    icon: "⏺",
    keywords: [
      "record", "recording", "screen", "capture", "video", "screencast", "system audio",
      "internal audio", "loom", "cleanshot", "obs", "demo", "gif",
    ],
    page: "screen-record",
    reach: 78,
    run: ({ actions }) => {
      actions.openTab({
        kind: "tool",
        commandId: "page.screen-record",
        title: "Record the screen",
      });
      return false;
    },
  },
  {
    id: "page.storage",
    title: "Free up disk space",
    detail:
      "What is taking your disk, and getting it back. Caches, logs, build intermediates and the leftovers of apps you removed — measured, explained one by one, and moved to the Trash rather than deleted. Also uninstalls an app properly, with everything it scattered through your Library.",
    group: "files",
    icon: "◒",
    keywords: [
      "storage", "disk", "space", "clean", "cleaner", "cleanup", "junk", "cache", "caches",
      "uninstall", "uninstaller", "remove app", "delete app", "size", "full", "purge",
      "cleanmymac", "ccleaner", "appcleaner", "daisydisk", "leftovers",
    ],
    page: "storage",
    reach: 86,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.storage", title: "Storage" });
      return false;
    },
  },
  {
    id: "page.processes",
    title: "Force quit something",
    detail:
      "Every running process, sorted by what is actually costing you, refreshed as you watch. Stop sends SIGTERM so a program can save first; Force is a second, separate choice.",
    group: "system",
    icon: "◑",
    keywords: [
      "force quit", "quit", "kill", "process", "processes", "task manager", "activity monitor",
      "cpu", "memory", "ram", "hung", "frozen", "not responding", "beachball", "pid",
    ],
    page: "processes",
    reach: 82,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.processes", title: "Processes" });
      return false;
    },
  },
  {
    id: "page.desktop-sort",
    title: "Tidy a folder",
    detail:
      "Files a messy Desktop (or any folder) into subfolders by what things are, when they were changed, or how big they are. Shows you the plan first and moves nothing until you say so — and Undo puts every file back.",
    group: "files",
    icon: "▦",
    keywords: [
      "tidy", "sort", "organise", "organize", "desktop", "clean desktop", "arrange", "file",
      "folder", "downloads", "declutter", "group",
    ],
    page: "desktop-sort",
    reach: 80,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.desktop-sort", title: "Tidy a folder" });
      return false;
    },
  },
  {
    id: "page.desktop-shapes",
    title: "Arrange desktop icons into a shape",
    detail:
      "Lays the icons on your Desktop out in a circle, a heart, a spiral, an even grid or a single line, scaled to fit your screen between the menu bar and the Dock. Shows the arrangement before it happens, and Undo puts every icon back exactly where it was. Needs Automation permission for Finder, and Finder's own Sort By has to be set to None — this says so if it is not.",
    group: "files",
    icon: "◌",
    keywords: [
      "desktop", "icons", "shape", "circle", "heart", "spiral", "grid", "line", "arrange",
      "rearrange", "layout", "position", "fun", "pattern", "align",
    ],
    page: "desktop-shapes",
    reach: 34,
    run: ({ actions }) => {
      actions.openTab({
        kind: "tool",
        commandId: "page.desktop-shapes",
        title: "Desktop icon shapes",
      });
      return false;
    },
  },
  {
    id: "page.citations",
    title: "Cite this page",
    detail:
      "Reads whatever your browser has in front and writes the citation in MLA, APA, Chicago, Harvard, IEEE, Vancouver and BibTeX at once. Fills in the author and date from the page when you ask it to, and admits when it cannot rather than inventing one.",
    group: "utilities",
    icon: "❝",
    keywords: [
      "cite", "citation", "reference", "bibliography", "mla", "apa", "chicago", "harvard",
      "ieee", "vancouver", "bibtex", "source", "essay", "paper", "zotero",
    ],
    page: "citations",
    reach: 74,
    run: ({ actions }) => {
      actions.openTab({ kind: "tool", commandId: "page.citations", title: "Cite this page" });
      return false;
    },
  },
];

// ---------------------------------------------------------------------------
// Other applications
// ---------------------------------------------------------------------------

/**
 * Driving apps you already have, over AppleScript.
 *
 * These need Automation permission for the app in question, which macOS asks
 * for the first time and Caduceus explains if it is refused. Nothing here
 * launches an app that is not already running — a "next track" that opens
 * Spotify to play nothing is not what anybody meant.
 */
interface AppCommandSpec {
  id: string;
  title: string;
  detail: string;
  app: string;
  script: string;
  keywords: string[];
  reach: number;
  /** Shows what it produced rather than a toast. */
  output?: string;
}

const SPOTIFY_SPECS: AppCommandSpec[] = [
  {
    id: "spotify.play-pause",
    title: "Spotify: play or pause",
    detail: "Toggles playback without switching to Spotify.",
    app: "Spotify",
    script: "playpause",
    keywords: ["spotify", "play", "pause", "music", "resume", "stop"],
    reach: 70,
  },
  {
    id: "spotify.next",
    title: "Spotify: next track",
    detail: "Skips forward.",
    app: "Spotify",
    script: "next track",
    keywords: ["spotify", "next", "skip", "forward", "track"],
    reach: 66,
  },
  {
    id: "spotify.previous",
    title: "Spotify: previous track",
    detail: "Back one track.",
    app: "Spotify",
    script: "previous track",
    keywords: ["spotify", "previous", "back", "last", "track"],
    reach: 60,
  },
  {
    id: "spotify.now-playing",
    title: "Spotify: what is playing",
    detail: "The current track, artist and album, ready to paste.",
    app: "Spotify",
    script:
      'return (name of current track) & " — " & (artist of current track) & " · " & (album of current track)',
    keywords: ["spotify", "now playing", "current", "track", "song", "what", "artist"],
    reach: 64,
    output: "Now playing",
  },
  {
    id: "spotify.copy-link",
    title: "Spotify: copy a link to this track",
    detail: "The share URL for whatever is playing.",
    app: "Spotify",
    script:
      'set u to spotify url of current track\nreturn "https://open.spotify.com/track/" & (last text item of (my split(u, ":")))',
    keywords: ["spotify", "link", "share", "url", "copy", "track"],
    reach: 52,
    output: "Track link",
  },
  {
    id: "spotify.shuffle",
    title: "Spotify: toggle shuffle",
    detail: "Turns shuffle on or off.",
    app: "Spotify",
    script: "set shuffling to not shuffling",
    keywords: ["spotify", "shuffle", "random"],
    reach: 46,
  },
  {
    id: "spotify.repeat",
    title: "Spotify: toggle repeat",
    detail: "Turns repeat on or off.",
    app: "Spotify",
    script: "set repeating to not repeating",
    keywords: ["spotify", "repeat", "loop"],
    reach: 44,
  },
  {
    id: "spotify.volume-up",
    title: "Spotify: louder",
    detail: "Raises Spotify's own volume by ten, without touching the system volume.",
    app: "Spotify",
    script: "set sound volume to (sound volume + 10)",
    keywords: ["spotify", "volume", "louder", "up"],
    reach: 40,
  },
  {
    id: "spotify.volume-down",
    title: "Spotify: quieter",
    detail: "Lowers Spotify's own volume by ten.",
    app: "Spotify",
    script: "set sound volume to (sound volume - 10)",
    keywords: ["spotify", "volume", "quieter", "down"],
    reach: 40,
  },
];

const BROWSER_SPECS: AppCommandSpec[] = [
  {
    id: "chrome.new-tab",
    title: "Chrome: new tab",
    detail: "Opens a tab in the front window.",
    app: "Google Chrome",
    script: "tell front window to make new tab",
    keywords: ["chrome", "tab", "new", "browser"],
    reach: 62,
  },
  {
    id: "chrome.new-window",
    title: "Chrome: new window",
    detail: "A fresh window.",
    app: "Google Chrome",
    script: "make new window",
    keywords: ["chrome", "window", "new", "browser"],
    reach: 58,
  },
  {
    id: "chrome.incognito",
    title: "Chrome: new incognito window",
    detail: "A window that keeps no history.",
    app: "Google Chrome",
    script: 'make new window with properties {mode:"incognito"}',
    keywords: ["chrome", "incognito", "private", "window", "browser"],
    reach: 60,
  },
  {
    id: "chrome.copy-url",
    title: "Chrome: copy this page's address",
    detail: "The URL of the tab in front.",
    app: "Google Chrome",
    script: "return URL of active tab of front window",
    keywords: ["chrome", "url", "link", "copy", "address", "page"],
    reach: 64,
    output: "Page address",
  },
  {
    id: "chrome.copy-title",
    title: "Chrome: copy this page's title",
    detail: "The title of the tab in front.",
    app: "Google Chrome",
    script: "return title of active tab of front window",
    keywords: ["chrome", "title", "copy", "page", "name"],
    reach: 44,
    output: "Page title",
  },
  {
    id: "chrome.copy-all-tabs",
    title: "Chrome: copy every open tab",
    detail: "Title and address of every tab in the front window, one per line.",
    app: "Google Chrome",
    script:
      'set out to ""\nrepeat with t in tabs of front window\nset out to out & (title of t) & " — " & (URL of t) & linefeed\nend repeat\nreturn out',
    keywords: ["chrome", "tabs", "all", "copy", "list", "session"],
    reach: 48,
    output: "Open tabs",
  },
  {
    id: "safari.copy-url",
    title: "Safari: copy this page's address",
    detail: "The URL of the tab in front.",
    app: "Safari",
    script: "return URL of current tab of front window",
    keywords: ["safari", "url", "link", "copy", "address", "page"],
    reach: 50,
    output: "Page address",
  },
];

const APP_NOT_RUNNING = "CADUCEUS_NOT_RUNNING";

/**
 *
 * The `tell application` wrapper is added here rather than written out in every
 * spec, and the "is it running" guard with it: every one of these should be a
 * no-op with an explanation when the app is closed, not a reason for it to
 * launch.
 */
function appCommand(spec: AppCommandSpec): CommandDef {
  return {
    id: spec.id,
    title: spec.title,
    detail: `${spec.detail} Needs Automation permission for ${spec.app}, which macOS asks for once.`,
    group: spec.id.startsWith("spotify") ? "sound" : "utilities",
    icon: spec.id.startsWith("spotify") ? "♫" : "◇",
    keywords: spec.keywords,
    reach: spec.reach,
    async run({ actions }) {
      const script = [
        `if application "${spec.app}" is not running then return "${APP_NOT_RUNNING}"`,
        `tell application "${spec.app}"`,
        spec.script,
        "end tell",
      ].join("\n");

      try {
        const result = await api.runAppleScript(script);
        if (result.trim() === APP_NOT_RUNNING) {
          actions.notify(`${spec.app} is not running.`, "error");
          return false;
        }
        if (spec.output) {
          const text = result.trim();
          if (!text) {
            actions.notify(`${spec.app} had nothing to report.`, "error");
            return false;
          }
          await copyText(text);
          actions.showOutput({ title: spec.output, text, message: "Copied" });
          return false;
        }
        actions.notify(`${spec.title.split(": ")[1] ?? "Done"}.`);
        return false;
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
        return false;
      }
    },
  };
}

const APP_COMMANDS: CommandDef[] = [...SPOTIFY_SPECS, ...BROWSER_SPECS].map(appCommand);

// ---------------------------------------------------------------------------
// The small system things
// ---------------------------------------------------------------------------

/**
 * The ones a launcher is expected to have.
 *
 * Every one of these is a thing macOS can already do and has hidden behind a
 * menu, a keyboard shortcut nobody remembers, or a Finder window. None of them
 * is clever; all of them are faster typed than found, which is the entire
 * argument for a launcher.
 */
const DESK_COMMANDS: CommandDef[] = [
  {
    id: "desk.emoji",
    title: "Emoji and symbols",
    detail:
      "Opens macOS's own emoji and symbol picker, which inserts straight into whatever you were typing in.",
    group: "system",
    icon: "☺",
    keywords: ["emoji", "symbol", "symbols", "character", "characters", "picker", "smiley", "unicode", "special"],
    reach: 72,
    async run({ actions }) {
      // The picker inserts into the frontmost text field, so Caduceus has to
      // get out of the way before opening it — otherwise the emoji lands in
      // the search box you were about to close.
      await api.hideCommandCenter();
      try {
        // The Edit menu item, which is the only supported way in. The key
        // equivalent is ⌃⌘Space; sending it needs Accessibility, and this does
        // not.
        await api.runAppleScript(
          'tell application "System Events" to tell (first process whose frontmost is true) to ' +
            'click menu item "Emoji & Symbols" of menu 1 of menu bar item "Edit" of menu bar 1',
        );
      } catch {
        // Not every app has that menu item. `Character Viewer` is the fallback
        // and works from anywhere.
        try {
          await api.runAppleScript(
            'tell application "System Events" to keystroke space using {control down, command down}',
          );
        } catch (error) {
          actions.notify(api.errorMessage(error), "error");
        }
      }
      return true;
    },
  },
  {
    id: "desk.open-trash",
    title: "Open the Trash",
    detail: "Shows what is in it, before you empty it.",
    group: "files",
    icon: "▽",
    keywords: ["trash", "bin", "rubbish", "deleted", "recycle", "open"],
    reach: 60,
    run: ({ actions }) =>
      outcome(actions, "Trash", async () => {
        await api.runAppleScript('tell application "Finder" to open trash\nactivate application "Finder"');
        return { ok: true, message: "Opened the Trash.", copied: null };
      }, true),
  },
  {
    id: "desk.empty-trash",
    title: "Empty the Trash",
    detail: "Deletes everything in it. There is no undo for this one, which is why it asks first.",
    group: "files",
    icon: "▼",
    keywords: ["trash", "bin", "empty", "delete", "purge", "clear", "space"],
    reach: 58,
    confirm: "Everything in the Trash will be gone for good.",
    run: ({ actions }) =>
      outcome(actions, "Trash", async () => {
        await api.runAppleScript('tell application "Finder" to empty trash');
        return { ok: true, message: "Trash emptied.", copied: null };
      }),
  },
  {
    id: "desk.open-camera",
    title: "Open the camera",
    detail: "Photo Booth, for when you need to check what the camera can see.",
    group: "system",
    icon: "◎",
    keywords: ["camera", "webcam", "photo", "booth", "video", "selfie", "mirror"],
    reach: 44,
    run: ({ actions }) =>
      outcome(actions, "Camera", async () => {
        await api.launchApp("Photo Booth");
        return { ok: true, message: "Opened Photo Booth.", copied: null };
      }, true),
  },
  {
    id: "desk.hide-others",
    title: "Hide every app except this one",
    detail: "Clears the screen down to what you are actually working in. ⌥⌘H, without the finger gymnastics.",
    group: "windows",
    icon: "◫",
    keywords: ["hide", "others", "focus", "declutter", "clear", "minimise", "minimize", "distraction"],
    reach: 66,
    run: ({ actions }) =>
      outcome(actions, "Windows", async () => {
        await api.runAppleScript(
          'tell application "System Events" to set visible of (every process whose visible is true ' +
            'and frontmost is false and name is not "Finder") to false',
        );
        return { ok: true, message: "Hid everything else.", copied: null };
      }, true),
  },
  {
    id: "desk.quit-others",
    title: "Quit every app except this one",
    detail:
      "Asks each app to quit, so anything with unsaved work gets to put its dialog up rather than losing it. Finder and Caduceus are left alone.",
    group: "system",
    icon: "⊗",
    keywords: ["quit", "close", "all", "apps", "everything", "except", "clean", "restart"],
    reach: 54,
    confirm: "Every other app will be asked to quit.",
    run: ({ actions }) =>
      outcome(actions, "Apps", async () => {
        const result = await api.runAppleScript(
          'set skipped to {"Finder", "Caduceus", "System Events"}\n' +
            'tell application "System Events" to set names to name of (every process whose ' +
            "background only is false and frontmost is false)\n" +
            "set closed to 0\n" +
            "repeat with n in names\n" +
            "  if skipped does not contain (n as text) then\n" +
            '    try\n      tell application (n as text) to quit\n      set closed to closed + 1\n    end try\n' +
            "  end if\n" +
            "end repeat\n" +
            "return closed",
        );
        const count = Number.parseInt(result.trim(), 10) || 0;
        return {
          ok: true,
          message: count ? `Asked ${count} app${count === 1 ? "" : "s"} to quit.` : "Nothing else was running.",
          copied: null,
        };
      }),
  },
  {
    id: "desk.mute",
    title: "Mute",
    detail: "Sets the system volume to zero. Running it again puts it back where it was.",
    group: "sound",
    icon: "◁",
    keywords: ["mute", "silence", "quiet", "volume", "zero", "sound", "off"],
    reach: 68,
    async run({ actions }) {
      try {
        // Remembering the level is the difference between a mute and a
        // volume-to-zero you then have to guess your way back from.
        const result = await api.runAppleScript(
          "set current to output volume of (get volume settings)\n" +
            "if current > 0 then\n" +
            "  do shell script \"defaults write com.caduceus.desktop lastVolume \" & current\n" +
            "  set volume output volume 0\n" +
            '  return "muted " & current\n' +
            "else\n" +
            '  set prior to (do shell script "defaults read com.caduceus.desktop lastVolume 2>/dev/null || echo 40")\n' +
            "  set volume output volume (prior as integer)\n" +
            '  return "restored " & prior\n' +
            "end if",
        );
        actions.notify(
          result.startsWith("muted") ? "Muted." : `Volume back to ${result.split(" ")[1]}%.`,
        );
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
      }
      return true;
    },
  },
];

// ---------------------------------------------------------------------------
// Apple Shortcuts
// ---------------------------------------------------------------------------

/**
 * Run a shortcut from the Shortcuts app.
 *
 * One command rather than one per shortcut: which shortcuts exist is a property
 * of the user's machine, and the palette can offer them by name through the
 * live-list provider instead of the registry pretending to know.
 */
const SHORTCUTS_COMMANDS: CommandDef[] = [
  {
    id: "apple.run-shortcut",
    title: "Run an Apple Shortcut",
    detail:
      "Runs any shortcut from the Shortcuts app by name, with optional text as its input. Everything you have already built in Shortcuts is reachable from the search bar.",
    group: "utilities",
    icon: "⌘",
    keywords: ["shortcut", "shortcuts", "apple", "automation", "workflow", "run", "siri"],
    trigger: "shortcut",
    argument: "the shortcut's name",
    reach: 62,
    form: {
      fields: [
        {
          kind: "text",
          id: "input",
          label: "Shortcut name",
          placeholder: "exactly as it is called in the Shortcuts app",
          required: true,
        },
        {
          kind: "text",
          id: "payload",
          label: "Input to give it (optional)",
          multiline: true,
          file: true,
        },
      ],
      submitLabel: "Run it",
    },
    async run({ input, values, actions }) {
      const name = (values.input ?? input).trim();
      if (!name) {
        actions.notify("Which shortcut?", "error");
        return false;
      }
      try {
        const result = await api.runAppleShortcut(name, values.payload ?? "");
        if (result.trim()) {
          actions.showOutput({ title: name, text: result.trim() });
        } else {
          actions.notify(`Ran “${name}”.`);
        }
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
      }
      return false;
    },
  },
  {
    id: "apple.list-shortcuts",
    title: "List your Apple Shortcuts",
    detail: "Every shortcut the Shortcuts app knows about, so you can see what is runnable.",
    group: "utilities",
    icon: "⌘",
    keywords: ["shortcut", "shortcuts", "apple", "list", "automation", "workflow"],
    reach: 40,
    async run({ actions }) {
      try {
        const names = await api.listAppleShortcuts();
        if (names.length === 0) {
          actions.notify("The Shortcuts app has nothing in it yet.");
          return false;
        }
        actions.showOutput({
          title: `${names.length} shortcuts`,
          text: names.join("\n"),
          message: "Run one with `shortcut <name>`",
        });
      } catch (error) {
        actions.notify(api.errorMessage(error), "error");
      }
      return false;
    },
  },
];

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

export const COMMANDS: CommandDef[] = [
  ...PAGE_COMMANDS,
  ...WINDOW_COMMANDS,
  ...TOOL_COMMANDS,
  ...CASE_COMMANDS,
  ...SYSTEM_COMMANDS,
  ...LIST_COMMANDS,
  ...OTHER_COMMANDS,
  ...DESK_COMMANDS,
  ...APP_COMMANDS,
  ...SHORTCUTS_COMMANDS,
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
