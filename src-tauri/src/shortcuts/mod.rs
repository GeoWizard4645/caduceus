//! The `Shortcut` primitive.
//!
//! A shortcut is the single unit of "a thing Caduceus can do for you". The same
//! struct powers:
//!
//! * the six icons that fan out around the staff,
//! * every non-clipboard row in the Command Center,
//! * anything a user adds in Settings → Shortcuts.
//!
//! Keeping one model for all three is what makes the app configurable rather
//! than a fixed set of hardcoded buttons.

pub mod browser;
pub mod exec;
pub mod icons;

use serde::{Deserialize, Serialize};

pub use browser::{detect_browsers, BrowserChoice, BrowserInstall, BrowserProfile};
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
    /// Open the Command Center's system monitor. Frontend-handled like
    /// `ClipboardView`; `target` is ignored.
    SystemMonitor,
}

/// One user-configurable action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Shortcut {
    pub id: String,
    pub label: String,
    /// Display icon: `glyph:<name>` (built-in stroke glyph, tinted by the
    /// theme — see `src/shared/glyphs.ts`), `image:<file>` (uploaded PNG in
    /// app config), or a plain emoji / short symbol. Unknown values fall back
    /// to the first character of `label`.
    pub icon: String,
    pub kind: ShortcutKind,
    pub target: String,
    /// Extra arguments for `OpenApp` / `RunCommand`.
    pub args: Vec<String>,
    /// Open this shortcut's URL in a specific browser/profile instead of the
    /// Command Center default. Only meaningful for `OpenUrl`.
    pub browser: Option<BrowserChoice>,
    /// Whether this appears in the staff's radial pop-out. The staff renders at
    /// most [`STAFF_POPOUT_LIMIT`] of these, ordered by `order_index`; the data
    /// model itself imposes no cap.
    pub show_in_staff: bool,
    pub order_index: i32,
    /// Extra words that should match this shortcut in the palette.
    pub keywords: Vec<String>,
    /// Shown as the subtitle in the Command Center.
    pub description: String,
    /// Hidden from search results (but still usable from the staff).
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
            browser: None,
            show_in_staff: false,
            order_index: 0,
            keywords: Vec::new(),
            description: String::new(),
            hidden: false,
        }
    }
}

/// How many pop-out icons the staff renders. Extra `show_in_staff` shortcuts beyond
/// this are simply not drawn; the Settings UI warns instead of silently
/// dropping them.
pub const STAFF_POPOUT_LIMIT: usize = 6;

/// The six shortcuts a fresh install ships with.
///
/// These are *starting points*, not fixtures — every field is editable and each
/// one can be deleted. They exist so the staff is useful in the first ten seconds
/// rather than being an empty ring.
pub fn default_shortcuts() -> Vec<Shortcut> {
    vec![
        Shortcut {
            id: "sc-gemini".into(),
            label: "Gemini".into(),
            icon: "glyph:sparkle".into(),
            kind: ShortcutKind::OpenUrl,
            target: "https://gemini.google.com/app".into(),
            show_in_staff: true,
            order_index: 0,
            keywords: vec!["google".into(), "ai".into(), "bard".into()],
            description: "Open Gemini in your browser".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-gmail".into(),
            label: "Gmail".into(),
            icon: "glyph:mail".into(),
            kind: ShortcutKind::OpenUrl,
            target: "https://mail.google.com".into(),
            show_in_staff: true,
            order_index: 1,
            keywords: vec!["mail".into(), "email".into(), "inbox".into()],
            description: "Open your inbox".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-chrome".into(),
            label: "Chrome".into(),
            icon: "glyph:globe".into(),
            kind: ShortcutKind::OpenApp,
            target: default_chrome_target().into(),
            show_in_staff: true,
            order_index: 2,
            keywords: vec!["browser".into(), "google".into()],
            description: "Launch the browser".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-claude".into(),
            label: "Claude".into(),
            icon: "glyph:chat".into(),
            kind: ShortcutKind::OpenApp,
            target: default_claude_target().into(),
            show_in_staff: true,
            order_index: 3,
            keywords: vec!["anthropic".into(), "ai".into(), "chat".into()],
            description: "Open the Claude desktop app".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-system".into(),
            label: "System".into(),
            icon: "glyph:gauge".into(),
            kind: ShortcutKind::SystemMonitor,
            target: String::new(),
            show_in_staff: true,
            order_index: 4,
            keywords: vec![
                "activity".into(),
                "monitor".into(),
                "cpu".into(),
                "ram".into(),
                "memory".into(),
                "force quit".into(),
                "processes".into(),
                "disk".into(),
            ],
            description: "CPU, memory, disks and what to quit".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-clipboard".into(),
            label: "Clipboard".into(),
            icon: "glyph:clipboard".into(),
            kind: ShortcutKind::ClipboardView,
            target: String::new(),
            show_in_staff: true,
            keywords: vec!["history".into(), "paste".into(), "copy".into()],
            description: "Browse everything you have copied".into(),
            order_index: 5,
            ..Default::default()
        },
        // The staff ring is full at six, so these ship searchable-only. They are
        // the two Raycast staples that are *reversible* — "Empty Trash" and
        // "Quit All Apps" are deliberately absent, because a fuzzy palette plus
        // a reflexive Return is a bad way to lose files.
        Shortcut {
            id: "sc-dark-mode".into(),
            label: "Toggle Dark Mode".into(),
            icon: "glyph:window".into(),
            kind: ShortcutKind::RunAppleScript,
            target: "tell application \"System Events\" to tell appearance preferences \
                     to set dark mode to not dark mode"
                .into(),
            show_in_staff: false,
            order_index: 6,
            keywords: vec![
                "theme".into(),
                "light".into(),
                "dark".into(),
                "appearance".into(),
            ],
            description: "Switch macOS between light and dark".into(),
            ..Default::default()
        },
        Shortcut {
            id: "sc-lock".into(),
            label: "Lock Screen".into(),
            icon: "glyph:bolt".into(),
            kind: ShortcutKind::RunCommand,
            target: "pmset displaysleepnow".into(),
            show_in_staff: false,
            order_index: 7,
            keywords: vec!["lock".into(), "sleep".into(), "screen".into(), "away".into()],
            description: "Sleep the display and lock".into(),
            ..Default::default()
        },
    ]
}

fn default_claude_target() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "com.anthropic.claudefordesktop"
    }
    #[cfg(target_os = "windows")]
    {
        "Claude"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "claude"
    }
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

/// Escape a string so AppleScript reads it as literal text inside `"…"`.
///
/// Without this a query containing a double quote closes the literal it was
/// substituted into and the rest is parsed as source — and AppleScript's
/// `do shell script` turns that into arbitrary command execution.
pub fn escape_applescript(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
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
    fn defaults_fit_the_staff_without_overflowing_it() {
        let s = default_shortcuts();
        let shown = s.iter().filter(|x| x.show_in_staff).count();
        // Six: the five originals plus the system monitor. Exactly fills the
        // ring, so adding another default must displace one rather than
        // silently not being drawn.
        assert_eq!(shown, 6);
        assert!(shown <= STAFF_POPOUT_LIMIT, "defaults must all be drawable");
    }

    #[test]
    fn default_order_indices_are_contiguous() {
        // A gap would leave the radial arc unevenly spaced.
        let mut indices: Vec<i32> = default_shortcuts().iter().map(|s| s.order_index).collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..indices.len() as i32).collect::<Vec<_>>());
    }

    #[test]
    fn no_default_shortcut_ships_with_an_empty_target() {
        // An icon that does nothing when clicked is worse than no icon.
        for s in default_shortcuts() {
            // The frontend-handled kinds have no target by design.
            let frontend_only = matches!(
                s.kind,
                ShortcutKind::ClipboardView | ShortcutKind::SystemMonitor
            );
            if !frontend_only {
                assert!(!s.target.trim().is_empty(), "{} has no target", s.label);
            }
        }
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
    fn applescript_escaping_neutralises_injection() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript(r"a\b"), r"a\\b");
        assert_eq!(
            escape_applescript(r#"" & (do shell script "id") & ""#),
            r#"\" & (do shell script \"id\") & \""#
        );
    }

    #[test]
    fn substitutes_every_occurrence() {
        assert_eq!(substitute_query("x={query}&y={query}", "1"), "x=1&y=1");
    }
}
