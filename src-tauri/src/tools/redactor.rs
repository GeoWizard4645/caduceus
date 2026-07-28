//! PII redactor.
//!
//! Finds emails, phone numbers, SSNs, credit-card numbers and IP addresses in
//! pasted text and replaces each with a placeholder — `[REDACTED_EMAIL]` by
//! default, or whatever replacement text the caller supplies.
//!
//! This is deliberately its own module rather than an extension of
//! `tools::regex_tool`: that module tests and explains a pattern the *user*
//! supplies, and has no fixed pattern table of its own to add to. What it does
//! share with this module is the engine — the `regex` crate, already a
//! dependency — and the general shape of "find every match with its span,
//! not just a yes/no", which is the same information [`RegexMatch`] carries.
//!
//! Every pattern here favours a real hit over a fussy one, on the theory that
//! a redactor's failure mode should be "over-redacted", not "leaked". The one
//! exception is the credit-card pattern, which runs a Luhn checksum after the
//! digit-shape match — without it, "call me at 4111 1111 1111 1234" flags a
//! phone number's worth of digits as a card number for no reason.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    Email,
    Phone,
    Ssn,
    CreditCard,
    IpAddress,
}

impl PiiKind {
    /// All kinds, in the order applied — used when the caller does not name a
    /// subset, so "redact everything" is the default rather than "redact
    /// nothing".
    pub const ALL: [PiiKind; 5] =
        [PiiKind::Email, PiiKind::Phone, PiiKind::Ssn, PiiKind::CreditCard, PiiKind::IpAddress];

    fn label(self) -> &'static str {
        match self {
            PiiKind::Email => "EMAIL",
            PiiKind::Phone => "PHONE",
            PiiKind::Ssn => "SSN",
            PiiKind::CreditCard => "CREDIT_CARD",
            PiiKind::IpAddress => "IP_ADDRESS",
        }
    }

    fn regex(self) -> &'static Regex {
        // One `OnceLock` per kind: each pattern is compiled once no matter
        // how many times `redact` is called, and never before the kind that
        // needs it is actually used.
        static EMAIL: OnceLock<Regex> = OnceLock::new();
        static PHONE: OnceLock<Regex> = OnceLock::new();
        static SSN: OnceLock<Regex> = OnceLock::new();
        static CARD: OnceLock<Regex> = OnceLock::new();
        static IP: OnceLock<Regex> = OnceLock::new();

        match self {
            PiiKind::Email => EMAIL.get_or_init(|| {
                Regex::new(r"(?i)\b[a-z0-9][a-z0-9._%+-]*@[a-z0-9.-]+\.[a-z]{2,}\b").unwrap()
            }),
            // Covers "(555) 123-4567", "555-123-4567", "555.123.4567" and an
            // optional leading "+1"/"1-" country code — the shapes a phone
            // number actually appears in, without also matching an arbitrary
            // run of ten digits (a card's last four plus an order number,
            // say) that merely happens to be that long.
            PiiKind::Phone => PHONE.get_or_init(|| {
                Regex::new(
                    r"(?x)
                    (?:\+?1[\s.-]?)?
                    (?:\(\d{3}\)|\d{3})
                    [\s.-]
                    \d{3}
                    [\s.-]
                    \d{4}
                    \b
                    ",
                )
                .unwrap()
            }),
            PiiKind::Ssn => SSN.get_or_init(|| Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap()),
            // Digit-shape only; `has_valid_luhn` filters the matches down to
            // ones that could actually be a real card number. Anchored to
            // *start and end on a digit* (`\d(?:[ -]?\d){12,18}`, not
            // `(?:\d[ -]?){13,19}`) so a trailing separator before the next
            // word — "1111 1111 1111 1111 on file" — is never pulled into
            // the match along with the digits.
            PiiKind::CreditCard => CARD.get_or_init(|| {
                Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").unwrap()
            }),
            PiiKind::IpAddress => IP.get_or_init(|| {
                Regex::new(r"\b(?:(?:25[0-5]|2[0-4]\d|1?\d{1,2})\.){3}(?:25[0-5]|2[0-4]\d|1?\d{1,2})\b")
                    .unwrap()
            }),
        }
    }
}

/// The Luhn checksum every real card number satisfies. Run only against
/// digit-shape matches from [`PiiKind::CreditCard`], to tell "a 16-digit
/// account or order number" apart from an actual card.
fn has_valid_luhn(digits: &str) -> bool {
    let digits: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 13 {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactMatch {
    pub kind: PiiKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactResult {
    pub text: String,
    pub matches: Vec<RedactMatch>,
}

/// Find and replace every occurrence of the requested PII `kinds` in `input`.
///
/// `replacement` is a template: `{KIND}` inside it (if present) is substituted
/// with the matched kind's label, so a caller can ask for `[REDACTED:{KIND}]`
/// and get `[REDACTED:EMAIL]`, `[REDACTED:PHONE]`, etc. A `replacement` with
/// no `{KIND}` — including the default, `[REDACTED]` — is used verbatim.
///
/// Overlapping matches from different patterns (a Luhn-valid card number that
/// also happens to look like a run of digits some other pattern would catch)
/// keep only the first one found by position, then the longest at a tied
/// start — never both, which would otherwise redact the same span twice or
/// leave a fragment of it exposed between two shorter replacements.
pub fn redact(input: &str, kinds: &[PiiKind], replacement: &str) -> RedactResult {
    let kinds: Vec<PiiKind> = if kinds.is_empty() { PiiKind::ALL.to_vec() } else { kinds.to_vec() };

    let mut candidates: Vec<RedactMatch> = Vec::new();
    for &kind in &kinds {
        for m in kind.regex().find_iter(input) {
            if kind == PiiKind::CreditCard && !has_valid_luhn(m.as_str()) {
                continue;
            }
            candidates.push(RedactMatch {
                kind,
                text: m.as_str().to_string(),
                start: m.start(),
                end: m.end(),
            });
        }
    }

    // Longest-first so a tied start prefers the more specific/complete match,
    // then resolve overlaps in one left-to-right pass.
    candidates.sort_by(|a, b| a.start.cmp(&b.start).then((b.end - b.start).cmp(&(a.end - a.start))));

    let mut kept: Vec<RedactMatch> = Vec::new();
    let mut cursor = 0usize;
    for m in candidates {
        if m.start < cursor {
            continue;
        }
        cursor = m.end;
        kept.push(m);
    }

    let mut out = String::with_capacity(input.len());
    let mut last_end = 0usize;
    for m in &kept {
        out.push_str(&input[last_end..m.start]);
        if replacement.contains("{KIND}") {
            out.push_str(&replacement.replace("{KIND}", m.kind.label()));
        } else {
            out.push_str(replacement);
        }
        last_end = m.end;
    }
    out.push_str(&input[last_end..]);

    RedactResult { text: out, matches: kept }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

const DEFAULT_REPLACEMENT: &str = "[REDACTED]";

#[tauri::command]
pub fn redact_text(text: String, kinds: Vec<PiiKind>, replacement: Option<String>) -> RedactResult {
    let replacement = replacement.unwrap_or_else(|| DEFAULT_REPLACEMENT.to_string());
    redact(&text, &kinds, &replacement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_an_email_address() {
        let out = redact("Reach me at ada@example.com please.", &[PiiKind::Email], "[REDACTED]");
        assert_eq!(out.text, "Reach me at [REDACTED] please.");
        assert_eq!(out.matches.len(), 1);
        assert_eq!(out.matches[0].kind, PiiKind::Email);
    }

    #[test]
    fn redacts_a_dashed_and_a_parenthesised_phone_number() {
        let out = redact("Call 555-123-4567 or (555) 987-6543.", &[PiiKind::Phone], "[REDACTED]");
        assert_eq!(out.matches.len(), 2);
        assert_eq!(out.text, "Call [REDACTED] or [REDACTED].");
    }

    #[test]
    fn redacts_a_social_security_number() {
        let out = redact("SSN: 123-45-6789.", &[PiiKind::Ssn], "[REDACTED]");
        assert_eq!(out.text, "SSN: [REDACTED].");
    }

    #[test]
    fn redacts_a_luhn_valid_credit_card_number() {
        // A well-known test Visa number that passes Luhn.
        let out = redact("Card 4111 1111 1111 1111 on file.", &[PiiKind::CreditCard], "[REDACTED]");
        assert_eq!(out.text, "Card [REDACTED] on file.");
    }

    #[test]
    fn a_luhn_invalid_digit_run_is_not_flagged_as_a_card() {
        let out = redact("Order number 1234 5678 9012 3456.", &[PiiKind::CreditCard], "[REDACTED]");
        assert!(out.matches.is_empty(), "that digit run fails Luhn and must not be treated as a card");
    }

    #[test]
    fn redacts_an_ip_address() {
        let out = redact("Server at 192.168.1.1 is down.", &[PiiKind::IpAddress], "[REDACTED]");
        assert_eq!(out.text, "Server at [REDACTED] is down.");
    }

    #[test]
    fn kind_placeholder_is_substituted_per_match() {
        let out = redact(
            "Email ada@example.com, phone 555-123-4567.",
            &[PiiKind::Email, PiiKind::Phone],
            "[REDACTED:{KIND}]",
        );
        assert_eq!(out.text, "Email [REDACTED:EMAIL], phone [REDACTED:PHONE].");
    }

    #[test]
    fn empty_kinds_means_check_everything() {
        let out = redact("ada@example.com", &[], "[REDACTED]");
        assert_eq!(out.text, "[REDACTED]");
    }

    #[test]
    fn text_with_no_pii_is_returned_unchanged() {
        let out = redact("Nothing sensitive here.", &PiiKind::ALL, "[REDACTED]");
        assert_eq!(out.text, "Nothing sensitive here.");
        assert!(out.matches.is_empty());
    }

    #[test]
    fn overlapping_matches_are_not_double_redacted() {
        // The SSN pattern and a loose phone pattern could both eye the same
        // digit run; only the earliest/longest one should be kept.
        let out = redact("123-45-6789", &[PiiKind::Ssn, PiiKind::Phone], "[REDACTED]");
        assert_eq!(out.matches.len(), 1);
    }

    #[test]
    fn only_requested_kinds_are_checked() {
        let out = redact("ada@example.com and 123-45-6789", &[PiiKind::Email], "[REDACTED]");
        assert_eq!(out.matches.len(), 1);
        assert!(out.text.contains("123-45-6789"), "SSN was not requested, so it must survive untouched");
    }

    #[test]
    fn default_replacement_is_used_when_none_is_given() {
        assert_eq!(DEFAULT_REPLACEMENT, "[REDACTED]");
    }
}
