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
}

impl Settings {
    pub const CURRENT_VERSION: u32 = 1;
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
    /// collapses again. Settings UI constrains this to 1000–10000ms.
    pub collapse_idle_ms: u64,
    /// Register Caduceus as a login item.
    pub launch_at_login: bool,
    /// Poll rate of the global cursor tracker that drives staff hover/collapse.
    /// Lower is more responsive, higher is cheaper. 16–100ms is sane.
    pub cursor_poll_ms: u64,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            toggle_orb_hotkey: "F12".into(),
            // Alt+Space avoids clashing with Spotlight (Cmd+Space) and most
            // Windows/Linux launchers.
            command_center_hotkey: "Alt+Space".into(),
            staff_visible: true,
            staff_edge: StaffEdge::Right,
            staff_position: None,
            hover_expand_delay_ms: 0,
            collapse_idle_ms: 3000,
            launch_at_login: false,
            cursor_poll_ms: 33,
        }
    }
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
    /// Optional Chrome profile directory used when opening URLs that don't
    /// specify their own (e.g. the plain web-search path).
    pub default_chrome_profile: Option<String>,
    /// Force every `open_url` through Chrome so profile selection works.
    /// When false, URLs go to the OS default browser.
    pub prefer_chrome: bool,
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
            default_chrome_profile: None,
            prefer_chrome: false,
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
    pub chrome_profile_directory: Option<String>,
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
            chrome_profile_directory: None,
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
    /// Where a transcript goes when no keyword group matches.
    pub fallback_route: RouteTarget,
    /// Hard cap on a single recording, so a stuck key can't fill the disk.
    pub max_recording_secs: u32,
    /// Automatically dispatch the routed action, or just fill the input and
    /// let the user press Enter. `false` is the safer default.
    pub auto_submit: bool,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            // On by default: the recogniser is Apple's, runs on-device, and the
            // microphone only opens while the key is physically held.
            enabled: true,
            push_to_talk_hotkey: "CommandOrControl+Shift+Space".into(),
            stt_backend: SttBackendKind::SystemNative,
            stt_endpoint: "http://127.0.0.1:8080/v1/audio/transcriptions".into(),
            stt_model: "whisper-1".into(),
            stt_language: String::new(),
            keyword_groups: default_keyword_groups(),
            fallback_route: RouteTarget::PrimaryAi,
            max_recording_secs: 60,
            auto_submit: false,
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

pub fn default_keyword_groups() -> Vec<KeywordGroup> {
    vec![
        KeywordGroup {
            id: "kw-search".into(),
            name: "Web search".into(),
            keywords: vec!["search".into(), "look up".into(), "browse".into()],
            route: RouteTarget::WebSearch,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        },
        KeywordGroup {
            id: "kw-computer".into(),
            name: "Computer use".into(),
            keywords: vec!["computer".into(), "jarvis".into(), "search my mac".into()],
            route: RouteTarget::ComputerUse,
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
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: Theme,
    /// Accent colour as `#rrggbb`. Drives `--c-accent` in the frontend.
    pub accent: String,
    /// Height of the caduceus mark in logical pixels (36–120).
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
