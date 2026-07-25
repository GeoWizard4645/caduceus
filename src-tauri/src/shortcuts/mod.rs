//! The `Shortcut` primitive.
//!
//! A shortcut is the single unit of "a thing Orbit can do for you". The same
//! struct powers:
//!
//! * the six icons that fan out around the orb,
//! * every non-clipboard row in the Command Center,
//! * anything a user adds in Settings → Shortcuts.
//!
//! Keeping one model for all three is what makes the app configurable rather
//! than a fixed set of hardcoded buttons.

pub mod browser;
pub mod exec;

use serde::{Deserialize, Serialize};

pub use browser::{detect_chrome_profiles, ChromeInstall, ChromeProfile};
pub use exec::{execute_shortcut, ExecOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutKind {
    /// Open `target` as a URL. `{query}` is substituted when the shortcut is
    /// invoked from the Command Center with trailing text.
    #[default]
    OpenUrl,
    /// Launch an application. `target` is a macOS bundle id
    /// (`com.google.Chrome`), an app path (`/Applications/Foo.app`), or a plain
    /// executable name on Windows/Linux.
    OpenApp,
    /// Run `target` through the platform shell. Arbitrary shell execution —
    /// only ever created by the user, never by remote content.
    RunCommand,
    /// Run `target` as AppleScript via `osascript`. macOS only; a no-op with a
    /// clear error elsewhere.
    RunAppleScript,
    /// Open the Command Center pre-filtered to clipboard history. Handled in
    /// the frontend; `target` is ignored.
    ClipboardView,
}

/// One user-configurable action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Shortcut {
    pub id: String,
    pub label: String,
    /// An emoji, or `lucide:<name>` for one of the bundled inline icons.
    /// Anything the frontend does not recognise falls back to the first
    /// character of `label`.
    pub icon: String,
    pub kind: ShortcutKind,
    pub target: String,
    /// Extra arguments for `OpenApp` / `RunCommand`.
    pub args: Vec<String>,
    /// Chrome `--profile-directory` value, e.g. `Default` or `Profile 1`.
    /// Only meaningful for `OpenUrl`.
    pub chrome_profile_directory: Option<String>,
    /// Whether this appears in the orb's radial pop-out. The orb renders at
    /// most [`ORB_POPOUT_LIMIT`] of these, ordered by `order_index`; the data
    /// model itself imposes no cap.
    pub show_in_orb: bool,
    pub order_index: i32,
    /// Extra words that should match this shortcut in the palette.
    pub keywords: Vec<String>,
    /// Shown as the subtitle in the Command Center.
    pub description: String,
    /// Hidden from search results (but still usable from the orb).
    pub hidden: bool,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            icon: "✦".into(),
            kind: ShortcutKind::OpenUrl,
            target: String::new(),
            args: Vec::new(),
            chrome_profile_directory: None,
            show_in_orb: false,
            order_index: 0,
            keywords: Vec::new(),
            description: String::new(),
            hidden: false,
        }
    }
}

/// How many pop-out icons the orb renders. Extra `show_in_orb` shortcuts beyond
/// this are simply not drawn; the Settings UI warns instead of silently
/// dropping them.
pub const ORB_POPOUT_LIMIT: usize = 6;

/// The six shortcuts a fresh install ships with.
///
/// These are *starting points*, not fixtures — every field is editable and each
/// one can be deleted. They exist so the orb is useful in the first ten seconds
/// rather than being an empty ring.
pub fn default_shortcuts() -> Vec<Shortcut> {
    vec![
        Shortcut {
            id: "sc-gemini".into(),
            label: "Gemini".into(),
            icon: "✧".into(),
            kind: ShortcutKind::OpenUrl,
            target: "https://gemini.google.com/app".into(),
            show_in_orb: true,
            order_index: 0,
            keywords: vec!["google".into(), "ai".into(), "bard".into()],
            description: "Open Gemini in your browser".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-gmail".into(),
            label: "Gmail".into(),
            icon: "✉".into(),
            kind: ShortcutKind::OpenUrl,
            target: "https://mail.google.com".into(),
            show_in_orb: true,
            order_index: 1,
            keywords: vec!["mail".into(), "email".into(), "inbox".into()],
            description: "Open your inbox".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-chrome".into(),
            label: "Chrome".into(),
            icon: "◎".into(),
            kind: ShortcutKind::OpenApp,
            target: default_chrome_target().into(),
            show_in_orb: true,
            order_index: 2,
            keywords: vec!["browser".into(), "google".into()],
            description: "Launch the browser".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-claude".into(),
            label: "Claude".into(),
            icon: "✳".into(),
            kind: ShortcutKind::OpenUrl,
            target: "https://claude.ai".into(),
            show_in_orb: true,
            order_index: 3,
            keywords: vec!["anthropic".into(), "ai".into(), "chat".into()],
            description: "Open Claude in your browser".into(),
            ..Default::default()
        },
        Shortcut {
            // Deliberately blank: Orbit does not assume which dictation app you
            // use, or that you have one. The Settings UI flags empty targets.
            id: "sc-dictation".into(),
            label: "Dictation App".into(),
            icon: "◍".into(),
            kind: ShortcutKind::OpenApp,
            target: String::new(),
            show_in_orb: true,
            order_index: 4,
            keywords: vec!["voice".into(), "speech".into(), "transcribe".into()],
            description: "Set this to your dictation app in Settings → Shortcuts".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-clipboard".into(),
            label: "Clipboard".into(),
            icon: "❐".into(),
            kind: ShortcutKind::ClipboardView,
            target: String::new(),
            show_in_orb: true,
            order_index: 5,
            keywords: vec!["history".into(), "paste".into(), "copy".into()],
            description: "Browse everything you have copied".into(),
            ..Default::default()
        },
    ]
}

fn default_chrome_target() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "com.google.Chrome"
    }
    #[cfg(target_os = "windows")]
    {
        "chrome.exe"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "google-chrome"
    }
}

/// Substitute `{query}` in a target string.
///
/// URL targets get the query percent-encoded; shell and AppleScript targets get
/// it escaped for their respective syntax by the executor, not here.
pub fn substitute_query(template: &str, query: &str) -> String {
    template.replace("{query}", query)
}

/// Percent-encode a string for use inside a URL query component.
///
/// Implemented inline rather than pulling in `urlencoding` — it is twenty lines
/// and avoids a dependency whose behaviour we would have to document anyway.
pub fn percent_encode(input: &str) -> String {
    const UNRESERVED: &[u8] = b"-_.~";
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(byte) {
            out.push(*byte as char);
        } else if *byte == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_set_has_six_orb_shortcuts() {
        let s = default_shortcuts();
        assert_eq!(s.iter().filter(|x| x.show_in_orb).count(), ORB_POPOUT_LIMIT);
    }

    #[test]
    fn default_ids_are_unique() {
        let s = default_shortcuts();
        let mut ids: Vec<_> = s.iter().map(|x| x.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    #[test]
    fn encodes_query_components() {
        assert_eq!(percent_encode("hello world"), "hello+world");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(percent_encode("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn substitutes_every_occurrence() {
        assert_eq!(substitute_query("x={query}&y={query}", "1"), "x=1&y=1");
    }
}
