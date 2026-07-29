/**
 * The Learn tab.
 *
 * Each entry teaches one thing and, where Caduceus is able to, performs it.
 * That split is the whole design: some setup is ours (a hotkey, a prefix, a
 * toggle) and some belongs to macOS (freeing ⌘Space, granting the microphone).
 * A tutorial that pretends it can do the second kind just appears to fail, so
 * those steps say plainly that they are yours and open the right pane instead.
 *
 * `done` drives the ✓ in the list. It answers "is this already true?", never
 * "have you read this?" — so a tutorial un-ticks itself if you undo the thing
 * later, and nothing has to be persisted to track progress.
 */

import { useEffect, useRef, useState, type ReactNode } from "react";

import * as api from "@/shared/api";
import { DOCS_CONFIGURE_AI } from "@/shared/docsUrls";
import { STAFF_POPOUT_LIMIT, type Settings } from "@/shared/types";
import { Button, Callout, Kbd, cx } from "@/shared/ui";

import type { Draft } from "../useDraft";

export type TutorialId = "spotlight" | "prefixes" | "staff" | "voice" | "clipboard" | "ai";

/** The accelerator format `HotkeyInput` produces — see `shared/ui.tsx`. */
const CMD_SPACE = "CommandOrControl+Space";

interface Ctx {
  draft: Draft;
  settings: Settings;
  /** Jump to another settings tab, for the parts that live there. */
  goTo: (tab: string) => void;
}

interface Tutorial {
  id: TutorialId;
  icon: string;
  title: string;
  blurb: string;
  /** Whether the thing being taught is currently in place. */
  done: (s: Settings) => boolean;
  body: (ctx: Ctx) => ReactNode;
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

const TUTORIALS: Tutorial[] = [
  {
    id: "spotlight",
    icon: "⌘",
    title: "Make ⌘Space open Caduceus",
    blurb: "Spotlight holds ⌘Space out of the box. Free it, then hand it over.",
    done: (s) => s.general.commandCenterHotkey === CMD_SPACE,
    body: ({ draft, settings }) => (
      <>
        <p>
          macOS binds ⌘Space to Spotlight at the system level, and the system wins every
          time. Caduceus can ask for the combination, but the key press will keep going to
          Spotlight until Spotlight lets go of it. So this is two jobs in order: macOS
          releases the key, then Caduceus takes it.
        </p>

        <Steps>
          <li>
            Open <b>Keyboard → Keyboard Shortcuts → Spotlight</b> in System Settings.
            <Action>
              <Button size="sm" onClick={() => void api.openSystemSettings("keyboard-shortcuts")}>
                Open Keyboard Shortcuts
              </Button>
            </Action>
          </li>
          <li>
            Untick <b>Show Spotlight search</b>, then close System Settings. (Leave{" "}
            <b>Show Finder search window</b> alone unless you also want ⌥⌘Space.)
          </li>
          <li>
            Come back here and take the key.
            <Action>
              <Button
                size="sm"
                tone="primary"
                disabled={settings.general.commandCenterHotkey === CMD_SPACE}
                onClick={() => draft.update((d) => (d.general.commandCenterHotkey = CMD_SPACE))}
              >
                {settings.general.commandCenterHotkey === CMD_SPACE
                  ? "⌘Space is bound"
                  : "Use ⌘Space for the Command Center"}
              </Button>
            </Action>
          </li>
        </Steps>

        <Callout tone="info">
          Do it in this order. If you bind ⌘Space before turning Spotlight off, the save
          succeeds and the key still does nothing — you would get a “could not be
          registered” warning at the top of this window with no obvious cause. Spotlight is
          still reachable afterwards from the menu-bar magnifying glass.
        </Callout>
      </>
    ),
  },

  {
    id: "prefixes",
    icon: "⌕",
    title: "Type less with prefixes",
    blurb: "A prefix routes the rest of the line somewhere specific.",
    done: (s) => s.commandCenter.prefixes.some((p) => p.prefix === "yt"),
    body: ({ draft, settings, goTo }) => (
      <>
        <p>
          Plain text in the Command Center goes to a web search. A prefix overrides that
          for one line: <Kbd>/</Kbd> sends it to your AI backend, <Kbd>/v</Kbd> searches
          your clipboard, <Kbd>/c</Kbd> hands the task to an agent that drives your screen.
          The longest match wins, so <Kbd>/c</Kbd> beats <Kbd>/</Kbd> no matter what order
          the list is in.
        </p>

        <Steps>
          <li>
            Add one you did not have — <Kbd>yt</Kbd> for YouTube. Anything after the prefix
            becomes the search.
            <Action>
              <Button
                size="sm"
                tone="primary"
                disabled={settings.commandCenter.prefixes.some((p) => p.prefix === "yt")}
                onClick={() =>
                  draft.update((d) => {
                    d.commandCenter.prefixes.push({
                      id: `prefix-${crypto.randomUUID().slice(0, 8)}`,
                      prefix: "yt",
                      label: "YouTube",
                      description: "Search YouTube",
                      action: "open_url_template",
                      target: "https://www.youtube.com/results?search_query={query}",
                      browser: null,
                      showHint: true,
                    });
                  })
                }
              >
                {settings.commandCenter.prefixes.some((p) => p.prefix === "yt")
                  ? "Added"
                  : "Add the yt prefix"}
              </Button>
            </Action>
          </li>
          <li>
            Open the Command Center and type <Kbd>yt tiny desk</Kbd>.
          </li>
          <li>
            Build your own on the Command Center tab. <code>{"{query}"}</code> in the target
            URL is where your text lands.
            <Action>
              <Button size="sm" onClick={() => goTo("command-center")}>
                Open the Command Center tab
              </Button>
            </Action>
          </li>
        </Steps>
      </>
    ),
  },

  {
    id: "staff",
    icon: "◐",
    title: "Put a shortcut on the staff",
    blurb: "The floating circle holds your six most-used actions.",
    done: (s) => s.shortcuts.some((sc) => sc.showInStaff && !sc.hidden),
    body: ({ settings, goTo }) => {
      const onStaff = settings.shortcuts.filter((s) => s.showInStaff && !s.hidden).length;
      return (
        <>
          <p>
            Hover the staff and its icons fan out. It draws at most{" "}
            {STAFF_POPOUT_LIMIT} of them, so treat the slots as a shortlist rather than a
            menu — everything else stays a keystroke away in the Command Center.
          </p>

          <Steps>
            <li>
              On the Shortcuts tab, turn on <b>Show in staff</b> for the ones you reach for
              daily.
              <Action>
                <Button size="sm" tone="primary" onClick={() => goTo("shortcuts")}>
                  Open the Shortcuts tab
                </Button>
              </Action>
            </li>
            <li>Drag the staff anywhere on screen; Caduceus remembers where you left it.</li>
            <li>
              Not using it? Turn the staff off in General — the hotkey and the menu-bar icon
              still open everything.
            </li>
          </Steps>

          <Callout tone={onStaff > STAFF_POPOUT_LIMIT ? "warn" : "info"}>
            {onStaff === 0
              ? "Nothing is on the staff right now, so hovering it will not fan anything out."
              : onStaff > STAFF_POPOUT_LIMIT
                ? `${onStaff} shortcuts are flagged for the staff but only the first ${STAFF_POPOUT_LIMIT} are drawn. Reorder them on the Shortcuts tab to choose which.`
                : `${onStaff} of ${STAFF_POPOUT_LIMIT} slots in use.`}
          </Callout>
        </>
      );
    },
  },

  {
    id: "voice",
    icon: "◍",
    title: "Talk instead of typing",
    blurb: "F1 or double-click the staff to dictate; hold a key if you prefer push-to-talk.",
    done: (s) => s.voice.enabled,
    body: ({ draft, settings, goTo }) => (
      <>
        <p>
          Dictation uses AVAudioEngine and local Parakeet transcription on Apple Silicon. Press{" "}
          <Kbd>F1</Kbd> or double-click the staff to start and stop, or hold your
          push-to-talk key. Keyword groups route the transcript: “search” to the web,
          “clipboard” to history, and so on.
        </p>

        <Steps>
          <li>
            Turn voice on.
            <Action>
              <Button
                size="sm"
                tone="primary"
                disabled={settings.voice.enabled}
                onClick={() => draft.update((d) => (d.voice.enabled = true))}
              >
                {settings.voice.enabled ? "Voice is on" : "Turn on voice"}
              </Button>
            </Action>
          </li>
          <li>
            Press <Kbd>F1</Kbd> or double-click the staff and speak (tap again to stop). Or hold{" "}
            <Kbd>{settings.voice.pushToTalkHotkey || "your push-to-talk key"}</Kbd>. macOS asks
            for Microphone access the first time; if you declined, fix it here.
            <Action>
              <Button size="sm" onClick={() => void api.openSystemSettings("microphone")}>
                Open Microphone privacy
              </Button>
            </Action>
          </li>
          <li>
            Pick where transcripts go, and choose a speech-to-text backend, on the Voice
            tab.
            <Action>
              <Button size="sm" onClick={() => goTo("voice")}>
                Open the Voice tab
              </Button>
            </Action>
          </li>
        </Steps>

        <Callout tone="info">
          The <b>Fn</b>/globe key cannot be a push-to-talk binding — macOS handles it in
          firmware and it never reaches an application. If your keyboard has{" "}
          <Kbd>F13</Kbd>–<Kbd>F20</Kbd>, those make excellent single-key holds.
        </Callout>
      </>
    ),
  },

  {
    id: "clipboard",
    icon: "❐",
    title: "Search everything you have copied",
    blurb: "Clipboard history, stored locally, searchable from the palette.",
    done: (s) => s.clipboard.enabled,
    body: ({ draft, settings, goTo }) => (
      <>
        <p>
          With history on, Caduceus keeps what you copy so you can pull back something from
          twenty copies ago. It never leaves your machine, and it can encrypt the store at
          rest.
        </p>

        <Steps>
          <li>
            Turn history on.
            <Action>
              <Button
                size="sm"
                tone="primary"
                disabled={settings.clipboard.enabled}
                onClick={() => draft.update((d) => (d.clipboard.enabled = true))}
              >
                {settings.clipboard.enabled ? "History is on" : "Turn on clipboard history"}
              </Button>
            </Action>
          </li>
          <li>
            Open the Command Center and type <Kbd>/v</Kbd> followed by anything you
            remember about what you copied.
          </li>
          <li>
            Exclude your password manager, and set how long entries live, on the Clipboard
            tab.
            <Action>
              <Button size="sm" onClick={() => goTo("clipboard")}>
                Open the Clipboard tab
              </Button>
            </Action>
          </li>
        </Steps>

        <Callout tone="warn">
          A clipboard history is a log of everything you copy, passwords included. Add the
          apps you do not want recorded to the exclusion list before you leave this on for
          long.
        </Callout>
      </>
    ),
  },

  {
    id: "ai",
    icon: "✳",
    title: "Set up AI",
    blurb: "Optional. Everything except / and /c already works without it.",
    done: (s) => s.agents.primaryBackendId !== null,
    body: ({ settings, goTo }) => {
      const connected = settings.agents.backends.find(
        (b) => b.id === settings.agents.primaryBackendId,
      );
      return (
        <>
          <p>
            Everything so far — the staff, prefixes, clipboard, launching apps, the system
            monitor, the calculator — runs with no API key and no network. A model only adds
            the <Kbd>/</Kbd> and <Kbd>/c</Kbd> prefixes. There are three ways to get one, in
            increasing order of effort.
          </p>

          <Steps>
            <li>
              <b>Let Caduceus find one.</b> The AI tab has a <b>Scan this Mac</b> button that
              checks the default ports for Ollama, LM Studio, llama.cpp, Jan and vLLM, and
              asks Hermes Agent whether it is configured. If you already run any of them,
              this is one click and you are done.
              <Action>
                <Button size="sm" tone="primary" onClick={() => goTo("ai")}>
                  Open the AI tab
                </Button>
              </Action>
            </li>
            <li>
              <b>Install the whole local stack.</b> The Caduceus site has a one-command
              installer that sets up Ollama, Hermes and the models, and wires{" "}
              <Kbd>/</Kbd> and <Kbd>/c</Kbd> for you. Nothing leaves your machine and there
              is no key to buy.
              <Action>
                <Button size="sm" onClick={() => void api.openExternalUrl(DOCS_CONFIGURE_AI)}>
                  Configure-AI guide (web)
                </Button>
              </Action>
            </li>
            <li>
              <b>Point it at a cloud provider.</b> Any OpenAI-compatible endpoint works — add
              it under <i>Advanced: direct model endpoint</i> on the AI tab and paste the key.
              Keys go to your OS keychain, never to a config file, and there is no command to
              read one back out, so a compromised webview cannot exfiltrate them.
            </li>
            <li>
              Whichever route you take, one backend has to be marked <b>primary</b>. That is
              the one <Kbd>/</Kbd> talks to. <Kbd>/c</Kbd> uses the computer-use backend,
              which should be Hermes if you want it to actually drive the screen.
            </li>
          </Steps>

          {connected ? (
            <Callout tone="positive" title="Connected">
              <Kbd>/</Kbd> is talking to <b>{connected.displayName || connected.id}</b>
              {connected.model ? ` (${connected.model})` : ""}.
              {settings.agents.computerUseBackendId
                ? " Computer use is set up too."
                : " Computer use has no backend yet, so /c will not work."}
            </Callout>
          ) : (
            <Callout tone="info">
              Nothing is connected yet, so <Kbd>/</Kbd> will tell you to set a model up rather
              than answering. Everything else in Caduceus is unaffected.
            </Callout>
          )}
        </>
      );
    },
  },
];

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

export function LearnTab({
  draft,
  focus,
  onNavigate,
}: {
  draft: Draft;
  /** Set when another tab deep-linked here; that entry opens on arrival. */
  focus: TutorialId | null;
  onNavigate: (tab: string) => void;
}) {
  const [open, setOpen] = useState<TutorialId | null>(focus);
  const focused = useRef<HTMLDivElement>(null);

  // A deep link can arrive while this tab is already mounted.
  useEffect(() => {
    if (focus) setOpen(focus);
  }, [focus]);

  useEffect(() => {
    if (focus && focused.current) {
      focused.current.scrollIntoView({ block: "start", behavior: "smooth" });
    }
  }, [focus]);

  const settings = draft.settings;
  if (!settings) return null;

  const ctx: Ctx = { draft, settings, goTo: onNavigate };
  const remaining = TUTORIALS.filter((t) => !t.done(settings)).length;

  return (
    <>
      <div className="mb-6">
        <h2 className="text-[15px] font-semibold tracking-[-0.01em] text-ink">Learn</h2>
        <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
          {remaining === 0
            ? "Everything here is set up. They are worth a second read anyway — each one explains why it works the way it does."
            : `Short walkthroughs that set things up as they explain them. ${remaining} of ${TUTORIALS.length} left to do.`}
        </p>
      </div>

      <div className="space-y-2.5">
        {TUTORIALS.map((tutorial) => {
          const isOpen = open === tutorial.id;
          const isDone = tutorial.done(settings);

          return (
            <div
              key={tutorial.id}
              ref={focus === tutorial.id ? focused : undefined}
              className={cx(
                "rounded-cad border bg-surface/50 transition-colors duration-150",
                isOpen ? "border-accent/40" : "border-line",
              )}
            >
              <button
                type="button"
                onClick={() => setOpen(isOpen ? null : tutorial.id)}
                className="no-drag flex w-full items-center gap-3.5 px-5 py-4 text-left"
              >
                <span
                  aria-hidden="true"
                  className={cx(
                    "flex h-8 w-8 shrink-0 items-center justify-center rounded-full border text-[13px]",
                    isDone
                      ? "border-positive/30 bg-positive/10 text-positive"
                      : "border-line bg-raised text-ink-faint",
                  )}
                >
                  {isDone ? "✓" : tutorial.icon}
                </span>

                <span className="min-w-0 flex-1">
                  <span className="block text-[13px] font-medium text-ink">{tutorial.title}</span>
                  <span className="mt-0.5 block text-2xs leading-relaxed text-ink-faint">
                    {tutorial.blurb}
                  </span>
                </span>

                <span
                  aria-hidden="true"
                  className={cx(
                    "shrink-0 text-ink-faint transition-transform duration-150",
                    isOpen && "rotate-90",
                  )}
                >
                  ›
                </span>
              </button>

              {isOpen && (
                <div className="space-y-4 border-t border-line px-5 pb-5 pt-4 text-[13px] leading-relaxed text-ink-mute [&>p]:max-w-prose">
                  {tutorial.body(ctx)}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </>
  );
}

// ---------------------------------------------------------------------------
// Bits shared by the tutorial bodies
// ---------------------------------------------------------------------------

function Steps({ children }: { children: ReactNode }) {
  return (
    <ol className="list-decimal space-y-3 pl-5 marker:text-ink-faint [&_b]:font-medium [&_b]:text-ink-soft">
      {children}
    </ol>
  );
}

/** Spacing wrapper so a step's button never sits flush against its text. */
function Action({ children }: { children: ReactNode }) {
  return <div className="mt-2">{children}</div>;
}
