//! The second shelf of the developer toolbox.
//!
//! `dev.rs` is the closed set of pure, instant, string-in-string-out tools
//! reached through one `ToolId` enum. Everything here needed something that
//! shape cannot express — a hand-rolled parser for a format Rust has no crate
//! for in this workspace, a subprocess, or a network round trip — so it lives
//! next door instead of forcing those into `dev.rs`'s contract.
//!
//! # Why YAML and XML are hand-rolled
//!
//! There is no YAML or XML crate in this workspace's dependency graph, and
//! `cargo add` is off the table for this change. Pulling one in is the
//! obviously-correct move for a *shipping* product; it was not available
//! here, so what follows is a parser for the subset of each format people
//! actually paste into a formatter — block and flow collections, quoted and
//! plain scalars, comments — not the full spec. Anchors, aliases, YAML tags,
//! merge keys, external DTDs and entity expansion are out of scope and are
//! reported as explicit errors rather than silently mishandled, because a
//! formatter that quietly reshapes the 5% it does not understand is worse
//! than one that refuses.
//!
//! # Why the AI tools take a `&SettingsManager`, not a provider
//!
//! [`agent::chat_with_history`] resolves whichever backend the user has
//! configured — a hardcoded provider here would silently ignore that choice
//! (and, worse, could send a user's diff to a service they never picked).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::agent::{self, Message};
use crate::settings::SettingsManager;

use super::dev::ToolResult;
use super::{output_with_timeout, TOOL_TIMEOUT};

// ---------------------------------------------------------------------------
// The synchronous, pure tools: dispatched the same way dev.rs's are.
// ---------------------------------------------------------------------------

/// Every tool in this file that is a plain string transform.
///
/// Kept as its own closed enum, mirroring [`super::dev::ToolId`], rather than
/// folded into that one — `dev.rs` is not this change's to edit, and a
/// second small enum costs nothing that a shared one would have saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtraToolId {
    YamlFormat,
    YamlValidate,
    XmlFormat,
    XmlValidate,
    HtmlEntityEncode,
    HtmlEntityDecode,
    SqlFormat,
    HostsView,
}

/// Whether a tool needs typed input, or produces output from the machine's
/// own state (`HostsView` reads a file, not the input box).
pub fn needs_input(id: ExtraToolId) -> bool {
    !matches!(id, ExtraToolId::HostsView)
}

/// `dev.rs`'s `ToolResult::ok`/`ToolResult::err` convenience constructors are
/// private to that module (only its fields are `pub`), so this file builds
/// the same shape by hand rather than editing `dev.rs` to export them.
fn ok_result(title: impl Into<String>, output: impl Into<String>) -> ToolResult {
    ToolResult { ok: true, title: title.into(), output: output.into(), message: String::new(), auto_copy: false }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult { ok: false, title: String::new(), output: String::new(), message: message.into(), auto_copy: false }
}

/// Run one of this file's plain-text tools.
pub fn run(id: ExtraToolId, input: &str) -> ToolResult {
    if needs_input(id) && input.trim().is_empty() {
        return err_result("There is nothing to work on yet — fill the box above.");
    }

    match id {
        ExtraToolId::YamlFormat => match yaml::format(input) {
            Ok(out) => ok_result("Formatted YAML", out),
            Err(e) => err_result(format!("That is not valid YAML: {e}")),
        },
        ExtraToolId::YamlValidate => match yaml::parse_document(input) {
            Ok(_) => ok_result("Valid YAML", "No problems found.".to_string()),
            Err(e) => err_result(format!("That is not valid YAML: {e}")),
        },

        ExtraToolId::XmlFormat => match xml::format(input) {
            Ok(out) => ok_result("Formatted XML", out),
            Err(e) => err_result(format!("That is not valid XML: {e}")),
        },
        ExtraToolId::XmlValidate => match xml::parse_document(input) {
            Ok(_) => ok_result("Valid XML", "No problems found.".to_string()),
            Err(e) => err_result(format!("That is not valid XML: {e}")),
        },

        ExtraToolId::HtmlEntityEncode => {
            ok_result("HTML entities", html_entities::encode(input))
        }
        ExtraToolId::HtmlEntityDecode => {
            ok_result("Decoded", html_entities::decode(input))
        }

        ExtraToolId::SqlFormat => ok_result("Formatted SQL", sql::format(input)),

        ExtraToolId::HostsView => match hosts::read() {
            Ok(entries) => {
                if entries.is_empty() {
                    ok_result("/etc/hosts", "No alias entries found.".to_string())
                } else {
                    let body = entries
                        .iter()
                        .map(|e| format!("{:<16} {}", e.ip, e.hosts.join(" ")))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ok_result(format!("/etc/hosts — {} entries", entries.len()), body)
                }
            }
            Err(e) => err_result(e),
        },
    }
}

// ---------------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------------

mod yaml {
    //! A hand-rolled parser and canonical printer for the block/flow YAML
    //! subset people actually write config files in.
    //!
    //! # Deliberately unsupported
    //!
    //! Anchors (`&x`), aliases (`*x`), explicit tags (`!!str`) and merge keys
    //! (`<<:`) are rejected with a named error rather than silently treated as
    //! plain scalars — a formatter that reshapes an alias into the literal
    //! text `*x` has produced a document that no longer means what it did.
    //! Multi-document streams (`---` separators) are supported because they
    //! are common (Kubernetes manifests, GitHub Actions) and cost little once
    //! the single-document parser exists.

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Null,
        Bool(bool),
        Int(i64),
        Float(f64),
        Str(String),
        Seq(Vec<Value>),
        Map(Vec<(String, Value)>),
    }

    struct Line {
        indent: usize,
        text: String,
        no: usize,
    }

    /// Parse every document in a `---`-separated stream.
    pub fn parse_document(input: &str) -> Result<Vec<Value>, String> {
        let mut docs = Vec::new();
        for (i, chunk) in split_documents(input).into_iter().enumerate() {
            let lines = preprocess(&chunk)?;
            if lines.is_empty() {
                docs.push(Value::Null);
                continue;
            }
            let mut idx = 0;
            let base_indent = lines[0].indent;
            let value = parse_block(&lines, &mut idx, base_indent)
                .map_err(|e| if i == 0 { e } else { format!("document {}: {e}", i + 1) })?;
            if idx != lines.len() {
                let bad = &lines[idx];
                return Err(format!(
                    "line {}: unexpected indentation (expected {} spaces)",
                    bad.no, base_indent
                ));
            }
            docs.push(value);
        }
        Ok(docs)
    }

    pub fn format(input: &str) -> Result<String, String> {
        let docs = parse_document(input)?;
        let rendered: Vec<String> = docs.iter().map(|d| print_value(d, 0)).collect();
        Ok(rendered.join("\n---\n"))
    }

    fn split_documents(input: &str) -> Vec<String> {
        let mut docs = Vec::new();
        let mut current = String::new();
        let mut started = false;
        for line in input.lines() {
            if line.trim_end() == "---" {
                if started || !current.trim().is_empty() {
                    docs.push(std::mem::take(&mut current));
                }
                started = true;
                continue;
            }
            if line.trim_end() == "..." {
                continue;
            }
            current.push_str(line);
            current.push('\n');
        }
        docs.push(current);
        docs.into_iter().filter(|d| !d.trim().is_empty() || docs_all_blank(input)).collect()
    }

    fn docs_all_blank(input: &str) -> bool {
        input.trim().is_empty()
    }

    /// Strip comments and blank lines, reject tabs, and record each
    /// remaining line's indentation.
    ///
    /// Tabs are rejected outright (not silently expanded) because the YAML
    /// spec forbids them as indentation and a formatter that guesses a tab
    /// width is guessing at the author's intent.
    fn preprocess(input: &str) -> Result<Vec<Line>, String> {
        let mut out = Vec::new();
        for (i, raw) in input.lines().enumerate() {
            let no = i + 1;
            if raw.trim().is_empty() {
                continue;
            }
            let indent_str: String = raw.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            if indent_str.contains('\t') {
                return Err(format!("line {no}: tabs are not allowed for indentation"));
            }
            let indent = indent_str.len();
            let content = &raw[indent..];
            if content.starts_with('#') {
                continue;
            }
            let stripped = strip_trailing_comment(content);
            if stripped.trim().is_empty() {
                continue;
            }
            if let Some(anchor) = rejected_construct(&stripped) {
                return Err(format!("line {no}: {anchor} is not supported"));
            }
            out.push(Line { indent, text: stripped.trim_end().to_string(), no });
        }
        Ok(out)
    }

    fn rejected_construct(s: &str) -> Option<&'static str> {
        let s = s.trim_start();
        // A leading `&name`/`*name`/`!tag` on a value position, not inside a
        // quoted string — this is a coarse check (it will not catch one
        // buried inside a flow collection) but flow collections are already a
        // minority of real-world YAML, and the common top-level case is worth
        // catching cleanly.
        let after_key = s.split_once(':').map(|(_, v)| v.trim()).unwrap_or(s);
        let candidate = if s.starts_with("- ") { s[2..].trim_start() } else { after_key };
        if candidate.starts_with('&') && !candidate.starts_with("&&") {
            Some("an anchor (&name)")
        } else if candidate.starts_with('*') {
            Some("an alias (*name)")
        } else if candidate.starts_with("!!") || (candidate.starts_with('!') && candidate.len() > 1) {
            Some("an explicit tag (!!type)")
        } else if s.starts_with("<<:") {
            Some("a merge key (<<:)")
        } else {
            None
        }
    }

    /// Find the top-level `#` that starts a comment: preceded by whitespace
    /// or line-start, and outside any quoting.
    fn strip_trailing_comment(s: &str) -> String {
        let chars: Vec<char> = s.chars().collect();
        let mut in_single = false;
        let mut in_double = false;
        let mut prev_ws = true;
        for (i, &c) in chars.iter().enumerate() {
            match c {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                '#' if !in_single && !in_double && prev_ws => {
                    return chars[..i].iter().collect();
                }
                _ => {}
            }
            prev_ws = c.is_whitespace();
        }
        s.to_string()
    }

    fn parse_block(lines: &[Line], idx: &mut usize, indent: usize) -> Result<Value, String> {
        if *idx >= lines.len() {
            return Ok(Value::Null);
        }
        let first = &lines[*idx];
        if first.indent != indent {
            return Err(format!(
                "line {}: unexpected indentation (expected {} spaces, found {})",
                first.no, indent, first.indent
            ));
        }

        if is_seq_item(&first.text) {
            return parse_seq(lines, idx, indent);
        }
        if find_key_colon(&first.text).is_some() {
            return parse_map(lines, idx, indent);
        }

        // A single scalar document (`format` given just `hello` or `[1,2]`).
        let value = parse_scalar(&first.text)?;
        *idx += 1;
        Ok(value)
    }

    fn is_seq_item(text: &str) -> bool {
        text == "-" || text.starts_with("- ")
    }

    fn parse_seq(lines: &[Line], idx: &mut usize, indent: usize) -> Result<Value, String> {
        let mut items = Vec::new();
        while *idx < lines.len() && lines[*idx].indent == indent && is_seq_item(&lines[*idx].text) {
            let line = &lines[*idx];
            let remainder = if line.text == "-" { "" } else { line.text[2..].trim_start() };
            let dash_col = line.indent;
            *idx += 1;

            if remainder.is_empty() {
                if *idx < lines.len() && lines[*idx].indent > dash_col {
                    let child_indent = lines[*idx].indent;
                    items.push(parse_block(lines, idx, child_indent)?);
                } else {
                    items.push(Value::Null);
                }
                continue;
            }

            if let Some(kind) = block_scalar_kind(remainder) {
                items.push(parse_block_scalar(lines, idx, dash_col + 1, kind)?);
                continue;
            }

            if find_key_colon(remainder).is_some() {
                // `- key: value` starts a nested mapping whose first entry is
                // on the dash's own line; siblings are whatever indent the
                // *next* line settles on, matching how every hand-written
                // YAML file actually aligns these.
                let synthetic = Line { indent: dash_col + 2, text: remainder.to_string(), no: line.no };
                let mut nested = vec![synthetic];
                let child_indent = if *idx < lines.len() && lines[*idx].indent > dash_col {
                    lines[*idx].indent
                } else {
                    dash_col + 2
                };
                while *idx < lines.len() && lines[*idx].indent == child_indent {
                    nested.push(Line {
                        indent: dash_col + 2,
                        text: lines[*idx].text.clone(),
                        no: lines[*idx].no,
                    });
                    *idx += 1;
                }
                let mut nidx = 0;
                let value = parse_block(&nested, &mut nidx, dash_col + 2)?;
                items.push(value);
                continue;
            }

            items.push(parse_scalar(remainder)?);
        }
        Ok(Value::Seq(items))
    }

    fn parse_map(lines: &[Line], idx: &mut usize, indent: usize) -> Result<Value, String> {
        let mut entries: Vec<(String, Value)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while *idx < lines.len() && lines[*idx].indent == indent {
            let line = &lines[*idx];
            let Some(colon) = find_key_colon(&line.text) else { break };
            let key_raw = line.text[..colon].trim();
            let key = unquote_scalar_string(key_raw)?;
            if !seen.insert(key.clone()) {
                return Err(format!("line {}: duplicate key \"{key}\"", line.no));
            }
            let remainder = line.text[colon + 1..].trim();
            *idx += 1;

            let value = if remainder.is_empty() {
                if *idx < lines.len() && lines[*idx].indent > indent {
                    let child_indent = lines[*idx].indent;
                    parse_block(lines, idx, child_indent)?
                } else {
                    Value::Null
                }
            } else if let Some(kind) = block_scalar_kind(remainder) {
                parse_block_scalar(lines, idx, indent + 1, kind)?
            } else {
                parse_scalar(remainder)?
            };
            entries.push((key, value));
        }
        Ok(Value::Map(entries))
    }

    #[derive(Clone, Copy)]
    enum BlockScalarKind {
        Literal, // `|` — keep newlines
        Folded,  // `>` — fold to spaces
    }

    fn block_scalar_kind(remainder: &str) -> Option<BlockScalarKind> {
        match remainder.trim_end_matches(['-', '+']) {
            "|" => Some(BlockScalarKind::Literal),
            ">" => Some(BlockScalarKind::Folded),
            _ => None,
        }
    }

    fn parse_block_scalar(
        lines: &[Line],
        idx: &mut usize,
        min_indent: usize,
        kind: BlockScalarKind,
    ) -> Result<Value, String> {
        let mut collected: Vec<(usize, &str)> = Vec::new();
        while *idx < lines.len() && lines[*idx].indent >= min_indent {
            collected.push((lines[*idx].indent, lines[*idx].text.as_str()));
            *idx += 1;
        }
        if collected.is_empty() {
            return Ok(Value::Str(String::new()));
        }
        let base = collected.iter().map(|(i, _)| *i).min().unwrap_or(min_indent);
        let dedented: Vec<String> = collected
            .iter()
            .map(|(i, t)| format!("{}{}", " ".repeat(i.saturating_sub(base)), t))
            .collect();
        let text = match kind {
            BlockScalarKind::Literal => dedented.join("\n"),
            BlockScalarKind::Folded => dedented.join(" "),
        };
        Ok(Value::Str(text))
    }

    /// Locate the colon that separates a mapping key from its value: the
    /// first `:` that is followed by whitespace or end-of-string, outside any
    /// quoting or flow nesting.
    fn find_key_colon(s: &str) -> Option<usize> {
        let bytes = s.as_bytes();
        let mut in_single = false;
        let mut in_double = false;
        let mut depth = 0i32;
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single => in_double = !in_double,
                b'[' | b'{' if !in_single && !in_double => depth += 1,
                b']' | b'}' if !in_single && !in_double => depth -= 1,
                b':' if !in_single && !in_double && depth == 0 => {
                    let next = bytes.get(i + 1);
                    if next.is_none() || next == Some(&b' ') || next == Some(&b'\t') {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn unquote_scalar_string(s: &str) -> Result<String, String> {
        match parse_scalar(s)? {
            Value::Str(s) => Ok(s),
            // A key that reads as e.g. `true` or `123` is still just a key —
            // YAML mapping keys are always strings once printed back.
            other => Ok(print_scalar_plain(&other)),
        }
    }

    fn parse_scalar(s: &str) -> Result<Value, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(Value::Null);
        }
        if let Some(rest) = s.strip_prefix('"') {
            let (text, consumed) = parse_double_quoted(rest)?;
            if rest[consumed..].trim().is_empty() {
                return Ok(Value::Str(text));
            }
            return Err("trailing characters after a closed quote".to_string());
        }
        if let Some(rest) = s.strip_prefix('\'') {
            let (text, consumed) = parse_single_quoted(rest)?;
            if rest[consumed..].trim().is_empty() {
                return Ok(Value::Str(text));
            }
            return Err("trailing characters after a closed quote".to_string());
        }
        if s.starts_with('[') || s.starts_with('{') {
            let (value, consumed) = parse_flow(s)?;
            if s[consumed..].trim().is_empty() {
                return Ok(value);
            }
            return Err("trailing characters after a flow collection".to_string());
        }
        Ok(parse_plain_scalar(s))
    }

    fn parse_plain_scalar(s: &str) -> Value {
        match s {
            "null" | "Null" | "NULL" | "~" => return Value::Null,
            "true" | "True" | "TRUE" => return Value::Bool(true),
            "false" | "False" | "FALSE" => return Value::Bool(false),
            _ => {}
        }
        if let Ok(i) = s.parse::<i64>() {
            return Value::Int(i);
        }
        if is_float_literal(s) {
            if let Ok(f) = s.parse::<f64>() {
                return Value::Float(f);
            }
        }
        Value::Str(s.to_string())
    }

    fn is_float_literal(s: &str) -> bool {
        let s = s.strip_prefix(['+', '-']).unwrap_or(s);
        !s.is_empty()
            && s.contains('.')
            && s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
    }

    fn parse_double_quoted(rest: &str) -> Result<(String, usize), String> {
        let mut out = String::new();
        let chars: Vec<char> = rest.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '"' => return Ok((out, i + 1)),
                '\\' if i + 1 < chars.len() => {
                    i += 1;
                    match chars[i] {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '0' => out.push('\0'),
                        'u' if i + 4 < chars.len() => {
                            let hex: String = chars[i + 1..i + 5].iter().collect();
                            let code = u32::from_str_radix(&hex, 16)
                                .map_err(|_| "invalid \\u escape".to_string())?;
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            i += 4;
                        }
                        other => out.push(other),
                    }
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        Err("unterminated double-quoted string".to_string())
    }

    fn parse_single_quoted(rest: &str) -> Result<(String, usize), String> {
        let mut out = String::new();
        let chars: Vec<char> = rest.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\'' {
                if chars.get(i + 1) == Some(&'\'') {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                return Ok((out, i + 1));
            }
            out.push(chars[i]);
            i += 1;
        }
        Err("unterminated single-quoted string".to_string())
    }

    /// A tiny recursive-descent parser for `[...]`/`{...}` flow collections.
    /// Returns the value and how many bytes of `s` it consumed.
    fn parse_flow(s: &str) -> Result<(Value, usize), String> {
        let bytes = s.as_bytes();
        if bytes[0] == b'[' {
            let mut i = 1;
            let mut items = Vec::new();
            skip_ws(bytes, &mut i);
            if bytes.get(i) == Some(&b']') {
                return Ok((Value::Seq(items), i + 1));
            }
            loop {
                skip_ws(bytes, &mut i);
                let (v, used) = parse_flow_scalar(&s[i..])?;
                items.push(v);
                i += used;
                skip_ws(bytes, &mut i);
                match bytes.get(i) {
                    Some(b',') => {
                        i += 1;
                    }
                    Some(b']') => return Ok((Value::Seq(items), i + 1)),
                    _ => return Err("expected ',' or ']' in a flow sequence".to_string()),
                }
            }
        } else if bytes[0] == b'{' {
            let mut i = 1;
            let mut entries = Vec::new();
            skip_ws(bytes, &mut i);
            if bytes.get(i) == Some(&b'}') {
                return Ok((Value::Map(entries), i + 1));
            }
            loop {
                skip_ws(bytes, &mut i);
                let (key_val, used) = parse_flow_scalar(&s[i..])?;
                i += used;
                skip_ws(bytes, &mut i);
                if bytes.get(i) != Some(&b':') {
                    return Err("expected ':' in a flow mapping".to_string());
                }
                i += 1;
                skip_ws(bytes, &mut i);
                let (v, used) = parse_flow_scalar(&s[i..])?;
                i += used;
                entries.push((print_scalar_plain(&key_val), v));
                skip_ws(bytes, &mut i);
                match bytes.get(i) {
                    Some(b',') => {
                        i += 1;
                    }
                    Some(b'}') => return Ok((Value::Map(entries), i + 1)),
                    _ => return Err("expected ',' or '}' in a flow mapping".to_string()),
                }
            }
        } else {
            Err("expected a flow collection".to_string())
        }
    }

    fn skip_ws(bytes: &[u8], i: &mut usize) {
        while matches!(bytes.get(*i), Some(b' ') | Some(b'\t')) {
            *i += 1;
        }
    }

    fn parse_flow_scalar(s: &str) -> Result<(Value, usize), String> {
        if s.starts_with('"') {
            let (text, consumed) = parse_double_quoted(&s[1..])?;
            return Ok((Value::Str(text), consumed + 1));
        }
        if s.starts_with('\'') {
            let (text, consumed) = parse_single_quoted(&s[1..])?;
            return Ok((Value::Str(text), consumed + 1));
        }
        if s.starts_with('[') || s.starts_with('{') {
            return parse_flow(s);
        }
        let end = s.find([',', ':', ']', '}']).unwrap_or(s.len());
        let token = s[..end].trim_end();
        if token.is_empty() {
            return Err("expected a value in a flow collection".to_string());
        }
        Ok((parse_plain_scalar(token), token.len() + (s[..end].len() - token.len())))
    }

    fn print_value(v: &Value, indent: usize) -> String {
        match v {
            Value::Map(entries) if !entries.is_empty() => entries
                .iter()
                .map(|(k, val)| print_map_entry(k, val, indent))
                .collect::<Vec<_>>()
                .join("\n"),
            Value::Seq(items) if !items.is_empty() => {
                items.iter().map(|it| print_seq_item(it, indent)).collect::<Vec<_>>().join("\n")
            }
            Value::Map(_) => "{}".to_string(),
            Value::Seq(_) => "[]".to_string(),
            other => print_scalar(other),
        }
    }

    fn print_map_entry(key: &str, val: &Value, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let key = print_key(key);
        match val {
            Value::Map(entries) if !entries.is_empty() => {
                format!("{pad}{key}:\n{}", print_value(val, indent + 2))
            }
            Value::Seq(items) if !items.is_empty() => {
                format!("{pad}{key}:\n{}", print_value(val, indent + 2))
            }
            _ => format!("{pad}{key}: {}", print_scalar(val)),
        }
    }

    fn print_seq_item(val: &Value, indent: usize) -> String {
        let pad = " ".repeat(indent);
        match val {
            Value::Map(entries) if !entries.is_empty() => {
                let mut lines = print_value(val, indent + 2);
                lines.replace_range(0..indent + 2, "");
                format!("{pad}- {lines}")
            }
            Value::Seq(items) if !items.is_empty() => {
                format!("{pad}-\n{}", print_value(val, indent + 2))
            }
            _ => format!("{pad}- {}", print_scalar(val)),
        }
    }

    fn print_key(k: &str) -> String {
        if needs_quoting(k) {
            format!("\"{}\"", escape_double(k))
        } else {
            k.to_string()
        }
    }

    fn print_scalar(v: &Value) -> String {
        match v {
            Value::Str(s) if needs_quoting(s) => format!("\"{}\"", escape_double(s)),
            other => print_scalar_plain(other),
        }
    }

    fn print_scalar_plain(v: &Value) -> String {
        match v {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Str(s) => s.clone(),
            Value::Seq(_) | Value::Map(_) => String::new(),
        }
    }

    /// Whether a plain string must be quoted to round-trip as a string —
    /// either because it is empty, would otherwise parse as another type, or
    /// contains a character that is structurally significant in plain scalars.
    fn needs_quoting(s: &str) -> bool {
        if s.is_empty() {
            return true;
        }
        if !matches!(parse_plain_scalar(s), Value::Str(_)) {
            return true;
        }
        if s.trim() != s {
            return true;
        }
        if s.contains('\n') || s.contains(": ") || s.ends_with(':') {
            return true;
        }
        let first = s.chars().next().unwrap();
        matches!(first, '-' | '?' | '#' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`' | '[' | ']' | '{' | '}' | ',')
    }

    fn escape_double(s: &str) -> String {
        s.chars()
            .flat_map(|c| match c {
                '"' => vec!['\\', '"'],
                '\\' => vec!['\\', '\\'],
                '\n' => vec!['\\', 'n'],
                '\t' => vec!['\\', 't'],
                other => vec![other],
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn formats_a_simple_mapping() {
            let out = format("name: caduceus\nversion: 3\n").unwrap();
            assert_eq!(out, "name: caduceus\nversion: 3");
        }

        #[test]
        fn formats_nested_sequences_and_maps() {
            let input = "fruits:\n  - apple\n  - banana\nmeta:\n  ok: true\n";
            let out = format(input).unwrap();
            assert!(out.contains("fruits:\n  - apple\n  - banana"));
            assert!(out.contains("meta:\n  ok: true"));
        }

        #[test]
        fn a_sequence_of_mappings_round_trips() {
            let input = "items:\n  - name: a\n    id: 1\n  - name: b\n    id: 2\n";
            let out = format(input).unwrap();
            assert!(out.contains("- name: a"));
            assert!(out.contains("id: 1"));
            assert!(out.contains("- name: b"));
        }

        #[test]
        fn flow_collections_parse() {
            let out = format("nums: [1, 2, 3]\ninfo: {a: 1, b: two}\n").unwrap();
            assert!(out.contains("nums:\n  - 1\n  - 2\n  - 3"));
            assert!(out.contains("info:\n  a: 1\n  b: two"));
        }

        #[test]
        fn quoted_strings_that_look_like_numbers_stay_strings() {
            let out = format("version: \"1.0\"\n").unwrap();
            assert_eq!(out, "version: \"1.0\"");
        }

        #[test]
        fn comments_are_dropped_and_do_not_break_parsing() {
            let out = format("# a comment\nname: caduceus # trailing\n").unwrap();
            assert_eq!(out, "name: caduceus");
        }

        #[test]
        fn tabs_in_indentation_are_rejected() {
            let err = format("name:\n\tvalue: x\n").unwrap_err();
            assert!(err.contains("tabs"));
        }

        #[test]
        fn duplicate_keys_are_rejected() {
            let err = format("a: 1\na: 2\n").unwrap_err();
            assert!(err.contains("duplicate key"));
        }

        #[test]
        fn anchors_are_reported_rather_than_mishandled() {
            let err = format("a: &anchor value\n").unwrap_err();
            assert!(err.contains("anchor"));
        }

        #[test]
        fn literal_block_scalars_keep_newlines() {
            let input = "script: |\n  line one\n  line two\n";
            let value = &parse_document(input).unwrap()[0];
            if let Value::Map(entries) = value {
                assert_eq!(entries[0].1, Value::Str("line one\nline two".to_string()));
            } else {
                panic!("expected a map");
            }
        }

        #[test]
        fn multi_document_streams_format_each_document() {
            let out = format("a: 1\n---\nb: 2\n").unwrap();
            assert_eq!(out, "a: 1\n---\nb: 2");
        }

        #[test]
        fn malformed_yaml_is_rejected_not_guessed_at() {
            assert!(format("a: 1\n  b: 2\n").is_err());
        }
    }
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

mod xml {
    //! A hand-rolled well-formedness checker and pretty-printer.
    //!
    //! This checks *well-formedness* (matching tags, quoted attributes,
    //! exactly one root element, no bare `&`/`<`) the way a browser's parser
    //! would reject markup — it does not validate against a DTD or XML
    //! Schema, since neither is fetched or supplied.

    #[derive(Debug, Clone, PartialEq)]
    pub enum Node {
        Element { name: String, attrs: Vec<(String, String)>, children: Vec<Node> },
        Text(String),
        Comment(String),
        CData(String),
        Pi { target: String, data: String },
    }

    pub fn parse_document(input: &str) -> Result<Vec<Node>, String> {
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;
        let mut top_level = Vec::new();
        while i < chars.len() {
            skip_ws(&chars, &mut i);
            if i >= chars.len() {
                break;
            }
            if chars[i] == '<' {
                top_level.push(parse_node(&chars, &mut i)?);
            } else {
                let (text, consumed) = read_text(&chars, i);
                if !text.trim().is_empty() {
                    return Err("text is not allowed outside the root element".to_string());
                }
                i = consumed;
            }
        }
        let roots: Vec<&Node> = top_level.iter().filter(|n| matches!(n, Node::Element { .. })).collect();
        if roots.is_empty() {
            return Err("no root element found".to_string());
        }
        if roots.len() > 1 {
            return Err(format!("a document can have only one root element (found {})", roots.len()));
        }
        Ok(top_level)
    }

    pub fn format(input: &str) -> Result<String, String> {
        let nodes = parse_document(input)?;
        let mut out = String::new();
        for node in &nodes {
            match node {
                Node::Element { .. } => print_node(node, 0, &mut out),
                Node::Comment(_) | Node::Pi { .. } => {
                    print_node(node, 0, &mut out);
                    out.push('\n');
                }
                _ => {}
            }
        }
        Ok(out.trim_end().to_string())
    }

    fn skip_ws(chars: &[char], i: &mut usize) {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    }

    fn read_text(chars: &[char], start: usize) -> (String, usize) {
        let mut i = start;
        while i < chars.len() && chars[i] != '<' {
            i += 1;
        }
        (chars[start..i].iter().collect(), i)
    }

    fn parse_node(chars: &[char], i: &mut usize) -> Result<Node, String> {
        // `*i` is at '<'.
        if starts_with(chars, *i, "<!--") {
            return parse_comment(chars, i);
        }
        if starts_with(chars, *i, "<![CDATA[") {
            return parse_cdata(chars, i);
        }
        if starts_with(chars, *i, "<!") {
            // DOCTYPE or another declaration: skip to the matching '>' at
            // depth 0 (declarations can nest '[' ']' internal subsets).
            return skip_declaration(chars, i);
        }
        if starts_with(chars, *i, "<?") {
            return parse_pi(chars, i);
        }
        parse_element(chars, i)
    }

    fn starts_with(chars: &[char], at: usize, needle: &str) -> bool {
        let needle: Vec<char> = needle.chars().collect();
        if at + needle.len() > chars.len() {
            return false;
        }
        chars[at..at + needle.len()] == needle[..]
    }

    fn find_from(chars: &[char], at: usize, needle: &str) -> Option<usize> {
        let needle: Vec<char> = needle.chars().collect();
        if needle.is_empty() || at >= chars.len() {
            return None;
        }
        (at..=chars.len().saturating_sub(needle.len())).find(|&j| chars[j..j + needle.len()] == needle[..])
    }

    fn parse_comment(chars: &[char], i: &mut usize) -> Result<Node, String> {
        let start = *i + 4;
        let end = find_from(chars, start, "-->").ok_or("unterminated comment (missing -->)")?;
        let text: String = chars[start..end].iter().collect();
        *i = end + 3;
        Ok(Node::Comment(text))
    }

    fn parse_cdata(chars: &[char], i: &mut usize) -> Result<Node, String> {
        let start = *i + 9;
        let end = find_from(chars, start, "]]>").ok_or("unterminated CDATA section (missing ]]>)")?;
        let text: String = chars[start..end].iter().collect();
        *i = end + 3;
        Ok(Node::CData(text))
    }

    fn skip_declaration(chars: &[char], i: &mut usize) -> Result<Node, String> {
        let start = *i;
        let mut depth = 0i32;
        let mut j = *i;
        loop {
            if j >= chars.len() {
                return Err("unterminated declaration (missing >)".to_string());
            }
            match chars[j] {
                '[' => depth += 1,
                ']' => depth -= 1,
                '>' if depth <= 0 => {
                    j += 1;
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let raw: String = chars[start..j].iter().collect();
        *i = j;
        Ok(Node::Pi { target: "!doctype".to_string(), data: raw })
    }

    fn parse_pi(chars: &[char], i: &mut usize) -> Result<Node, String> {
        let start = *i + 2;
        let end = find_from(chars, start, "?>").ok_or("unterminated processing instruction (missing ?>)")?;
        let raw: String = chars[start..end].iter().collect();
        let (target, data) = raw.split_once(char::is_whitespace).unwrap_or((raw.as_str(), ""));
        *i = end + 2;
        Ok(Node::Pi { target: target.to_string(), data: data.trim().to_string() })
    }

    fn parse_element(chars: &[char], i: &mut usize) -> Result<Node, String> {
        // `*i` is at '<' of an opening tag.
        let open_pos = *i;
        *i += 1;
        let name = read_name(chars, i).ok_or_else(|| format!("expected a tag name at position {open_pos}"))?;
        let mut attrs = Vec::new();
        loop {
            skip_ws(chars, i);
            if *i >= chars.len() {
                return Err(format!("unterminated tag <{name}>"));
            }
            if chars[*i] == '/' && chars.get(*i + 1) == Some(&'>') {
                *i += 2;
                return Ok(Node::Element { name, attrs, children: Vec::new() });
            }
            if chars[*i] == '>' {
                *i += 1;
                break;
            }
            let attr_name = read_name(chars, i)
                .ok_or_else(|| format!("expected an attribute name inside <{name}>"))?;
            skip_ws(chars, i);
            if chars.get(*i) != Some(&'=') {
                return Err(format!("attribute \"{attr_name}\" on <{name}> is missing its value"));
            }
            *i += 1;
            skip_ws(chars, i);
            let quote = chars.get(*i).copied();
            if quote != Some('"') && quote != Some('\'') {
                return Err(format!(
                    "attribute \"{attr_name}\" on <{name}> must be quoted"
                ));
            }
            let quote = quote.unwrap();
            *i += 1;
            let val_start = *i;
            while *i < chars.len() && chars[*i] != quote {
                *i += 1;
            }
            if *i >= chars.len() {
                return Err(format!("attribute \"{attr_name}\" on <{name}> has an unterminated value"));
            }
            let value: String = chars[val_start..*i].iter().collect();
            check_entities(&value).map_err(|e| format!("in attribute \"{attr_name}\" on <{name}>: {e}"))?;
            *i += 1;
            attrs.push((attr_name, value));
        }

        let mut children = Vec::new();
        loop {
            if *i >= chars.len() {
                return Err(format!("<{name}> is never closed"));
            }
            if chars[*i] == '<' {
                if starts_with(chars, *i, "</") {
                    let close_start = *i;
                    *i += 2;
                    let closing = read_name(chars, i)
                        .ok_or_else(|| format!("malformed closing tag near position {close_start}"))?;
                    skip_ws(chars, i);
                    if chars.get(*i) != Some(&'>') {
                        return Err(format!("closing tag </{closing}> is missing '>'"));
                    }
                    *i += 1;
                    if closing != name {
                        return Err(format!("<{name}> is closed by </{closing}>"));
                    }
                    return Ok(Node::Element { name, attrs, children });
                }
                children.push(parse_node(chars, i)?);
            } else {
                let (text, consumed) = read_text(chars, *i);
                check_entities(&text).map_err(|e| format!("in text inside <{name}>: {e}"))?;
                *i = consumed;
                if !text.is_empty() {
                    children.push(Node::Text(text));
                }
            }
        }
    }

    fn read_name(chars: &[char], i: &mut usize) -> Option<String> {
        let start = *i;
        while *i < chars.len()
            && (chars[*i].is_alphanumeric() || matches!(chars[*i], '_' | '-' | '.' | ':'))
        {
            *i += 1;
        }
        if *i == start {
            None
        } else {
            Some(chars[start..*i].iter().collect())
        }
    }

    /// Every bare `&` must start a recognised entity or numeric reference,
    /// and a bare `<` may never appear in text — the two well-formedness
    /// rules people most often paste broken markup past.
    fn check_entities(text: &str) -> Result<(), String> {
        if text.contains('<') {
            return Err("contains an unescaped '<'".to_string());
        }
        let bytes: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == '&' {
                let rest: String = bytes[i..].iter().collect();
                let end = rest.find(';');
                let valid = end.is_some_and(|e| {
                    let body = &rest[1..e];
                    !body.is_empty()
                        && (body.starts_with('#')
                            || body.chars().all(|c| c.is_alphanumeric()))
                });
                if !valid {
                    return Err("contains an unescaped '&' (use &amp;)".to_string());
                }
            }
            i += 1;
        }
        Ok(())
    }

    fn print_node(node: &Node, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        match node {
            Node::Element { name, attrs, children } => {
                out.push_str(&pad);
                out.push('<');
                out.push_str(name);
                for (k, v) in attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(v);
                    out.push('"');
                }
                let meaningful: Vec<&Node> = children
                    .iter()
                    .filter(|c| !matches!(c, Node::Text(t) if t.trim().is_empty()))
                    .collect();
                if meaningful.is_empty() {
                    out.push_str(" />");
                    return;
                }
                if let [Node::Text(t)] = meaningful[..] {
                    out.push('>');
                    out.push_str(t.trim());
                    out.push_str("</");
                    out.push_str(name);
                    out.push('>');
                    return;
                }
                out.push_str(">\n");
                for child in meaningful {
                    print_node(child, indent + 2, out);
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
            Node::Text(t) => {
                out.push_str(&pad);
                out.push_str(t.trim());
            }
            Node::Comment(c) => {
                out.push_str(&pad);
                out.push_str("<!--");
                out.push_str(c);
                out.push_str("-->");
            }
            Node::CData(c) => {
                out.push_str(&pad);
                out.push_str("<![CDATA[");
                out.push_str(c);
                out.push_str("]]>");
            }
            Node::Pi { target, data } => {
                out.push_str(&pad);
                if target == "!doctype" {
                    out.push_str(data);
                } else {
                    out.push_str("<?");
                    out.push_str(target);
                    if !data.is_empty() {
                        out.push(' ');
                        out.push_str(data);
                    }
                    out.push_str("?>");
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn formats_a_simple_document() {
            let out = format("<root><a>1</a><b>2</b></root>").unwrap();
            assert_eq!(out, "<root>\n  <a>1</a>\n  <b>2</b>\n</root>");
        }

        #[test]
        fn self_closing_tags_stay_self_closing() {
            let out = format("<root><empty/></root>").unwrap();
            assert!(out.contains("<empty />"));
        }

        #[test]
        fn attributes_are_preserved() {
            let out = format("<root a=\"1\" b='2'></root>").unwrap();
            assert!(out.contains("a=\"1\" b=\"2\""));
        }

        #[test]
        fn keeps_the_xml_declaration_and_comments() {
            let out = format("<?xml version=\"1.0\"?>\n<!-- hi -->\n<root/>").unwrap();
            assert!(out.starts_with("<?xml version=\"1.0\"?>"));
            assert!(out.contains("<!-- hi -->"));
        }

        #[test]
        fn mismatched_tags_are_rejected() {
            let err = format("<a><b></a></b>").unwrap_err();
            assert!(err.contains("closed by"));
        }

        #[test]
        fn multiple_roots_are_rejected() {
            let err = format("<a/><b/>").unwrap_err();
            assert!(err.contains("one root element"));
        }

        #[test]
        fn unquoted_attributes_are_rejected() {
            let err = format("<a b=1></a>").unwrap_err();
            assert!(err.contains("quoted"));
        }

        #[test]
        fn bare_ampersands_are_rejected() {
            let err = format("<a>Q&A</a>").unwrap_err();
            assert!(err.contains('&'));
        }

        #[test]
        fn escaped_ampersands_are_accepted() {
            assert!(format("<a>Q&amp;A</a>").is_ok());
            assert!(format("<a>&#169;</a>").is_ok());
        }

        #[test]
        fn cdata_sections_round_trip() {
            let out = format("<a><![CDATA[<raw>]]></a>").unwrap();
            assert!(out.contains("<![CDATA[<raw>]]>"));
        }
    }
}

// ---------------------------------------------------------------------------
// HTML entities
// ---------------------------------------------------------------------------

mod html_entities {
    //! A wider entity table than `dev.rs`'s `HtmlEncode`/`HtmlDecode`.
    //!
    //! Those two exist already and are deliberately minimal — the five
    //! characters that make text unsafe to drop into markup (`& < > " '`).
    //! This is the other job people mean by "HTML entity encoder": turning
    //! arbitrary Unicode into an ASCII-safe, numeric-and-named entity form,
    //! and decoding the full classic named-entity set (not just those five)
    //! plus numeric references back to text.

    /// The HTML 4 / Latin-1 + symbol named character references — the set
    /// people mean by "HTML entities" outside the five XML-significant ones,
    /// which are included here too so encode/decode are each a single table.
    const NAMED: &[(&str, char)] = &[
        ("amp", '&'), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("apos", '\''),
        ("nbsp", '\u{00A0}'), ("cent", '¢'), ("pound", '£'), ("yen", '¥'), ("euro", '€'),
        ("copy", '©'), ("reg", '®'), ("trade", '™'), ("deg", '°'), ("plusmn", '±'),
        ("times", '×'), ("divide", '÷'), ("micro", 'µ'), ("para", '¶'), ("sect", '§'),
        ("laquo", '«'), ("raquo", '»'), ("ndash", '–'), ("mdash", '—'),
        ("lsquo", '\u{2018}'), ("rsquo", '\u{2019}'), ("ldquo", '\u{201C}'), ("rdquo", '\u{201D}'),
        ("hellip", '…'), ("dagger", '†'), ("Dagger", '‡'), ("bull", '•'),
        ("prime", '′'), ("Prime", '″'), ("larr", '←'), ("uarr", '↑'), ("rarr", '→'), ("darr", '↓'), ("harr", '↔'),
        ("spades", '♠'), ("clubs", '♣'), ("hearts", '♥'), ("diams", '♦'),
        ("infin", '∞'), ("ne", '≠'), ("le", '≤'), ("ge", '≥'), ("radic", '√'), ("sum", '∑'), ("prod", '∏'), ("part", '∂'),
        ("alpha", 'α'), ("beta", 'β'), ("gamma", 'γ'), ("delta", 'δ'), ("epsilon", 'ε'),
        ("pi", 'π'), ("sigma", 'σ'), ("omega", 'ω'), ("Omega", 'Ω'), ("mu", 'μ'), ("lambda", 'λ'), ("theta", 'θ'),
        ("frac12", '½'), ("frac14", '¼'), ("frac34", '¾'), ("sup1", '¹'), ("sup2", '²'), ("sup3", '³'),
        ("Agrave", 'À'), ("Aacute", 'Á'), ("Acirc", 'Â'), ("Atilde", 'Ã'), ("Auml", 'Ä'), ("Aring", 'Å'), ("AElig", 'Æ'),
        ("Ccedil", 'Ç'), ("Egrave", 'È'), ("Eacute", 'É'), ("Ecirc", 'Ê'), ("Euml", 'Ë'),
        ("Igrave", 'Ì'), ("Iacute", 'Í'), ("Icirc", 'Î'), ("Iuml", 'Ï'), ("ETH", 'Ð'), ("Ntilde", 'Ñ'),
        ("Ograve", 'Ò'), ("Oacute", 'Ó'), ("Ocirc", 'Ô'), ("Otilde", 'Õ'), ("Ouml", 'Ö'), ("Oslash", 'Ø'),
        ("Ugrave", 'Ù'), ("Uacute", 'Ú'), ("Ucirc", 'Û'), ("Uuml", 'Ü'), ("Yacute", 'Ý'), ("THORN", 'Þ'), ("szlig", 'ß'),
        ("agrave", 'à'), ("aacute", 'á'), ("acirc", 'â'), ("atilde", 'ã'), ("auml", 'ä'), ("aring", 'å'), ("aelig", 'æ'),
        ("ccedil", 'ç'), ("egrave", 'è'), ("eacute", 'é'), ("ecirc", 'ê'), ("euml", 'ë'),
        ("igrave", 'ì'), ("iacute", 'í'), ("icirc", 'î'), ("iuml", 'ï'), ("eth", 'ð'), ("ntilde", 'ñ'),
        ("ograve", 'ò'), ("oacute", 'ó'), ("ocirc", 'ô'), ("otilde", 'õ'), ("ouml", 'ö'), ("oslash", 'ø'),
        ("ugrave", 'ù'), ("uacute", 'ú'), ("ucirc", 'û'), ("uuml", 'ü'), ("yacute", 'ý'), ("thorn", 'þ'), ("yuml", 'ÿ'),
    ];

    /// Encode: escape the XML-significant five by name, and every other
    /// non-ASCII character as a decimal numeric reference. ASCII printable
    /// text (the overwhelmingly common case) passes through untouched, so a
    /// round trip on plain English text is a no-op.
    pub fn encode(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for c in input.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c if (c as u32) > 0x7F => {
                    out.push_str("&#");
                    out.push_str(&(c as u32).to_string());
                    out.push(';');
                }
                c => out.push(c),
            }
        }
        out
    }

    /// Decode named entities (the full table above), decimal (`&#169;`) and
    /// hex (`&#xA9;`) numeric references. An entity missing its terminating
    /// `;`, or naming something not in the table, is left exactly as
    /// written — guessing at a truncated or unknown entity would corrupt
    /// text that was not actually an entity at all (a bare `&rarr` in prose
    /// about arrows, say).
    pub fn decode(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '&' {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            let Some(end) = chars[i..].iter().position(|&c| c == ';').map(|p| i + p) else {
                out.push(chars[i]);
                i += 1;
                continue;
            };
            let body: String = chars[i + 1..end].iter().collect();
            if let Some(hex) = body.strip_prefix('#').and_then(|b| b.strip_prefix(['x', 'X'])) {
                if let Ok(code) = u32::from_str_radix(hex, 16) {
                    out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    i = end + 1;
                    continue;
                }
            } else if let Some(dec) = body.strip_prefix('#') {
                if let Ok(code) = dec.parse::<u32>() {
                    out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    i = end + 1;
                    continue;
                }
            } else if let Some((_, c)) = NAMED.iter().find(|(name, _)| *name == body) {
                out.push(*c);
                i = end + 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ascii_text_is_unchanged() {
            assert_eq!(encode("hello, world"), "hello, world");
        }

        #[test]
        fn the_five_xml_significant_characters_are_named() {
            assert_eq!(encode("<a href=\"x\">'&'</a>"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
        }

        #[test]
        fn non_ascii_becomes_a_numeric_reference() {
            assert_eq!(encode("café"), "caf&#233;");
        }

        #[test]
        fn decode_reverses_named_and_numeric_forms() {
            assert_eq!(decode("caf&#233;"), "café");
            assert_eq!(decode("&amp;copy;"), "&copy;");
            assert_eq!(decode("&copy; &euro; &mdash;"), "© € —");
        }

        #[test]
        fn hex_numeric_references_decode() {
            assert_eq!(decode("&#xA9;"), "©");
        }

        #[test]
        fn unknown_or_unterminated_entities_pass_through() {
            assert_eq!(decode("Ships &rarr today"), "Ships &rarr today");
            assert_eq!(decode("&notreal;"), "&notreal;");
        }

        #[test]
        fn round_trips_through_encode_then_decode() {
            let original = "café — “quoted” <tag> & more";
            assert_eq!(decode(&encode(original)), original);
        }
    }
}

// ---------------------------------------------------------------------------
// SQL formatter
// ---------------------------------------------------------------------------

mod sql {
    //! A keyword-driven line-breaker, not a parser with an AST — SQL dialects
    //! diverge enough (backtick vs bracket identifiers, `LIMIT` vs `TOP`,
    //! vendor-specific hints) that "correctly formats every dialect" is not
    //! achievable without a real grammar. What this does is what every
    //! lightweight SQL formatter does: tokenize respecting strings and
    //! comments, then put each major clause on its own line and indent what
    //! hangs off it, which is what makes a wall of SQL readable at all.

    const MAJOR_CLAUSES: &[&str] = &[
        "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET",
        "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE FROM", "UNION ALL", "UNION",
    ];
    const JOIN_CLAUSES: &[&str] = &[
        "LEFT OUTER JOIN", "RIGHT OUTER JOIN", "FULL OUTER JOIN", "LEFT JOIN", "RIGHT JOIN",
        "INNER JOIN", "CROSS JOIN", "JOIN",
    ];
    const CONJUNCTIONS: &[&str] = &["AND", "OR"];
    const KEYWORDS: &[&str] = &[
        "SELECT", "FROM", "WHERE", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "INSERT",
        "INTO", "VALUES", "UPDATE", "SET", "DELETE", "UNION", "ALL", "JOIN", "LEFT", "RIGHT",
        "FULL", "OUTER", "INNER", "CROSS", "ON", "AND", "OR", "NOT", "NULL", "IS", "IN", "AS",
        "DISTINCT", "CASE", "WHEN", "THEN", "ELSE", "END", "ASC", "DESC", "LIKE", "BETWEEN",
        "EXISTS", "COUNT", "SUM", "AVG", "MIN", "MAX",
    ];

    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Word(String),
        Symbol(char),
        Str(String),
        Comment(String),
    }

    fn tokenize(input: &str) -> Vec<Token> {
        let chars: Vec<char> = input.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            if c == '-' && chars.get(i + 1) == Some(&'-') {
                let start = i;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                tokens.push(Token::Comment(chars[start..i].iter().collect()));
                continue;
            }
            if c == '/' && chars.get(i + 1) == Some(&'*') {
                let start = i;
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
                tokens.push(Token::Comment(chars[start..i].iter().collect()));
                continue;
            }
            if c == '\'' || c == '"' || c == '`' {
                let quote = c;
                let start = i;
                i += 1;
                while i < chars.len() {
                    if chars[i] == quote && chars.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    if chars[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                tokens.push(Token::Str(chars[start..i].iter().collect()));
                continue;
            }
            if c.is_alphanumeric() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
                tokens.push(Token::Word(chars[start..i].iter().collect()));
                continue;
            }
            tokens.push(Token::Symbol(c));
            i += 1;
        }
        tokens
    }

    /// Regroup tokens so that multi-word clauses (`GROUP BY`, `LEFT JOIN`,
    /// `DELETE FROM`) are matched as a unit rather than breaking mid-phrase.
    fn phrase_at(tokens: &[Token], i: usize, phrase: &str) -> bool {
        let words: Vec<&str> = phrase.split(' ').collect();
        if i + words.len() > tokens.len() {
            return false;
        }
        for (offset, w) in words.iter().enumerate() {
            match &tokens[i + offset] {
                Token::Word(t) if t.eq_ignore_ascii_case(w) => {}
                _ => return false,
            }
        }
        true
    }

    pub fn format(input: &str) -> String {
        let tokens = tokenize(input);
        let mut out = String::new();
        let mut i = 0;
        let mut indent = 0usize;
        let mut line_has_content = false;
        let mut paren_depth = 0i32;

        let newline = |out: &mut String, indent: usize| {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
            out.push_str(&"  ".repeat(indent));
        };

        while i < tokens.len() {
            if let Some(phrase) = MAJOR_CLAUSES.iter().find(|p| phrase_at(&tokens, i, p)) {
                if line_has_content {
                    newline(&mut out, indent.saturating_sub(1));
                }
                out.push_str(&phrase.to_uppercase());
                out.push(' ');
                i += phrase.split(' ').count();
                indent = 1;
                line_has_content = true;
                continue;
            }
            if let Some(phrase) = JOIN_CLAUSES.iter().find(|p| phrase_at(&tokens, i, p)) {
                newline(&mut out, 0);
                out.push_str(&phrase.to_uppercase());
                out.push(' ');
                i += phrase.split(' ').count();
                indent = 1;
                line_has_content = true;
                continue;
            }
            if let Some(phrase) = CONJUNCTIONS.iter().find(|p| phrase_at(&tokens, i, p)) {
                newline(&mut out, 1);
                out.push_str(&phrase.to_uppercase());
                out.push(' ');
                i += 1;
                line_has_content = true;
                continue;
            }

            match &tokens[i] {
                Token::Word(w) => {
                    if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(w)) {
                        out.push_str(&w.to_uppercase());
                    } else {
                        out.push_str(w);
                    }
                    out.push(' ');
                }
                Token::Str(s) => {
                    out.push_str(s);
                    out.push(' ');
                }
                Token::Comment(c) => {
                    if line_has_content {
                        newline(&mut out, indent);
                    }
                    out.push_str(c);
                    newline(&mut out, indent);
                    line_has_content = false;
                    i += 1;
                    continue;
                }
                Token::Symbol('(') => {
                    paren_depth += 1;
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push('(');
                }
                Token::Symbol(')') => {
                    paren_depth -= 1;
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str(") ");
                }
                Token::Symbol(',') => {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    if indent >= 1 && paren_depth == 0 {
                        out.push(',');
                        newline(&mut out, indent);
                    } else {
                        out.push_str(", ");
                    }
                }
                Token::Symbol(c) => {
                    while out.ends_with(' ') && matches!(c, '.' | ';') {
                        out.pop();
                    }
                    out.push(*c);
                    out.push(' ');
                }
            }
            line_has_content = true;
            i += 1;
        }
        out.trim().to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn breaks_major_clauses_onto_their_own_lines() {
            let out = format("select a, b from t where a = 1");
            assert_eq!(out, "SELECT a,\n  b\nFROM t\nWHERE a = 1");
        }

        #[test]
        fn joins_and_conjunctions_get_their_own_lines() {
            let out = format("select * from a join b on a.id = b.id where a.x = 1 and b.y = 2");
            assert!(out.contains("\nJOIN b ON"));
            assert!(out.contains("\n  AND b.y"));
        }

        #[test]
        fn keywords_are_uppercased_identifiers_are_not() {
            let out = format("select myColumn from myTable");
            assert!(out.contains("SELECT myColumn"));
            assert!(out.contains("FROM myTable"));
        }

        #[test]
        fn string_literals_are_left_alone() {
            let out = format("select * from t where name = 'DROP TABLE'");
            assert!(out.contains("'DROP TABLE'"));
        }

        #[test]
        fn line_comments_survive() {
            let out = format("select a -- pick a\nfrom t");
            assert!(out.contains("-- pick a"));
        }

        #[test]
        fn group_by_is_matched_as_one_phrase() {
            let out = format("select a, count(*) from t group by a");
            assert!(out.contains("\nGROUP BY a"));
        }
    }
}

// ---------------------------------------------------------------------------
// cURL parser + HTTP playground
// ---------------------------------------------------------------------------

/// A parsed `curl` invocation, ready to either display or replay.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurlRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// `(username, password)` from `-u`/`--user`, kept separate from
    /// `headers` so a caller can decide whether to show it in the clear.
    pub basic_auth: Option<(String, String)>,
    pub follow_redirects: bool,
    /// Recorded from `-k`/`--insecure`, but never acted on — see
    /// [`execute`]'s doc comment.
    pub insecure: bool,
    pub compressed: bool,
    /// Flags this parser recognised but does not model (e.g. `-o file`,
    /// `--max-time`) — recorded so a caller can show "N flags ignored"
    /// instead of the parse silently dropping them.
    pub ignored_flags: Vec<String>,
}

impl Default for CurlRequest {
    fn default() -> Self {
        Self {
            method: "GET".to_string(),
            url: String::new(),
            headers: Vec::new(),
            body: None,
            basic_auth: None,
            follow_redirects: false,
            insecure: false,
            compressed: false,
            ignored_flags: Vec::new(),
        }
    }
}

/// The result of replaying a parsed `curl` command through `reqwest`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpPlaygroundResult {
    pub ok: bool,
    pub request: CurlRequest,
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub body_truncated: bool,
    pub error: Option<String>,
}

/// Flags known to take a following argument that this parser does not model
/// (so the tokenizer must still skip that argument, or it would be mistaken
/// for the URL or another flag).
const IGNORED_VALUE_FLAGS: &[&str] = &[
    "-o", "--output", "-w", "--write-out", "--max-time", "--connect-timeout", "-c",
    "--cookie-jar", "--retry", "--limit-rate", "-T", "--upload-file", "-e", "--referer",
    "--interface", "--resolve", "--cacert", "--cert", "--key", "-A", "--user-agent",
];
/// Flags known to take no argument that this parser does not otherwise act
/// on (kept boolean so the tokenizer does not eat the next real argument).
const IGNORED_BOOLEAN_FLAGS: &[&str] = &[
    "-s", "--silent", "-v", "--verbose", "-i", "--include", "-f", "--fail", "-O", "-g",
    "--globoff", "-N", "--no-buffer", "-#", "--progress-bar", "--http1.1", "--http2",
];

/// Split a pasted shell command into argv-style tokens: quote-aware
/// (single quotes literal, double quotes with backslash escapes for
/// `\" \\ \$ \`` per bash rules) and joining `\`-continued lines, because
/// that is exactly how a browser's "copy as cURL" and a terminal history
/// both produce multi-line pastes.
fn shell_tokenize(command: &str) -> Vec<String> {
    let joined = command.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let chars: Vec<char> = joined.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if has_current {
                tokens.push(std::mem::take(&mut current));
                has_current = false;
            }
            i += 1;
            continue;
        }
        has_current = true;
        match c {
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    current.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote (tolerated if missing: EOF just stops)
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '"' | '\\' | '$' | '`') {
                        current.push(chars[i + 1]);
                        i += 2;
                    } else {
                        current.push(chars[i]);
                        i += 1;
                    }
                }
                i += 1;
            }
            '\\' if i + 1 < chars.len() => {
                current.push(chars[i + 1]);
                i += 2;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }
    if has_current {
        tokens.push(current);
    }
    tokens
}

/// Parse a pasted `curl ...` command into a [`CurlRequest`].
///
/// Handles the flags people actually paste: `-X`/`--request`,
/// `-H`/`--header` (repeatable), `-d`/`--data`/`--data-raw`/`--data-binary`
/// (repeatable, `&`-joined, implies `POST`), `--data-urlencode`, `-u`/`--user`,
/// `-L`/`--location`, `-G`/`--get`, `--compressed`, both quote styles, and
/// backslash line continuations.
pub fn parse_curl(command: &str) -> Result<CurlRequest, String> {
    let tokens = shell_tokenize(command.trim());
    let mut tokens = tokens.into_iter().peekable();

    match tokens.peek().map(|s| s.as_str()) {
        Some("curl") => {
            tokens.next();
        }
        Some("sudo") => {
            tokens.next();
            if tokens.peek().map(|s| s.as_str()) == Some("curl") {
                tokens.next();
            }
        }
        _ => {}
    }

    let mut req = CurlRequest::default();
    let mut explicit_method: Option<String> = None;
    let mut data_parts: Vec<String> = Vec::new();
    let mut implies_post = false;
    let mut force_get = false;

    while let Some(tok) = tokens.next() {
        // getopt's short-option-with-attached-value form: `-XPOST` means the
        // same as `-X POST`, and it is common enough (curl's own docs use it)
        // that a parser which only accepts the spaced form would reject a
        // real share of what people paste.
        if tok.starts_with("-X") && tok.len() > 2 && !tok.starts_with("--") {
            explicit_method = Some(tok[2..].to_uppercase());
            continue;
        }

        let (flag, attached) = match tok.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (tok.clone(), None),
        };

        macro_rules! value {
            () => {
                attached.clone().or_else(|| tokens.next())
            };
        }

        match flag.as_str() {
            "-X" | "--request" => {
                if let Some(v) = value!() {
                    explicit_method = Some(v.to_uppercase());
                }
            }
            "-H" | "--header" => {
                if let Some(v) = value!() {
                    if let Some((name, val)) = v.split_once(':') {
                        req.headers.push((name.trim().to_string(), val.trim().to_string()));
                    }
                }
            }
            "-d" | "--data" | "--data-ascii" | "--data-raw" | "--data-binary" => {
                if let Some(v) = value!() {
                    implies_post = true;
                    data_parts.push(v);
                }
            }
            "--data-urlencode" => {
                if let Some(v) = value!() {
                    implies_post = true;
                    data_parts.push(urlencode_data_urlencode_arg(&v));
                }
            }
            "-u" | "--user" => {
                if let Some(v) = value!() {
                    let (user, pass) = v.split_once(':').unwrap_or((v.as_str(), ""));
                    req.basic_auth = Some((user.to_string(), pass.to_string()));
                }
            }
            "-L" | "--location" => req.follow_redirects = true,
            "-k" | "--insecure" => req.insecure = true,
            "--compressed" => req.compressed = true,
            "-G" | "--get" => force_get = true,
            "-b" | "--cookie" => {
                if let Some(v) = value!() {
                    if !v.starts_with('@') {
                        req.headers.push(("Cookie".to_string(), v));
                    } else {
                        req.ignored_flags.push(format!("{flag} {v} (cookie jar file)"));
                    }
                }
            }
            "--url" => {
                if let Some(v) = value!() {
                    req.url = v;
                }
            }
            _ if IGNORED_VALUE_FLAGS.contains(&flag.as_str()) => {
                let v = value!();
                req.ignored_flags.push(match v {
                    Some(v) => format!("{flag} {v}"),
                    None => flag,
                });
            }
            _ if IGNORED_BOOLEAN_FLAGS.contains(&flag.as_str()) => {
                req.ignored_flags.push(flag);
            }
            _ if flag.starts_with('-') && flag != "-" => {
                req.ignored_flags.push(flag);
            }
            _ => {
                if req.url.is_empty() {
                    req.url = tok.trim_matches(|c| c == '\'' || c == '"').to_string();
                }
            }
        }
    }

    if req.url.is_empty() {
        return Err("No URL found in that command.".to_string());
    }

    if !data_parts.is_empty() {
        let joined = data_parts.join("&");
        if force_get {
            let sep = if req.url.contains('?') { '&' } else { '?' };
            req.url = format!("{}{sep}{joined}", req.url);
        } else {
            req.body = Some(joined);
            if !req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                req.headers.push(("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()));
            }
        }
    }

    req.method = explicit_method.unwrap_or_else(|| {
        if force_get {
            "GET".to_string()
        } else if implies_post {
            "POST".to_string()
        } else {
            "GET".to_string()
        }
    });

    Ok(req)
}

/// `--data-urlencode`'s `name=value` / bare-`value` / `@file` forms. Only the
/// first two are handled — reading an arbitrary local file referenced by a
/// pasted command is more than a paste-and-run parser should do unprompted.
fn urlencode_data_urlencode_arg(v: &str) -> String {
    if let Some(rest) = v.strip_prefix('@') {
        return format!("(file:{rest} not read)");
    }
    match v.split_once('=') {
        Some((name, val)) => format!("{name}={}", percent_encode(val)),
        None => percent_encode(v),
    }
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Cap on the body read into memory and shown back — large enough for any
/// API response worth reading by eye, small enough that a curl pasted
/// against a file-download endpoint does not fill the app's memory.
const MAX_PLAYGROUND_BODY: usize = 2_000_000;

/// Parse and replay a pasted `curl` command.
///
/// `-k`/`--insecure` is recorded on the parsed request but never applied —
/// this executes with the platform's normal certificate validation
/// regardless of what the pasted command asked for. A tool that silently
/// turned off TLS verification because a curl snippet said to would be a
/// standing footgun for anything pasted from an untrusted source (a support
/// ticket, a "try this" in chat); if the pasted command truly needs it, the
/// user can say so by running it in a real terminal themselves.
pub async fn execute(command: &str) -> HttpPlaygroundResult {
    let request = match parse_curl(command) {
        Ok(r) => r,
        Err(e) => {
            return HttpPlaygroundResult {
                ok: false,
                request: CurlRequest::default(),
                status: None,
                status_text: None,
                headers: Vec::new(),
                body: String::new(),
                body_truncated: false,
                error: Some(e),
            }
        }
    };

    let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return HttpPlaygroundResult {
                ok: false,
                error: Some(format!("\"{}\" is not a valid HTTP method.", request.method)),
                request,
                status: None,
                status_text: None,
                headers: Vec::new(),
                body: String::new(),
                body_truncated: false,
            }
        }
    };

    let policy = if request.follow_redirects {
        reqwest::redirect::Policy::limited(10)
    } else {
        reqwest::redirect::Policy::none()
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .redirect(policy)
        .user_agent(concat!("Caduceus/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HttpPlaygroundResult {
                ok: false,
                error: Some(format!("Could not start the request: {e}")),
                request,
                status: None,
                status_text: None,
                headers: Vec::new(),
                body: String::new(),
                body_truncated: false,
            }
        }
    };

    let mut builder = client.request(method, &request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if let Some((user, pass)) = &request.basic_auth {
        builder = builder.basic_auth(user, Some(pass));
    }
    if let Some(body) = &request.body {
        builder = builder.body(body.clone());
    }

    match builder.send().await {
        Ok(response) => {
            let status = response.status();
            let headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                .collect();
            let is_json = headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("json"));
            match response.bytes().await {
                Ok(bytes) => {
                    let truncated = bytes.len() > MAX_PLAYGROUND_BODY;
                    let slice = &bytes[..bytes.len().min(MAX_PLAYGROUND_BODY)];
                    let mut text = String::from_utf8_lossy(slice).to_string();
                    if is_json && !truncated {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                                text = pretty;
                            }
                        }
                    }
                    HttpPlaygroundResult {
                        ok: true,
                        request,
                        status: Some(status.as_u16()),
                        status_text: status.canonical_reason().map(str::to_string),
                        headers,
                        body: text,
                        body_truncated: truncated,
                        error: None,
                    }
                }
                Err(e) => HttpPlaygroundResult {
                    ok: false,
                    request,
                    status: Some(status.as_u16()),
                    status_text: status.canonical_reason().map(str::to_string),
                    headers,
                    body: String::new(),
                    body_truncated: false,
                    error: Some(format!("Could not read the response body: {e}")),
                },
            }
        }
        Err(e) if e.is_timeout() => HttpPlaygroundResult {
            ok: false,
            request,
            status: None,
            status_text: None,
            headers: Vec::new(),
            body: String::new(),
            body_truncated: false,
            error: Some("The request timed out.".to_string()),
        },
        Err(e) => HttpPlaygroundResult {
            ok: false,
            request,
            status: None,
            status_text: None,
            headers: Vec::new(),
            body: String::new(),
            body_truncated: false,
            error: Some(format!("Request failed: {e}")),
        },
    }
}

#[cfg(test)]
mod curl_tests {
    use super::*;

    #[test]
    fn parses_a_minimal_get() {
        let req = parse_curl("curl https://example.com").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://example.com");
    }

    #[test]
    fn parses_chrome_style_copy_as_curl() {
        let cmd = r#"curl 'https://api.example.com/v1/things' \
  -H 'accept: application/json' \
  -H 'authorization: Bearer abc123' \
  --data-raw '{"a":1}' \
  --compressed"#;
        let req = parse_curl(cmd).unwrap();
        assert_eq!(req.url, "https://api.example.com/v1/things");
        assert_eq!(req.method, "POST");
        assert!(req.compressed);
        assert!(req.headers.iter().any(|(k, v)| k == "accept" && v == "application/json"));
        assert!(req.headers.iter().any(|(k, v)| k == "authorization" && v == "Bearer abc123"));
        assert_eq!(req.body.as_deref(), Some(r#"{"a":1}"#));
    }

    #[test]
    fn dash_x_attached_form_is_understood() {
        let req = parse_curl("curl -XPOST https://example.com").unwrap();
        assert_eq!(req.method, "POST");
    }

    #[test]
    fn explicit_x_wins_over_data_implying_post() {
        let req = parse_curl("curl -X PATCH -d 'a=1' https://example.com").unwrap();
        assert_eq!(req.method, "PATCH");
    }

    #[test]
    fn multiple_data_flags_join_with_ampersand() {
        let req = parse_curl("curl -d a=1 -d b=2 https://example.com").unwrap();
        assert_eq!(req.body.as_deref(), Some("a=1&b=2"));
        assert_eq!(req.method, "POST");
    }

    #[test]
    fn basic_auth_is_parsed_separately_from_headers() {
        let req = parse_curl("curl -u alice:hunter2 https://example.com").unwrap();
        assert_eq!(req.basic_auth, Some(("alice".to_string(), "hunter2".to_string())));
    }

    #[test]
    fn double_quotes_with_escapes_are_honoured() {
        let req = parse_curl(r#"curl -H "X-Name: say \"hi\"" https://example.com"#).unwrap();
        assert!(req.headers.iter().any(|(k, v)| k == "X-Name" && v == "say \"hi\""));
    }

    #[test]
    fn single_quotes_are_literal() {
        let req = parse_curl(r#"curl -d 'a=$HOME' https://example.com"#).unwrap();
        assert_eq!(req.body.as_deref(), Some("a=$HOME"));
    }

    #[test]
    fn location_flag_sets_follow_redirects() {
        let req = parse_curl("curl -L https://example.com").unwrap();
        assert!(req.follow_redirects);
        let req2 = parse_curl("curl https://example.com").unwrap();
        assert!(!req2.follow_redirects);
    }

    #[test]
    fn insecure_flag_is_recorded_not_ignored() {
        let req = parse_curl("curl -k https://example.com").unwrap();
        assert!(req.insecure);
    }

    #[test]
    fn unknown_flags_with_values_do_not_swallow_the_url() {
        let req = parse_curl("curl --max-time 5 https://example.com -o out.json").unwrap();
        assert_eq!(req.url, "https://example.com");
        assert!(req.ignored_flags.iter().any(|f| f.starts_with("--max-time")));
        assert!(req.ignored_flags.iter().any(|f| f.starts_with("-o")));
    }

    #[test]
    fn a_missing_url_is_an_error() {
        assert!(parse_curl("curl -H 'a: b'").is_err());
    }

    #[test]
    fn get_flag_moves_data_into_the_query_string() {
        let req = parse_curl("curl -G -d a=1 -d b=2 https://example.com").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.url, "https://example.com?a=1&b=2");
        assert!(req.body.is_none());
    }

    #[test]
    fn header_style_equals_form_is_accepted() {
        let req = parse_curl("curl --header='X-Test: 1' https://example.com").unwrap();
        assert!(req.headers.iter().any(|(k, v)| k == "X-Test" && v == "1"));
    }

    #[test]
    fn line_continuations_join_into_one_command() {
        let cmd = "curl https://example.com \\\n  -H 'a: b'";
        let req = parse_curl(cmd).unwrap();
        assert!(req.headers.iter().any(|(k, _)| k == "a"));
    }
}

// ---------------------------------------------------------------------------
// Git status + AI commit message
// ---------------------------------------------------------------------------

/// One file's entry in `git status --porcelain`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    pub path: String,
    /// Human words for the porcelain status letter (`Modified`, `Added`,
    /// `Deleted`, `Renamed`, `Untracked`, `Copied`) rather than the raw code
    /// — nobody but git itself finds `"M "` self-explanatory.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitAssist {
    pub ok: bool,
    pub branch: Option<String>,
    pub staged: Vec<GitFileChange>,
    pub unstaged: Vec<GitFileChange>,
    pub suggested_message: Option<String>,
    pub error: Option<String>,
}

fn git(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let out = output_with_timeout(
        Command::new("git").arg("-C").arg(repo_path).args(args),
        TOOL_TIMEOUT,
        "git did not answer in time.",
    )?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn status_word(code: char) -> &'static str {
    match code {
        'M' => "Modified",
        'A' => "Added",
        'D' => "Deleted",
        'R' => "Renamed",
        'C' => "Copied",
        'U' => "Conflicted",
        '?' => "Untracked",
        ' ' => "",
        other => {
            let _ = other;
            "Changed"
        }
    }
}

fn parse_porcelain(raw: &str) -> (Vec<GitFileChange>, Vec<GitFileChange>) {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    for line in raw.lines() {
        if line.len() < 3 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let rest = &line[3..];
        // A rename/copy entry reads "old -> new"; the destination is what
        // matters to someone deciding what to stage or describe.
        let path = rest.split(" -> ").last().unwrap_or(rest).to_string();

        if x == '?' && y == '?' {
            unstaged.push(GitFileChange { path, status: "Untracked".to_string() });
            continue;
        }
        if x != ' ' {
            staged.push(GitFileChange { path: path.clone(), status: status_word(x).to_string() });
        }
        if y != ' ' {
            unstaged.push(GitFileChange { path, status: status_word(y).to_string() });
        }
    }
    (staged, unstaged)
}

/// Cap on how much diff text gets sent to the AI backend — generous enough
/// for almost any real commit, small enough not to burn an outsized number
/// of tokens (or blow past a local model's context window) on one request.
const MAX_DIFF_CHARS: usize = 12_000;

/// Git status for `repo_path`, plus a commit message drafted from the diff
/// by whichever AI backend is configured.
///
/// Every git call here is read-only (`status`, `diff`, `rev-parse`) — this
/// never stages, commits, or otherwise writes to the repository. Drafting is
/// the whole feature; the user still presses commit themselves.
pub async fn git_commit_assist(settings: &SettingsManager, repo_path: &str) -> GitCommitAssist {
    let path = Path::new(repo_path);
    if !path.is_dir() {
        return GitCommitAssist {
            ok: false,
            branch: None,
            staged: Vec::new(),
            unstaged: Vec::new(),
            suggested_message: None,
            error: Some("That path does not exist.".to_string()),
        };
    }
    if let Err(e) = git(repo_path, &["rev-parse", "--is-inside-work-tree"]) {
        return GitCommitAssist {
            ok: false,
            branch: None,
            staged: Vec::new(),
            unstaged: Vec::new(),
            suggested_message: None,
            error: Some(format!("Not a git repository: {e}")),
        };
    }

    let branch = git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .ok();

    let (staged, unstaged) = match git(repo_path, &["status", "--porcelain"]) {
        Ok(raw) => parse_porcelain(&raw),
        Err(e) => {
            return GitCommitAssist {
                ok: false,
                branch,
                staged: Vec::new(),
                unstaged: Vec::new(),
                suggested_message: None,
                error: Some(format!("git status failed: {e}")),
            }
        }
    };

    if staged.is_empty() && unstaged.is_empty() {
        return GitCommitAssist {
            ok: true,
            branch,
            staged,
            unstaged,
            suggested_message: None,
            error: Some("Nothing to describe — the working tree is clean.".to_string()),
        };
    }

    // Staged changes are what a commit right now would actually contain;
    // only fall back to the unstaged diff when nothing is staged, so the
    // message describes what will really be committed whenever possible.
    let (diff, describing) = if !staged.is_empty() {
        (git(repo_path, &["diff", "--staged"]), "staged")
    } else {
        (git(repo_path, &["diff"]), "unstaged")
    };
    let diff = match diff {
        Ok(d) => d,
        Err(e) => {
            return GitCommitAssist {
                ok: false,
                branch,
                staged,
                unstaged,
                suggested_message: None,
                error: Some(format!("git diff failed: {e}")),
            }
        }
    };

    let truncated = diff.chars().count() > MAX_DIFF_CHARS;
    let diff_excerpt: String = diff.chars().take(MAX_DIFF_CHARS).collect();
    let prompt = format!(
        "Write a concise, conventional git commit message (a short summary \
         line under 72 characters, optionally a blank line and a body) for \
         the following {describing} diff. Reply with only the commit \
         message, no commentary or code fences.\n\n{diff_excerpt}{}",
        if truncated { "\n\n[diff truncated]" } else { "" }
    );

    let messages = vec![
        Message::system(
            "You write git commit messages: imperative mood, specific about \
             what changed and why when the diff makes that clear, no filler.",
        ),
        Message::user(prompt),
    ];

    match agent::chat_with_history(settings, messages).await {
        Ok(response) => GitCommitAssist {
            ok: true,
            branch,
            staged,
            unstaged,
            suggested_message: Some(response.text.trim().to_string()),
            error: None,
        },
        Err(e) => GitCommitAssist {
            ok: false,
            branch,
            staged,
            unstaged,
            suggested_message: None,
            error: Some(e.user_message()),
        },
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;

    #[test]
    fn parses_staged_and_unstaged_from_porcelain() {
        let raw = "M  staged.txt\n M unstaged.txt\nMM both.txt\n?? new.txt\n";
        let (staged, unstaged) = parse_porcelain(raw);
        assert!(staged.iter().any(|f| f.path == "staged.txt" && f.status == "Modified"));
        assert!(unstaged.iter().any(|f| f.path == "unstaged.txt" && f.status == "Modified"));
        assert!(staged.iter().any(|f| f.path == "both.txt"));
        assert!(unstaged.iter().any(|f| f.path == "both.txt"));
        assert!(unstaged.iter().any(|f| f.path == "new.txt" && f.status == "Untracked"));
    }

    #[test]
    fn renames_report_the_destination_path() {
        let raw = "R  old.txt -> new.txt\n";
        let (staged, _) = parse_porcelain(raw);
        assert_eq!(staged[0].path, "new.txt");
        assert_eq!(staged[0].status, "Renamed");
    }

    #[tokio::test]
    async fn a_nonexistent_path_is_reported_before_any_git_call() {
        let settings = SettingsManager::new(crate::settings::Settings::default());
        let result = git_commit_assist(&settings, "/definitely/not/a/real/path").await;
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("does not exist"));
    }
}

// ---------------------------------------------------------------------------
// /etc/hosts alias viewer (read-only)
// ---------------------------------------------------------------------------

mod hosts {
    //! Reads `/etc/hosts`. Nothing in this module ever opens the file for
    //! writing.
    //!
    //! Editing `/etc/hosts` needs root (it is owned by root, mode 644), so a
    //! write path here would mean either shelling out through `sudo` — which
    //! means either storing a password or interrupting the user with an
    //! authentication prompt neither of which belongs in a quick palette
    //! action — or silently failing on every unprivileged run. A viewer that
    //! never claims to be able to write is worth more than an editor that
    //! usually can't.

    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Entry {
        pub ip: String,
        pub hosts: Vec<String>,
    }

    pub fn read() -> Result<Vec<Entry>, String> {
        parse(&std::fs::read_to_string("/etc/hosts").map_err(|e| format!("Could not read /etc/hosts: {e}"))?)
    }

    fn parse(content: &str) -> Result<Vec<Entry>, String> {
        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(ip) = parts.next() else { continue };
            let hosts: Vec<String> = parts.map(str::to_string).collect();
            if hosts.is_empty() {
                continue;
            }
            entries.push(Entry { ip: ip.to_string(), hosts });
        }
        Ok(entries)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_ip_and_aliases_ignoring_comments() {
            let content = "127.0.0.1 localhost\n# a comment\n::1 localhost ip6-localhost\n";
            let entries = parse(content).unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].ip, "127.0.0.1");
            assert_eq!(entries[1].hosts, vec!["localhost", "ip6-localhost"]);
        }

        #[test]
        fn blank_and_comment_only_lines_are_skipped() {
            let entries = parse("\n   \n# just a comment\n").unwrap();
            assert!(entries.is_empty());
        }

        #[test]
        fn inline_comments_after_an_entry_are_stripped() {
            let entries = parse("10.0.0.1 dev.local # staging box\n").unwrap();
            assert_eq!(entries[0].hosts, vec!["dev.local"]);
        }
    }
}

// ---------------------------------------------------------------------------
// Package dependency inspector
// ---------------------------------------------------------------------------

/// How tightly a dependency's version is pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    /// Resolves to exactly one version forever (`"1.2.3"` in a lockfile
    /// sense: npm bare semver, pip `==`, Cargo `=1.2.3`).
    Exact,
    /// Resolves to a range that can move (`^1.2.3`, `>=1.0`, pip `~=`, a
    /// bare Cargo version — Cargo's *default* operator is caret, not exact).
    Range,
    /// No version constraint at all.
    Unpinned,
    /// A git/path/workspace/URL dependency — not wrong, just not something
    /// "exact vs. loose" describes the same way a registry version does.
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyEntry {
    pub name: String,
    pub version: String,
    pub group: String,
    pub pin: PinKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyReport {
    pub manifest: String,
    pub entries: Vec<DependencyEntry>,
    pub exact_count: usize,
    pub loose_count: usize,
}

fn build_report(manifest: &str, entries: Vec<DependencyEntry>) -> DependencyReport {
    let exact_count = entries.iter().filter(|e| e.pin == PinKind::Exact).count();
    let loose_count = entries.iter().filter(|e| matches!(e.pin, PinKind::Range | PinKind::Unpinned)).count();
    DependencyReport { manifest: manifest.to_string(), entries, exact_count, loose_count }
}

/// Parse `package.json`, `Cargo.toml`, or `requirements.txt`, chosen by
/// filename. No network lookups happen here — no vulnerability or
/// freshness data, just what the manifest itself says.
pub fn inspect_dependencies(manifest_path: &str) -> Result<DependencyReport, String> {
    let path = Path::new(manifest_path);
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read {manifest_path}: {e}"))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "package.json" => Ok(build_report("package.json", parse_package_json(&content)?)),
        "Cargo.toml" => Ok(build_report("Cargo.toml", parse_cargo_toml(&content))),
        n if n == "requirements.txt" || n.ends_with(".txt") => {
            Ok(build_report("requirements.txt", parse_requirements_txt(&content)))
        }
        _ => Err(format!(
            "\"{name}\" is not a manifest this inspector knows — expected package.json, \
             Cargo.toml, or requirements.txt."
        )),
    }
}

fn npm_pin_kind(version: &str) -> PinKind {
    let v = version.trim();
    if v.is_empty() || v == "*" || v == "latest" || v.eq_ignore_ascii_case("x") {
        return PinKind::Unpinned;
    }
    if v.starts_with("workspace:") || v.starts_with("file:") || v.starts_with("link:")
        || v.starts_with("git+") || v.starts_with("git:") || v.starts_with("github:")
        || v.starts_with("http:") || v.starts_with("https:")
    {
        return PinKind::Other;
    }
    let exact = regex::Regex::new(r"^\d+(\.\d+){0,2}(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$").unwrap();
    if exact.is_match(v) {
        PinKind::Exact
    } else {
        PinKind::Range
    }
}

fn parse_package_json(content: &str) -> Result<Vec<DependencyEntry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("That is not valid JSON: {e}"))?;
    let mut entries = Vec::new();
    for group in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
        let Some(obj) = value.get(group).and_then(|v| v.as_object()) else { continue };
        for (name, ver) in obj {
            let version = ver.as_str().unwrap_or("").to_string();
            entries.push(DependencyEntry {
                pin: npm_pin_kind(&version),
                name: name.clone(),
                version,
                group: group.to_string(),
            });
        }
    }
    Ok(entries)
}

fn cargo_pin_kind(version: &str) -> PinKind {
    let v = version.trim();
    if v.is_empty() {
        return PinKind::Other; // path/git dependency with no version key
    }
    if v == "workspace" {
        return PinKind::Other;
    }
    // Cargo's own operator prefixes: `=` is the only exact one. Everything
    // else — including a bare `"1.2.3"`, which looks exact to a human eye —
    // is a caret requirement by default and can resolve to a newer version.
    if v.starts_with('=') {
        PinKind::Exact
    } else {
        PinKind::Range
    }
}

fn parse_cargo_toml(content: &str) -> Vec<DependencyEntry> {
    let mut entries = Vec::new();
    let mut current_group: Option<String> = None;
    let inline_version = regex::Regex::new(r#"version\s*=\s*"([^"]*)""#).unwrap();
    let section_re = regex::Regex::new(r"^\[(.+)\]$").unwrap();

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        i += 1;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(caps) = section_re.captures(trimmed) {
            let section = caps[1].to_string();
            current_group = if section.to_ascii_lowercase().contains("dependencies") {
                Some(section)
            } else {
                None
            };
            continue;
        }
        let Some(group) = current_group.clone() else { continue };
        let Some((name_part, mut value_part)) = trimmed.split_once('=') else { continue };
        let name = name_part.trim().trim_end_matches(".workspace").to_string();
        if name.is_empty() {
            continue;
        }

        let mut collected = value_part.to_string();
        // A dependency's inline table can wrap across lines in a
        // hand-formatted Cargo.toml; keep reading until the braces balance.
        while collected.matches('{').count() > collected.matches('}').count() && i < lines.len() {
            collected.push('\n');
            collected.push_str(lines[i]);
            i += 1;
        }
        value_part = collected.trim();

        if trimmed.contains(".workspace") {
            entries.push(DependencyEntry {
                name,
                version: "workspace".to_string(),
                group,
                pin: PinKind::Other,
            });
            continue;
        }

        let version = if let Some(caps) = inline_version.captures(value_part) {
            caps[1].to_string()
        } else if value_part.trim_start().starts_with('"') {
            value_part.trim().trim_matches('"').to_string()
        } else {
            String::new() // path/git-only inline table, no explicit version
        };

        entries.push(DependencyEntry { pin: cargo_pin_kind(&version), name, version, group });
    }
    entries
}

fn pip_pin_kind(spec: &str) -> PinKind {
    let s = spec.trim();
    if s.is_empty() {
        return PinKind::Unpinned;
    }
    if s.starts_with('@') || s.contains("://") || s.starts_with("git+") {
        return PinKind::Other;
    }
    if s.contains("==") && !s.contains(',') {
        return PinKind::Exact;
    }
    PinKind::Range
}

fn parse_requirements_txt(content: &str) -> Vec<DependencyEntry> {
    let mut entries = Vec::new();
    let spec_re = regex::Regex::new(r"^([A-Za-z0-9._-]+)(\[[^\]]*\])?\s*(.*)$").unwrap();
    for raw in content.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        let line = if line.starts_with('#') { "" } else { line };
        if line.is_empty() {
            continue;
        }
        if line.starts_with('-') {
            continue; // -r other.txt / -e ./local / --index-url ... — options, not packages
        }
        let without_marker = line.split(';').next().unwrap_or(line).trim();
        let Some(caps) = spec_re.captures(without_marker) else { continue };
        let name = caps[1].to_string();
        let spec = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("").to_string();
        entries.push(DependencyEntry {
            pin: pip_pin_kind(&spec),
            name,
            version: if spec.is_empty() { "unpinned".to_string() } else { spec },
            group: "requirements".to_string(),
        });
    }
    entries
}

#[cfg(test)]
mod package_tests {
    use super::*;

    #[test]
    fn package_json_classifies_exact_and_range_pins() {
        let json = r#"{"dependencies": {"react": "18.2.0", "left-pad": "^1.3.0"},
                        "devDependencies": {"typescript": "*"}}"#;
        let entries = parse_package_json(json).unwrap();
        let react = entries.iter().find(|e| e.name == "react").unwrap();
        let left_pad = entries.iter().find(|e| e.name == "left-pad").unwrap();
        let ts = entries.iter().find(|e| e.name == "typescript").unwrap();
        assert_eq!(react.pin, PinKind::Exact);
        assert_eq!(left_pad.pin, PinKind::Range);
        assert_eq!(ts.pin, PinKind::Unpinned);
    }

    #[test]
    fn cargo_toml_bare_version_is_a_range_not_exact() {
        let toml = "[dependencies]\nserde = \"1.0\"\nregex = { version = \"1.13\", features = [\"std\"] }\npinned = \"=2.0.0\"\n";
        let entries = parse_cargo_toml(toml);
        let serde = entries.iter().find(|e| e.name == "serde").unwrap();
        let regex_dep = entries.iter().find(|e| e.name == "regex").unwrap();
        let pinned = entries.iter().find(|e| e.name == "pinned").unwrap();
        assert_eq!(serde.pin, PinKind::Range);
        assert_eq!(regex_dep.pin, PinKind::Range);
        assert_eq!(regex_dep.version, "1.13");
        assert_eq!(pinned.pin, PinKind::Exact);
    }

    #[test]
    fn cargo_toml_only_reads_dependency_sections() {
        let toml = "[package]\nname = \"x\"\nversion = \"1.0.0\"\n[dependencies]\nserde = \"1\"\n";
        let entries = parse_cargo_toml(toml);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "serde");
    }

    #[test]
    fn cargo_toml_path_dependencies_are_other_not_loose() {
        let toml = "[dependencies]\nlocal = { path = \"../local\" }\n";
        let entries = parse_cargo_toml(toml);
        assert_eq!(entries[0].pin, PinKind::Other);
    }

    #[test]
    fn requirements_txt_classifies_pins() {
        let content = "requests==2.31.0\nflask>=2.0\nnumpy\n# a comment\n-r other.txt\nblack~=23.1  # formatter\n";
        let entries = parse_requirements_txt(content);
        let requests = entries.iter().find(|e| e.name == "requests").unwrap();
        let flask = entries.iter().find(|e| e.name == "flask").unwrap();
        let numpy = entries.iter().find(|e| e.name == "numpy").unwrap();
        let black = entries.iter().find(|e| e.name == "black").unwrap();
        assert_eq!(requests.pin, PinKind::Exact);
        assert_eq!(flask.pin, PinKind::Range);
        assert_eq!(numpy.pin, PinKind::Unpinned);
        assert_eq!(black.pin, PinKind::Range);
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn requirements_txt_extras_and_markers_do_not_break_the_name() {
        let entries = parse_requirements_txt("uvicorn[standard]==0.23.0; python_version >= '3.8'\n");
        assert_eq!(entries[0].name, "uvicorn");
        assert_eq!(entries[0].pin, PinKind::Exact);
    }

    #[test]
    fn unknown_manifest_names_are_rejected() {
        assert!(inspect_dependencies("/tmp/does-not-matter/README.md").is_err());
    }
}
