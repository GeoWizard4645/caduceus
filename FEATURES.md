# Feature requests: what is free, what is not, what is impossible

The rule for Caduceus is that a built-in feature must work for free, with no
account, no API key and no subscription. This is the triage of the requested
list against that rule. Anything in the "costs money" or "cannot be done"
sections is **not** built, deliberately.

## Shipped in 1.1.0

| Feature | How |
|---|---|
| Change case | 8 cases, pure-Rust transforms, camelCase humps handled |
| Paste as plain text | Same path, `Plain` case |
| Copy path | Finder selection via AppleScript |
| Download manager (`cdown` / `odown`) | Newest file in `~/Downloads`, part-files skipped |
| Eject mounted disks | `diskutil` |
| Keep the computer awake | `caffeinate -w <pid>`, so it dies with the app |
| Search files | `mdfind` (Spotlight) |
| Define word | `dict://`, the built-in Dictionary |
| Compress / resize / convert images | `sips`, never overwrites the original |
| Add to Apple Notes | AppleScript, with the automation prompt explained |
| Saved AI chats | SQLite, inline in the palette and in a full window |
| Clipboard history | Already shipped before this release |
| Kill process | Already shipped, in the system monitor |

All of the above use tools macOS already has. Nothing was added to the
dependency list to make them work.

## Free, not yet built — no blockers, just time

Ordered roughly by value per hour of work.

- **Snippets** — storage plus expansion; the settings schema already has a
  shape to copy from the prefix rules
- **Quicklinks** — mostly present already as `OpenUrlTemplate` prefixes; needs
  a nicer editor rather than new machinery
- **Emoji picker** — needs a bundled emoji dataset (free, CLDR)
- **QR code generator** — one crate (`qrcode`, MIT), renders to PNG
- **Sticky notes** — a small window plus the storage pattern from chats
- **Incognito clone** — per-browser AppleScript; Arc, Chrome, Safari all support it
- **Set audio device (`/i`, `/o`)** — needs a small Swift CoreAudio helper; the
  Swift build infrastructure already exists for the dictation helpers
- **Screen OCR** — Vision framework via a Swift helper; free and on-device
- **Window management** — Accessibility API; the biggest of these, and the one
  most worth doing properly
- **Typing practice** — a self-contained mini-app
- **Apple Music control** — AppleScript
- **Hide apps from search results** — a settings list the app index filters on
- **Auto-quit inactive apps** — needs a background watcher and careful defaults
- **Hyper Key** — a `CGEventTap`; doable, but it is a keyboard driver and
  deserves its own review
- **Dictate anywhere (double-click staff / F1)** — the dictation fixes in this
  release are the prerequisite

## Needs something the user installs

These work, but only if a third-party tool is present. Caduceus should detect
and use it, never silently fail.

- **Bluetooth management (Toothpick)** — needs `blueutil` (Homebrew). macOS has
  no supported CLI or public API for toggling connections.
- **Download videos from a website** — needs `yt-dlp`. Also carries obvious
  terms-of-service questions per site.
- **Keyboard brightness** — no public API. Requires a private framework or a
  helper binary, and Apple has broken both across releases.
- **Amphetamine UI** — only if Amphetamine is installed. `caffeinate` is the
  free fallback and is what shipped.

## Costs money — deliberately not built

Per your rule, these are excluded and should be called out on the website
rather than half-built.

- **Google Translate** — the Cloud Translation API is billed per character.
  There is a free unofficial endpoint; it is undocumented, rate-limited and
  against Google's terms. Not worth shipping.
- **Jira / Linear** — the APIs are free to call, but both products require a
  paid plan for any real team use. Better as community extensions.
- **Internet speed test** — needs a bandwidth-serving endpoint. Cloudflare's is
  free to use but is someone else's bandwidth; worth asking before depending
  on it.

## Cannot be done

- **Authy** — Authy has no public API, and Twilio shut down its desktop apps in
  2024. Reading its token store would mean reverse-engineering an encrypted
  database, which is both fragile and a bad idea for a 2FA secret. If you want
  TOTP in Caduceus, the honest version is a built-in TOTP generator where *you*
  paste the seed — free, offline, and not pretending to be Authy.
- **Starting a macOS Focus session** — Focus cannot be set programmatically.
  The only supported route is a Shortcuts action the user triggers, which
  Caduceus can call, but it cannot flip Focus directly.

## Better as extensions than built-ins

Each of these is a thin wrapper over one product's API or CLI. Building them in
means shipping and maintaining integrations most users will never enable — which
is exactly the argument for the extension system in `EXTENSIONS.md`.

VS Code · Homebrew · GitHub · Arc · Tailwind CSS docs · Warp / iTerm2 / Alacritty
/ Terminal · Apple Music

The right order is: build the extension host, port these as extensions, and let
people write the ones nobody thought of.
