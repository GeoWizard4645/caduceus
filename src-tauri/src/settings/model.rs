//! The complete Caduceus configuration schema.
//!
//! # Design rules for this file
//!
//! 1. **Everything is data.** Nothing about Caduceus's behaviour is hardcoded in
//!    logic if a user could plausibly want it different. Prefixes, keyword
//!    routes, API endpoints, beta headers, tool-version strings and search URLs
//!    all live here so they can be changed from Settings without recompiling.
//! 2. **Every field has a `Default`.** A brand-new install must be fully usable
//!    with zero configuration and zero API keys.
//! 3. **No secrets.** This struct is serialised to plain JSON on disk. API keys
//!    live in the OS keychain and are referenced here only by an opaque handle
//!    (see [`secrets`](super::secrets)).
//! 4. **Additive changes only.** Every struct is `#[serde(default)]` so an older
//!    config file keeps working after an upgrade; see [`Settings::CURRENT_VERSION`].

use serde::{Deserialize, Serialize};

use crate::shortcuts::Shortcut;

/// Root configuration object, persisted as one JSON blob via `tauri-plugin-store`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Schema version, for future migrations. Bump only on breaking changes.
    pub version: u32,
    pub general: GeneralSettings,
    pub shortcuts: Vec<Shortcut>,
    pub command_center: CommandCenterSettings,
    pub voice: VoiceSettings,
    pub agents: AgentSettings,
    pub clipboard: ClipboardSettings,
    pub appearance: AppearanceSettings,
    pub update: UpdateSettings,
    /// Reaching the agent from outside the desktop — see `crate::gateway`'s
    /// module doc. Only non-secret configuration lives here; the bot token
    /// itself is in the OS keychain (rule 3 above) via
    /// `secrets::telegram_bot_token`.
    pub gateway: GatewaySettings,
}

impl Settings {
    /// Bump only when `migrate` needs to know a config predates a change.
    ///
    /// v2: `launch_at_login` became true by default. The version gate is what
    /// makes the migration run once — a user who turns it back off afterwards
    /// must stay off.
    pub const CURRENT_VERSION: u32 = 2;
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            general: GeneralSettings::default(),
            shortcuts: crate::shortcuts::default_shortcuts(),
            command_center: CommandCenterSettings::default(),
            voice: VoiceSettings::default(),
            agents: AgentSettings::default(),
            clipboard: ClipboardSettings::default(),
            appearance: AppearanceSettings::default(),
            update: UpdateSettings::default(),
            gateway: GatewaySettings::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneralSettings {
    /// Global hotkey that shows/hides the floating staff.
    ///
    /// Uses the `tauri-plugin-global-shortcut` accelerator syntax, e.g.
    /// `"F12"`, `"CommandOrControl+Shift+O"`. An empty string disables it.
    pub toggle_orb_hotkey: String,
    /// Global hotkey that opens the Command Center directly, bypassing the staff.
    pub command_center_hotkey: String,
    /// Whether the staff is currently shown. Persisted so the choice survives
    /// restarts.
    pub staff_visible: bool,
    /// Which screen edge the staff snaps to when it has no saved position.
    pub staff_edge: StaffEdge,
    /// Last dragged position in *physical* pixels. `None` = use `staff_edge`.
    pub staff_position: Option<Point>,
    /// Delay before the pop-out expands after the pointer reaches the staff.
    /// `0` means "immediately", which is the documented default behaviour.
    pub hover_expand_delay_ms: u64,
    /// How long the pointer must be away from the staff before the pop-out
    /// collapses again. Settings UI constrains this to 500–10000ms.
    pub collapse_idle_ms: u64,
    /// Register Caduceus as a login item.
    pub launch_at_login: bool,
    /// Poll rate of the global cursor tracker that drives staff hover/collapse.
    /// Lower is more responsive, higher is cheaper. 16–100ms is sane.
    pub cursor_poll_ms: u64,
    /// Whether the first-run walkthrough has been finished or dismissed.
    ///
    /// Lives in settings rather than a marker file so "show me that again" is a
    /// checkbox rather than a support question about deleting hidden state.
    pub onboarding_done: bool,
    /// Whether the three-question personalization quiz has been completed.
    pub onboarding_quiz_done: bool,
    /// Answers from the quiz — drives favorites and ranking nudges.
    pub personalization: PersonalizationProfile,
    /// Per-key actions for `F1`–`F12` (and `F13`–`F20` when present). macOS
    /// users often remap hardware keys to standard function keys in System
    /// Settings so Caduceus can intercept them globally.
    pub function_keys: Vec<FunctionKeyBinding>,
    /// The version of Caduceus that last started with this settings file.
    ///
    /// Settings survive a reinstall — the app data directory is not part of the
    /// bundle — so someone who had hidden the staff months ago installs a new
    /// version, sees nothing at all, and reasonably concludes it is broken.
    /// A version that does not match brings the staff back once, so a fresh
    /// install always looks like something happened. See `lib.rs`.
    pub last_launched_version: Option<String>,
}

/// Default accelerator for [`GeneralSettings::command_center_hotkey`].
///
/// Pulled out to a named constant rather than left as a literal in
/// [`Default for GeneralSettings`](struct.GeneralSettings.html) so `tray.rs`'s
/// menu label can fall back to the same string instead of a second, easily
/// stale copy — a tray menu advertising a hotkey the app does not actually
/// respond to is worse than advertising none.
///
/// # Why not `Control+Space`
///
/// It used to be, on the stated reasoning that Spotlight holds `Cmd+Space` and
/// therefore `Ctrl+Space` was free. It is not: `Ctrl+Space` is macOS's *select
/// the previous input source* shortcut, shipped enabled, and on any Mac with
/// more than one input source the system consumes the key before any
/// application sees it. `RegisterEventHotKey` still returns success — this is
/// the same trap `hotkeys::SYSTEM_RESERVED` exists to document — so the app
/// looked correctly configured while its one entry point silently did nothing.
///
/// `Option+Space` is genuinely unbound on a stock install, and it is what
/// Spotlight-alikes (Raycast, Alfred) use, so it is also the combination a
/// user most likely expects to try first.
pub const DEFAULT_COMMAND_CENTER_HOTKEY: &str = "Alt+Space";

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            // Empty by default: F12 toggles the staff via the function-key
            // table below, so there is exactly one place F-keys are configured.
            // This field still exists for non-F-key accelerators like ⌥⇧S.
            toggle_orb_hotkey: String::new(),
            // See the constant's doc for why this is Option+Space and not
            // Control+Space, which macOS itself claims for input-source
            // switching.
            command_center_hotkey: DEFAULT_COMMAND_CENTER_HOTKEY.into(),
            staff_visible: true,
            staff_edge: StaffEdge::Right,
            staff_position: None,
            hover_expand_delay_ms: 0,
            collapse_idle_ms: 50,
            // On by default: the hotkeys and the staff only exist while the
            // process runs, and a launcher that is not running when you press
            // its hotkey has failed at the one moment it was needed. This is
            // how Raycast and Spotlight behave. It is a visible checkbox in
            // Settings → General, and turning it off is respected permanently
            // (see the v2 migration).
            launch_at_login: true,
            cursor_poll_ms: 33,
            onboarding_done: false,
            onboarding_quiz_done: false,
            personalization: PersonalizationProfile::default(),
            function_keys: default_function_key_bindings(),
            // `None` means "never started" — which is true both of a genuinely
            // fresh install and of one upgrading from before this field
            // existed. Both want the staff on screen.
            last_launched_version: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Personalization (first-run quiz)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct PersonalizationProfile {
    pub is_developer: bool,
    /// `launcher`, `clipboard`, `windows`, `system`, `ai`, or `developer`.
    pub primary_focus: String,
    /// Command ids (no `command:` prefix) the user picked in the quiz.
    pub favorite_command_ids: Vec<String>,
}

/// Keys exposed in Settings → General → Function keys.
pub const FUNCTION_KEY_LABELS: &[&str] = &[
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
    "F16", "F17", "F18", "F19", "F20",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct FunctionKeyBinding {
    pub key: String,
    pub action: FunctionKeyAction,
    /// When `action` is [`FunctionKeyAction::RunShortcut`], which shortcut to run.
    pub shortcut_id: String,
}

impl Default for FunctionKeyBinding {
    fn default() -> Self {
        Self {
            key: String::new(),
            action: FunctionKeyAction::None,
            shortcut_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKeyAction {
    #[default]
    None,
    ToggleStaff,
    CommandCenter,
    /// Same hold-to-record behaviour as Voice → push-to-talk.
    PushToTalk,
    /// Tap again (or press F1) to stop. Uses AVAudioEngine + on-device Speech on macOS.
    StartDictation,
    /// Open Voice Memos and start a new recording (macOS only).
    VoiceMemo,
    Screenshot,
    RunShortcut,
}

pub fn default_function_key_bindings() -> Vec<FunctionKeyBinding> {
    FUNCTION_KEY_LABELS
        .iter()
        .map(|label| FunctionKeyBinding {
            key: (*label).into(),
            action: match *label {
                "F1" => FunctionKeyAction::StartDictation,
                "F3" => FunctionKeyAction::VoiceMemo,
                // Was a separate `toggle_orb_hotkey` field. Living here means
                // F1-F20 have exactly one place they are configured.
                "F12" => FunctionKeyAction::ToggleStaff,
                _ => FunctionKeyAction::None,
            },
            shortcut_id: String::new(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StaffEdge {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

// ---------------------------------------------------------------------------
// Command Center
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommandCenterSettings {
    /// URL template used for the no-prefix "just search" path. `{query}` is
    /// replaced with the percent-encoded input. Any engine works — this is a
    /// plain text field, not a dropdown of blessed providers.
    pub search_url_template: String,
    /// Ordered prefix rules. The *longest* matching prefix wins, so `/c` beats
    /// `/` regardless of list order.
    pub prefixes: Vec<PrefixRule>,
    /// Browser and profile used for URLs that don't specify their own (e.g. the
    /// plain web-search path). Defaults to the OS default browser.
    pub browser: crate::shortcuts::BrowserChoice,
    /// How many recent commands to keep for the history view.
    pub history_limit: usize,
    /// Close the Command Center as soon as an action is dispatched.
    pub close_on_action: bool,
    /// Max result rows rendered per source in the palette.
    pub max_results_per_source: usize,
}

impl Default for CommandCenterSettings {
    fn default() -> Self {
        Self {
            search_url_template: "https://www.google.com/search?q={query}".into(),
            prefixes: default_prefixes(),
            browser: crate::shortcuts::BrowserChoice::default(),
            history_limit: 100,
            close_on_action: true,
            max_results_per_source: 8,
        }
    }
}

/// A prefix rule is just "when the input starts with X, do Y with the rest".
///
/// This is the same primitive the staff shortcuts use, which is why users can add
/// arbitrary prefixes that open URLs or run commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrefixRule {
    pub id: String,
    /// The literal token typed at the start of the input, e.g. `/` or `/c`.
    /// Matched case-sensitively and must be followed by a space or end of input.
    pub prefix: String,
    pub label: String,
    pub description: String,
    pub action: PrefixAction,
    /// Meaning depends on `action`: a URL template for `OpenUrlTemplate`,
    /// a shell command for `RunCommand`, ignored for the built-in routes.
    pub target: String,
    /// Open this rule's URL in a specific browser/profile instead of the
    /// Command Center default. Only meaningful for `OpenUrlTemplate`.
    pub browser: Option<crate::shortcuts::BrowserChoice>,
    /// Show this rule as a hint row when the palette is empty.
    pub show_hint: bool,
}

impl Default for PrefixRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            prefix: String::new(),
            label: String::new(),
            description: String::new(),
            action: PrefixAction::WebSearch,
            target: String::new(),
            browser: None,
            show_hint: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixAction {
    /// Open `search_url_template` in the browser.
    WebSearch,
    /// Send to the configured *primary* agent backend as a chat message.
    PrimaryAi,
    /// Start a computer-use agent session with the text as the task.
    ComputerUse,
    /// Open `target` as a URL template, substituting `{query}`.
    OpenUrlTemplate,
    /// Run `target` as a shell command, substituting `{query}`.
    RunCommand,
    /// Run `target` as AppleScript, substituting `{query}` (macOS only).
    RunAppleScript,
    /// Show clipboard history filtered by the remaining text.
    ClipboardSearch,
}

/// Ships three prefixes matching the documented defaults. All are editable and
/// deletable; none are special-cased anywhere in the routing code.
pub fn default_prefixes() -> Vec<PrefixRule> {
    vec![
        PrefixRule {
            id: "prefix-ai".into(),
            prefix: "/".into(),
            label: "Ask AI".into(),
            description: "Send the rest of the line to your primary AI backend".into(),
            action: PrefixAction::PrimaryAi,
            ..Default::default()
        },
        PrefixRule {
            id: "prefix-computer".into(),
            prefix: "/c".into(),
            label: "Computer use".into(),
            description: "Let an agent drive your screen to complete the task".into(),
            action: PrefixAction::ComputerUse,
            ..Default::default()
        },
        PrefixRule {
            id: "prefix-clipboard".into(),
            prefix: "/v".into(),
            label: "Clipboard".into(),
            description: "Search your clipboard history".into(),
            action: PrefixAction::ClipboardSearch,
            ..Default::default()
        },
    ]
}

// ---------------------------------------------------------------------------
// Voice
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceSettings {
    pub enabled: bool,
    /// Push-to-talk accelerator: hold to record, release to transcribe.
    ///
    /// A bare `Fn` key cannot be captured portably (on macOS it is handled in
    /// firmware and never reaches the app), and modifier-only bindings are not
    /// expressible as global shortcuts. The default is therefore a normal
    /// combo; `F13` is a good alternative on keyboards that have it.
    pub push_to_talk_hotkey: String,
    pub stt_backend: SttBackendKind,
    /// OpenAI-compatible transcription endpoint. Works with a local
    /// `whisper.cpp` server, `faster-whisper-server`, LM Studio, or OpenAI.
    pub stt_endpoint: String,
    pub stt_model: String,
    /// BCP-47 hint passed to the STT backend; empty = auto-detect.
    pub stt_language: String,
    /// Keyword groups evaluated top-to-bottom against the transcript.
    pub keyword_groups: Vec<KeywordGroup>,
    /// Where a transcript goes when no keyword group matches, if the user has
    /// deliberately chosen somewhere.
    ///
    /// `None` — the default — means "decide automatically": an unmatched
    /// transcript goes to [`RouteTarget::WebSearch`] when no usable AI backend
    /// is configured, and to [`RouteTarget::InsertOnly`] (plain Command Center
    /// search, exactly as if it had been typed) when one is.
    ///
    /// The `Option` is doing real work here. This used to be a bare
    /// `RouteTarget` defaulting to `PrimaryAi`, which made "the user never
    /// chose" and "the user chose the AI" the same value — so there was no way
    /// to vary the default by whether the AI was configured without also
    /// overriding people who had genuinely picked it. `Some(_)` is now a
    /// deliberate choice and always wins outright, including
    /// `Some(RouteTarget::PrimaryAi)`, which `None` can never mean.
    ///
    /// See `voice::router::effective_fallback` for how the two compose.
    pub fallback_route: Option<RouteTarget>,
    /// Hard cap on a single recording, so a stuck key can't fill the disk.
    pub max_recording_secs: u32,
    /// Automatically dispatch the routed action, or just fill the input and
    /// let the user press Enter. `false` is the safer default.
    pub auto_submit: bool,
    /// Open an unambiguous Command Center result on its own, once a short
    /// spoken utterance has settled — saying "Terminal" and stopping launches
    /// Terminal.
    ///
    /// This is the off switch, and it is the only part of the behaviour that is
    /// configurable. The guards around it — how short is short, how long the
    /// recogniser must be quiet, and how certain the match has to be — are
    /// deliberately not user-tunable, because loosening any one of them turns a
    /// convenience into launching the wrong application off a mis-transcription.
    /// `HomeTab.tsx`'s auto-open effect documents each guard.
    pub auto_open_short_utterance: bool,

    // ---- Text-to-speech (spoken replies) -----------------------------------
    // Named `tts_*` rather than nested in their own struct, matching how the
    // `stt_*` fields above sit directly on `VoiceSettings` — voice input and
    // voice output are the same feature area, and this file already has one
    // established way of grouping a backend choice with its config.
    /// Master switch for text-to-speech. Off by default: push-to-talk input
    /// only ever runs because a key is physically held, but spoken *output*
    /// can start on its own the instant a reply arrives — the one voice
    /// feature that must stay strictly opt-in. An app that starts talking
    /// without being asked is a bug, not a feature.
    pub tts_enabled: bool,
    pub tts_backend: TtsBackendKind,
    /// OpenAI-compatible speech endpoint, e.g.
    /// `https://api.openai.com/v1/audio/speech`, or a local server's
    /// equivalent. Always called with `response_format=wav` — see
    /// `voice::tts::OpenAiCompatibleTts` for why.
    pub tts_endpoint: String,
    pub tts_model: String,
    /// Backend-specific voice name. Empty means "the backend's own default":
    /// on macOS, whatever `say` uses with no `-v` flag; for the HTTP backend,
    /// whatever the server falls back to when `voice` is omitted from the
    /// request. `say -v ?` lists what is installed locally.
    pub tts_voice: String,
    /// Speaking rate, in whichever unit the active backend natively uses —
    /// `say -r`'s words-per-minute for the system backend, the HTTP backend's
    /// 0.25-4.0 `speed` multiplier for the other. `0.0` means "leave it at
    /// the backend's own default" rather than forcing both backends onto one
    /// shared unit that would be meaningless for at least one of them.
    pub tts_rate: f32,
    /// Speak the assistant's replies aloud automatically — the JARVIS-style
    /// behaviour this feature exists for. Kept distinct from `tts_enabled` so
    /// a Settings "preview this voice" button can call `speak` on demand
    /// without also switching on unprompted narration of every reply. Nothing
    /// in this crate reads this flag yet to decide *when* to call `speak` —
    /// that decision belongs to whatever assembles a finished reply (the chat
    /// pipeline), not to voice settings themselves; this is the switch for it
    /// to consult once it does.
    pub tts_speak_replies: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            // On by default: the recogniser is Apple's, runs on-device, and the
            // microphone only opens while the key is physically held.
            enabled: true,
            push_to_talk_hotkey: "Alt+Shift+V".into(),
            stt_backend: SttBackendKind::SystemNative,
            stt_endpoint: "http://127.0.0.1:8080/v1/audio/transcriptions".into(),
            stt_model: "whisper-1".into(),
            stt_language: String::new(),
            keyword_groups: default_keyword_groups(),
            fallback_route: None,
            max_recording_secs: 60,
            auto_submit: false,
            auto_open_short_utterance: true,
            // Off by default — see the field doc. Backend/endpoint/model are
            // still pre-filled with usable values so flipping the switch
            // alone is enough to hear something, exactly like the STT fields
            // above are pre-filled for the equivalent reason.
            tts_enabled: false,
            tts_backend: TtsBackendKind::SystemNative,
            tts_endpoint: "https://api.openai.com/v1/audio/speech".into(),
            tts_model: "tts-1".into(),
            tts_voice: String::new(),
            tts_rate: 0.0,
            tts_speak_replies: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SttBackendKind {
    /// Do not transcribe; push-to-talk is inert.
    Disabled,
    /// The OS speech recogniser (macOS `Speech.framework` via a bundled shim).
    #[default]
    SystemNative,
    /// Any OpenAI-compatible `/audio/transcriptions` endpoint.
    ///
    /// Spelled explicitly because `rename_all = "snake_case"` turns
    /// `OpenAiCompatible` into `open_ai_compatible`, and every other place this
    /// value exists — the TypeScript union, `voice/stt.rs`, the installer —
    /// writes `openai_compatible`. See [`BackendKind::OpenAiCompatible`].
    #[serde(rename = "openai_compatible", alias = "open_ai_compatible")]
    OpenAiCompatible,
}

/// Mirrors [`SttBackendKind`] on the output side — see `voice::tts` for the
/// backends themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TtsBackendKind {
    /// Do not speak anything; `speak` is inert.
    Disabled,
    /// `/usr/bin/say`, bundled with every Mac. No helper process to build or
    /// sign, no entitlement, no permission prompt — see
    /// `voice::tts::SystemNativeTts`.
    #[default]
    SystemNative,
    /// Any OpenAI-compatible `/audio/speech` endpoint.
    ///
    /// Spelled explicitly for the same reason as
    /// [`SttBackendKind::OpenAiCompatible`] — `rename_all = "snake_case"`
    /// would otherwise derive `open_ai_compatible` from the Rust casing,
    /// disagreeing with every other place this project writes the string.
    #[serde(rename = "openai_compatible", alias = "open_ai_compatible")]
    OpenAiCompatible,
}

/// How a keyword group decides whether it matches a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeywordMatch {
    /// The transcript *starts with* the keyword (after lowercasing and
    /// stripping punctuation). The keyword is then removed from the text.
    /// This is the documented default: saying "search cheap flights" searches
    /// for "cheap flights", not for "search cheap flights".
    #[default]
    LeadingWords,
    /// The keyword appears anywhere in the transcript. The text is passed
    /// through unchanged.
    Anywhere,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeywordGroup {
    pub id: String,
    pub name: String,
    /// Compared case-insensitively.
    pub keywords: Vec<String>,
    pub route: RouteTarget,
    pub match_mode: KeywordMatch,
    pub enabled: bool,
}

impl Default for KeywordGroup {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            keywords: Vec::new(),
            route: RouteTarget::PrimaryAi,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        }
    }
}

/// Where a piece of text should be sent. Shared by voice routing and by any
/// future result provider that needs to hand text to a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTarget {
    /// Open the browser search.
    WebSearch,
    /// Chat with the primary agent backend.
    PrimaryAi,
    /// Start a computer-use session.
    ComputerUse,
    /// Just put the text in the Command Center input and stop.
    InsertOnly,
    /// Search clipboard history.
    ClipboardSearch,
}

/// What a spoken phrase does, before anyone has configured anything.
///
/// The shape of it: **saying something puts it in the Command Center**, exactly
/// as if it had been typed, and three kinds of phrase announce a destination in
/// their first words — a search verb wants the web, a control verb wants the
/// machine driven, and an "ask" verb wants the AI.
///
/// Dictation types into the Command Center's search bar and the search bar
/// decides what the text means, so the useful default is the one that behaves
/// like typing rather than one that guesses. That is why the AI is a group here
/// rather than the catch-all it used to be: routing every unmatched sentence to
/// a model meant an unconfigured Caduceus sent everything nowhere, and a
/// configured one turned "invoices 2026" into a conversation. What happens when
/// nothing matches now depends on whether an AI backend is actually usable —
/// see [`VoiceSettings::fallback_route`].
///
/// Keywords are stripped when they lead, so "search the best pasta in Rome"
/// searches for *the best pasta in Rome* rather than including the instruction
/// in the query. Longest match wins across groups, which is why "control my
/// computer" beats the bare "computer" and "search my mac" beats "search".
pub fn default_keyword_groups() -> Vec<KeywordGroup> {
    vec![
        KeywordGroup {
            id: "kw-search".into(),
            name: "Web search".into(),
            keywords: vec![
                "search".into(),
                "search for".into(),
                "search the web for".into(),
                "search the internet for".into(),
                "look up".into(),
                "lookup".into(),
                "browse".into(),
                "browse for".into(),
                "google".into(),
                "bing".into(),
                "web search".into(),
                "on the web".into(),
                "on the internet".into(),
                "internet".into(),
                "web".into(),
            ],
            route: RouteTarget::WebSearch,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        },
        KeywordGroup {
            id: "kw-computer".into(),
            name: "Computer use".into(),
            keywords: vec![
                "computer use".into(),
                "computer".into(),
                "control my computer".into(),
                "control my mac".into(),
                "control the computer".into(),
                "use my computer".into(),
                "drive my mac".into(),
                "jarvis".into(),
                "search my mac".into(),
            ],
            route: RouteTarget::ComputerUse,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        },
        // The bare "ask" is last in its own list on purpose. Longest match wins
        // across every group, so "ask" cannot shadow "ask ai" here, nor
        // "search my mac" over in computer use — but it is still the loosest
        // keyword Caduceus ships, and reading it last is how anyone editing
        // this list finds that out.
        KeywordGroup {
            id: "kw-ai".into(),
            name: "Ask AI".into(),
            keywords: vec![
                "ask ai".into(),
                "ask claude".into(),
                "ask chat".into(),
                "hey caduceus".into(),
                "hey ai".into(),
                "ask".into(),
            ],
            route: RouteTarget::PrimaryAi,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSettings {
    pub backends: Vec<BackendConfig>,
    /// Backend used by the `/` prefix and the `PrimaryAi` route.
    pub primary_backend_id: Option<String>,
    /// Backend used by the `/c` prefix and the `ComputerUse` route. Must be a
    /// backend whose `supports_computer_use` is true.
    pub computer_use_backend_id: Option<String>,
    /// Ask before an agent is allowed to control the machine. Strongly
    /// recommended, and on by default: `/c` is one keystroke away from `/`.
    pub confirm_before_first_action: bool,
    /// Master switch for smart routing (`tools::routing`).
    ///
    /// `false` sends every prompt to `primary_backend_id`, which is exactly
    /// what happened before routing existed — so turning this off is a return
    /// to known behaviour rather than a different one.
    pub auto_routing_enabled: bool,
    /// A backend pinned by hand, which always beats the classifier.
    ///
    /// `None` means "let routing decide". An explicit choice must win: a user
    /// who has picked a model is answering the question routing was guessing at.
    pub routing_override_backend_id: Option<String>,
    /// Prefix conversations with the JARVIS butler persona fragment — see
    /// [`jarvis_persona_prompt`]. Off by default: the persona changes the
    /// register of every reply, which is exactly the kind of behavioural
    /// change that must be asked for rather than assumed, the same reasoning
    /// as `voice.tts_enabled`. Turning this on is independent of whether
    /// replies are also spoken aloud (`voice.tts_speak_replies`) — the two
    /// compose (persona with no speech reads like JARVIS; speech with no
    /// persona just reads replies aloud plainly) rather than one implying
    /// the other.
    pub jarvis_persona_enabled: bool,
    /// How the persona addresses the user — "sir", "ma'am", a first name,
    /// anything. Never hardcoded: Caduceus has no way to know how anyone
    /// wants to be addressed, so this is the one part of the fragment that is
    /// always a setting rather than a constant. An empty string omits the
    /// address from the example line entirely rather than leaving something
    /// stilted like "Quite well, . Ever ready".
    pub jarvis_honorific: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            backends: default_backends(),
            // Hermes is the default out of the box. If it is not installed the
            // backend says so and tells you how to get it, which is a better
            // first run than an empty dropdown.
            primary_backend_id: Some("hermes".into()),
            computer_use_backend_id: Some("hermes".into()),
            confirm_before_first_action: true,
            // On by default: the whole point is that fast work feels fast
            // without anyone having to configure it.
            auto_routing_enabled: true,
            routing_override_backend_id: None,
            // Off by default — see the field doc. "sir" is only ever used
            // once the toggle above is switched on, so pre-filling it here
            // costs a silent, inactive default rather than an unprompted one.
            jarvis_persona_enabled: false,
            jarvis_honorific: "sir".into(),
        }
    }
}

/// The JARVIS butler persona, as a system-prompt fragment.
///
/// Only meaningful when [`AgentSettings::jarvis_persona_enabled`] is on — see
/// that field's doc for why the default is off. This function only *produces*
/// the fragment; splicing it into an actual request is the job of whatever
/// assembles the effective system prompt for a call (today that is `agent`'s
/// business, not this module's). The intended composition mirrors how a
/// user-written [`BackendConfig::system_prompt`] already layers onto
/// Caduceus's behaviour elsewhere rather than replacing it outright: append
/// this fragment after the backend's own prompt when the toggle is set,
/// rather than switching the prompt out entirely.
///
/// `honorific` is [`AgentSettings::jarvis_honorific`], substituted in
/// wherever the persona addresses the user directly. An empty honorific drops
/// the address from the example line rather than leaving "Quite well, . Ever
/// ready" — nobody asked to be addressed as nothing.
pub fn jarvis_persona_prompt(honorific: &str) -> String {
    let honorific = honorific.trim();
    let greeting = if honorific.is_empty() {
        "Quite well. Ever ready to assist you.".to_string()
    } else {
        format!("Quite well, {honorific}. Ever ready to assist you.")
    };
    format!(
        "Adopt the register of a poised, unflappable butler in the JARVIS mould: warm, \
         precise, and economical with words rather than chatty. Acknowledge requests \
         plainly and get on with them — for instance, asked how you are, you might say: \
         \u{201c}{greeting}\u{201d} Never break this register to explain that you are an \
         AI playing a role."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// The zero-config default: explains how to configure a real backend.
    #[default]
    Null,
    /// Any endpoint speaking the OpenAI `/chat/completions` dialect —
    /// OpenAI, Ollama, LM Studio, vLLM, llama.cpp, OpenRouter, Groq, and
    /// `hermes proxy start`.
    ///
    /// The wire name is spelled out rather than left to `rename_all`, which
    /// would derive `open_ai_compatible` from the Rust casing. Nothing else in
    /// the project uses that spelling: the `BackendKind` union in
    /// `shared/types.ts`, the settings UI, `agent/openai.rs` and the installer
    /// all say `openai_compatible`. When they disagreed, a backend added in the
    /// UI wrote a variant the loader could not read, and since one unknown
    /// variant fails the whole document, the next launch discarded every
    /// setting the user had. The alias keeps files written during the
    /// disagreement readable.
    #[serde(rename = "openai_compatible", alias = "open_ai_compatible")]
    OpenAiCompatible,
    /// Hermes Agent, driven through its CLI. The default: it brings its own
    /// model routing, tools, memory and screen control, so Caduceus does not
    /// reimplement any of that.
    Hermes,
}

/// One configured AI provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackendConfig {
    pub id: String,
    pub display_name: String,
    pub kind: BackendKind,
    /// Base URL *without* a trailing slash, e.g. `https://api.openai.com/v1`
    /// or `http://localhost:11434/v1` for Ollama.
    pub base_url: String,
    pub model: String,
    /// True when an API key for this backend exists in the OS keychain.
    /// The key itself is never stored here. See `secrets::api_key_handle`.
    pub has_api_key: bool,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub system_prompt: String,
    /// Whether this backend may be selected for screen control.
    pub supports_computer_use: bool,
    /// Extra HTTP headers sent with every request, as `[name, value]` pairs.
    /// Useful for gateways and proxies that require custom auth. Ignored by
    /// the Hermes backend, which is a subprocess rather than an HTTP call.
    pub extra_headers: Vec<[String; 2]>,
    /// Request timeout in seconds. Agent tasks that drive the screen legitimately
    /// take minutes, so this is generous by default.
    pub timeout_secs: u64,
    /// `reasoning_effort`, passed through to servers that understand it, or
    /// `None` to leave the server's own default alone.
    ///
    /// This exists because a small *reasoning* model is a trap for any caller
    /// doing short mechanical work. Asked to compress one paragraph,
    /// `qwen3.5:2b` spends its entire completion budget thinking, returns empty
    /// content with `finish_reason: length`, and takes twenty-two seconds to do
    /// it. Setting this to `"none"` on the same request returns real content in
    /// seven. Nothing in the UI sets it — it is for code that knows its call is
    /// mechanical and wants to say so, such as `tools::promptopt`.
    ///
    /// Sent only when set, because a server that does not recognise the field
    /// is likelier to reject the request than to ignore it.
    pub reasoning_effort: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
            kind: BackendKind::Null,
            base_url: String::new(),
            model: String::new(),
            has_api_key: false,
            max_tokens: 4096,
            temperature: None,
            system_prompt: String::new(),
            supports_computer_use: false,
            extra_headers: Vec::new(),
            timeout_secs: 600,
            reasoning_effort: None,
        }
    }
}

/// The command that installs Hermes Agent, shown in the UI and the docs.
pub const HERMES_INSTALL_COMMAND: &str =
    "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash";

/// A fresh install ships with Hermes selected, plus the no-op as a fallback so
/// every AI code path still resolves if Hermes is deleted.
pub fn default_backends() -> Vec<BackendConfig> {
    vec![
        hermes_template("hermes"),
        BackendConfig {
            id: "null".into(),
            display_name: "None".into(),
            kind: BackendKind::Null,
            ..Default::default()
        },
    ]
}

/// The Hermes backend. No API key, no base URL, no model by default — it uses
/// whatever `hermes setup` already configured, which is the whole point.
pub fn hermes_template(id: impl Into<String>) -> BackendConfig {
    BackendConfig {
        id: id.into(),
        display_name: "Hermes Agent".into(),
        kind: BackendKind::Hermes,
        supports_computer_use: true,
        timeout_secs: 600,
        ..Default::default()
    }
}

/// A ready-made local-model config pointing at Ollama's OpenAI-compatible API.
pub fn openai_compatible_template(id: impl Into<String>) -> BackendConfig {
    BackendConfig {
        id: id.into(),
        display_name: "Local model".into(),
        kind: BackendKind::OpenAiCompatible,
        base_url: "http://localhost:11434/v1".into(),
        model: "llama3.2".into(),
        max_tokens: 2048,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Gateway (remote messaging bridge)
// ---------------------------------------------------------------------------
//
// One nested struct per platform, the same shape `VoiceSettings` already uses
// for its `stt_*`/`tts_*` fields — except a platform's config genuinely
// differs in shape from another's (a bot token versus, say, a phone-number
// pairing), so each gets its own struct rather than forcing a shared shape
// nothing needs yet. See `crate::gateway`'s module doc for how these are used
// and the security model around them.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct GatewaySettings {
    pub telegram: TelegramGatewaySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramGatewaySettings {
    /// Persisted intent — "should this be running" — read once at launch by
    /// `gateway::autostart_if_enabled` so a restart resumes where it left
    /// off. Independent of the live connection state, which always starts at
    /// `Stopped` on a fresh process and is never itself persisted (the same
    /// split `crate::mcp::ServerStatus` keeps from a server's persisted
    /// `enabled` flag).
    pub enabled: bool,
    /// Telegram numeric user ids allowed to talk to the bot. Mandatory and
    /// deny-by-default: empty — the out-of-the-box value — means nobody is
    /// allowed, not everybody. See `crate::gateway`'s module doc, security
    /// rule 1.
    pub allowed_user_ids: Vec<i64>,
}

impl Default for TelegramGatewaySettings {
    fn default() -> Self {
        Self { enabled: false, allowed_user_ids: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClipboardSettings {
    pub enabled: bool,
    /// Maximum number of unpinned entries retained. Pinned entries are never
    /// pruned by count.
    pub max_items: usize,
    /// Drop unpinned entries older than this. `None` = no age limit.
    pub max_age_days: Option<u32>,
    pub poll_interval_ms: u64,
    pub capture_text: bool,
    pub capture_images: bool,
    pub capture_files: bool,
    /// Skip clipboard payloads larger than this (bytes). Guards against
    /// copying a 200MB image into SQLite.
    pub max_entry_bytes: usize,
    /// Encrypt entry contents at rest with ChaCha20-Poly1305, keyed from the
    /// OS keychain. Toggling this triggers a one-time re-encryption pass.
    pub encrypt_at_rest: bool,
    /// Applications whose clipboard writes are ignored. Matched
    /// case-insensitively against the frontmost app name (best effort — see
    /// `docs/PLATFORM_SUPPORT.md`).
    pub excluded_apps: Vec<String>,
    /// Skip anything that looks like a password-manager payload: entries that
    /// arrive while an excluded app is frontmost, or that carry the
    /// `org.nspasteboard.ConcealedType` marker on macOS.
    pub respect_concealed_marker: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: 500,
            max_age_days: Some(30),
            poll_interval_ms: 700,
            capture_text: true,
            capture_images: true,
            capture_files: true,
            max_entry_bytes: 8 * 1024 * 1024,
            encrypt_at_rest: false,
            excluded_apps: vec![
                "1Password".into(),
                "Bitwarden".into(),
                "Dashlane".into(),
                "Enpass".into(),
                "KeePassXC".into(),
                "LastPass".into(),
                "Proton Pass".into(),
                "Keychain Access".into(),
            ],
            respect_concealed_marker: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: Theme,
    /// Accent colour as `#rrggbb`. Drives `--c-accent` in the frontend.
    pub accent: String,
    /// Height of the caduceus mark in logical pixels (28–160).
    ///
    /// This is a *height*, not a diameter: the mark is tall and narrow, so it
    /// needs noticeably more of it than a circle would to carry the same
    /// visual weight.
    pub staff_size: u32,
    /// Distance from the staff centre to the pop-out icons (64–120).
    pub popout_radius: u32,
    /// Diameter of each pop-out icon (28–48).
    pub popout_icon_size: u32,
    /// Staff opacity when idle and un-hovered (0.2–1.0).
    pub staff_idle_opacity: f32,
    /// Replace translucency with solid surfaces. Helps on low-end GPUs and for
    /// users who find blur uncomfortable.
    pub reduce_transparency: bool,
    /// Draw the slow "breathing" animation on the idle staff.
    pub staff_idle_animation: bool,
    /// Built-in caduceus pixel mark when empty; `image:staff-mark.png` after upload.
    pub staff_mark_icon: String,
    /// A background image for the Command Center.
    ///
    /// Empty for none; `image:command-center-background.png` once one has been
    /// chosen. See [`crate::backdrop`].
    pub command_center_background: String,
    /// How strongly the background image shows through, 0.0–1.0.
    ///
    /// Low by default. A wallpaper behind a list of results is decoration, and
    /// decoration that makes the results hard to read has failed at being
    /// either.
    pub background_opacity: f32,
    /// Blur applied over the background image, in pixels (0–40).
    ///
    /// The thing that makes an arbitrary photograph usable behind text.
    pub background_blur: u32,
    /// Corner radius of the Command Center window, in pixels (0–28).
    pub window_radius: u32,
    /// Font scale for the Command Center, 0.85–1.4.
    ///
    /// Not a full accessibility story — macOS's own text size is that — but
    /// enough for "this palette is too small on a 5K display".
    pub ui_scale: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            accent: "#7c7cff".into(),
            staff_size: 72,
            popout_radius: 96,
            popout_icon_size: 38,
            staff_idle_opacity: 0.9,
            reduce_transparency: false,
            staff_idle_animation: true,
            staff_mark_icon: String::new(),
            command_center_background: String::new(),
            background_opacity: 0.35,
            background_blur: 8,
            window_radius: 14,
            ui_scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

/// How Caduceus's background updater is allowed to behave. See
/// `crate::update::spawn_update_watcher`, which is the only thing that reads
/// this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    /// Never check. The Settings → Update panel still has a manual "Check for
    /// updates" button for anyone who wants to look themselves.
    Off,
    /// Check on a schedule; if something newer is out, say so once (a macOS
    /// notification plus a card in Settings) and wait for a click before
    /// touching anything on disk.
    #[default]
    Notify,
    /// Check on a schedule and install without asking, unless the copy is
    /// Homebrew-managed (Homebrew's own `upgrade` is the correct path there,
    /// and Caduceus must not fight it) or something that looks like active use
    /// — a recording — is in progress, in which case it falls back to
    /// `Notify`'s behaviour for that cycle and tries again next time.
    Auto,
}

/// Settings for the background updater, plus the two pieces of state it needs
/// to remember between launches.
///
/// The state fields (`last_checked_at`, `last_announced_version`) live beside
/// `mode` rather than in a separate file for the same reason
/// `last_launched_version` lives on `GeneralSettings`: it is one more thing
/// that already gets loaded, saved and broadcast on every settings change, so
/// there is no second persistence path to keep in sync with the first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    pub mode: UpdateMode,
    /// Unix seconds of the last time the watcher actually asked GitHub,
    /// successful or not. `None` means "never" — true of a fresh install and
    /// of an upgrade from before this field existed, both of which should get
    /// their first check on the usual launch delay rather than waiting out a
    /// full interval.
    ///
    /// This is what stops a user who restarts Caduceus ten times in a minute
    /// from spending ten of GitHub's sixty-per-hour unauthenticated requests
    /// doing it: the watcher checks this before it checks the network.
    pub last_checked_at: Option<i64>,
    /// The newest version a `Notify` popup has already been shown for.
    /// Matching the latest release means "already said this one out loud, stay
    /// quiet" — the difference between one useful nudge per release and a
    /// notification every twelve hours for the same update, which is exactly
    /// the kind of thing this app already gets complained about elsewhere.
    pub last_announced_version: Option<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            // Checks automatically and asks before installing. `Auto` is more
            // convenient but Caduceus replaces itself in place and is not
            // notarised, so the safer default is the one that puts a human in
            // the loop the first time; `Off` is available but should be a
            // choice, not the out-of-the-box behaviour for a launcher that is
            // otherwise entirely automatic about everything else it does.
            mode: UpdateMode::Notify,
            last_checked_at: None,
            last_announced_version: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one assertion this whole feature exists to guarantee: a fresh
    /// install — or an existing one that just upgraded — must not start
    /// talking, or adopt a persona, without being asked. Both toggles are
    /// additive fields on structs that are `#[serde(default)]`, so an old
    /// settings file missing them entirely lands here too, not just a
    /// brand-new one.
    #[test]
    fn speech_and_the_jarvis_persona_are_off_by_default() {
        let voice = VoiceSettings::default();
        assert!(!voice.tts_enabled);
        assert!(!voice.tts_speak_replies);

        let agents = AgentSettings::default();
        assert!(!agents.jarvis_persona_enabled);
    }

    /// Backend/endpoint/model are still pre-filled even though the master
    /// switch is off, so turning `tts_enabled` on alone is enough to hear
    /// something — the same shape as the STT fields it sits beside.
    #[test]
    fn tts_is_pre_configured_despite_being_off() {
        let voice = VoiceSettings::default();
        assert_eq!(voice.tts_backend, TtsBackendKind::SystemNative);
        assert!(!voice.tts_endpoint.is_empty());
        assert!(!voice.tts_model.is_empty());
    }

    #[test]
    fn tts_backend_kind_uses_the_shared_openai_compatible_spelling() {
        let json = serde_json::to_string(&TtsBackendKind::OpenAiCompatible).unwrap();
        assert_eq!(json, "\"openai_compatible\"");

        let parsed: TtsBackendKind = serde_json::from_str("\"openai_compatible\"").unwrap();
        assert_eq!(parsed, TtsBackendKind::OpenAiCompatible);

        // Files written while Rust and the UI could disagree (see
        // `SttBackendKind`'s identical alias) must still load.
        let legacy: TtsBackendKind = serde_json::from_str("\"open_ai_compatible\"").unwrap();
        assert_eq!(legacy, TtsBackendKind::OpenAiCompatible);
    }

    #[test]
    fn jarvis_persona_prompt_uses_the_configured_honorific() {
        assert!(jarvis_persona_prompt("sir").contains("Quite well, sir."));
        assert!(jarvis_persona_prompt("ma'am").contains("Quite well, ma'am."));

        // An empty honorific must not leave a dangling ", ." in the example.
        let bare = jarvis_persona_prompt("");
        assert!(bare.contains("Quite well."));
        assert!(!bare.contains(", ."));
        assert!(!bare.contains(",."));
    }
}
