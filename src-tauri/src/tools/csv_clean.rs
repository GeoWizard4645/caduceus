//! CSV / table cleaner.
//!
//! Takes whatever a spreadsheet, a database export or a pasted email table
//! produced — inconsistent delimiters, stray whitespace, ragged rows, exact
//! duplicate rows — and hands back a normalised CSV: comma-delimited, quoted
//! only where a field actually needs it, every row the same width.
//!
//! No new dependency: `Cargo.toml` has no `csv` crate, and this file is a
//! small enough RFC 4180-style parser/writer (quoted fields, doubled-quote
//! escaping, embedded delimiters and newlines inside quotes) that adding one
//! for it would be a heavier dependency than the fifty lines it replaces —
//! the same call `tools::cron` and `tools::expander::markdown_to_html` make
//! for their own hand-written parsers.

use serde::{Deserialize, Serialize};

/// Delimiters this module knows how to auto-detect. Order matters only as a
/// tie-break: earlier wins a tied vote, and comma is the most common format by
/// a wide margin, so it goes first.
const CANDIDATE_DELIMITERS: [char; 4] = [',', ';', '\t', '|'];

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CsvCleanOptions {
    /// A single character to split fields on. `None` auto-detects from the
    /// input's first few lines.
    pub delimiter: Option<String>,
    /// Trim leading/trailing whitespace from every field.
    pub trim: bool,
    /// Drop exact-duplicate rows (compared after trimming), keeping the first
    /// occurrence.
    pub dedupe: bool,
    /// Treat the first row as a header: it is never counted as a duplicate of
    /// a later row, and it sets the "correct" column count that ragged rows
    /// are padded or truncated to. Turn off for headerless data, where the
    /// most common row length is used instead.
    pub has_header: bool,
}

impl Default for CsvCleanOptions {
    fn default() -> Self {
        Self { delimiter: None, trim: true, dedupe: true, has_header: true }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CsvCleanResult {
    pub csv: String,
    pub rows: usize,
    pub columns: usize,
    pub duplicates_removed: usize,
    pub ragged_rows_fixed: usize,
    /// The delimiter actually used to parse the input — useful to show back
    /// to the user when it was auto-detected rather than chosen.
    pub detected_delimiter: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse `input` into rows of fields, honouring RFC 4180 quoting: a field
/// wrapped in `"..."` may contain the delimiter, a newline, or a doubled `""`
/// standing for one literal quote.
fn parse(input: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();
    let mut saw_any_content = false;

    while let Some(c) = chars.next() {
        saw_any_content = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }

        match c {
            '"' if field.is_empty() => in_quotes = true,
            '"' => field.push(c),
            c if c == delimiter => {
                row.push(std::mem::take(&mut field));
            }
            '\r' => {
                // Bare CR or the CR half of CRLF; the LF (if any) is consumed
                // on the next iteration and closes the row on its own.
                if chars.peek() != Some(&'\n') {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }

    // A trailing field/row with no final newline still counts.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }

    // Drop a single wholly-empty trailing row, the artefact of a file that
    // ends with a newline — not real data, and it would otherwise show up as
    // a one-column ragged row of "".
    if saw_any_content {
        if let Some(last) = rows.last() {
            if last.len() == 1 && last[0].is_empty() {
                rows.pop();
            }
        }
    }

    rows
}

/// Count how many times `delimiter` appears outside quotes on one line —
/// enough signal to compare candidates without running the full parser on
/// each.
fn count_unquoted(line: &str, delimiter: char) -> usize {
    let mut in_quotes = false;
    let mut count = 0;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c == delimiter && !in_quotes => count += 1,
            _ => {}
        }
    }
    count
}

/// Auto-detect a delimiter by counting each candidate across up to the first
/// five non-empty lines and picking whichever is both present and most
/// consistent (same count on every sampled line beats a higher but uneven
/// count, since a real table has the same number of columns throughout).
fn detect_delimiter(input: &str) -> char {
    let sample: Vec<&str> = input.lines().filter(|l| !l.trim().is_empty()).take(5).collect();
    if sample.is_empty() {
        return ',';
    }

    let mut best: Option<(char, usize, bool)> = None; // (delimiter, count, consistent)
    for &delim in &CANDIDATE_DELIMITERS {
        let counts: Vec<usize> = sample.iter().map(|l| count_unquoted(l, delim)).collect();
        let first = counts[0];
        if first == 0 {
            continue;
        }
        let consistent = counts.iter().all(|&c| c == first);
        let better = match &best {
            None => true,
            Some((_, best_count, best_consistent)) => {
                (consistent && !best_consistent)
                    || (consistent == *best_consistent && first > *best_count)
            }
        };
        if better {
            best = Some((delim, first, consistent));
        }
    }

    best.map(|(d, _, _)| d).unwrap_or(',')
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Quote a field only if it needs it: contains the delimiter, a quote, or a
/// newline. Internal quotes are doubled.
fn write_field(out: &mut String, field: &str) {
    let needs_quoting =
        field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r');
    if !needs_quoting {
        out.push_str(field);
        return;
    }
    out.push('"');
    for c in field.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
}

fn write_csv(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        for (i, field) in row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_field(&mut out, field);
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

/// The column count ragged rows get normalised to: the header's width when
/// `has_header` is set, otherwise the most common row width (ties keep the
/// first one seen).
fn target_width(rows: &[Vec<String>], has_header: bool) -> usize {
    if has_header {
        return rows.first().map(|r| r.len()).unwrap_or(0);
    }
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for row in rows {
        match counts.iter_mut().find(|(w, _)| *w == row.len()) {
            Some((_, n)) => *n += 1,
            None => counts.push((row.len(), 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(w, _)| w).unwrap_or(0)
}

/// Parse, trim, de-ragged and dedupe `input`, returning the cleaned CSV plus
/// what was done to it.
pub fn clean(input: &str, options: &CsvCleanOptions) -> Result<CsvCleanResult, String> {
    if input.trim().is_empty() {
        return Err("There is no table to clean yet.".into());
    }

    let delimiter = match &options.delimiter {
        Some(d) if !d.is_empty() => d.chars().next().unwrap(),
        _ => detect_delimiter(input),
    };

    let mut rows = parse(input, delimiter);
    if rows.is_empty() {
        return Err("Could not find any rows in that input.".into());
    }

    if options.trim {
        for row in &mut rows {
            for field in row.iter_mut() {
                let trimmed = field.trim();
                if trimmed.len() != field.len() {
                    *field = trimmed.to_string();
                }
            }
        }
    }

    let width = target_width(&rows, options.has_header).max(1);
    let mut ragged_rows_fixed = 0;
    for row in &mut rows {
        if row.len() != width {
            ragged_rows_fixed += 1;
            row.resize(width, String::new());
        }
    }

    let mut duplicates_removed = 0;
    if options.dedupe {
        let mut seen: std::collections::HashSet<Vec<String>> = std::collections::HashSet::new();
        let header = if options.has_header && !rows.is_empty() { Some(rows.remove(0)) } else { None };

        let mut deduped = Vec::with_capacity(rows.len());
        for row in rows {
            if seen.insert(row.clone()) {
                deduped.push(row);
            } else {
                duplicates_removed += 1;
            }
        }

        rows = match header {
            Some(h) => {
                let mut with_header = Vec::with_capacity(deduped.len() + 1);
                with_header.push(h);
                with_header.extend(deduped);
                with_header
            }
            None => deduped,
        };
    }

    Ok(CsvCleanResult {
        csv: write_csv(&rows),
        rows: rows.len(),
        columns: width,
        duplicates_removed,
        ragged_rows_fixed,
        detected_delimiter: delimiter.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn csv_clean(input: String, options: Option<CsvCleanOptions>) -> Result<CsvCleanResult, String> {
    clean(&input, &options.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> CsvCleanOptions {
        CsvCleanOptions::default()
    }

    #[test]
    fn trims_whitespace_from_every_field() {
        let out = clean(" a , b \n c , d \n", &opts()).unwrap();
        assert_eq!(out.csv, "a,b\nc,d\n");
    }

    #[test]
    fn pads_a_short_row_to_the_header_width() {
        let out = clean("a,b,c\n1,2\n", &opts()).unwrap();
        assert_eq!(out.csv, "a,b,c\n1,2,\n");
        assert_eq!(out.ragged_rows_fixed, 1);
        assert_eq!(out.columns, 3);
    }

    #[test]
    fn truncates_a_long_row_to_the_header_width() {
        let out = clean("a,b\n1,2,3,4\n", &opts()).unwrap();
        assert_eq!(out.csv, "a,b\n1,2\n");
        assert_eq!(out.ragged_rows_fixed, 1);
    }

    #[test]
    fn removes_exact_duplicate_data_rows_but_keeps_the_header() {
        let out = clean("name,age\nAda,30\nAda,30\nGrace,40\n", &opts()).unwrap();
        assert_eq!(out.csv, "name,age\nAda,30\nGrace,40\n");
        assert_eq!(out.duplicates_removed, 1);
        assert_eq!(out.rows, 3); // header + 2 unique data rows
    }

    #[test]
    fn dedupe_can_be_turned_off() {
        let mut o = opts();
        o.dedupe = false;
        let out = clean("name\nAda\nAda\n", &o).unwrap();
        assert_eq!(out.csv, "name\nAda\nAda\n");
        assert_eq!(out.duplicates_removed, 0);
    }

    #[test]
    fn auto_detects_semicolon_delimited_input() {
        let out = clean("a;b;c\n1;2;3\n", &opts()).unwrap();
        assert_eq!(out.detected_delimiter, ";");
        assert_eq!(out.csv, "a,b,c\n1,2,3\n");
    }

    #[test]
    fn auto_detects_tab_delimited_input() {
        let out = clean("a\tb\n1\t2\n", &opts()).unwrap();
        assert_eq!(out.detected_delimiter, "\t");
        assert_eq!(out.csv, "a,b\n1,2\n");
    }

    #[test]
    fn a_quoted_field_may_contain_the_delimiter() {
        let out = clean("name,note\n\"Doe, Jane\",hi\n", &opts()).unwrap();
        assert_eq!(out.csv, "name,note\n\"Doe, Jane\",hi\n");
    }

    #[test]
    fn a_doubled_quote_inside_a_quoted_field_is_one_literal_quote() {
        let out = clean("name\n\"She said \"\"hi\"\"\"\n", &opts()).unwrap();
        assert_eq!(out.csv, "name\n\"She said \"\"hi\"\"\"\n");
    }

    #[test]
    fn a_quoted_field_may_contain_an_embedded_newline() {
        let input = "name,note\n\"Bob\",\"line one\nline two\"\n";
        let out = clean(input, &opts()).unwrap();
        assert_eq!(out.csv, "name,note\nBob,\"line one\nline two\"\n");
        assert_eq!(out.rows, 2);
    }

    #[test]
    fn an_explicit_delimiter_overrides_detection() {
        let mut o = opts();
        o.delimiter = Some(";".into());
        let out = clean("a,b;c\n", &o).unwrap();
        assert_eq!(out.detected_delimiter, ";");
    }

    #[test]
    fn empty_input_is_refused_before_parsing() {
        assert!(clean("   ", &opts()).is_err());
    }

    #[test]
    fn a_trailing_newline_does_not_create_a_phantom_empty_row() {
        let out = clean("a,b\n1,2\n", &opts()).unwrap();
        assert_eq!(out.rows, 2);
    }

    #[test]
    fn headerless_mode_uses_the_most_common_row_width() {
        let mut o = opts();
        o.has_header = false;
        // Two rows of width 2, one ragged row of width 3 — target is 2.
        let out = clean("a,b\n1,2\nx,y,z\n", &o).unwrap();
        assert_eq!(out.columns, 2);
        assert_eq!(out.ragged_rows_fixed, 1);
    }

    #[test]
    fn without_a_header_the_first_row_can_be_deduped_too() {
        let mut o = opts();
        o.has_header = false;
        let out = clean("a,b\na,b\n", &o).unwrap();
        assert_eq!(out.duplicates_removed, 1);
        assert_eq!(out.rows, 1);
    }
}
