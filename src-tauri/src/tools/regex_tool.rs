//! Testing a regular expression against sample text, and explaining one in
//! plain English.
//!
//! The engine is the `regex` crate — RE2-style, so it cannot backtrack
//! catastrophically the way PCRE-flavoured patterns can, which matters here
//! because the pattern and the text are both typed by whoever is testing them:
//! nothing here needs a timeout to stay responsive against pathological input.
//! The trade is that a handful of PCRE features do not exist — backreferences
//! and possessive quantifiers chief among them — and a pattern using either
//! is refused at compile time with the crate's own message rather than
//! silently doing something else.
//!
//! The explainer is not a PCRE grammar and does not try to be one. It walks
//! the pattern left to right, describing one atom (and the quantifier
//! immediately after it, if any) at a time, which is what "token by token"
//! means for a regular expression: nobody wants a parse tree, they want to
//! know what `\d+` is asking for.

use regex::{Regex, RegexBuilder};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Testing
// ---------------------------------------------------------------------------

/// A search this tester will not attempt.
///
/// Not a safety limit — the `regex` crate's matching is linear in the input
/// regardless of the pattern — just a sign that whatever was pasted is a file,
/// not a sample, and a representative excerpt will answer the same question
/// far more readably.
pub const MAX_TEXT_LEN: usize = 200_000;

/// How many matches to collect before stopping.
///
/// A pattern that matches on every character of a long input (`x*` against a
/// page of text) can produce more results than anyone is going to read; this
/// caps the work and the payload at the point a match list stops being useful
/// and starts being a second copy of the input.
pub const MAX_MATCHES: usize = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureGroup {
    /// 1-based, matching how people already think and talk about `$1`.
    pub index: usize,
    pub name: Option<String>,
    /// `None` when this group did not participate in the match — an
    /// alternative inside it was not the one taken, which is a normal outcome
    /// and not the same thing as an empty string.
    pub text: Option<String>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegexMatch {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub groups: Vec<CaptureGroup>,
}

/// Run `pattern` against `haystack` and report every match.
///
/// `flags` is any combination of `i` (case-insensitive), `m` (`^`/`$` match at
/// line boundaries, not just the string's), `s` (`.` matches a newline too)
/// and `x` (ignore unescaped whitespace and `#` comments in the pattern, for
/// writing a long pattern across several lines). Any other letter is ignored
/// rather than rejected, on the theory that a typo in a flags box should not
/// be the reason the pattern does not run at all.
pub fn test(pattern: &str, flags: &str, haystack: &str) -> Result<Vec<RegexMatch>, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("There is no pattern to test yet.".into());
    }
    if haystack.chars().count() > MAX_TEXT_LEN {
        return Err(format!(
            "That is more text than a tester needs — {MAX_TEXT_LEN} characters is already a very \
             thorough sample. Try trimming it to the part that matters."
        ));
    }

    let re = build_regex(pattern, flags)?;
    let mut results = Vec::new();

    for caps in re.captures_iter(haystack) {
        if results.len() >= MAX_MATCHES {
            break;
        }
        // `captures_iter` only yields captures for an actual match, and group
        // 0 (the whole match) is always present in one — this cannot be `None`.
        let whole = caps.get(0).expect("a match always has group 0");

        let groups = (1..caps.len())
            .map(|index| {
                let name = re.capture_names().nth(index).flatten().map(str::to_string);
                match caps.get(index) {
                    Some(m) => CaptureGroup {
                        index,
                        name,
                        text: Some(m.as_str().to_string()),
                        start: Some(char_index(haystack, m.start())),
                        end: Some(char_index(haystack, m.end())),
                    },
                    None => CaptureGroup { index, name, text: None, start: None, end: None },
                }
            })
            .collect();

        results.push(RegexMatch {
            text: whole.as_str().to_string(),
            start: char_index(haystack, whole.start()),
            end: char_index(haystack, whole.end()),
            groups,
        });
    }

    Ok(results)
}

fn build_regex(pattern: &str, flags: &str) -> Result<Regex, String> {
    let mut builder = RegexBuilder::new(pattern);
    for flag in flags.chars() {
        match flag {
            'i' => {
                builder.case_insensitive(true);
            }
            'm' => {
                builder.multi_line(true);
            }
            's' => {
                builder.dot_matches_new_line(true);
            }
            'x' => {
                builder.ignore_whitespace(true);
            }
            _ => {} // unrecognised — see the doc comment on `test`
        }
    }
    builder.build().map_err(|e| readable_regex_error(&e))
}

fn readable_regex_error(e: &regex::Error) -> String {
    // The crate's own `Display` already names the offending character and
    // shows where it is in the pattern — better than anything worth
    // reconstructing from its (non-exhaustive) `Error` enum.
    format!("That is not a valid pattern:\n{e}")
}

/// Byte offset → character offset.
///
/// `regex` works in UTF-8 bytes because Rust strings are UTF-8, but a
/// position shown next to a textarea needs to line up with what the browser
/// (and the person counting on their fingers) considers a character, not a
/// byte — the two only diverge on non-ASCII input, which is exactly the input
/// where getting this wrong would be most confusing.
fn char_index(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset].chars().count()
}

// ---------------------------------------------------------------------------
// Explaining
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainToken {
    /// The exact slice of the pattern this description covers.
    pub token: String,
    pub description: String,
}

/// Explain `pattern`, one atom (plus its quantifier, if any) at a time.
pub fn explain(pattern: &str) -> Result<Vec<ExplainToken>, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("There is no pattern to explain yet.".into());
    }
    // Compiled first: walking a broken pattern token by token would just
    // reach the same syntax error one character later, which is a worse way
    // to find out than the engine's own message, given up front.
    Regex::new(pattern).map_err(|e| readable_regex_error(&e))?;

    let chars: Vec<char> = pattern.chars().collect();
    let mut group_counter = 0usize;
    Ok(tokenize(&chars, &mut group_counter))
}

/// Walk one sequence of the pattern (the whole thing, or the inside of a
/// group) into a flat list of described atoms. Shared by [`explain`] and
/// group handling, which is what makes a group's own contents show up
/// summarised rather than opaque.
fn tokenize(chars: &[char], group_counter: &mut usize) -> Vec<ExplainToken> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (atom_text, atom_desc, next_i) = parse_atom(chars, i, group_counter);
        i = next_i;

        if let Some((q_text, q_desc, next_i2)) = parse_quantifier(chars, i) {
            tokens.push(ExplainToken {
                token: format!("{atom_text}{q_text}"),
                description: format!("{atom_desc}, {q_desc}"),
            });
            i = next_i2;
        } else {
            tokens.push(ExplainToken { token: atom_text, description: atom_desc });
        }
    }
    tokens
}

fn is_special(c: char) -> bool {
    matches!(c, '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '|')
}

/// One "thing that can be quantified": a literal run, an escape, a class, a
/// group, an anchor, or `|`.
fn parse_atom(chars: &[char], i: usize, group_counter: &mut usize) -> (String, String, usize) {
    match chars[i] {
        '^' => ("^".into(), "the start of the string (or of a line, in multi-line mode)".into(), i + 1),
        '$' => ("$".into(), "the end of the string (or of a line, in multi-line mode)".into(), i + 1),
        '.' => (".".into(), "any character except a line break".into(), i + 1),
        '\\' => parse_escape(chars, i),
        '[' => parse_class(chars, i),
        '(' => parse_group(chars, i, group_counter),
        '|' => (
            "|".into(),
            "OR — matches everything before this point, or everything after it".into(),
            i + 1,
        ),
        _ => {
            let end = literal_run_end(chars, i);
            let text: String = chars[i..end].iter().collect();
            let desc = if end - i == 1 {
                format!("the character '{text}'")
            } else {
                format!("the literal text '{text}'")
            };
            (text, desc, end)
        }
    }
}

/// How far a run of plain literal characters extends from `start`.
///
/// Stops one character early whenever the *next* position starts a
/// quantifier, because a quantifier binds to a single preceding atom, not to
/// a run — `abc+` repeats `c`, not `abc`, and folding all three into one
/// token would describe the wrong thing repeating.
fn literal_run_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && !is_special(chars[i]) {
        if parse_quantifier(chars, i + 1).is_some() {
            // If the run has nothing in it yet, this character has to be
            // included (alone) so it has something to attach the quantifier
            // to next; otherwise it is left for the *next* call to pick up on
            // its own.
            return if i == start { i + 1 } else { i };
        }
        i += 1;
    }
    i
}

fn parse_escape(chars: &[char], i: usize) -> (String, String, usize) {
    if i + 1 >= chars.len() {
        return ("\\".into(), "a trailing backslash".into(), i + 1);
    }
    let c = chars[i + 1];
    let simple = |desc: &str| (chars[i..i + 2].iter().collect::<String>(), desc.to_string(), i + 2);
    match c {
        'd' => simple("a digit (0-9)"),
        'D' => simple("any character that is not a digit"),
        'w' => simple("a word character (a letter, digit or underscore)"),
        'W' => simple("any character that is not a word character"),
        's' => simple("a whitespace character"),
        'S' => simple("any character that is not whitespace"),
        'b' => simple("a word boundary"),
        'B' => simple("a position that is not a word boundary"),
        'n' => simple("a newline"),
        't' => simple("a tab"),
        'r' => simple("a carriage return"),
        'A' => simple("the very start of the string, ignoring multi-line mode"),
        'z' => simple("the very end of the string, ignoring multi-line mode"),
        'p' | 'P' => {
            let negate = c == 'P';
            let verb = if negate { "is not in" } else { "is in" };
            if chars.get(i + 2) == Some(&'{') {
                if let Some(close) = chars[i + 3..].iter().position(|&ch| ch == '}').map(|p| p + i + 3) {
                    let name: String = chars[i + 3..close].iter().collect();
                    return (
                        chars[i..=close].iter().collect(),
                        format!("a character that {verb} the Unicode category '{name}'"),
                        close + 1,
                    );
                }
            } else if let Some(&name_char) = chars.get(i + 2) {
                return (
                    chars[i..i + 3].iter().collect(),
                    format!("a character that {verb} the Unicode category '{name_char}'"),
                    i + 3,
                );
            }
            simple("an incomplete Unicode property escape")
        }
        // Any other escaped character is just that character, literally —
        // covers `\.`, `\(`, `\/` and the rest of the punctuation someone
        // escapes out of habit or to be safe.
        other => (chars[i..i + 2].iter().collect(), format!("a literal '{other}' character"), i + 2),
    }
}

/// Find the `]` closing a class that opened at `content_start` (the position
/// right after `[`), honouring the two special cases that make character
/// classes their own little grammar: a leading `^` negates without closing,
/// and a `]` immediately after that (or after `[`) is a literal member rather
/// than the closer.
fn find_class_end(chars: &[char], content_start: usize) -> usize {
    let mut i = content_start;
    if chars.get(i) == Some(&'^') {
        i += 1;
    }
    if chars.get(i) == Some(&']') {
        i += 1;
    }
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == ']' {
            return i;
        }
        i += 1;
    }
    // Unreachable once the whole pattern has already compiled successfully —
    // an unterminated class would have failed at `Regex::new` — but a
    // fallback avoids indexing past the end if that assumption is ever wrong.
    chars.len()
}

fn parse_class(chars: &[char], i: usize) -> (String, String, usize) {
    let content_start = i + 1;
    let end = find_class_end(chars, content_start);
    let inner: String = chars[content_start..end].iter().collect();
    let (negated, body) = match inner.strip_prefix('^') {
        Some(rest) => (true, rest),
        None => (false, inner.as_str()),
    };
    let desc = if negated {
        format!("any character NOT in the set [{body}]")
    } else {
        format!("any character in the set [{body}]")
    };
    // `+ 1` to include the closing `]` itself; clamped because `find_class_end`
    // falls back to `chars.len()` for an unterminated class, which would
    // otherwise put the slice's end one past the last valid index.
    let text_end = (end + 1).min(chars.len());
    (chars[i..text_end].iter().collect(), desc, end + 1)
}

/// A `(...)` of some kind: a capture group, a non-capturing group, a named
/// capture, or a lookaround. Everything else that starts `(?` — inline flag
/// toggles chief among them — falls back to being described as a plain
/// group, since they are rare enough in hand-written patterns that inventing
/// prose for their syntax would cost more clarity than it buys.
fn parse_group(chars: &[char], i: usize, group_counter: &mut usize) -> (String, String, usize) {
    let close = matching_paren(chars, i).unwrap_or(chars.len().saturating_sub(1));

    let (prefix, content_start) = if chars.get(i + 1) == Some(&'?') {
        match chars.get(i + 2) {
            Some(':') => ("a non-capturing group".to_string(), i + 3),
            Some('=') => ("a lookahead (matches only if what follows here is)".to_string(), i + 3),
            Some('!') => {
                ("a negative lookahead (matches only if what follows here is NOT)".to_string(), i + 3)
            }
            Some('<') if chars.get(i + 3) == Some(&'=') => {
                ("a lookbehind (matches only if what came before here is)".to_string(), i + 4)
            }
            Some('<') if chars.get(i + 3) == Some(&'!') => (
                "a negative lookbehind (matches only if what came before here is NOT)".to_string(),
                i + 4,
            ),
            Some('<') => named_group(chars, i + 3, group_counter),
            Some('P') if chars.get(i + 3) == Some(&'<') => named_group(chars, i + 4, group_counter),
            _ => ("a group".to_string(), i + 2),
        }
    } else {
        *group_counter += 1;
        (format!("capture group {}", *group_counter), i + 1)
    };

    let inner = tokenize(&chars[content_start..close], group_counter);
    let inner_desc = if inner.is_empty() {
        "nothing — an empty group".to_string()
    } else {
        inner.iter().map(|t| t.description.as_str()).collect::<Vec<_>>().join(", then ")
    };

    (chars[i..=close].iter().collect(), format!("{prefix}, matching: {inner_desc}"), close + 1)
}

/// Parse the `name>` in `(?<name>` or `(?P<name>`, starting just after the
/// `<`. Returns the group's description and where its body begins.
fn named_group(chars: &[char], name_start: usize, group_counter: &mut usize) -> (String, usize) {
    *group_counter += 1;
    match chars[name_start..].iter().position(|&c| c == '>') {
        Some(offset) => {
            let name: String = chars[name_start..name_start + offset].iter().collect();
            (format!("capture group {} (named '{name}')", *group_counter), name_start + offset + 1)
        }
        None => (format!("capture group {}", *group_counter), name_start),
    }
}

/// Find the `)` matching the `(` at `open`, skipping over escaped characters
/// and the insides of character classes — both can contain an unescaped `(`
/// or `)` that means nothing to the paren nesting.
fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 1;
    let mut i = open + 1;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
            }
            i += 1;
            continue;
        }
        match c {
            '[' => in_class = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// If `chars[i]` starts a quantifier (`*`, `+`, `?`, or a `{...}` interval),
/// describe it and say how far it extends — including a trailing `?` that
/// makes it lazy.
fn parse_quantifier(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let (mut desc, mut len) = match *chars.get(i)? {
        '*' => ("repeated zero or more times".to_string(), 1),
        '+' => ("repeated one or more times".to_string(), 1),
        '?' => ("optional — zero or one time".to_string(), 1),
        '{' => {
            let close = chars[i..].iter().position(|&c| c == '}').map(|p| p + i)?;
            let body: String = chars[i + 1..close].iter().collect();
            (parse_interval(&body)?, close - i + 1)
        }
        _ => return None,
    };
    if chars.get(i + len) == Some(&'?') {
        desc.push_str(", as few times as possible");
        len += 1;
    }
    Some((chars[i..i + len].iter().collect(), desc, i + len))
}

fn parse_interval(body: &str) -> Option<String> {
    if let Some((a, b)) = body.split_once(',') {
        let a: u32 = a.parse().ok()?;
        if b.is_empty() {
            Some(format!("repeated {a} or more times"))
        } else {
            let b: u32 = b.parse().ok()?;
            Some(format!("repeated between {a} and {b} times"))
        }
    } else {
        let n: u32 = body.parse().ok()?;
        Some(format!("repeated exactly {n} times"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- test() --------------------------------------------------------

    #[test]
    fn finds_every_match_with_its_position() {
        let matches = test(r"\d+", "", "there are 12 cats and 340 dogs").unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].text, "12");
        assert_eq!(matches[0].start, 10);
        assert_eq!(matches[0].end, 12);
        assert_eq!(matches[1].text, "340");
    }

    #[test]
    fn reports_named_and_numbered_capture_groups() {
        let matches = test(r"(?P<year>\d{4})-(\d{2})-(\d{2})", "", "born 1990-04-12").unwrap();
        assert_eq!(matches.len(), 1);
        let groups = &matches[0].groups;
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].name.as_deref(), Some("year"));
        assert_eq!(groups[0].text.as_deref(), Some("1990"));
        assert_eq!(groups[1].name, None);
        assert_eq!(groups[2].text.as_deref(), Some("12"));
    }

    #[test]
    fn a_group_that_did_not_participate_reports_no_text() {
        // The second alternative wins, so group 1 (the first) never matched.
        let matches = test(r"(foo)|(bar)", "", "bar").unwrap();
        assert_eq!(matches[0].groups[0].text, None);
        assert_eq!(matches[0].groups[1].text.as_deref(), Some("bar"));
    }

    #[test]
    fn the_case_insensitive_flag_is_honoured() {
        assert!(test("hello", "", "HELLO").unwrap().is_empty());
        assert_eq!(test("hello", "i", "HELLO").unwrap().len(), 1);
    }

    #[test]
    fn positions_are_character_offsets_not_byte_offsets() {
        // "café " is 5 characters but 6 bytes (é is 2 bytes in UTF-8); the
        // digit after it must be reported at character position 5, not 6.
        let matches = test(r"\d+", "", "café 9").unwrap();
        assert_eq!(matches[0].start, 5);
    }

    #[test]
    fn an_invalid_pattern_is_refused_with_a_readable_message() {
        let err = test("(unclosed", "", "text").unwrap_err();
        assert!(err.contains("not a valid pattern"));
    }

    #[test]
    fn an_empty_pattern_is_refused_before_it_reaches_the_engine() {
        assert!(test("   ", "", "text").unwrap_err().contains("no pattern"));
    }

    #[test]
    fn unknown_flag_letters_are_ignored_rather_than_rejected() {
        assert!(test("abc", "iqz", "abc").is_ok());
    }

    // --- explain() -------------------------------------------------------

    #[test]
    fn digit_plus_reads_as_one_or_more_digits() {
        let tokens = explain(r"\d+").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token, r"\d+");
        assert!(tokens[0].description.contains("digit"));
        assert!(tokens[0].description.contains("one or more"));
    }

    #[test]
    fn a_quantifier_only_binds_to_the_character_right_before_it() {
        // `abc+` repeats only the 'c'; 'a' and 'b' are their own tokens.
        let tokens = explain("abc+").unwrap();
        let joined: Vec<&str> = tokens.iter().map(|t| t.token.as_str()).collect();
        assert_eq!(joined, vec!["ab", "c+"]);
    }

    #[test]
    fn a_character_class_is_described_with_its_contents() {
        let tokens = explain("[a-z0-9]").unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].description.contains("a-z0-9"));
    }

    #[test]
    fn a_negated_class_says_so() {
        let tokens = explain("[^0-9]").unwrap();
        assert!(tokens[0].description.contains("NOT"));
    }

    #[test]
    fn capture_groups_are_numbered_in_order() {
        let tokens = explain("(a)(b)").unwrap();
        assert!(tokens[0].description.contains("capture group 1"));
        assert!(tokens[1].description.contains("capture group 2"));
    }

    #[test]
    fn a_named_group_reports_its_name() {
        let tokens = explain(r"(?P<word>\w+)").unwrap();
        assert!(tokens[0].description.contains("named 'word'"));
    }

    #[test]
    fn a_non_capturing_group_is_told_apart_from_a_capturing_one() {
        let tokens = explain("(?:abc)").unwrap();
        assert!(tokens[0].description.starts_with("a non-capturing group"));
    }

    #[test]
    fn an_interval_quantifier_states_its_bounds() {
        let tokens = explain(r"a{2,5}").unwrap();
        assert!(tokens[0].description.contains("between 2 and 5"));
    }

    #[test]
    fn a_lazy_quantifier_is_called_out() {
        let tokens = explain(r"a+?").unwrap();
        assert!(tokens[0].description.contains("as few times as possible"));
    }

    #[test]
    fn anchors_and_alternation_get_their_own_tokens() {
        let tokens = explain("^a|b$").unwrap();
        let joined: Vec<&str> = tokens.iter().map(|t| t.token.as_str()).collect();
        assert_eq!(joined, vec!["^", "a", "|", "b", "$"]);
    }

    #[test]
    fn an_invalid_pattern_is_refused_before_explaining_anything() {
        assert!(explain("a(b").unwrap_err().contains("not a valid pattern"));
    }

    #[test]
    fn non_ascii_literals_explain_without_panicking() {
        let tokens = explain("café+").unwrap();
        assert!(tokens.iter().any(|t| t.token.contains('é')));
    }
}
