/**
 * Help: the two walkthroughs, and every doc link in one place.
 *
 * The short one is the first-run overlay, replayable on demand — it lives on
 * the staff because it points at the staff, so this tab just re-arms it and
 * hands focus back. The long one ("Still confused?") is a reference that covers
 * every feature and every settings tab, and is deliberately *here* rather than
 * on the website: the answer to "what does this setting do" should not require
 * a network connection.
 */

import { useState } from "react";

import * as api from "@/shared/api";
import {
  DOCS_CONFIGURE_AI,
  DOCS_FEATURES,
  DOCS_GUIDE,
  DOCS_HOME,
  DOCS_INSTALL,
  DOCS_ISSUES,
  DOCS_SOURCE,
  SUPPORT_EMAIL,
  SUPPORT_MAILTO,
} from "@/shared/docsUrls";
import { commandCenterKey, hotkeyLabel, toggleStaffKey } from "@/shared/hotkeyLabel";
import type { RuntimeInfo } from "@/shared/types";
import { countCommands, countFeatures } from "@/shared/featuresCatalog";
import { Button, Callout, Kbd, Section, cx } from "@/shared/ui";

import { UninstallSection } from "../UninstallSection";
import type { Draft } from "../useDraft";

export function HelpTab({
  draft,
  info,
  onNavigate,
}: {
  draft: Draft;
  info: RuntimeInfo | null;
  onNavigate: (tab: string) => void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const settings = draft.settings;
  if (!settings) return null;

  const platform = info?.platform ?? "macos";
  const ccKey = hotkeyLabel(commandCenterKey(settings), platform);
  const staffKey = hotkeyLabel(toggleStaffKey(settings), platform);

  const replay = () => {
    draft.update((d) => (d.general.onboardingDone = false));
  };

  const retakeQuiz = () => {
    draft.update((d) => {
      d.general.onboardingQuizDone = false;
    });
  };

  return (
    <>
      <Section
        title="Feature checklist"
        description="Everything Caduceus does, explained — the same catalogue as on the website."
      >
        <div className="rounded-lg border border-line bg-base/20 px-3.5 py-3">
          <p className="text-[13px] font-medium text-ink">All features</p>
          <p className="mt-1 text-2xs leading-relaxed text-ink-mute">
            {countFeatures()} capabilities, of which {countCommands()} are commands you can run from
            the Command Center. Grouped, searchable, and readable offline.
          </p>
          <Button
            size="sm"
            tone="primary"
            className="mt-3"
            onClick={() => onNavigate("features")}
          >
            Open Features tab
          </Button>
        </div>
      </Section>

      <Section
        title="Walkthroughs"
        description="Two of them: a two-minute tour on the staff itself, and a full reference below."
      >
        <div className="space-y-2">
          <div className="row items-start justify-between gap-4 rounded-lg border border-line bg-base/20 px-3.5 py-3">
            <div className="min-w-0">
              <p className="text-[13px] font-medium text-ink">The two-minute tour</p>
              <p className="mt-1 text-2xs leading-relaxed text-ink-mute">
                The first-run overlay: hover the staff, open the Command Center, use the
                shortcut, then what the search bar understands. Each step waits for you to
                actually do it.
              </p>
            </div>
            <Button size="sm" tone="primary" onClick={replay}>
              {settings.general.onboardingDone ? "Replay" : "Showing now"}
            </Button>
          </div>

          {!settings.general.onboardingDone && (
            <Callout tone="info">
              It is running on the staff right now. If the staff is hidden, press{" "}
              {staffKey ? <Kbd>{staffKey}</Kbd> : "the toggle key"} or use the menu-bar icon.
            </Callout>
          )}

          <div className="row items-start justify-between gap-4 rounded-lg border border-line bg-base/20 px-3.5 py-3">
            <div className="min-w-0">
              <p className="text-[13px] font-medium text-ink">Personalization quiz</p>
              <p className="mt-1 text-2xs leading-relaxed text-ink-mute">
                Three quick questions: developer or not, what you use your Mac for, and which
                features you wanted to try. Answers update your Favorites section and how commands
                rank in search.
              </p>
            </div>
            <Button size="sm" tone="default" onClick={retakeQuiz}>
              {settings.general.onboardingQuizDone === false ? "Showing now" : "Retake"}
            </Button>
          </div>

          {settings.general.onboardingQuizDone === false && (
            <Callout tone="info">
              The quiz is on the staff now. Show the staff to answer it before the two-minute tour.
            </Callout>
          )}
        </div>
      </Section>

      <Section
        title="Still confused?"
        description="Every feature and every settings tab, in the order you are likely to meet them."
      >
        <div className="space-y-2">
          {topics({ ccKey, staffKey }).map((topic) => {
            const isOpen = open === topic.id;
            return (
              <div
                key={topic.id}
                className={cx(
                  "rounded-lg border bg-base/20 transition-colors",
                  isOpen ? "border-accent/40" : "border-line",
                )}
              >
                <button
                  type="button"
                  onClick={() => setOpen(isOpen ? null : topic.id)}
                  className="flex w-full items-center gap-3 px-3.5 py-2.5 text-left"
                >
                  <span
                    aria-hidden="true"
                    className={cx(
                      "shrink-0 text-ink-faint transition-transform duration-150",
                      isOpen && "rotate-90",
                    )}
                  >
                    ›
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-[13px] font-medium text-ink">{topic.title}</span>
                    <span className="mt-0.5 block text-2xs text-ink-faint">{topic.blurb}</span>
                  </span>
                  {topic.tab && (
                    <span
                      role="button"
                      tabIndex={0}
                      onClick={(e) => {
                        e.stopPropagation();
                        onNavigate(topic.tab!);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.stopPropagation();
                          onNavigate(topic.tab!);
                        }
                      }}
                      className="shrink-0 rounded-md px-2 py-0.5 text-2xs text-ink-mute transition-colors hover:bg-raised hover:text-ink"
                    >
                      Open tab
                    </span>
                  )}
                </button>

                {isOpen && (
                  <div className="space-y-2.5 border-t border-line px-3.5 pb-3.5 pt-3 text-2xs leading-relaxed text-ink-mute">
                    {topic.body}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </Section>

      <Section title="Documentation" description="Everything published about Caduceus.">
        <div className="grid grid-cols-2 gap-2">
          {[
            { label: "Full docs", detail: "User guide + developer reference", url: DOCS_GUIDE },
            { label: "Overview", detail: "What Caduceus is", url: DOCS_HOME },
            { label: "All features", detail: "The complete list", url: DOCS_FEATURES },
            { label: "Configure AI", detail: "Local models and cloud keys", url: DOCS_CONFIGURE_AI },
            { label: "Install", detail: "One-liner, bundle, or .dmg", url: DOCS_INSTALL },
            { label: "Source", detail: "GitHub, MIT licensed", url: DOCS_SOURCE },
          ].map((doc) => (
            <button
              key={doc.url}
              type="button"
              onClick={() => void api.openExternalUrl(doc.url)}
              className="rounded-lg border border-line bg-base/20 px-3 py-2.5 text-left transition-colors hover:border-accent/40"
            >
              <span className="row text-[13px] font-medium text-ink">
                {doc.label}
                <span aria-hidden="true" className="text-2xs text-ink-faint">
                  ↗
                </span>
              </span>
              <span className="mt-0.5 block text-2xs text-ink-faint">{doc.detail}</span>
            </button>
          ))}
        </div>
      </Section>

      <Section
        title="Contact"
        description="Bug reports, questions, or feedback."
      >
        <p className="text-[13px] leading-relaxed text-ink-mute">
          Email{" "}
          <button
            type="button"
            className="font-medium text-accent underline decoration-accent/40 underline-offset-2"
            onClick={() => void api.openExternalUrl(SUPPORT_MAILTO)}
          >
            {SUPPORT_EMAIL}
          </button>{" "}
          — or file an issue on GitHub if something is broken and you can describe steps to
          reproduce.
        </p>
        <div className="row mt-3">
          <Button size="sm" onClick={() => void api.openExternalUrl(SUPPORT_MAILTO)}>
            Email support
          </Button>
          <Button size="sm" onClick={() => void api.openExternalUrl(DOCS_ISSUES)}>
            GitHub issues
          </Button>
        </div>
      </Section>

      <UninstallSection />

      <Section title="This install">
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-[13px]">
          <dt className="text-ink-faint">Command Center</dt>
          <dd className="text-ink-soft">{ccKey ? <Kbd>{ccKey}</Kbd> : "not bound"}</dd>
          <dt className="text-ink-faint">Show / hide staff</dt>
          <dd className="text-ink-soft">{staffKey ? <Kbd>{staffKey}</Kbd> : "not bound"}</dd>
          <dt className="text-ink-faint">Version</dt>
          <dd className="text-ink-soft">{info?.version ?? "—"}</dd>
        </dl>
        <p className="mt-3 text-2xs leading-relaxed text-ink-faint">
          If a shortcut here is not the one you set, another app was already holding it and
          Caduceus moved to a free alternative rather than leaving a dead key. Change it on the
          General tab.
        </p>
      </Section>
    </>
  );
}

// ---------------------------------------------------------------------------
// The long walkthrough
// ---------------------------------------------------------------------------

interface Topic {
  id: string;
  title: string;
  blurb: string;
  /** Settings tab this is about, if any. */
  tab?: string;
  body: React.ReactNode;
}

function topics({ ccKey, staffKey }: { ccKey: string; staffKey: string }): Topic[] {
  const CC = ccKey ? <Kbd>{ccKey}</Kbd> : <span>your Command Center key</span>;

  return [
    {
      id: "staff",
      title: "The staff",
      blurb: "The floating mark, and the ring of shortcuts around it",
      tab: "general",
      body: (
        <>
          <p>
            The staff floats above every window and every space, including another app's
            full-screen. Hover it and up to six shortcuts fan out on an arc; click one to run
            it. Drag the staff anywhere — it remembers where you left it, and the arc flips
            side depending on which half of the screen it is on.
          </p>
          <p>
            A single click opens the Command Center. A double-click starts dictation. Right-click
            opens Settings. {staffKey ? <>Press {<Kbd>{staffKey}</Kbd>} to hide or show it.</> : null}
          </p>
          <p>
            <b>General</b> controls the docked edge, how long the pointer must rest before the
            ring opens, how long before it folds back, and whether the staff is shown at all —
            Caduceus works fine without it.
          </p>
        </>
      ),
    },
    {
      id: "command-center",
      title: "The Command Center",
      blurb: "One bar that searches, launches, calculates and routes",
      tab: "command-center",
      body: (
        <>
          <p>Press {CC} anywhere. What you type is interpreted in this order:</p>
          <ul className="list-disc space-y-1 pl-5">
            <li>
              <b>An app name</b> — <Kbd>chrome</Kbd> launches it. Fuzzy, so <Kbd>chrm</Kbd> works.
            </li>
            <li>
              <b>Maths</b> — <Kbd>2+2</Kbd>, <Kbd>18% of 240</Kbd>. Answers inline, Enter copies.
            </li>
            <li>
              <b>A prefix</b> — see below. The longest match wins.
            </li>
            <li>
              <b>Anything else</b> — searches the web with your configured engine.
            </li>
          </ul>
          <p>
            Drag the bottom-right corner to resize; double-click that grip to hide. Escape closes.
          </p>
        </>
      ),
    },
    {
      id: "prefixes",
      title: "Prefixes",
      blurb: "/ for AI, /v for clipboard, /c for screen control — and your own",
      tab: "command-center",
      body: (
        <>
          <p>
            A prefix routes the rest of the line somewhere specific. Three ship by default:{" "}
            <Kbd>/</Kbd> asks your AI backend, <Kbd>/v</Kbd> searches clipboard history, and{" "}
            <Kbd>/c</Kbd> hands the task to an agent that can drive your screen.
          </p>
          <p>
            You can add your own on the Command Center tab. A prefix can open a URL template
            (<code>{"{query}"}</code> is where your text lands), run a shell command, run
            AppleScript, or point at any of the built-in routes. The longest matching prefix
            wins, so <Kbd>/c</Kbd> beats <Kbd>/</Kbd> regardless of list order.
          </p>
        </>
      ),
    },
    {
      id: "shortcuts",
      title: "Shortcuts",
      blurb: "The one primitive behind the staff ring and the palette",
      tab: "shortcuts",
      body: (
        <>
          <p>
            A shortcut opens a URL, launches an app, runs a command, runs AppleScript, or opens
            one of the built-in views. The same list powers the staff's ring and the Command
            Center's results, which is why everything is editable — including the ones Caduceus
            ships with.
          </p>
          <p>
            <b>Show in staff</b> puts it on the ring; the ring draws six, and Settings warns
            rather than silently dropping the rest. <b>Keywords</b> are extra words that should
            match it in search. <b>Open in</b> sends a URL to a specific browser and profile,
            overriding the Command Center default.
          </p>
        </>
      ),
    },
    {
      id: "system",
      title: "System monitor",
      blurb: "CPU, memory, disks, network — and quitting things",
      body: (
        <>
          <p>
            On the staff ring, or type <Kbd>system</Kbd> in the Command Center. Live CPU and
            memory, per-disk free space, network throughput, uptime and load average, plus the
            heaviest processes sorted by CPU or memory.
          </p>
          <p>
            Hover a row for <b>Quit</b> (asks politely) and <b>Force</b> (does not). Processes
            you do not own show "system" instead — they need privileges Caduceus does not have,
            and a button that always fails is worse than no button.
          </p>
        </>
      ),
    },
    {
      id: "voice",
      title: "Voice and dictation",
      blurb: "Hold to talk, or double-tap the staff",
      tab: "voice",
      body: (
        <>
          <p>
            Push-to-talk records while you hold the key; F1 and double-clicking the staff toggle
            it instead. A red dot appears on the staff while the microphone is live, so it is
            never ambiguous.
          </p>
          <p>
            What happens to the transcript is decided by <b>keyword groups</b>: say "search" and
            it goes to the web, "clipboard" and it searches your history. Anything that matches
            nothing falls through to the fallback route, which is your AI model by default.
          </p>
          <p>
            The first run asks for Microphone and Speech Recognition. If you said no, the
            Learn tab can reopen those panes.
          </p>
        </>
      ),
    },
    {
      id: "clipboard",
      title: "Clipboard history",
      blurb: "Everything you copied, searchable",
      tab: "clipboard",
      body: (
        <>
          <p>
            Off by default. Once on, <Kbd>/v</Kbd> searches everything you have copied — text,
            images and file paths, with pinning for the ones you want to keep.
          </p>
          <p>
            It never leaves your machine and can be encrypted at rest. Add your password manager
            to the exclusion list: a clipboard history is a log of every password you copy.
          </p>
        </>
      ),
    },
    {
      id: "ai",
      title: "AI",
      blurb: "Optional — everything but / and /c works without it",
      tab: "ai",
      body: (
        <>
          <p>
            <b>Scan this Mac</b> checks the default ports for Ollama, LM Studio, llama.cpp, Jan
            and vLLM, and asks Hermes Agent whether it is configured. One click connects what it
            finds.
          </p>
          <p>
            Otherwise add any OpenAI-compatible endpoint by hand, cloud or local. Keys go to your
            OS keychain, never to a config file, and there is no command to read one back out.
          </p>
          <p>
            One backend is <b>primary</b> — that is what <Kbd>/</Kbd> talks to. <Kbd>/c</Kbd>{" "}
            uses the computer-use backend, which should be Hermes if you want it to actually
            control the screen. Leave <b>Ask before an agent controls this Mac</b> on.
          </p>
        </>
      ),
    },
    {
      id: "keys",
      title: "Hotkeys and function keys",
      blurb: "What is bound, and what happens when something else holds it",
      tab: "general",
      body: (
        <>
          <p>
            Two dedicated accelerators — Command Center and push-to-talk — plus the F1–F20 table,
            which is the single place function keys are configured. F12 shows and hides the staff
            out of the box.
          </p>
          <p>
            If another app already holds a key, Caduceus moves that action to a free alternative
            at startup and saves the change, rather than leaving a shortcut that silently does
            nothing. You will see a note at the top of Settings when that happens, and the Help
            tab always shows what is actually bound.
          </p>
          <p>
            On a Mac, set <i>Keyboard → Keyboard Shortcuts → Function Keys</i> to use F-keys as
            standard function keys if you want them to reach Caduceus globally.
          </p>
        </>
      ),
    },
    {
      id: "appearance",
      title: "Appearance",
      blurb: "Theme, accent, and the size of everything",
      tab: "appearance",
      body: (
        <p>
          Light, dark or follow the system, plus the accent colour used across all three windows.
          The staff's size, how far the ring reaches, icon size, and how faint it goes when idle
          are all adjustable — turn idle opacity up if you keep losing it on a busy desktop.
        </p>
      ),
    },
  ];
}
