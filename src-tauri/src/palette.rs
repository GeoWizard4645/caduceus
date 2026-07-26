//! Command Center input parsing and dispatch.
//!
//! Everything typed into the palette goes through here:
//!
//! ```text
//!   "cheap flights"     → no prefix     → web search
//!   "/ explain OAuth"   → PrimaryAi     → chat with the primary backend
//!   "/c open my email"  → ComputerUse   → start an agent session
//!   "/v invoice"        → ClipboardSearch → filter clipboard history
//! ```
//!
//! Prefixes are data, not code: they come from
//! [`CommandCenterSettings::prefixes`] and a user can add, edit or delete any of
//! them — including the shipped defaults.
//!
//! **The longest matching prefix wins**, so `/c` beats `/` regardless of the
//! order they appear in the list.
//!
//! A prefix whose last character is alphanumeric must be followed by whitespace
//! or end-of-input, so `/computer` is not the `/c` prefix with "omputer" after
//! it. Punctuation-only prefixes like `/` have no such requirement, because
//! `/explain this` should obviously work without a space after the slash.

use serde::Serialize;
use tauri::Emitter;
use tauri::Manager;

use crate::settings::{CommandCenterSettings, PrefixAction, PrefixRule, Settings};
use crate::shortcuts;

/// The result of parsing palette input.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedInput {
    /// The matched rule, or `None` for the plain-search path.
    pub rule: Option<PrefixRule>,
    /// The input with the prefix removed.
    pub remainder: String,
    /// What was typed, unchanged.
    pub raw: String,
}

impl ParsedInput {
    /// The action this input will take.
    pub fn action(&self) -> PrefixAction {
        self.rule.as_ref().map_or(PrefixAction::WebSearch, |r| r.action)
    }
}

/// Split input into a prefix rule and the rest.
pub fn parse(input: &str, settings: &CommandCenterSettings) -> ParsedInput {
    let raw = input.to_string();
    let trimmed = input.trim_start();

    let mut best: Option<(&PrefixRule, usize)> = None;
    for rule in &settings.prefixes {
        let prefix = rule.prefix.trim();
        if prefix.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(prefix) else {
            continue;
        };
        // A prefix ending in a letter or digit must be a whole token, or `/c`
        // would swallow the start of `/computer`. Punctuation-only prefixes
        // (`/`, `>`, `=`) need no separator: `/explain this` is the common case.
        let needs_boundary = prefix.chars().next_back().is_some_and(char::is_alphanumeric);
        if needs_boundary && !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            continue;
        }
        if best.is_none_or(|(_, len)| prefix.len() > len) {
            best = Some((rule, prefix.len()));
        }
    }

    match best {
        Some((rule, len)) => ParsedInput {
            rule: Some(rule.clone()),
            remainder: trimmed[len..].trim().to_string(),
            raw,
        },
        None => ParsedInput {
            rule: None,
            remainder: trimmed.trim_end().to_string(),
            raw,
        },
    }
}

/// Build the URL for a web search.
pub fn search_url(query: &str, settings: &CommandCenterSettings) -> String {
    let template = if settings.search_url_template.trim().is_empty() {
        "https://www.google.com/search?q={query}"
    } else {
        settings.search_url_template.trim()
    };
    let encoded = shortcuts::percent_encode(query);
    if template.contains("{query}") {
        template.replace("{query}", &encoded)
    } else {
        // A template without the token is still usable as a plain bookmark.
        format!("{template}{encoded}")
    }
}

/// What the frontend should do after dispatch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchOutcome {
    pub ok: bool,
    /// Message for a toast, or the assistant's reply for a chat action.
    pub message: String,
    /// The action that ran, for the UI to decide how to present the result.
    pub action: PrefixAction,
    /// Set for `ComputerUse`, so the UI can subscribe to that session's steps.
    pub session_id: Option<String>,
    /// Set for `ClipboardSearch`: the text to filter history by.
    pub clipboard_query: Option<String>,
    /// Set for `PrimaryAi`: the thread the reply was saved into, so the palette
    /// can show the conversation and hand it to the chat window.
    pub conversation_id: Option<i64>,
    /// Whether the Command Center should close.
    pub close_window: bool,
}

impl DispatchOutcome {
    fn simple(ok: bool, action: PrefixAction, message: impl Into<String>, close: bool) -> Self {
        Self {
            ok,
            message: message.into(),
            action,
            session_id: None,
            conversation_id: None,
            clipboard_query: None,
            close_window: close,
        }
    }
}

/// Route parsed input to its destination.
pub async fn dispatch<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &crate::settings::SettingsManager,
    input: &str,
) -> DispatchOutcome {
    let snapshot: Settings = settings.get();
    let parsed = parse(input, &snapshot.command_center);
    let action = parsed.action();
    let close = snapshot.command_center.close_on_action;

    // An empty query after a prefix is a no-op, not an error — the user is
    // mid-thought.
    if parsed.remainder.is_empty() && !matches!(action, PrefixAction::ClipboardSearch) {
        return DispatchOutcome::simple(false, action, "Type something first.", false);
    }

    match action {
        PrefixAction::WebSearch => {
            let url = search_url(&parsed.remainder, &snapshot.command_center);
            let outcome =
                shortcuts::exec::open_url(&url, &snapshot.command_center.browser).await;
            DispatchOutcome::simple(outcome.ok, action, outcome.message, close && outcome.ok)
        }

        PrefixAction::PrimaryAi => {
            // Routed through the chat store rather than `agent::chat` so `/` has
            // a memory: the thread is sent with the question and both turns are
            // saved. A one-shot call is what made follow-ups useless.
            let Some(store) = app.try_state::<crate::chat::ChatStore>() else {
                return DispatchOutcome::simple(
                    false,
                    action,
                    "Chat history is unavailable.",
                    false,
                );
            };
            let store = store.inner().clone();

            let conversation = match crate::chat::active_conversation(&store) {
                Ok(id) => id,
                Err(e) => return DispatchOutcome::simple(false, action, e.user_message(), false),
            };

            match crate::chat::ask(&store, settings, conversation, &parsed.remainder).await {
                Ok(text) => {
                    let _ = app.emit(crate::chat::CHAT_CHANGED_EVENT, conversation);
                    DispatchOutcome {
                        ok: true,
                        message: text,
                        action,
                        session_id: None,
                        clipboard_query: None,
                        conversation_id: Some(conversation),
                        // Chat replies are shown *in* the palette, so it stays open.
                        close_window: false,
                    }
                }
                Err(e) => DispatchOutcome::simple(false, action, e.user_message(), false),
            }
        }

        PrefixAction::ComputerUse => {
            let runtime = match app.try_state::<crate::agent::AgentRuntime>() {
                Some(r) => r.inner().clone(),
                None => {
                    return DispatchOutcome::simple(
                        false,
                        action,
                        "The agent runtime is not available.",
                        false,
                    )
                }
            };
            match crate::agent::start_session(
                app.clone(),
                runtime,
                settings.clone(),
                parsed.remainder.clone(),
            ) {
                Ok(session_id) => DispatchOutcome {
                    ok: true,
                    message: "Starting\u{2026}".into(),
                    action,
                    session_id: Some(session_id),
                    clipboard_query: None,
                    conversation_id: None,
                    close_window: false,
                },
                Err(e) => DispatchOutcome::simple(false, action, e.user_message(), false),
            }
        }

        PrefixAction::ClipboardSearch => DispatchOutcome {
            ok: true,
            message: String::new(),
            action,
            session_id: None,
            clipboard_query: Some(parsed.remainder.clone()),
            conversation_id: None,
            close_window: false,
        },

        PrefixAction::OpenUrlTemplate => {
            let Some(rule) = &parsed.rule else {
                return DispatchOutcome::simple(false, action, "This prefix has no target.", false);
            };
            let url = shortcuts::substitute_query(
                &rule.target,
                &shortcuts::percent_encode(&parsed.remainder),
            );
            let choice = rule
                .browser
                .as_ref()
                .unwrap_or(&snapshot.command_center.browser);
            let outcome = shortcuts::exec::open_url(&url, choice).await;
            DispatchOutcome::simple(outcome.ok, action, outcome.message, close && outcome.ok)
        }

        PrefixAction::RunCommand => {
            let Some(rule) = &parsed.rule else {
                return DispatchOutcome::simple(false, action, "This prefix has no command.", false);
            };
            let outcome = shortcuts::exec::run_command_capture(&rule.target, &parsed.remainder, 30).await;
            DispatchOutcome::simple(outcome.ok, action, outcome.message, false)
        }

        PrefixAction::RunAppleScript => {
            let Some(rule) = &parsed.rule else {
                return DispatchOutcome::simple(false, action, "This prefix has no script.", false);
            };
            let source = shortcuts::substitute_query(&rule.target, &parsed.remainder);
            match shortcuts::exec::run_applescript(&source).await {
                Ok(out) => DispatchOutcome::simple(
                    true,
                    action,
                    if out.trim().is_empty() {
                        "Done.".to_string()
                    } else {
                        out.trim().to_string()
                    },
                    false,
                ),
                Err(e) => DispatchOutcome::simple(false, action, format!("AppleScript failed: {e}"), false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CommandCenterSettings;

    fn cfg() -> CommandCenterSettings {
        CommandCenterSettings::default()
    }

    #[test]
    fn plain_text_has_no_prefix_and_becomes_a_search() {
        let p = parse("cheap flights to Lisbon", &cfg());
        assert!(p.rule.is_none());
        assert_eq!(p.remainder, "cheap flights to Lisbon");
        assert_eq!(p.action(), PrefixAction::WebSearch);
    }

    #[test]
    fn slash_routes_to_the_primary_ai() {
        let p = parse("/ explain OAuth", &cfg());
        assert_eq!(p.action(), PrefixAction::PrimaryAi);
        assert_eq!(p.remainder, "explain OAuth");
    }

    #[test]
    fn longest_prefix_wins_over_a_shorter_one() {
        let p = parse("/c open my email", &cfg());
        assert_eq!(p.action(), PrefixAction::ComputerUse);
        assert_eq!(p.remainder, "open my email");
    }

    #[test]
    fn a_prefix_must_end_at_a_word_boundary() {
        // "/computer" is not the "/c" prefix followed by "omputer".
        let p = parse("/computer stuff", &cfg());
        assert_eq!(p.action(), PrefixAction::PrimaryAi);
        assert_eq!(p.remainder, "computer stuff");
    }

    #[test]
    fn a_bare_prefix_yields_an_empty_remainder() {
        let p = parse("/c", &cfg());
        assert_eq!(p.action(), PrefixAction::ComputerUse);
        assert_eq!(p.remainder, "");
    }

    #[test]
    fn leading_whitespace_does_not_break_matching() {
        let p = parse("   /c do a thing", &cfg());
        assert_eq!(p.action(), PrefixAction::ComputerUse);
        assert_eq!(p.remainder, "do a thing");
    }

    #[test]
    fn custom_prefixes_work_the_same_as_built_in_ones() {
        let mut c = cfg();
        c.prefixes.push(PrefixRule {
            id: "gh".into(),
            prefix: "gh".into(),
            label: "GitHub".into(),
            action: PrefixAction::OpenUrlTemplate,
            target: "https://github.com/search?q={query}".into(),
            ..Default::default()
        });
        let p = parse("gh tauri apps", &c);
        assert_eq!(p.action(), PrefixAction::OpenUrlTemplate);
        assert_eq!(p.remainder, "tauri apps");
    }

    #[test]
    fn deleting_every_prefix_leaves_search_working() {
        let mut c = cfg();
        c.prefixes.clear();
        let p = parse("/ still a search", &c);
        assert!(p.rule.is_none());
        assert_eq!(p.action(), PrefixAction::WebSearch);
    }

    #[test]
    fn empty_prefix_strings_are_ignored() {
        let mut c = cfg();
        c.prefixes.push(PrefixRule {
            id: "bad".into(),
            prefix: "  ".into(),
            action: PrefixAction::ComputerUse,
            ..Default::default()
        });
        assert!(parse("anything", &c).rule.is_none());
    }

    #[test]
    fn search_urls_encode_the_query() {
        let url = search_url("a b&c", &cfg());
        assert_eq!(url, "https://www.google.com/search?q=a+b%26c");
    }

    #[test]
    fn any_search_engine_template_works() {
        let mut c = cfg();
        c.search_url_template = "https://duckduckgo.com/?q={query}".into();
        assert_eq!(search_url("rust", &c), "https://duckduckgo.com/?q=rust");

        c.search_url_template = "https://kagi.com/search?q={query}&r=uk".into();
        assert_eq!(search_url("x", &c), "https://kagi.com/search?q=x&r=uk");
    }

    #[test]
    fn a_template_without_the_token_appends_the_query() {
        let mut c = cfg();
        c.search_url_template = "https://example.com/?s=".into();
        assert_eq!(search_url("hi", &c), "https://example.com/?s=hi");
    }

    #[test]
    fn a_blank_template_falls_back_to_a_working_default() {
        let mut c = cfg();
        c.search_url_template = "  ".into();
        assert!(search_url("x", &c).starts_with("https://www.google.com/search?q="));
    }
}
