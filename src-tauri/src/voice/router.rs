//! Keyword routing for voice transcripts.
//!
//! After transcription, the text is matched against the user's keyword groups to
//! decide *where it goes* before it is treated as a plain query.
//!
//! # Matching rules (the documented behaviour)
//!
//! Both modes lowercase the text and ignore leading/trailing punctuation.
//!
//! * **Leading words** (default) — the transcript must *start with* the keyword.
//!   The keyword is then **stripped**, so saying "search cheap flights to Lisbon"
//!   searches for `cheap flights to Lisbon`, not for the word "search" as well.
//!   A trailing "for", "up" or "about" is also removed
//!   ("look up the weather" \u{2192} "the weather").
//! * **Anywhere** — the keyword may appear at any position and the text is
//!   passed through **unchanged**.
//!
//! The **longest matching keyword across every group** wins, with group order
//! breaking ties. That means "search my mac" routes to computer use even though
//! the plain "search" keyword sits in an earlier group — specificity beats list
//! position, so reordering groups in Settings cannot silently break a phrase.
//! The same rule is why a bare "ask" in the AI group never shadows a longer,
//! more specific phrase in another group: "ask" is 3 characters, so anything
//! longer that also matches — "ask ai", "search my mac", "control my mac" —
//! outranks it regardless of which group it lives in or where it sits in the
//! list.
//!
//! # The fallback (no keyword matched)
//!
//! What happens when nothing matches is not a fixed default — it depends on
//! whether a usable AI backend is actually configured, via the `ai_configured`
//! flag callers pass to [`route`]:
//!
//! * **AI not configured** — everything unmatched goes to [`RouteTarget::WebSearch`].
//!   Sending it to [`RouteTarget::PrimaryAi`] would just bounce off
//!   `AgentError::NotConfigured`, and silently trying the AI and failing is a
//!   worse experience than a search that actually returns something.
//! * **AI configured** — everything unmatched becomes [`RouteTarget::InsertOnly`],
//!   i.e. plain Command Center search, exactly as if it had been typed. It does
//!   **not** go to the AI: most short utterances are not questions for an
//!   assistant, and routing every unrecognised phrase at the AI made "ask ai"
//!   (see [`crate::settings::default_keyword_groups`]) pointless to say, since
//!   silence already got you there.
//!
//! Both of those describe the *default*. A user who has picked a fallback in
//! Settings → Voice gets what they picked, whatever the AI's state — see
//! [`effective_fallback`] for how the two compose.

use serde::Serialize;

use crate::settings::{AgentSettings, BackendKind, KeywordMatch, RouteTarget, VoiceSettings};

/// The outcome of routing a transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedText {
    /// Where this should go.
    pub route: RouteTarget,
    /// The text after any keyword was stripped. Never empty unless the input
    /// was only a keyword.
    pub text: String,
    /// The transcript exactly as recognised, before any keyword stripping.
    /// The voice-typing page wants what was *said*, not what would be run.
    pub raw: String,
    /// Which group matched, for the UI to explain itself. `None` = fallback.
    pub matched_group: Option<String>,
    /// The keyword that matched.
    pub matched_keyword: Option<String>,
}

/// Route a transcript according to the user's voice settings.
///
/// `ai_configured` decides what an unmatched transcript falls back to — see the
/// module doc and [`effective_fallback`]. Callers compute it once per call with
/// [`ai_is_configured`] rather than this function reaching into `AgentSettings`
/// itself, so that routing stays what it has always been: a pure function of a
/// transcript and the settings that describe how to read it, easy to exercise
/// in a test without constructing a whole backend.
pub fn route(transcript: &str, settings: &VoiceSettings, ai_configured: bool) -> RoutedText {
    let cleaned = transcript.trim();
    let haystack = normalize(cleaned);

    // Collect every candidate first, then pick the most specific one. Returning
    // on the first hit would make routing depend on how the groups happen to be
    // ordered in Settings.
    let mut best: Option<Candidate> = None;

    for group in settings.keyword_groups.iter().filter(|g| g.enabled) {
        for keyword in group.keywords.iter().filter(|k| !k.trim().is_empty()) {
            let needle = normalize(keyword);
            if needle.is_empty() {
                continue;
            }

            let text = match group.match_mode {
                KeywordMatch::LeadingWords if starts_with_word(&haystack, &needle) => {
                    strip_leading(cleaned, &needle)
                }
                KeywordMatch::Anywhere if contains_word(&haystack, &needle) => cleaned.to_string(),
                _ => continue,
            };

            let candidate = Candidate {
                specificity: needle.len(),
                route: group.route,
                text,
                group_name: group.name.clone(),
                keyword: keyword.trim().to_string(),
            };
            // Strictly greater, so an earlier group wins a tie.
            if best.as_ref().is_none_or(|b| candidate.specificity > b.specificity) {
                best = Some(candidate);
            }
        }
    }

    match best {
        Some(c) => RoutedText {
            route: c.route,
            text: c.text,
            raw: cleaned.to_string(),
            matched_group: Some(c.group_name),
            matched_keyword: Some(c.keyword),
        },
        None => RoutedText {
            route: effective_fallback(settings.fallback_route, ai_configured),
            text: cleaned.to_string(),
            raw: cleaned.to_string(),
            matched_group: None,
            matched_keyword: None,
        },
    }
}

/// Resolve `VoiceSettings::fallback_route` against whether AI is actually
/// usable right now.
///
/// This answers a question the settings type is shaped to answer: has the user
/// *deliberately chosen* a fallback, or has this field never been touched? The
/// two need different treatment — a deliberate choice must be honoured
/// outright, while an untouched default should track whether AI is configured
/// now, rather than freezing on whatever was true the day the settings file was
/// written.
///
/// `None` is "never chosen", and is resolved live:
///
/// * **no usable AI backend** → [`RouteTarget::WebSearch`]. There is nothing to
///   ask, so an unmatched sentence is a search.
/// * **AI configured** → [`RouteTarget::InsertOnly`], which puts the words in
///   the Command Center input and stops, exactly as typing them would. Dictation
///   is a way of typing; the default should behave like typing rather than
///   guess, and anyone who wants the model can say "ask" (see the `kw-ai` group
///   in `settings::model::default_keyword_groups`).
///
/// `Some(_)` always wins, including `Some(RouteTarget::PrimaryAi)` — which is
/// the whole reason the field is an `Option`. It used to be a bare `RouteTarget`
/// defaulting to `PrimaryAi`, so "wants the AI" and "never opened this dropdown"
/// were the same value and neither could be served without breaking the other.
fn effective_fallback(
    configured_fallback: Option<RouteTarget>,
    ai_configured: bool,
) -> RouteTarget {
    match configured_fallback {
        Some(chosen) => chosen,
        None if ai_configured => RouteTarget::InsertOnly,
        None => RouteTarget::WebSearch,
    }
}

/// Whether a usable primary AI backend is configured, purely from settings —
/// no network probe, no subprocess call to check whether e.g. Hermes is
/// actually installed. That mirrors what `agent::resolve_backend` and
/// `agent::openai::validate_config` already check *before* a real call is
/// attempted, so routing agrees with what would actually happen if the text
/// were sent to the AI, without paying for an I/O round trip on every
/// dictated word. An installed-but-not-yet-set-up backend (Hermes present but
/// `hermes setup` never run) still counts as "configured" here for the same
/// reason `resolve_backend` accepts it: that failure mode already has its own
/// actionable error message at call time, and this flag exists to pick a
/// *default*, not to duplicate that diagnosis.
///
/// A backend that is merely *present in the list* is not enough — an
/// `OpenAiCompatible` entry with an empty base URL cannot answer anything, and
/// treating it as configured would silently swallow dictated speech into a
/// dead end. See `agent::openai::validate_config` for the identical check made
/// at call time.
pub fn ai_is_configured(agents: &AgentSettings) -> bool {
    let Some(id) = agents.primary_backend_id.as_deref() else {
        return false;
    };
    let Some(backend) = agents.backends.iter().find(|b| b.id == id) else {
        return false;
    };
    match backend.kind {
        // The explicit no-op backend: a fresh install can point here after
        // Hermes is removed, and it answers every chat with NotConfigured.
        BackendKind::Null => false,
        // No endpoint or key required by design — see `hermes_template`'s own
        // doc. Whether the `hermes` binary is actually on disk is a runtime
        // question `HermesBackend::chat` answers with its own actionable
        // error, not one this settings-only check tries to guess at.
        BackendKind::Hermes => true,
        // Needs somewhere to send the request and something to ask for.
        // Neither requires an API key — a local Ollama/LM Studio server takes
        // none — so `has_api_key` is deliberately not part of this check.
        BackendKind::OpenAiCompatible => {
            !backend.base_url.trim().is_empty() && !backend.model.trim().is_empty()
        }
    }
}

struct Candidate {
    /// Length of the normalised keyword; longer means more specific.
    specificity: usize,
    route: RouteTarget,
    text: String,
    group_name: String,
    keyword: String,
}

/// Lowercase, collapse whitespace, and drop characters that are neither
/// alphanumeric nor a separator — so "Computer," matches "computer".
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_was_space = false;
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        }
        // Everything else (punctuation) is dropped.
    }
    out.trim_end().to_string()
}

/// True when `haystack` begins with `needle` at a word boundary.
fn starts_with_word(haystack: &str, needle: &str) -> bool {
    match haystack.strip_prefix(needle) {
        Some(rest) => rest.is_empty() || rest.starts_with(' '),
        None => false,
    }
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(idx) = haystack[from..].find(needle) {
        let start = from + idx;
        let end = start + needle.len();
        let before_ok = start == 0 || haystack.as_bytes()[start - 1] == b' ';
        let after_ok = end == haystack.len() || haystack.as_bytes()[end] == b' ';
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// Remove the matched keyword (and a trailing filler word) from the *original*
/// text, so capitalisation and punctuation in the remainder are preserved.
fn strip_leading(original: &str, normalized_needle: &str) -> String {
    // Walk the original in step with its normalised form to find where the
    // keyword ends. Counting words is enough: `normalize` never merges or
    // splits them.
    let word_count = normalized_needle.split(' ').filter(|w| !w.is_empty()).count();
    let rest: String = original
        .split_whitespace()
        .skip(word_count)
        .collect::<Vec<_>>()
        .join(" ");

    // "look up the weather" reads better as "the weather" than "up the weather".
    const FILLERS: &[&str] = &["for", "up", "about", "the internet for", "online for"];
    let lower = rest.to_lowercase();
    for filler in FILLERS {
        if let Some(stripped) = lower.strip_prefix(filler) {
            if stripped.is_empty() || stripped.starts_with(' ') {
                return rest[filler.len()..].trim().to_string();
            }
        }
    }

    rest.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{BackendConfig, BackendKind, KeywordGroup, VoiceSettings};

    fn defaults() -> VoiceSettings {
        VoiceSettings::default()
    }

    /// Every test below that is only exercising keyword matching (not the
    /// fallback) routes with AI configured — that is the common case, and it
    /// keeps these assertions reading exactly as they did before Delta 1.
    const AI_CONFIGURED: bool = true;
    const AI_NOT_CONFIGURED: bool = false;

    /// The `kw-ai` group this patch proposes for
    /// `settings::model::default_keyword_groups` (see the report's exact
    /// diff). `default_keyword_groups()` itself is owned by another change,
    /// so tests that need this group build it here rather than assuming
    /// `VoiceSettings::default()` already carries it.
    fn ai_keyword_group() -> KeywordGroup {
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
        }
    }

    /// The shipped defaults plus the proposed `kw-ai` group, so tests can
    /// exercise the three built-in groups together the way a real install
    /// will once the `model.rs` diff lands.
    fn defaults_with_ai() -> VoiceSettings {
        let mut s = defaults();
        s.keyword_groups.push(ai_keyword_group());
        s
    }

    #[test]
    fn default_search_keywords_route_to_the_browser_and_strip_themselves() {
        let r = route("search cheap flights to Lisbon", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.text, "cheap flights to Lisbon");
        assert_eq!(r.matched_keyword.as_deref(), Some("search"));
    }

    #[test]
    fn multiword_keywords_are_stripped_whole() {
        let r = route("look up the weather in Tokyo", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::WebSearch);
        // "up" is a filler after "look", so it goes too.
        assert_eq!(r.text, "the weather in Tokyo");
    }

    #[test]
    fn computer_keywords_route_to_computer_use() {
        for phrase in ["computer open my email", "jarvis close all tabs"] {
            let r = route(phrase, &defaults(), AI_CONFIGURED);
            assert_eq!(r.route, RouteTarget::ComputerUse, "{phrase}");
        }
    }

    #[test]
    fn the_longest_keyword_wins_regardless_of_group_order() {
        // "search my mac" is in the computer-use group; "search" is in the web
        // group and comes first in the list.
        let r = route("search my mac for that pdf", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("search my mac"));
        assert_eq!(r.text, "that pdf");
    }

    // -----------------------------------------------------------------
    // Delta 1 — the fallback depends on whether AI is configured.
    //
    // `no_match_falls_back_to_the_configured_default` and
    // `an_ordinary_sentence_still_goes_to_the_ai` (below) previously asserted
    // that an unmatched transcript reaches `RouteTarget::PrimaryAi`. The spec
    // explicitly reverses that ("today the fallback is PrimaryAi... this is
    // the change people will notice most"), so both are rewritten rather than
    // kept passing by accident.
    // -----------------------------------------------------------------

    #[test]
    fn unmatched_transcript_is_plain_search_when_ai_is_configured() {
        // Delta 1, rule 2: no keyword matched, AI configured → InsertOnly,
        // not PrimaryAi. This is the replacement for the old
        // `no_match_falls_back_to_the_configured_default`.
        let r = route("what is the capital of Peru", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::InsertOnly);
        assert_eq!(r.text, "what is the capital of Peru");
        assert!(r.matched_group.is_none());
    }

    #[test]
    fn unmatched_transcript_is_web_search_when_ai_is_not_configured() {
        // Delta 1, rule 1: "no exceptions" — even an ordinary question goes to
        // the browser rather than bouncing off an unconfigured AI backend.
        let r = route("what is the capital of Peru", &defaults(), AI_NOT_CONFIGURED);
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.text, "what is the capital of Peru");
        assert!(r.matched_group.is_none());
    }

    #[test]
    fn a_keyword_match_ignores_ai_configured_either_way() {
        // The fallback split only applies once nothing has matched. A phrase
        // that matches a real keyword group must route the same way
        // regardless of whether AI happens to be configured.
        for ai_configured in [AI_CONFIGURED, AI_NOT_CONFIGURED] {
            let r = route("search cheap flights to Lisbon", &defaults(), ai_configured);
            assert_eq!(r.route, RouteTarget::WebSearch, "ai_configured={ai_configured}");
        }
    }

    #[test]
    fn an_unset_fallback_follows_whether_ai_is_configured() {
        // Direct unit tests for the resolver, independent of keyword matching.
        assert_eq!(effective_fallback(None, true), RouteTarget::InsertOnly);
        assert_eq!(effective_fallback(None, false), RouteTarget::WebSearch);
    }

    #[test]
    fn a_chosen_fallback_always_wins_including_primary_ai() {
        // `Some(PrimaryAi)` is the case the `Option` exists for: it used to be
        // indistinguishable from "never chose", so honouring it and varying the
        // default by `ai_configured` were mutually exclusive.
        for chosen in [
            RouteTarget::WebSearch,
            RouteTarget::PrimaryAi,
            RouteTarget::ComputerUse,
            RouteTarget::InsertOnly,
            RouteTarget::ClipboardSearch,
        ] {
            assert_eq!(effective_fallback(Some(chosen), true), chosen);
            assert_eq!(effective_fallback(Some(chosen), false), chosen);
        }
    }

    // -----------------------------------------------------------------
    // `ai_is_configured` — settings-only, no I/O.
    // -----------------------------------------------------------------

    fn agents_with(backends: Vec<BackendConfig>, primary_id: Option<&str>) -> AgentSettings {
        let mut agents = AgentSettings::default();
        agents.backends = backends;
        agents.primary_backend_id = primary_id.map(str::to_string);
        agents
    }

    #[test]
    fn no_primary_backend_id_is_not_configured() {
        let agents = agents_with(vec![], None);
        assert!(!ai_is_configured(&agents));
    }

    #[test]
    fn a_dangling_primary_backend_id_is_not_configured() {
        let agents = agents_with(vec![], Some("deleted"));
        assert!(!ai_is_configured(&agents));
    }

    #[test]
    fn the_null_backend_is_not_configured() {
        let backend = BackendConfig {
            id: "null".into(),
            kind: BackendKind::Null,
            ..Default::default()
        };
        let agents = agents_with(vec![backend], Some("null"));
        assert!(!ai_is_configured(&agents));
    }

    #[test]
    fn hermes_is_configured_with_no_endpoint_or_key_at_all() {
        // Hermes needs neither — it uses whatever `hermes setup` already
        // configured. Whether the binary is actually installed is a runtime
        // question this settings-only check does not attempt to answer.
        let backend = BackendConfig {
            id: "hermes".into(),
            kind: BackendKind::Hermes,
            ..Default::default()
        };
        let agents = agents_with(vec![backend], Some("hermes"));
        assert!(ai_is_configured(&agents));
    }

    #[test]
    fn openai_compatible_needs_a_base_url_and_a_model() {
        let base = BackendConfig {
            id: "local".into(),
            kind: BackendKind::OpenAiCompatible,
            ..Default::default()
        };

        // Neither set.
        let agents = agents_with(vec![base.clone()], Some("local"));
        assert!(!ai_is_configured(&agents));

        // Base URL only.
        let mut with_url = base.clone();
        with_url.base_url = "http://localhost:11434/v1".into();
        let agents = agents_with(vec![with_url], Some("local"));
        assert!(!ai_is_configured(&agents));

        // Both set — genuinely usable, no API key required (a local server
        // takes none).
        let mut ready = base;
        ready.base_url = "http://localhost:11434/v1".into();
        ready.model = "llama3.2".into();
        let agents = agents_with(vec![ready], Some("local"));
        assert!(ai_is_configured(&agents));
    }

    #[test]
    fn a_fresh_install_is_configured_out_of_the_box() {
        // `AgentSettings::default()` ships Hermes as the primary backend, so
        // a brand-new install should already read as "AI configured" — the
        // no-config default is Hermes, not silence.
        assert!(ai_is_configured(&AgentSettings::default()));
    }

    #[test]
    fn matching_ignores_case_and_punctuation() {
        let r = route("Computer, open Finder.", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "open Finder.");
    }

    #[test]
    fn keywords_only_match_at_a_word_boundary() {
        // "searching" must not trigger the "search" keyword, so this falls
        // all the way through to the fallback.
        let r = route("searching for meaning", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::InsertOnly);
    }

    #[test]
    fn anywhere_mode_matches_mid_sentence_and_keeps_the_text() {
        let mut s = defaults();
        s.keyword_groups = vec![KeywordGroup {
            id: "k".into(),
            name: "Computer".into(),
            keywords: vec!["jarvis".into()],
            route: RouteTarget::ComputerUse,
            match_mode: KeywordMatch::Anywhere,
            enabled: true,
        }];
        let r = route("okay jarvis please open Finder", &s, AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "okay jarvis please open Finder");
    }

    #[test]
    fn disabled_groups_are_skipped() {
        let mut s = defaults();
        for g in &mut s.keyword_groups {
            g.enabled = false;
        }
        assert_eq!(
            route("search for cats", &s, AI_CONFIGURED).route,
            RouteTarget::InsertOnly
        );
    }

    #[test]
    fn a_bare_keyword_yields_empty_text_rather_than_repeating_itself() {
        let r = route("search", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.text, "");
    }

    #[test]
    fn empty_and_whitespace_input_is_safe() {
        assert_eq!(route("", &defaults(), AI_CONFIGURED).text, "");
        assert_eq!(route("    ", &defaults(), AI_CONFIGURED).text, "");
    }

    #[test]
    fn empty_keywords_never_match_everything() {
        let mut s = defaults();
        s.keyword_groups = vec![KeywordGroup {
            id: "k".into(),
            name: "Broken".into(),
            keywords: vec!["".into(), "   ".into()],
            route: RouteTarget::ComputerUse,
            match_mode: KeywordMatch::LeadingWords,
            enabled: true,
        }];
        assert_eq!(
            route("anything at all", &s, AI_CONFIGURED).route,
            RouteTarget::InsertOnly
        );
    }

    #[test]
    fn every_search_verb_people_actually_say_reaches_the_web() {
        // Each of these is a phrase somebody will say out loud expecting a
        // search, and each must strip the instruction rather than search for it.
        for (said, expected) in [
            ("google the best pasta in Rome", "the best pasta in Rome"),
            ("bing tide times", "tide times"),
            ("search for a train to Bath", "a train to Bath"),
            ("look up the weather", "the weather"),
            ("browse for cheap flights", "cheap flights"),
            ("search the web for rust lifetimes", "rust lifetimes"),
            ("search the internet for rust lifetimes", "rust lifetimes"),
            ("internet who won the match", "who won the match"),
        ] {
            let r = route(said, &defaults(), AI_CONFIGURED);
            assert_eq!(r.route, RouteTarget::WebSearch, "{said} did not route to the web");
            assert_eq!(r.text, expected, "{said} kept the instruction in the query");
        }
    }

    #[test]
    fn control_phrases_beat_the_bare_computer_keyword() {
        // "control my computer" is longer than "computer", so specificity has
        // to pick it — otherwise the stripped text would keep "my computer".
        let r = route("control my computer and open Mail", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("control my computer"));
        assert_eq!(r.text, "and open Mail");

        let r = route("computer use close every tab", &defaults(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "close every tab");
    }

    #[test]
    fn an_ordinary_sentence_goes_to_plain_search_when_ai_is_configured() {
        // Renamed from `an_ordinary_sentence_still_goes_to_the_ai`: Delta 1
        // deliberately reverses this. Adding search and control keywords must
        // still not turn every sentence containing one of those words into a
        // search — that part of the point survives — but the unmatched
        // destination is now plain Command Center search, not the AI.
        for said in [
            "how do I get a web server running on this machine",
            "write me an email about the internet outage",
            "what does the computer science department do",
        ] {
            assert_eq!(
                route(said, &defaults(), AI_CONFIGURED).route,
                RouteTarget::InsertOnly,
                "{said}"
            );
        }
    }

    #[test]
    fn normalization_collapses_whitespace_and_strips_punctuation() {
        assert_eq!(normalize("  Hello,   World!  "), "hello world");
        assert_eq!(normalize("search-my-mac"), "search my mac");
        assert_eq!(normalize("!!!"), "");
    }

    // -----------------------------------------------------------------
    // Delta 2 — the `kw-ai` group.
    // -----------------------------------------------------------------

    #[test]
    fn ai_keyword_phrases_route_to_primary_ai_and_strip_themselves() {
        for (said, expected_text) in [
            ("ask ai what's the weather", "what's the weather"),
            ("ask claude to summarise this", "to summarise this"),
            ("ask chat how tall is Everest", "how tall is Everest"),
            ("hey caduceus what time is it", "what time is it"),
            ("hey ai open my calendar", "open my calendar"),
        ] {
            let r = route(said, &defaults_with_ai(), AI_CONFIGURED);
            assert_eq!(r.route, RouteTarget::PrimaryAi, "{said}");
            assert_eq!(r.text, expected_text, "{said}");
        }
    }

    #[test]
    fn a_bare_ask_still_reaches_the_ai_but_loses_to_a_longer_ai_phrase() {
        // Bare "ask" is the shortest keyword in the group and must lose to
        // "ask ai" on the same transcript — the same specificity rule that
        // already makes "search my mac" beat "search".
        let r = route("ask ai to open Finder", &defaults_with_ai(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::PrimaryAi);
        assert_eq!(r.matched_keyword.as_deref(), Some("ask ai"));

        // On its own, with nothing more specific to lose to, "ask" still
        // works — it is the group's catch-all.
        let r = route("ask what time it is", &defaults_with_ai(), AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::PrimaryAi);
        assert_eq!(r.matched_keyword.as_deref(), Some("ask"));
        assert_eq!(r.text, "what time it is");
    }

    #[test]
    fn the_ai_group_does_not_shadow_search_or_computer_phrases() {
        // Mind the existing longest-match-wins rule: adding a bare "ask"
        // keyword must not change the outcome for phrases that belong to the
        // other two groups, including the specificity cases their own tests
        // already cover ("search my mac" beating "search").
        let s = defaults_with_ai();

        let r = route("search my mac for that pdf", &s, AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("search my mac"));

        let r = route("search cheap flights to Lisbon", &s, AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.matched_keyword.as_deref(), Some("search"));

        let r = route("control my computer and open Mail", &s, AI_CONFIGURED);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("control my computer"));
    }

    #[test]
    fn without_the_ai_group_voice_can_still_reach_the_ai_via_the_keyword() {
        // Why the `kw-ai` group had to ship in the same change as the new
        // fallback, and why nobody should quietly delete it later: with the
        // fallback no longer defaulting to the AI, that group is the *only*
        // route voice has to it. Strip it — which is also what an older
        // settings file looks like — and "ask ai …" lands in the Command
        // Center input like any other sentence.
        let mut stripped = defaults();
        stripped.keyword_groups.retain(|g| g.id != "kw-ai");

        let r = route(
            "ask ai what's the capital of France",
            &stripped,
            AI_CONFIGURED,
        );
        assert_ne!(r.route, RouteTarget::PrimaryAi);
        assert_eq!(r.route, RouteTarget::InsertOnly);

        // And with the group present — the shipped default — it gets there.
        let r = route(
            "ask ai what's the capital of France",
            &defaults(),
            AI_CONFIGURED,
        );
        assert_eq!(r.route, RouteTarget::PrimaryAi);
        assert_eq!(r.text, "what's the capital of France");
    }
}
