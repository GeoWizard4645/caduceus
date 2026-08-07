//! A hand-rolled parser for `SKILL.md`'s YAML frontmatter.
//!
//! There is no YAML crate in this workspace and this feature may not add
//! one, so this is not "a small YAML parser" in the general sense — it
//! accepts exactly the bounded subset real skill files use and rejects
//! everything else with a line-numbered error, on the theory that a clear
//! rejection is safer than silently mis-parsing a construct we do not fully
//! understand. The subset was chosen by reading every bundled skill in the
//! reference implementation (`~/.hermes/skills/**/SKILL.md`), not guessed —
//! see the shape below, which is real.
//!
//! # Supported shape
//!
//! ```yaml
//! ---
//! name: apple-notes
//! description: "Manage Apple Notes via memo CLI: create, search, edit."
//! version: 1.0.1
//! platforms: [macos]
//! metadata:
//!   hermes:
//!     tags: [Notes, Apple, macOS, note-taking]
//!     related_skills: [obsidian]
//! prerequisites:
//!   commands: [memo]
//! ---
//! ```
//!
//! Concretely, a frontmatter document is:
//!
//! * A first line that is exactly `---` (a single leading UTF-8 BOM is
//!   stripped before this check — Windows editors add one, and left in
//!   place it would defeat the fence check the same way it does for
//!   Hermes; see `agent/skill_utils.py::parse_frontmatter`'s doc). `\r\n`
//!   line endings are normalized to `\n` before anything else runs, so CRLF
//!   files parse identically to LF ones.
//! * Closed by the next line that is exactly `---`. Everything after that
//!   line is the skill body, untouched.
//! * Between the fences, every line is one of:
//!   - blank, or a comment (first non-space character is `#`) — ignored;
//!   - a **mapping entry**, `key:` or `key: value`, where `key` is a bare
//!     identifier (`[A-Za-z0-9_.-]+`, no quoting, no spaces) and indentation
//!     (spaces only — a tab anywhere in the indent is rejected) determines
//!     nesting: lines indented deeper than a bare `key:` are that key's
//!     nested mapping or list, and sibling keys must share exactly one
//!     indent width;
//!   - a **block-list item**, `- value`, contributing one scalar to the
//!     list opened by the nearest bare `key:` at a shallower indent.
//! * A mapping entry's value, when present on the same line, is one of:
//!   - an inline flow sequence, `[a, b, c]` or `[]` (scalars only — no
//!     nested `[...]` or mappings inside the brackets);
//!   - a double-quoted scalar (`"..."`, understanding only `\"` and `\\`
//!     — any other backslash sequence, e.g. `\n`, passes through literally
//!     rather than being interpreted, so do not rely on it meaning anything
//!     other than the two characters it is);
//!   - a single-quoted scalar (`'...'`, where `''` is a literal `'`, YAML's
//!     usual single-quote escape and the only one this subset knows);
//!   - a plain, unquoted scalar — the rest of the line, comment-stripped
//!     (a ` #` preceded by whitespace and outside any quotes starts a
//!     comment) and trimmed.
//! * Colons that are not followed by a space or end-of-line do **not** end
//!   a key — `description: Ratio is 3:2` keeps `3:2` inside the value,
//!   matching real YAML's rule, not a naive "split on first colon."
//! * A plain scalar may not open with `& * ! | > % @ \`` or `{` — anchors,
//!   aliases, tags, block scalars, and flow mappings are refused outright
//!   rather than misread as text.
//! * A block-list item that itself looks like `key: value` is refused —
//!   lists of mappings (e.g. Hermes' `required_credential_files: [{path:
//!   ..., description: ...}]`-shaped fields) are out of scope; the field is
//!   rejected, not silently flattened into a wrong shape.
//! * Duplicate keys at the same nesting level are refused.
//!
//! `name` and `description` are the only fields any caller in this module
//! requires; everything else in the parsed map is preserved in
//! [`Frontmatter::raw`] for callers that want it (or nothing reads it at
//! all, and that is fine — an unrecognized key is not an error).

use std::collections::BTreeMap;

/// One parsed frontmatter value. YAML has more shapes than this — this is
/// deliberately only the three a skill file actually needs (see the module
/// doc's supported shape).
#[derive(Debug, Clone, PartialEq)]
pub enum YamlValue {
    Str(String),
    List(Vec<String>),
    Map(BTreeMap<String, YamlValue>),
}

impl YamlValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            YamlValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, YamlValue>> {
        match self {
            YamlValue::Map(m) => Some(m),
            _ => None,
        }
    }

    /// A list reading that also accepts a bare scalar as a one-element list
    /// (`platforms: macos` as well as `platforms: [macos]`) — the same
    /// leniency `agent/skill_utils.py::_normalize_prerequisite_values` gives
    /// real skill authors who forget the brackets. An empty string or an
    /// unrelated shape (a nested map) reads as an empty list rather than an
    /// error: list-shaped fields are all optional, so "not list-shaped" and
    /// "absent" should behave the same way to callers.
    pub fn as_list_lenient(&self) -> Vec<String> {
        match self {
            YamlValue::List(items) => items.clone(),
            YamlValue::Str(s) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }
}

/// A parsed `SKILL.md` frontmatter block.
///
/// Holds the full parsed mapping so callers can reach fields this module
/// does not special-case, plus typed accessors for the handful every caller
/// in this crate actually needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub raw: BTreeMap<String, YamlValue>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&YamlValue> {
        self.raw.get(key)
    }

    /// `None` for a missing, non-string, or blank-after-trim `name` — all
    /// three mean the same thing to a caller deciding whether the field was
    /// usably present.
    pub fn name(&self) -> Option<&str> {
        self.raw.get("name").and_then(YamlValue::as_str).map(str::trim).filter(|s| !s.is_empty())
    }

    pub fn description(&self) -> Option<&str> {
        self.raw.get("description").and_then(YamlValue::as_str).filter(|s| !s.trim().is_empty())
    }

    pub fn version(&self) -> Option<&str> {
        self.raw.get("version").and_then(YamlValue::as_str)
    }

    pub fn license(&self) -> Option<&str> {
        self.raw.get("license").and_then(YamlValue::as_str)
    }

    /// Empty means "every platform" — see `discovery::platform_matches`.
    pub fn platforms(&self) -> Vec<String> {
        self.raw.get("platforms").map(YamlValue::as_list_lenient).unwrap_or_default()
    }

    /// `metadata.hermes.tags`, falling back to a top-level `tags` — the same
    /// fallback chain `tools/skills_tool.py::skill_view` uses, so tags on a
    /// real Hermes skill file (nested) and a hand-written one (flat) both
    /// resolve.
    pub fn tags(&self) -> Vec<String> {
        self.nested_list(&["metadata", "hermes", "tags"])
            .filter(|v| !v.is_empty())
            .or_else(|| self.raw.get("tags").map(YamlValue::as_list_lenient))
            .unwrap_or_default()
    }

    /// `metadata.hermes.related_skills`, falling back to a top-level
    /// `related_skills` — mirrors [`Frontmatter::tags`].
    pub fn related_skills(&self) -> Vec<String> {
        self.nested_list(&["metadata", "hermes", "related_skills"])
            .filter(|v| !v.is_empty())
            .or_else(|| self.raw.get("related_skills").map(YamlValue::as_list_lenient))
            .unwrap_or_default()
    }

    fn nested_list(&self, path: &[&str]) -> Option<Vec<String>> {
        Some(self.nested(path)?.as_list_lenient())
    }

    fn nested(&self, path: &[&str]) -> Option<&YamlValue> {
        let (first, rest) = path.split_first()?;
        let mut current = self.raw.get(*first)?;
        for key in rest {
            current = current.as_map()?.get(*key)?;
        }
        Some(current)
    }
}

/// Parse `content` (a whole `SKILL.md` file) into its frontmatter and body.
///
/// `Err` carries a line-numbered, human-readable reason — every rejection in
/// this module explains itself precisely because "reject rather than guess"
/// is only a good trade when the rejection tells the author what to fix.
pub fn parse(content: &str) -> Result<(Frontmatter, String), String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let normalized = content.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Err(
            "SKILL.md must start with '---' on the first line (no leading blank line or BOM-after-content) to open the YAML frontmatter block".to_string(),
        );
    }

    let close_idx = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim_end() == "---")
        .map(|(i, _)| i);
    let Some(close_idx) = close_idx else {
        return Err(
            "frontmatter is not closed — expected a line containing only '---' after the opening fence".to_string(),
        );
    };

    let fm_lines = &lines[1..close_idx];
    let body = lines[close_idx + 1..].join("\n");

    // fm_lines[0] is line 2 of the original file: line 1 is the opening fence.
    let classified = classify_lines(fm_lines, 2)?;
    if classified.is_empty() {
        return Err("frontmatter is empty".to_string());
    }
    if classified[0].indent != 0 {
        return Err(format!("line {}: frontmatter keys must start at column 0", classified[0].lineno));
    }

    let mut pos = 0usize;
    let raw = parse_mapping(&classified, &mut pos, 0)?;
    if pos != classified.len() {
        return Err(format!(
            "line {}: unexpected indentation (dedented past the top level)",
            classified[pos].lineno
        ));
    }

    Ok((Frontmatter { raw }, body))
}

// ---------------------------------------------------------------------------
// Line classification
// ---------------------------------------------------------------------------

struct Line<'a> {
    indent: usize,
    content: &'a str,
    lineno: usize,
}

/// Turn the raw lines between the fences into `(indent, content, lineno)`
/// triples, dropping blank lines and full-line comments. `line_offset` is
/// the 1-based line number of `raw_lines[0]` in the original file, so every
/// error downstream can point at a real line a person can open and look at.
fn classify_lines<'a>(raw_lines: &[&'a str], line_offset: usize) -> Result<Vec<Line<'a>>, String> {
    let mut out = Vec::new();
    for (i, raw) in raw_lines.iter().enumerate() {
        let lineno = line_offset + i;
        if raw.contains('\t') {
            return Err(format!("line {lineno}: tabs are not supported in frontmatter; indent with spaces"));
        }
        let trimmed = raw.trim_start_matches(' ');
        let indent = raw.len() - trimmed.len();
        let content = trimmed.trim_end();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        out.push(Line { indent, content, lineno });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Recursive-descent parse over the classified lines
// ---------------------------------------------------------------------------

/// Parse a run of sibling `key:`/`key: value` entries that all share
/// `indent`, consuming from `*pos` and stopping at the first line whose
/// indent is less than `indent` (returned to the caller to continue with)
/// or the end of the input.
fn parse_mapping(lines: &[Line], pos: &mut usize, indent: usize) -> Result<BTreeMap<String, YamlValue>, String> {
    let mut map = BTreeMap::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(format!("line {}: unexpected indentation", line.lineno));
        }
        if is_list_item(line.content) {
            return Err(format!("line {}: found a list item ('- ...') where a 'key:' entry was expected", line.lineno));
        }

        let (key, rest) = split_key_value(line.content)
            .ok_or_else(|| format!("line {}: expected 'key:' or 'key: value'", line.lineno))?;
        if !is_bare_key(&key) {
            return Err(format!(
                "line {}: unsupported key '{key}' — keys must be letters, digits, '.', '_' or '-', with no quoting or spaces",
                line.lineno
            ));
        }
        if map.contains_key(&key) {
            return Err(format!("line {}: duplicate key '{key}'", line.lineno));
        }
        let lineno = line.lineno;
        *pos += 1;

        let value = if rest.is_empty() {
            parse_possibly_nested_value(lines, pos, indent)?
        } else {
            parse_scalar_or_flow_list(&rest, lineno)?
        };
        map.insert(key, value);
    }
    Ok(map)
}

/// Resolve a bare `key:` (nothing after the colon on its own line): either a
/// nested block belonging to it, if the next line is indented deeper, or an
/// empty string when nothing follows.
fn parse_possibly_nested_value(lines: &[Line], pos: &mut usize, indent: usize) -> Result<YamlValue, String> {
    let Some(next) = lines.get(*pos) else {
        return Ok(YamlValue::Str(String::new()));
    };
    if next.indent <= indent {
        return Ok(YamlValue::Str(String::new()));
    }
    let child_indent = next.indent;
    if is_list_item(next.content) {
        Ok(YamlValue::List(parse_block_list(lines, pos, child_indent)?))
    } else {
        Ok(YamlValue::Map(parse_mapping(lines, pos, child_indent)?))
    }
}

/// Parse a run of sibling `- value` items sharing `indent`.
fn parse_block_list(lines: &[Line], pos: &mut usize, indent: usize) -> Result<Vec<String>, String> {
    let mut items = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(format!("line {}: unexpected indentation inside a list", line.lineno));
        }
        if !is_list_item(line.content) {
            return Err(format!("line {}: expected a list item ('- ...') or a dedent", line.lineno));
        }
        let item_text = line.content.strip_prefix('-').unwrap_or(line.content).trim_start();

        if looks_like_mapping_item(item_text) {
            return Err(format!(
                "line {}: lists of mappings are not supported ('- key: value'); use a flat list of scalars instead",
                line.lineno
            ));
        }
        match parse_scalar_or_flow_list(item_text, line.lineno)? {
            YamlValue::Str(s) => items.push(s),
            _ => {
                return Err(format!("line {}: nested lists are not supported inside a list item", line.lineno));
            }
        }
        *pos += 1;
    }
    Ok(items)
}

fn is_list_item(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

/// Whether a block-list item's text is itself shaped like `key: value` —
/// used only to produce a precise rejection for lists-of-mappings, never to
/// accept one. A leading quote means the item is a genuine scalar (its
/// colon, if any, is inside the string), not a mapping key, so those are
/// never flagged here.
fn looks_like_mapping_item(item_text: &str) -> bool {
    if item_text.starts_with('"') || item_text.starts_with('\'') {
        return false;
    }
    split_key_value(item_text).map(|(k, _)| is_bare_key(&k)).unwrap_or(false)
}

fn is_bare_key(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Split `key: value` / `key:` at the first colon that is followed by a
/// space or is the last character — real YAML's rule, which is what lets
/// `description: Ratio is 3:2` keep `3:2` in the value instead of treating
/// it as a second key. Scans by byte: `:` and ` ` are both single-byte
/// ASCII, and no UTF-8 continuation byte ever equals either value, so
/// slicing `content` at these byte offsets is always on a char boundary
/// regardless of any multibyte characters elsewhere on the line.
fn split_key_value(content: &str) -> Option<(String, String)> {
    let bytes = content.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b':' {
            continue;
        }
        let at_eol = i + 1 == bytes.len();
        let followed_by_space = !at_eol && bytes[i + 1] == b' ';
        if at_eol || followed_by_space {
            let key = content[..i].trim().to_string();
            let rest = content[i + 1..].trim().to_string();
            return Some((key, rest));
        }
    }
    None
}

/// Characters real YAML gives special meaning to that this subset refuses
/// outright at the start of a plain scalar, rather than reading them as
/// literal text: `&`/`*` (anchors/aliases), `!` (tags), `|`/`>` (block
/// scalars), `%` (directives), `@`/backtick (reserved by the spec), `{`
/// (flow mapping — `[` is handled separately as the one flow construct this
/// subset does support).
const REJECTED_SCALAR_LEADERS: &[char] = &['&', '*', '!', '|', '>', '%', '@', '`', '{'];

/// Parse the value half of a `key: value` line, or one block-list item's
/// text: an inline flow list, a quoted scalar, or a plain scalar.
fn parse_scalar_or_flow_list(rest: &str, lineno: usize) -> Result<YamlValue, String> {
    let rest = strip_trailing_comment(rest).trim();
    if rest.is_empty() {
        return Ok(YamlValue::Str(String::new()));
    }

    if let Some(inner) = rest.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| format!("line {lineno}: unterminated '[' — inline lists must open and close on the same line"))?;
        if inner.trim().is_empty() {
            return Ok(YamlValue::List(Vec::new()));
        }
        let mut items = Vec::new();
        for raw_item in inner.split(',') {
            let raw_item = raw_item.trim();
            if raw_item.is_empty() {
                // Tolerates a trailing comma ("[a, b,]") rather than
                // emitting a spurious empty tag.
                continue;
            }
            items.push(unquote_scalar(raw_item, lineno)?);
        }
        return Ok(YamlValue::List(items));
    }

    if let Some(c) = rest.chars().next() {
        if REJECTED_SCALAR_LEADERS.contains(&c) {
            return Err(format!(
                "line {lineno}: unsupported YAML construct starting with '{c}' — anchors, aliases, tags, block scalars and flow mappings are not part of the supported frontmatter subset"
            ));
        }
    }

    Ok(YamlValue::Str(unquote_scalar(rest, lineno)?))
}

/// Strip a trailing `# comment`, only when the `#` is outside any quoted
/// region and preceded by whitespace (or opens the string) — so
/// `version: 1.0#nightly` keeps its literal `#` (nothing precedes it but a
/// non-space character) while `platforms: [macos]  # mac only` does not.
///
/// The quote tracking here is a simple toggle, not a full scanner: it does
/// not understand a backslash-escaped quote inside a double-quoted value.
/// That is a deliberate, documented limit of this subset (see the module
/// doc) rather than a bug — a description containing both an escaped quote
/// and a `#` is expected to be rare enough not to be worth the extra
/// parser complexity.
fn strip_trailing_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_is_boundary = true; // start-of-string counts as a boundary
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && prev_is_boundary => return s[..i].trim_end(),
            _ => {}
        }
        prev_is_boundary = bytes[i] == b' ';
        i += 1;
    }
    s
}

/// Unquote a single scalar token: `"..."`, `'...'`, or a bare word — never
/// receives a flow list, which [`parse_scalar_or_flow_list`] peels off
/// first.
fn unquote_scalar(s: &str, lineno: usize) -> Result<String, String> {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return unquote_double(&s[1..s.len() - 1], lineno);
    }
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return Ok(s[1..s.len() - 1].replace("''", "'"));
    }
    if s.starts_with('"') || s.starts_with('\'') {
        return Err(format!("line {lineno}: unterminated quoted scalar"));
    }
    Ok(s.to_string())
}

/// Unescape a double-quoted scalar's inner text. Understands `\"` and `\\`
/// only — any other backslash sequence is kept exactly as written (both
/// characters), which is the module doc's documented limit, not a missing
/// case. An unescaped `"` before the end is an error: it means the scalar
/// closed early and there is trailing garbage before the real closing quote
/// `unquote_scalar` already matched.
fn unquote_double(inner: &str, lineno: usize) -> Result<String, String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return Err(format!("line {lineno}: trailing backslash in quoted scalar")),
            },
            '"' => return Err(format!("line {lineno}: unexpected '\"' before the end of a quoted scalar")),
            _ => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(content: &str) -> (Frontmatter, String) {
        parse(content).unwrap_or_else(|e| panic!("expected parse to succeed, got error: {e}"))
    }

    fn err(content: &str) -> String {
        parse(content).expect_err("expected parse to fail")
    }

    // -- Real-world shapes -----------------------------------------------

    #[test]
    fn parses_the_apple_notes_style_frontmatter() {
        let content = "---\nname: apple-notes\ndescription: \"Manage Apple Notes via memo CLI: create, search, edit.\"\nversion: 1.0.1\nplatforms: [macos]\nmetadata:\n  hermes:\n    tags: [Notes, Apple, macOS, note-taking]\n    related_skills: [obsidian]\nprerequisites:\n  commands: [memo]\n---\n\n# Apple Notes\n\nBody text.\n";
        let (fm, body) = ok(content);
        assert_eq!(fm.name(), Some("apple-notes"));
        assert_eq!(fm.description(), Some("Manage Apple Notes via memo CLI: create, search, edit."));
        assert_eq!(fm.version(), Some("1.0.1"));
        assert_eq!(fm.platforms(), vec!["macos"]);
        assert_eq!(fm.tags(), vec!["Notes", "Apple", "macOS", "note-taking"]);
        assert_eq!(fm.related_skills(), vec!["obsidian"]);
        assert!(body.trim_start().starts_with("# Apple Notes"));
    }

    #[test]
    fn parses_block_list_tags_and_scalar_list_author() {
        // Mirrors skills/creative/comfyui/SKILL.md: block-style tags nested
        // two levels deep, and a top-level field given as a flow list.
        let content = "---\nname: comfyui\ndescription: Generate images with ComfyUI.\nauthor: [alice, bob]\nplatforms: [macos, linux, windows]\nmetadata:\n  hermes:\n    tags:\n      - comfyui\n      - image-generation\n      - stable-diffusion\n    related_skills: [stable-diffusion-image-generation]\n---\nBody.\n";
        let (fm, _) = ok(content);
        assert_eq!(fm.get("author").unwrap().as_list_lenient(), vec!["alice", "bob"]);
        assert_eq!(fm.platforms(), vec!["macos", "linux", "windows"]);
        assert_eq!(fm.tags(), vec!["comfyui", "image-generation", "stable-diffusion"]);
        assert_eq!(fm.related_skills(), vec!["stable-diffusion-image-generation"]);
    }

    #[test]
    fn a_bare_scalar_platform_is_read_as_a_one_element_list() {
        let (fm, _) = ok("---\nname: x\ndescription: y\nplatforms: macos\n---\nBody\n");
        assert_eq!(fm.platforms(), vec!["macos"]);
    }

    #[test]
    fn only_name_and_description_are_required_everything_else_is_optional() {
        let (fm, body) = ok("---\nname: minimal\ndescription: the bare minimum\n---\nJust a body.\n");
        assert_eq!(fm.name(), Some("minimal"));
        assert_eq!(fm.description(), Some("the bare minimum"));
        assert_eq!(fm.version(), None);
        assert!(fm.platforms().is_empty());
        assert_eq!(body, "Just a body.\n");
    }

    // -- Colon-in-value edge case ------------------------------------------

    #[test]
    fn a_colon_with_no_following_space_does_not_start_a_new_key() {
        let (fm, _) = ok("---\nname: x\ndescription: Ratio is 3:2\n---\nBody\n");
        assert_eq!(fm.description(), Some("Ratio is 3:2"));
    }

    // -- Quoting -------------------------------------------------------------

    #[test]
    fn double_quoted_scalar_understands_escaped_quote_and_backslash() {
        let (fm, _) = ok(r#"---
name: x
description: "She said \"hi\" then a backslash \\ then more"
---
Body
"#);
        assert_eq!(fm.description(), Some(r#"She said "hi" then a backslash \ then more"#));
    }

    #[test]
    fn single_quoted_scalar_understands_doubled_quote_escape() {
        let (fm, _) = ok("---\nname: x\ndescription: 'It''s a test'\n---\nBody\n");
        assert_eq!(fm.description(), Some("It's a test"));
    }

    #[test]
    fn unterminated_double_quote_is_rejected() {
        let e = err("---\nname: x\ndescription: \"unterminated\n---\nBody\n");
        assert!(e.contains("unterminated quoted scalar") || e.contains("line 3"), "{e}");
    }

    // -- Comments --------------------------------------------------------

    #[test]
    fn full_line_comments_and_blank_lines_are_ignored() {
        let (fm, _) = ok("---\n# a comment\n\nname: x\n\n# another\ndescription: y\n---\nBody\n");
        assert_eq!(fm.name(), Some("x"));
        assert_eq!(fm.description(), Some("y"));
    }

    #[test]
    fn trailing_comment_after_a_value_is_stripped() {
        let (fm, _) = ok("---\nname: x\ndescription: y\nplatforms: [macos]  # mac only\n---\nBody\n");
        assert_eq!(fm.platforms(), vec!["macos"]);
    }

    #[test]
    fn a_hash_glued_to_a_value_is_not_treated_as_a_comment() {
        let (fm, _) = ok("---\nname: x\ndescription: y\nversion: 1.0#nightly\n---\nBody\n");
        assert_eq!(fm.version(), Some("1.0#nightly"));
    }

    // -- Inline flow lists -------------------------------------------------

    #[test]
    fn empty_inline_list_parses_to_an_empty_list() {
        let (fm, _) = ok("---\nname: x\ndescription: y\ntags: []\n---\nBody\n");
        assert_eq!(fm.get("tags").unwrap().as_list_lenient(), Vec::<String>::new());
    }

    #[test]
    fn inline_list_tolerates_a_trailing_comma() {
        let (fm, _) = ok("---\nname: x\ndescription: y\ntags: [a, b,]\n---\nBody\n");
        assert_eq!(fm.get("tags").unwrap().as_list_lenient(), vec!["a", "b"]);
    }

    #[test]
    fn unterminated_inline_list_is_rejected() {
        let e = err("---\nname: x\ndescription: y\ntags: [a, b\n---\nBody\n");
        assert!(e.contains("unterminated"), "{e}");
    }

    // -- Rejections: the whole point of "reject rather than guess" ----------

    #[test]
    fn missing_opening_fence_is_rejected() {
        let e = err("name: x\ndescription: y\n");
        assert!(e.contains("start with '---'"), "{e}");
    }

    #[test]
    fn unclosed_fence_is_rejected() {
        let e = err("---\nname: x\ndescription: y\n");
        assert!(e.contains("not closed"), "{e}");
    }

    #[test]
    fn missing_required_fields_is_a_discovery_level_concern_not_a_parse_error() {
        // The parser itself never enforces "name and description present" —
        // that is `manage::validate_frontmatter`'s job for writes, so a
        // read-only caller (skills_list scanning a directory it didn't
        // author) can still see whatever fields *are* there.
        let (fm, _) = ok("---\nversion: 1.0\n---\nBody\n");
        assert_eq!(fm.name(), None);
        assert_eq!(fm.version(), Some("1.0"));
    }

    #[test]
    fn tab_indentation_is_rejected() {
        let e = err("---\nname: x\ndescription: y\nmetadata:\n\thermes:\n---\nBody\n");
        assert!(e.contains("tabs"), "{e}");
    }

    #[test]
    fn duplicate_key_is_rejected() {
        let e = err("---\nname: x\nname: y\ndescription: z\n---\nBody\n");
        assert!(e.contains("duplicate key"), "{e}");
    }

    #[test]
    fn anchor_is_rejected() {
        let e = err("---\nname: x\ndescription: y\nfoo: &anchor bar\n---\nBody\n");
        assert!(e.contains("unsupported YAML construct"), "{e}");
    }

    #[test]
    fn block_scalar_is_rejected() {
        let e = err("---\nname: x\ndescription: |\n  multi\n  line\n---\nBody\n");
        assert!(e.contains("unsupported YAML construct"), "{e}");
    }

    #[test]
    fn flow_mapping_is_rejected() {
        let e = err("---\nname: x\ndescription: y\nfoo: {a: b}\n---\nBody\n");
        assert!(e.contains("unsupported YAML construct"), "{e}");
    }

    #[test]
    fn list_of_mappings_is_rejected_with_a_precise_message() {
        // Real example: skills/productivity/google-workspace/SKILL.md's
        // `required_credential_files:` field.
        let content = "---\nname: x\ndescription: y\nrequired_credential_files:\n  - path: google_token.json\n    description: token\n---\nBody\n";
        let e = err(content);
        assert!(e.contains("lists of mappings are not supported"), "{e}");
    }

    #[test]
    fn a_quoted_list_item_containing_a_colon_is_not_mistaken_for_a_mapping_item() {
        let (fm, _) = ok("---\nname: x\ndescription: y\nnotes:\n  - \"Time is 3: 00\"\n---\nBody\n");
        assert_eq!(fm.get("notes").unwrap().as_list_lenient(), vec!["Time is 3: 00"]);
    }

    #[test]
    fn inconsistent_sibling_indentation_is_rejected() {
        let e = err("---\nname: x\ndescription: y\nmetadata:\n  hermes:\n    tags: [a]\n   related_skills: [b]\n---\nBody\n");
        assert!(e.contains("line"), "{e}");
    }

    #[test]
    fn a_list_item_at_mapping_position_is_rejected() {
        let e = err("---\nname: x\ndescription: y\n- oops\n---\nBody\n");
        assert!(e.contains("list item"), "{e}");
    }

    #[test]
    fn quoted_key_syntax_is_rejected_as_unsupported() {
        let e = err("---\nname: x\ndescription: y\n\"quoted key\": value\n---\nBody\n");
        assert!(e.contains("unsupported key") || e.contains("expected"), "{e}");
    }

    #[test]
    fn frontmatter_that_is_only_whitespace_is_rejected() {
        let e = err("---\n   \n\n---\nBody\n");
        assert!(e.contains("empty"), "{e}");
    }

    #[test]
    fn top_level_key_not_at_column_zero_is_rejected() {
        let e = err("---\n  name: x\n  description: y\n---\nBody\n");
        assert!(e.contains("column 0"), "{e}");
    }

    // -- BOM / CRLF ---------------------------------------------------------

    #[test]
    fn a_leading_bom_is_stripped_before_the_fence_check() {
        let content = "\u{feff}---\nname: x\ndescription: y\n---\nBody\n";
        let (fm, _) = ok(content);
        assert_eq!(fm.name(), Some("x"));
    }

    #[test]
    fn crlf_line_endings_parse_identically_to_lf() {
        let content = "---\r\nname: x\r\ndescription: y\r\n---\r\nBody\r\n";
        let (fm, body) = ok(content);
        assert_eq!(fm.name(), Some("x"));
        assert_eq!(body, "Body\n");
    }

    // -- YamlValue::as_list_lenient ------------------------------------------

    #[test]
    fn as_list_lenient_treats_an_empty_string_as_an_empty_list_not_one_blank_entry() {
        assert_eq!(YamlValue::Str(String::new()).as_list_lenient(), Vec::<String>::new());
    }
}
