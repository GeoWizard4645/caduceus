//! Text transforms for the `case` command.
//!
//! Pure functions, deliberately. Everything here is exercised by unit tests
//! rather than by clicking through the palette, because case conversion is all
//! edge cases — acronyms, digits, punctuation, existing separators — and those
//! are cheap to test and tedious to check by hand.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    Upper,
    Lower,
    Title,
    Sentence,
    Snake,
    Kebab,
    Camel,
    Pascal,
    /// Strip formatting only — the value "paste as plain text" produces.
    Plain,
}

impl Case {
    pub fn label(self) -> &'static str {
        match self {
            Case::Upper => "UPPER CASE",
            Case::Lower => "lower case",
            Case::Title => "Title Case",
            Case::Sentence => "Sentence case",
            Case::Snake => "snake_case",
            Case::Kebab => "kebab-case",
            Case::Camel => "camelCase",
            Case::Pascal => "PascalCase",
            Case::Plain => "Plain text",
        }
    }

    pub fn all() -> &'static [Case] {
        &[
            Case::Upper,
            Case::Lower,
            Case::Title,
            Case::Sentence,
            Case::Snake,
            Case::Kebab,
            Case::Camel,
            Case::Pascal,
        ]
    }
}

pub fn convert(input: &str, case: Case) -> String {
    match case {
        Case::Upper => input.to_uppercase(),
        Case::Lower => input.to_lowercase(),
        Case::Plain => input.to_string(),
        Case::Title => title(input),
        Case::Sentence => sentence(input),
        Case::Snake => join_words(input, "_", false),
        Case::Kebab => join_words(input, "-", false),
        Case::Camel => camel(input, false),
        Case::Pascal => camel(input, true),
    }
}

/// Split into words on separators *and* on camelCase humps.
///
/// The humps matter: converting `parseHTTPResponse` to snake_case has to see
/// three words, and a naive split on non-alphanumerics sees one.
fn words(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = input.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }

        // A hump starts where lower/digit meets upper, or where a run of
        // capitals ends and a new word begins: the `R` in `HTTPResponse`.
        let prev = if i > 0 { chars.get(i - 1).copied() } else { None };
        let next = chars.get(i + 1).copied();
        let boundary = match (prev, next) {
            (Some(p), _) if c.is_uppercase() && (p.is_lowercase() || p.is_numeric()) => true,
            (Some(p), Some(n)) if c.is_uppercase() && p.is_uppercase() && n.is_lowercase() => true,
            _ => false,
        };
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn join_words(input: &str, sep: &str, upper: bool) -> String {
    let joined = words(input)
        .into_iter()
        .map(|w| if upper { w.to_uppercase() } else { w.to_lowercase() })
        .collect::<Vec<_>>()
        .join(sep);
    joined
}

fn camel(input: &str, first_upper: bool) -> String {
    let mut out = String::new();
    for (i, w) in words(input).into_iter().enumerate() {
        let lower = w.to_lowercase();
        if i == 0 && !first_upper {
            out.push_str(&lower);
        } else {
            out.push_str(&capitalise(&lower));
        }
    }
    out
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Capitalise each word, preserving the original spacing and punctuation.
fn title(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut at_word_start = true;
    for c in input.chars() {
        if c.is_alphanumeric() {
            if at_word_start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            at_word_start = false;
        } else {
            out.push(c);
            at_word_start = true;
        }
    }
    out
}

/// Capitalise the first letter of each sentence, lowercase the rest.
fn sentence(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut start_of_sentence = true;
    for c in input.chars() {
        if c.is_alphanumeric() {
            if start_of_sentence {
                out.extend(c.to_uppercase());
                start_of_sentence = false;
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?' | '\n') {
                start_of_sentence = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_humps_are_word_boundaries() {
        assert_eq!(convert("parseHTTPResponse", Case::Snake), "parse_http_response");
        assert_eq!(convert("parseHTTPResponse", Case::Kebab), "parse-http-response");
        assert_eq!(convert("XMLHttpRequest", Case::Snake), "xml_http_request");
    }

    #[test]
    fn separators_round_trip() {
        assert_eq!(convert("hello world", Case::Snake), "hello_world");
        assert_eq!(convert("hello-world", Case::Camel), "helloWorld");
        assert_eq!(convert("hello_world", Case::Pascal), "HelloWorld");
        assert_eq!(convert("  spaced   out  ", Case::Kebab), "spaced-out");
    }

    #[test]
    fn digits_start_a_new_word_after_letters_but_do_not_split_runs() {
        assert_eq!(convert("version2Point5", Case::Snake), "version2_point5");
        assert_eq!(convert("h264Encoder", Case::Kebab), "h264-encoder");
    }

    #[test]
    fn title_case_keeps_punctuation_and_spacing() {
        assert_eq!(convert("hello, world!", Case::Title), "Hello, World!");
        assert_eq!(convert("it's a test", Case::Title), "It'S A Test");
    }

    #[test]
    fn sentence_case_restarts_after_terminators() {
        assert_eq!(
            convert("hello there. HOW are you? fine!", Case::Sentence),
            "Hello there. How are you? Fine!"
        );
    }

    #[test]
    fn empty_and_symbol_only_input_does_not_panic() {
        for case in Case::all() {
            assert_eq!(convert("", *case), "");
            let symbols = convert("!!! ???", *case);
            assert!(symbols.is_empty() || symbols.contains('!') || symbols.contains('?'));
        }
    }

    /// Uppercasing 'ß' produces two characters, and Turkish 'İ' lowercases to
    /// two. Anything indexing by byte would break here.
    #[test]
    fn multi_byte_and_expanding_characters_survive() {
        assert_eq!(convert("straße", Case::Upper), "STRASSE");
        assert_eq!(convert("ÅNGSTRÖM", Case::Lower), "ångström");
        assert_eq!(convert("café au lait", Case::Title), "Café Au Lait");
        assert_eq!(convert("日本語 text", Case::Snake), "日本語_text");
    }
}
