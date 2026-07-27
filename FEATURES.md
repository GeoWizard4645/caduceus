# Features: what is built, what is not, and why

The **complete catalogue** lives in one place and is generated, not written
twice:

- commands come from [`src/shared/commands.ts`](src/shared/commands.ts) — the
  registry the palette itself searches, so the explanation you read in the
  catalogue is the same string the palette shows as a subtitle;
- everything that is a *capability* rather than a runnable command lives in
  [`website/features-catalog.json`](website/features-catalog.json);
- `npm run catalog` merges the two into that JSON for the website. It runs as
  part of `npm run build`, so a command cannot ship without the website
  knowing about it.

Rendered on [All features](https://caduceus.vivaanshahani.com/features) and in
the app under **Settings → Features**.

The rule for Caduceus is unchanged: **a built-in must work for free, offline,
with no account and no API key.** Anything that cannot is named in the "not
built" section rather than half-shipped.

## What 2.0 added

124 commands, all of them in the same ranked list as your apps, shortcuts and
clipboard — there is no separate menu to go and find.

| Area | Commands | What it needs |
|---|---|---|
| Window management | 22 | Accessibility permission |
| Developer tools | 28 | nothing |
| Text | 20 | nothing |
| System | 16 | Automation permission for a few |
| Files & Finder | 11 | Automation permission for Finder |
| Sound & media | 9 | nothing |
| Network | 6 | nothing (one command is deliberately online) |
| Utilities | 6 | nothing |
| Developer environment | 4 | Docker only for the Docker list |
| Screen & text | 2 | Screen Recording permission |

### Window management

Halves, quarters, thirds, two-thirds, maximize, almost-maximize, centre,
grow/shrink, move between displays, and native full screen. Built on the
Accessibility API in the **main binary**, not in a helper: macOS grants
Accessibility per code signature, so a separately-signed helper would need its
own entry in System Settings and would lose it on every rebuild. One switch,
with Caduceus's name on it.

The geometry is pure arithmetic with no macOS types in it, and has 19 unit tests
covering the things that are actually easy to get wrong — thirds that do not add
up, a window moved to a differently-shaped display, and a window dragged
entirely off-screen.

### On-device OCR

`Copy text from the screen` drags a box over anything on screen and copies the
text inside it. Recognition runs through Apple's Vision framework in a small
Swift helper; the capture is written to a temporary file and deleted before the
command returns, whether or not recognition succeeded.

### Sound device switching

Lists every connected input and output and switches the system default.
Devices are tracked by their CoreAudio UID, which survives a reboot — the
numeric device id macOS also exposes is reassigned freely and must never be
persisted.

### The developer toolbox

Encoders, hashes, identifiers, formatters and inspectors, reached through one
IPC command with a closed `ToolId` enum rather than sixty entry points. The
point of having these locally is that a JWT or an API payload should never be
pasted into a website to be read.

Hashes shell out to `shasum` and `md5`, which macOS already ships, rather than
linking three crypto crates into the binary for a feature most people use twice
a year.

### The 1.1 utilities, now reachable

Change case, copy path, the download manager, eject, keep-awake, file search,
define word and image conversion all shipped as Rust commands in 1.1.0 — and
none of them had a caller. The release notes listed them and nothing in the UI
could run them. They are wired into the palette in 2.0.

## Ranking

The palette ships an opinion about what you are most likely to want — window
snapping first, the commands that end your session last — and then lets your own
use overrule it. One use of anything puts it above everything untouched; more
uses outrank fewer.

Typing still wins. History breaks ties, it does not override what you asked for:
`shut down` finds Shut down however rarely you run it.

`scripts/check-ranking.mjs` asserts all four of those properties at build time,
because getting the constants wrong does not fail a type check — it just quietly
produces a list in the wrong order.

## Needs something you install

Detected and used when present; never a silent failure.

- **Docker** — the container list says plainly whether Docker is missing or
  installed-but-not-running.
- **`yt-dlp`** for downloading media from a website. Not bundled, and the
  per-site terms-of-service question belongs to the person running it.

## First-party extensions (coming soon)

The full list and rationale are in the last section of the in-app catalogue and on the
website. These ship as official Caduceus extensions (same drag-in format as community
ones) rather than silent built-ins — so API keys, helpers you install, and permissions
stay explicit. Planned first-party work includes Google Translate, Jira and Linear,
speed test, Authy import, macOS Focus via Shortcuts, Bluetooth switching, keyboard
backlight, per-application volume, yt-dlp media download, and system-wide text expansion.

### Text expansion that types for you

Still called out separately because it is the one people ask for most. Expanding a
trigger as you type anywhere in macOS needs a system-wide keyboard event tap —
a keylogger by any technical definition. It will ship as its own first-party extension
after a focused security review and opt-in. Snippets you pick from the palette and
paste are the built-in alternative today.

## Better as extensions than built-ins

Each of these is a thin wrapper over one product's API or CLI. Building them in
means shipping and maintaining integrations most users will never enable — which
is the argument for the extension system in [`EXTENSIONS.md`](EXTENSIONS.md).

VS Code · Homebrew · GitHub · Arc · Tailwind CSS docs · Warp / iTerm2 /
Alacritty · Apple Music beyond play/pause/next
