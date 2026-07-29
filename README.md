<div align="center">
  <img src="assets/caduceus-mark.png" width="96" alt="The Caduceus mark: a pixel-art winged staff with two coiled snakes">
  <h1>Caduceus</h1>
  <p><strong>A fast, local-first command centre for your Mac.</strong></p>
  <p>
    A floating staff that stays out of your way, a command palette that launches
    apps and does maths, a clipboard that remembers, and an AI agent that can
    drive your machine when you ask it to.
  </p>
  <p>
    <a href="https://caduceus.vivaanshahani.com">Website</a> ·
    <a href="#install">Install</a> ·
    <a href="#what-it-does">What it does</a> ·
    <a href="https://caduceus.vivaanshahani.com/docs">Docs</a> ·
    <a href="#the-ai-part">The AI part</a> ·
    <a href="#building-from-source">Build from source</a>
  </p>
  <p>
    <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-111?logo=apple&logoColor=white">
    <img alt="Apple Silicon and Intel" src="https://img.shields.io/badge/universal-Apple%20Silicon%20%2B%20Intel-111">
    <img alt="Built with Tauri and Rust" src="https://img.shields.io/badge/built%20with-Tauri%20%2B%20Rust-111?logo=rust&logoColor=white">
    <img alt="GPL-3.0-or-later" src="https://img.shields.io/badge/licence-GPL--3.0--or--later-111">
  </p>
</div>

---

**Caduceus is an open-source macOS command palette, app launcher and clipboard
manager, built with Tauri and Rust.** It is a free alternative to Raycast,
Alfred and Spotlight that runs entirely on your machine: no account, no
subscription, no telemetry, and no network connection required for anything
except the parts you explicitly point at a model. The market and sports widgets are the one exception, and only once you add one: they fetch public quotes and scores from CoinGecko, Yahoo Finance and ESPN. No account, no key, and nothing about you goes with the request — but those hosts do see your IP, so a widget you have not added fetches nothing at all. One ~10 MB universal binary,
Apple Silicon and Intel, macOS 11 and later.

If you have been looking for a Raycast alternative that is actually open source,
a Spotlight replacement with clipboard history, or a Mac launcher you can read
the source of before you run it — that is what this is.

**Website:** [caduceus.vivaanshahani.com](https://caduceus.vivaanshahani.com) ·
**Docs:** [caduceus.vivaanshahani.com/docs](https://caduceus.vivaanshahani.com/docs) ·
**Releases:** [every version](https://github.com/GeoWizard4645/caduceus/releases)

---

## Install

**macOS 11+, Apple Silicon or Intel.** One line:

```bash
curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash
```

That downloads the universal `.dmg` from the latest release, checks it against
the published checksum, mounts it, copies the app to `/Applications`, removes
the quarantine flag, and launches it. About 10 MB and ten seconds, with no
toolchain and nothing to configure.

Or with Homebrew:

```bash
brew install --cask geowizard4645/caduceus/caduceus
```

Same app. It taps [`geowizard4645/caduceus`](https://github.com/GeoWizard4645/homebrew-caduceus)
on the way through, so there is nothing to add first, and `brew upgrade --cask
caduceus` keeps it current. `brew uninstall --cask caduceus` removes the app;
add `--zap` to take the settings and clipboard history with it. The cask clears
the quarantine flag in a `postflight` block for the reason below — it is
[twelve lines you can read](./homebrew/caduceus.rb) — and `--no-quarantine`
turns that into a no-op if you would rather approve the app yourself.

Prefer to do it yourself? Download the `.dmg` from
[Releases](https://github.com/GeoWizard4645/caduceus/releases), drag Caduceus to
Applications, then run this once:

```bash
xattr -dr com.apple.quarantine /Applications/Caduceus.app
```

<details>
<summary><strong>Why that last command is needed, and about the "malware" warning</strong></summary>

Caduceus is not signed with an Apple Developer certificate — that costs $99/year
and this project does not have one yet — so anything that downloads it gets
tagged "quarantined" by macOS, and opening a quarantined, unsigned app is what
produces **"Apple could not verify that Caduceus is free of malware..."**. That
message is not specific to this app or this download; it is what macOS says
about *any* app from outside the App Store that lacks a paid developer
signature. `xattr -dr com.apple.quarantine` removes the tag — it is the same
thing the right-click → Open dance does, only in one command instead of a
guided tour of dialog boxes. You are telling macOS you trust this specific
app, which is a reasonable thing to decide for yourself given the source is
right here in this repo.

**The one-liner above and the Homebrew cask both run that command for you**,
after first checking the download against a published SHA-256 checksum — so
neither path should ever show you the warning. If you see it anyway, or you
downloaded the `.dmg` by hand instead:

- **macOS 14 and earlier:** right-click `Caduceus.app` in Finder → **Open** →
  **Open** in the dialog that follows. Once is enough — it does not ask again.
- **macOS 15 (Sequoia) and later:** open the app once (it will refuse), then go
  to **System Settings → Privacy & Security**, scroll to the bottom, and click
  **Open Anyway** next to the mention of Caduceus.
- Or run the one command yourself: `xattr -dr com.apple.quarantine /Applications/Caduceus.app`

None of this is a workaround for something being wrong with the download —
it is the one-time cost of "no paid certificate," and it goes away entirely
the day this project has a Developer ID. See
[RELEASE.md § Notarization](./RELEASE.md#notarization) for exactly what that
takes and how close it is to done in code.

</details>

Would rather run only code you compiled? The installer will do that too, at the
cost of a Rust and Node toolchain and a few minutes:

```bash
curl -fsSL https://raw.githubusercontent.com/GeoWizard4645/caduceus/main/website/install.sh | bash -s -- --from-source
```

Caduceus lives in your menu bar. There is no Dock icon.

---

## What it does

### The floating staff

A pixel Caduceus staff that sits on top of everything, on the right edge by default.

- **Hover it** — six shortcut icons fan out around it
- **Click it** — opens the Command Center
- **Right-click it** — opens Settings
- **Drag it** — anywhere you like; it remembers
- **`F12`** — hide or show it

It goes back to being click-through the moment your pointer leaves, so it never
eats a click meant for something behind it.

### The Command Center

`Alt+Space`, or click the staff.

| You type | What happens |
|---|---|
| `figma` | launches Figma — every app on your Mac is searchable |
| `1920/16*9` | shows `1,080`, Enter copies it |
| `18% of 240` | shows `43.2` |
| `left half` | snaps the frontmost window to the left half of its display |
| `sha256 hunter2` | hashes it and copies the digest |
| `jwt eyJhbGci…` | decodes the token's header and payload, locally |
| `port 3000` | shows what is holding the port; Enter stops it |
| `dark` | toggles macOS dark mode |
| `repo` | lists your git repositories with their branches |
| `flights to lisbon` | searches the web |
| `/ explain OAuth` | asks Hermes |
| `/c open my email and find the invoice` | Hermes drives your Mac |
| `/v invoice` | searches clipboard history |

Arrow keys to move, Enter to run, Esc to close. Prefixes are configurable — add
your own in Settings, or delete the ones you don't use.

### 154 built-in features

Open the Command Center with nothing typed and the whole catalogue is there,
ordered by how often *you* run each one. Counts live in a file next to your
clipboard history, are never sent anywhere, and record only which built-in row
was run — never what you typed. Settings → Command Center → Ranking clears them.


Sticky notes, meeting recording with a live on-device transcript, screen
recording *with system audio*, colours (screen picker, every notation, WCAG
contrast, palettes from an image), unit and currency conversion, disk cleanup
and proper app uninstall, folder tidying, citations in seven styles, process
management, window management, screen OCR, sound devices, encoders, hashes,
JSON and JWT inspection, Finder actions, ports, repositories, containers,
Spotify and browser control, and anything you have built in Apple Shortcuts —
all in the *same* ranked list as your apps, with no submenu to go and find.

**Everything opens as a page built for it.** Pick "sort lines" and you get a box
and a direction, not an error telling you to have typed something first. Every
text field takes a file as well as pasted text. Typing `sha256 hello` still runs
in one keystroke — that is the shortcut, not the only door.

The registry is one file. Adding a feature means appending an entry to
`src/shared/commands.ts` — it describes its own inputs and the page builds
itself — and it then appears in search, in the in-app catalogue and on the
website with no further wiring.

### Clipboard history

Everything you copy — text, images, file paths — searchable, pinnable, and
prunable. Password managers that mark their copies as concealed are skipped
automatically, and there is an app exclusion list on top of that.

Optional **encryption at rest** with ChaCha20-Poly1305, keyed from your macOS
keychain. This protects your history from anything that can read the database
file. It does *not* protect against software running as you while Caduceus is
unlocked — that could ask the keychain for the key exactly as Caduceus does.
Lose the keychain entry and the old history is gone for good; that is the point.

### Voice

Hold `⌘⇧Space`, talk, let go. Transcription runs **on-device** through Apple's
Speech framework — audio never leaves your Mac.

What you said is then routed by keyword:

- "**search** cheap flights" → web search
- "**computer** close all my tabs" → Hermes drives the Mac
- anything else → asked as a question

All the keywords and their destinations are editable.

**There is no wake word, deliberately.** Always-on listening would mean a
process with permanent microphone access that also has screen control. The
microphone opens when you hold the key and closes when you let go.

---

## The AI part

Caduceus does not ship its own model, its own agent loop, or its own screen
control. It drives [**Hermes Agent**](https://github.com/NousResearch/hermes-agent)
— the open-source agent from Nous Research — which already does all three, far
better than a side project would.

Caduceus is the *surface*: a hotkey, a palette, a place for the answer to land.
Hermes is the engine.

**Setup is two commands.** Settings → AI will run them for you, or:

```bash
curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash
hermes setup --portal
```

Hermes works with whatever model you want — Nous Portal, OpenRouter, OpenAI,
Ollama running locally, your own endpoint. Caduceus follows whatever
`hermes model` is set to and has no opinion about it.

**Everything else works without any of this.** Shortcuts, the app launcher, the
calculator, clipboard history, voice transcription and web search all run with
Hermes uninstalled and zero API keys. The `/` and `/c` prefixes are the only
things that need it.

### Screen control

`/c` hands the task to Hermes' `computer_use` toolset. Caduceus asks you first,
every session, and shows a Stop button for as long as it runs. Turning the
confirmation off is possible and is a bad idea.

Because Hermes owns the screen control, *Caduceus itself* never asks for
Accessibility or Screen Recording permission. Hermes does, when you first use
it.

---

## Building from source

You need [Rust](https://rustup.rs), [Node](https://nodejs.org) 20+, and Xcode
Command Line Tools (`xcode-select --install`).

```bash
git clone https://github.com/GeoWizard4645/caduceus.git
cd caduceus
npm install
npm start                             # run in development
npm run tauri -- build --bundles app  # Caduceus.app into src-tauri/target/release/bundle/macos
npm run bundle                        # the same, plus a .dmg
```

Then drag it across:

```bash
cp -R src-tauri/target/release/bundle/macos/Caduceus.app /Applications/
```

Releases ship a single universal `.dmg` covering Apple Silicon and Intel. See **[RELEASE.md](./RELEASE.md)** for the full release checklist (version bump, lipo, `gh release create`).

For a local Intel slice only: `rustup target add x86_64-apple-darwin`, then:

```bash
npm run tauri -- build --target x86_64-apple-darwin --bundles app
```

Tests:

```bash
npm run test:rust  # 125 unit tests
npm run typecheck
```

### Layout

```
src/                     React — three webviews
  staff/                 the floating staff + radial pop-out
  command-center/        the palette, agent panel, clipboard view
  settings/              settings tabs, including the feature catalogue
  shared/                IPC bindings, result providers, command registry
  shared/commands.ts     every built-in command and its explanation
src-tauri/src/           Rust
  agent/                 AgentBackend trait; Hermes + OpenAI-compatible impls
  apps.rs                installed-application index (the launcher)
  calc.rs                the calculator's expression parser
  clipboard/             watcher, SQLite store, encryption
  shortcuts/             the Shortcut primitive and how each kind executes
  tools/                 the built-in commands: dev, system, files, net, media
  voice/                 push-to-talk capture, speech-to-text, keyword routing
  palette.rs             prefix parsing and dispatch
  window/                staff placement, cursor tracking, window management
  window/accessibility.rs  hand-written AXUIElement + Core Foundation bindings
  window/manage.rs       window snapping; the geometry is pure and unit-tested
src-tauri/macos/         Swift helpers (speech, and Vision OCR + CoreAudio)
scripts/make-icons.py    generates every icon from one pixel grid
scripts/build-features-catalog.mjs   merges the registry into the website JSON
```

Two design rules worth knowing before you change things:

1. **The webview can only call named commands.** The `shell`, `fs` and `http`
   Tauri plugins are deliberately not enabled. Shortcuts run *by id* against
   saved settings — the frontend never hands Rust a command string to execute.
2. **Secrets only ever go in the keychain.** There is no command that reads an
   API key back out, so a compromised webview cannot exfiltrate one.
3. **Wide surfaces are closed enums, not strings.** The 40-odd developer tools
   are one command taking a `ToolId`; the system controls are one command taking
   a `SystemAction`. The webview can name a tool that exists and nothing else.

### Adding a command

Append one entry to `COMMANDS` in `src/shared/commands.ts`:

```ts
{
  id: "utility.example",
  title: "Do the thing",
  detail: "One or two sentences saying what happens and what you get back.",
  group: "utilities",
  icon: "◇",
  keywords: ["thing", "example"],
  trigger: "thing",              // optional: `thing some input`
  argument: "text to use",       // optional: shown as a placeholder
  run: ({ input, actions }) => outcome(actions, "Thing", () => api.doTheThing(input)),
}
```

It is now searchable, documented in Settings → Features, and on the website
after the next `npm run build`. `detail` is the only description that exists —
there is no second copy to keep in sync.

### Adding a result source

The palette is a list of providers. Add one object, append it to
`defaultProviders` in `src/shared/providers.ts`, done:

```ts
export const emojiProvider: ResultProvider = {
  id: "emoji",
  title: "Emoji",
  search({ query }) {
    if (!query) return [];
    return findEmoji(query).map((e) => ({
      id: `emoji:${e.name}`,
      title: e.char,
      subtitle: e.name,
      icon: e.char,
      group: "Emoji",
      score: 400,
      run: () => navigator.clipboard.writeText(e.char),
    }));
  },
};
```

### Adding an AI backend

Implement `AgentBackend` in `src-tauri/src/agent/`, add a `BackendKind` variant,
and add one arm to `backend_for()` in `agent/mod.rs`. The trait is small — look
at `hermes.rs`, which is a complete implementation in about 200 lines of real
logic.

### Redesigning the mark

Every icon — the app icon, the menu-bar template, and the staff you see on
screen — is generated from one pixel grid in `scripts/make-icons.py`. Edit the
grid, run `python3 scripts/make-icons.py`, and all of them update together. The
script prints an ASCII preview so you can check the shape without opening a PNG.

---

## Platform support

macOS only for now. The Rust source keeps its per-platform `cfg` blocks and
`Cargo.toml` keeps its target-specific dependency sections, so Windows and Linux
are a build-target change rather than a rewrite — but they are not built,
tested, or supported today. The pieces that would need real work are the
application index (`apps.rs`), frontmost-app detection for clipboard exclusions,
and the on-device speech helper.

---

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE), and [NOTICE.md](NOTICE.md) for why it
changed in v3.0.0 and who is credited. Releases up to v2.3.1 remain MIT.

Hermes Agent is a separate project by [Nous Research](https://nousresearch.com),
under its own licence.
