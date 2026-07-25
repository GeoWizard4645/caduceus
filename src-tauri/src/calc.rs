//! Inline calculator for the palette.
//!
//! Typing `1920/16*9` or `18% of 240` shows the answer as the top result. This
//! is a real recursive-descent parser rather than a regex or an `eval`: it has
//! to get operator precedence right (`2+3*4` is 14, not 20), and shelling out
//! to something that evaluates arbitrary expressions would be a code-execution
//! hole in a box that also runs an AI agent.
//!
//! # Grammar
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := power (('*' | '/' | '%' | 'mod') power)*
//! power   := unary ('^' power)?           -- right associative
//! unary   := ('-' | '+')? postfix
//! postfix := primary '%'?                 -- trailing % means "percent of"
//! primary := number | '(' expr ')' | func '(' expr ')' | constant
//! ```
//!
//! Deliberately *not* supported: variables, assignment, comparison. The point
//! is arithmetic you would otherwise open Spotlight for, not a language.

/// A successfully evaluated expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Calculation {
    /// The input, normalised for display.
    pub expression: String,
    /// Formatted result, e.g. `1,080` or `3.14159`.
    pub display: String,
    pub value: f64,
}

/// Evaluate `input` if it looks like arithmetic, otherwise `None`.
///
/// Returning `None` for non-mathematical input is the important half: the
/// palette shows this result above web search, so a false positive on "2 cats"
/// would be worse than missing an edge case.
pub fn evaluate(input: &str) -> Option<Calculation> {
    let cleaned = normalise(input);
    if cleaned.is_empty() || !looks_like_math(&cleaned) {
        return None;
    }

    let mut parser = Parser {
        chars: cleaned.chars().collect(),
        pos: 0,
    };
    let value = parser.expression()?;
    parser.skip_whitespace();
    // Trailing junk means we misread the input; better to show nothing.
    if parser.pos != parser.chars.len() || !value.is_finite() {
        return None;
    }

    Some(Calculation {
        expression: input.trim().to_string(),
        display: format_number(value),
        value,
    })
}

/// Rewrite the friendly forms into the grammar's vocabulary.
fn normalise(input: &str) -> String {
    let mut s = input.trim().to_lowercase();

    // "18% of 240" -> "18% * 240"
    s = s.replace(" of ", " * ");
    // Typographic and worded operators.
    s = s
        .replace('\u{d7}', "*") // ×
        .replace('\u{f7}', "/") // ÷
        .replace('\u{2212}', "-") // −
        .replace(" plus ", "+")
        .replace(" minus ", "-")
        .replace(" times ", "*")
        .replace(" divided by ", "/");
    // Thousands separators, but only between digits, so "1,234" works while
    // a stray comma still fails to parse.
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, c) in bytes.iter().enumerate() {
        if *c == ',' {
            let before = i.checked_sub(1).and_then(|j| bytes.get(j)).is_some_and(|c| c.is_ascii_digit());
            let after = bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit());
            if before && after {
                continue;
            }
        }
        out.push(*c);
    }
    // Strip a trailing '=' so "2+2=" works.
    out.trim().trim_end_matches('=').trim().to_string()
}

/// Cheap gate before parsing: the input must contain a digit or a constant, and
/// at least one operator or function. Without this, a bare `5` would render a
/// calculator row for every number typed.
fn looks_like_math(s: &str) -> bool {
    let has_operator = s.contains(['+', '-', '*', '/', '^', '%', '(']);
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_named = ["sqrt", "sin", "cos", "tan", "log", "ln", "abs", "round", "pi", "e"]
        .iter()
        .any(|f| s.contains(f));

    // Reject anything with letters that are not a known function or constant,
    // so "3 apples" and "win 10" do not become calculations.
    let letters_ok = {
        let mut rest = s.to_string();
        for name in ["sqrt", "sin", "cos", "tan", "log", "ln", "abs", "round", "mod", "pi"] {
            rest = rest.replace(name, "");
        }
        // A lone `e` survives as the constant.
        rest.chars().all(|c| !c.is_alphabetic() || c == 'e')
    };

    letters_ok && (has_digit || has_named) && (has_operator || has_named)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn skip_whitespace(&mut self) {
        while matches!(self.chars.get(self.pos), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_whitespace();
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn eat_word(&mut self, word: &str) -> bool {
        self.skip_whitespace();
        let end = self.pos + word.chars().count();
        if end <= self.chars.len() && self.chars[self.pos..end].iter().collect::<String>() == word {
            self.pos = end;
            return true;
        }
        false
    }

    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                value += self.term()?;
            } else if self.eat('-') {
                value -= self.term()?;
            } else {
                return Some(value);
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        loop {
            if self.eat('*') {
                value *= self.power()?;
            } else if self.eat('/') {
                let divisor = self.power()?;
                if divisor == 0.0 {
                    return None; // no result beats showing `inf`
                }
                value /= divisor;
            } else if self.eat_word("mod") {
                let divisor = self.power()?;
                if divisor == 0.0 {
                    return None;
                }
                value %= divisor;
            } else {
                return Some(value);
            }
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat('^') {
            // Right associative: 2^3^2 is 2^(3^2).
            let exponent = self.power()?;
            return Some(base.powf(exponent));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Option<f64> {
        let value = self.primary()?;
        // A trailing % is "per cent": 18% is 0.18, so "18% * 240" gives 43.2.
        if self.peek() == Some('%') {
            self.pos += 1;
            return Some(value / 100.0);
        }
        Some(value)
    }

    fn primary(&mut self) -> Option<f64> {
        self.skip_whitespace();

        if self.eat('(') {
            let value = self.expression()?;
            return self.eat(')').then_some(value);
        }

        for (name, f) in [
            ("sqrt", f64::sqrt as fn(f64) -> f64),
            ("sin", f64::sin),
            ("cos", f64::cos),
            ("tan", f64::tan),
            ("log", f64::log10),
            ("ln", f64::ln),
            ("abs", f64::abs),
            ("round", f64::round),
        ] {
            if self.eat_word(name) {
                // Parentheses are optional: `sqrt 16` reads fine.
                let argument = if self.eat('(') {
                    let v = self.expression()?;
                    if !self.eat(')') {
                        return None;
                    }
                    v
                } else {
                    self.unary()?
                };
                return Some(f(argument));
            }
        }

        if self.eat_word("pi") {
            return Some(std::f64::consts::PI);
        }

        // The constant `e`, but only when not the exponent marker in `1e5`.
        if self.peek() == Some('e')
            && !matches!(self.chars.get(self.pos + 1), Some(c) if c.is_ascii_digit() || *c == '-' || *c == '+')
        {
            self.pos += 1;
            return Some(std::f64::consts::E);
        }

        self.number()
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_whitespace();
        let start = self.pos;

        while matches!(self.chars.get(self.pos), Some(c) if c.is_ascii_digit() || *c == '.') {
            self.pos += 1;
        }
        // Scientific notation: 1e5, 2.5e-3.
        if matches!(self.chars.get(self.pos), Some('e'))
            && matches!(self.chars.get(self.pos + 1), Some(c) if c.is_ascii_digit() || *c == '-' || *c == '+')
        {
            self.pos += 2;
            while matches!(self.chars.get(self.pos), Some(c) if c.is_ascii_digit()) {
                self.pos += 1;
            }
        }

        if self.pos == start {
            return None;
        }
        self.chars[start..self.pos].iter().collect::<String>().parse().ok()
    }
}

/// Format for display: thousands separators, and no trailing `.0` on integers.
fn format_number(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return group_thousands(&format!("{}", value as i64));
    }
    // Round to 10 significant decimals, then drop trailing zeros, so
    // 0.1+0.2 shows 0.3 rather than 0.30000000000000004.
    let mut s = format!("{value:.10}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    match s.split_once('.') {
        Some((whole, frac)) => format!("{}.{}", group_thousands(whole), frac),
        None => group_thousands(&s),
    }
}

fn group_thousands(digits: &str) -> String {
    let (sign, body) = digits.strip_prefix('-').map_or(("", digits), |b| ("-", b));
    let mut out = String::with_capacity(body.len() + body.len() / 3);
    for (i, c) in body.chars().enumerate() {
        if i > 0 && (body.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(input: &str) -> f64 {
        evaluate(input).unwrap_or_else(|| panic!("{input:?} should evaluate")).value
    }

    fn display(input: &str) -> String {
        evaluate(input).unwrap_or_else(|| panic!("{input:?} should evaluate")).display
    }

    #[test]
    fn respects_operator_precedence() {
        assert_eq!(value("2+3*4"), 14.0);
        assert_eq!(value("(2+3)*4"), 20.0);
        assert_eq!(value("2+3*4-6/3"), 12.0);
    }

    #[test]
    fn exponentiation_is_right_associative() {
        assert_eq!(value("2^3^2"), 512.0);
        assert_eq!(value("2^10"), 1024.0);
    }

    #[test]
    fn handles_unary_minus() {
        assert_eq!(value("-5+3"), -2.0);
        assert_eq!(value("3*-2"), -6.0);
        assert_eq!(value("-(4+1)"), -5.0);
    }

    #[test]
    fn percentages_read_naturally() {
        // Compared with a tolerance: 18/100 * 240 is 43.199999999999996 in
        // binary floating point. What matters is that the *display* is clean,
        // which is asserted separately below.
        assert!((value("18% * 240") - 43.2).abs() < 1e-9);
        assert!((value("18% of 240") - 43.2).abs() < 1e-9);
        assert_eq!(value("50%"), 0.5);
        assert_eq!(display("18% of 240"), "43.2");
    }

    #[test]
    fn supports_functions_and_constants() {
        assert_eq!(value("sqrt(16)"), 4.0);
        assert_eq!(value("sqrt 16"), 4.0);
        assert_eq!(value("round(2.7)"), 3.0);
        assert!((value("pi*2") - std::f64::consts::TAU).abs() < 1e-12);
    }

    #[test]
    fn the_practical_case_works() {
        // Working out a 16:9 height from a width is the example that made me
        // want this in the first place.
        assert_eq!(display("1920/16*9"), "1,080");
    }

    #[test]
    fn formats_large_and_fractional_numbers_readably() {
        assert_eq!(display("1000000*3"), "3,000,000");
        assert_eq!(display("10/4"), "2.5");
        assert_eq!(display("-1234*2"), "-2,468");
        // Floating point noise must not leak into the UI.
        assert_eq!(display("0.1+0.2"), "0.3");
    }

    #[test]
    fn accepts_thousands_separators_and_trailing_equals() {
        assert_eq!(value("1,234+1"), 1235.0);
        assert_eq!(value("2+2="), 4.0);
    }

    #[test]
    fn accepts_typographic_operators() {
        assert_eq!(value("6\u{d7}7"), 42.0);
        assert_eq!(value("84\u{f7}2"), 42.0);
    }

    #[test]
    fn ignores_input_that_is_not_arithmetic() {
        // These all reach the palette constantly; none may show a result row.
        for input in [
            "", "hello", "3 apples", "how to add 2+2", "win 10", "claude",
            "meeting at 5", "v2.1", "a+b",
        ] {
            assert!(evaluate(input).is_none(), "{input:?} must not be a calculation");
        }
    }

    #[test]
    fn a_bare_number_is_not_a_calculation() {
        // Otherwise every numeric search would show a pointless result row.
        assert!(evaluate("42").is_none());
        assert!(evaluate("2024").is_none());
    }

    #[test]
    fn division_by_zero_yields_nothing_rather_than_infinity() {
        assert!(evaluate("1/0").is_none());
        assert!(evaluate("5 mod 0").is_none());
    }

    #[test]
    fn unbalanced_or_trailing_input_is_rejected() {
        assert!(evaluate("(2+3").is_none());
        assert!(evaluate("2+3)").is_none());
        assert!(evaluate("2+3 4").is_none());
        assert!(evaluate("2+").is_none());
    }

    #[test]
    fn scientific_notation_parses() {
        assert_eq!(value("1e3+1"), 1001.0);
        assert_eq!(value("2.5e-1*4"), 1.0);
    }
}
