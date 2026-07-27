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

use serde::Serialize;

use crate::settings::{KeywordMatch, RouteTarget, VoiceSettings};

/// The outcome of routing a transcript.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedText {
    /// Where this should go.
    pub route: RouteTarget,
    /// The text after any keyword was stripped. Never empty unless the input
    /// was only a keyword.
    pub text: String,
    /// Which group matched, for the UI to explain itself. `None` = fallback.
    pub matched_group: Option<String>,
    /// The keyword that matched.
    pub matched_keyword: Option<String>,
}

/// Route a transcript according to the user's voice settings.
pub fn route(transcript: &str, settings: &VoiceSettings) -> RoutedText {
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
            matched_group: Some(c.group_name),
            matched_keyword: Some(c.keyword),
        },
        None => RoutedText {
            route: settings.fallback_route,
            text: cleaned.to_string(),
            matched_group: None,
            matched_keyword: None,
        },
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
    use crate::settings::{KeywordGroup, VoiceSettings};

    fn defaults() -> VoiceSettings {
        VoiceSettings::default()
    }

    #[test]
    fn default_search_keywords_route_to_the_browser_and_strip_themselves() {
        let r = route("search cheap flights to Lisbon", &defaults());
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.text, "cheap flights to Lisbon");
        assert_eq!(r.matched_keyword.as_deref(), Some("search"));
    }

    #[test]
    fn multiword_keywords_are_stripped_whole() {
        let r = route("look up the weather in Tokyo", &defaults());
        assert_eq!(r.route, RouteTarget::WebSearch);
        // "up" is a filler after "look", so it goes too.
        assert_eq!(r.text, "the weather in Tokyo");
    }

    #[test]
    fn computer_keywords_route_to_computer_use() {
        for phrase in ["computer open my email", "jarvis close all tabs"] {
            let r = route(phrase, &defaults());
            assert_eq!(r.route, RouteTarget::ComputerUse, "{phrase}");
        }
    }

    #[test]
    fn the_longest_keyword_wins_regardless_of_group_order() {
        // "search my mac" is in the computer-use group; "search" is in the web
        // group and comes first in the list.
        let r = route("search my mac for that pdf", &defaults());
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("search my mac"));
        assert_eq!(r.text, "that pdf");
    }

    #[test]
    fn no_match_falls_back_to_the_configured_default() {
        let r = route("what is the capital of Peru", &defaults());
        assert_eq!(r.route, RouteTarget::PrimaryAi);
        assert_eq!(r.text, "what is the capital of Peru");
        assert!(r.matched_group.is_none());
    }

    #[test]
    fn matching_ignores_case_and_punctuation() {
        let r = route("Computer, open Finder.", &defaults());
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "open Finder.");
    }

    #[test]
    fn keywords_only_match_at_a_word_boundary() {
        // "searching" must not trigger the "search" keyword.
        let r = route("searching for meaning", &defaults());
        assert_eq!(r.route, RouteTarget::PrimaryAi);
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
        let r = route("okay jarvis please open Finder", &s);
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "okay jarvis please open Finder");
    }

    #[test]
    fn disabled_groups_are_skipped() {
        let mut s = defaults();
        for g in &mut s.keyword_groups {
            g.enabled = false;
        }
        assert_eq!(route("search for cats", &s).route, RouteTarget::PrimaryAi);
    }

    #[test]
    fn a_bare_keyword_yields_empty_text_rather_than_repeating_itself() {
        let r = route("search", &defaults());
        assert_eq!(r.route, RouteTarget::WebSearch);
        assert_eq!(r.text, "");
    }

    #[test]
    fn empty_and_whitespace_input_is_safe() {
        assert_eq!(route("", &defaults()).text, "");
        assert_eq!(route("    ", &defaults()).text, "");
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
        assert_eq!(route("anything at all", &s).route, RouteTarget::PrimaryAi);
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
            let r = route(said, &defaults());
            assert_eq!(r.route, RouteTarget::WebSearch, "{said} did not route to the web");
            assert_eq!(r.text, expected, "{said} kept the instruction in the query");
        }
    }

    #[test]
    fn control_phrases_beat_the_bare_computer_keyword() {
        // "control my computer" is longer than "computer", so specificity has
        // to pick it — otherwise the stripped text would keep "my computer".
        let r = route("control my computer and open Mail", &defaults());
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.matched_keyword.as_deref(), Some("control my computer"));
        assert_eq!(r.text, "and open Mail");

        let r = route("computer use close every tab", &defaults());
        assert_eq!(r.route, RouteTarget::ComputerUse);
        assert_eq!(r.text, "close every tab");
    }

    #[test]
    fn an_ordinary_sentence_still_goes_to_the_ai() {
        // The point of the default. Adding search and control keywords must not
        // turn every sentence containing one of those words into a search.
        for said in [
            "how do I get a web server running on this machine",
            "write me an email about the internet outage",
            "what does the computer science department do",
        ] {
            assert_eq!(route(said, &defaults()).route, RouteTarget::PrimaryAi, "{said}");
        }
    }

    #[test]
    fn normalization_collapses_whitespace_and_strips_punctuation() {
        assert_eq!(normalize("  Hello,   World!  "), "hello world");
        assert_eq!(normalize("search-my-mac"), "search my mac");
        assert_eq!(normalize("!!!"), "");
    }
}
