//! Semantic highlight with `tree-sitter-highlight`. The captures of the grammar queries map to
//! the fixed set of [`TokenKind`] values.

use std::ops::Range;
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{QueryCursor, Tree};
use tree_sitter_highlight::{HighlightEvent, Highlighter};

use crate::language::highlight_config;
use crate::{Error, HighlightSpan, Language, TokenKind};

use TokenKind::*;

/// The capture names that the highlight configuration recognizes, with their token kind.
/// A capture such as `keyword.control.return` matches the longest listed prefix.
const MAP: &[(&str, TokenKind)] = &[
    ("attribute", Attribute),
    ("attribute.builtin", Attribute),
    ("boolean", Constant),
    ("character", String),
    ("comment", Comment),
    ("comment.documentation", Comment),
    ("conditional", Keyword),
    ("constant", Constant),
    ("constant.builtin", Constant),
    ("constructor", Type),
    ("escape", String),
    ("exception", Keyword),
    ("function", Function),
    ("function.builtin", Function),
    ("function.call", Function),
    ("function.macro", Macro),
    ("function.method", Function),
    ("function.method.builtin", Function),
    ("function.special", Macro),
    ("include", Keyword),
    ("keyword", Keyword),
    ("label", Constant),
    ("macro", Macro),
    ("module", Type),
    ("namespace", Type),
    ("number", Number),
    ("number.float", Number),
    ("operator", Operator),
    ("preproc", Macro),
    ("property", Variable),
    ("property.builtin", Variable),
    ("punctuation", Punctuation),
    ("punctuation.bracket", Punctuation),
    ("punctuation.delimiter", Punctuation),
    ("punctuation.special", Punctuation),
    ("repeat", Keyword),
    ("string", String),
    ("string.escape", String),
    ("string.regexp", String),
    ("string.special", String),
    ("string.special.key", Variable),
    ("string.special.symbol", String),
    ("tag", Keyword),
    ("tag.attribute", Attribute),
    ("tag.delimiter", Punctuation),
    ("type", Type),
    ("type.builtin", Type),
    ("type.parameter", Type),
    ("variable", Variable),
    ("variable.builtin", Variable),
    ("variable.member", Variable),
    ("variable.parameter", Variable),
];

pub(crate) fn names() -> Vec<&'static str> {
    MAP.iter().map(|(n, _)| *n).collect()
}

/// Highlight a whole source text. The function parses the text itself.
///
/// The spans are sorted, they do not overlap, and they leave out text without a token kind
/// (white space and plain text).
pub fn highlight(language: Language, source: &str) -> Result<Vec<HighlightSpan>, Error> {
    if source.len() > u32::MAX as usize {
        return Err(Error::TooLarge);
    }
    let config = highlight_config(language)?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, source.as_bytes(), None, None, |_| None)
        .map_err(|_| Error::Parse)?;
    let mut stack: Vec<TokenKind> = Vec::new();
    let mut out: Vec<HighlightSpan> = Vec::new();
    for event in events {
        match event.map_err(|_| Error::Parse)? {
            HighlightEvent::HighlightStart(h) => stack.push(MAP[h.0].1),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(&kind) = stack.last() else { continue };
                if start >= end {
                    continue;
                }
                match out.last_mut() {
                    Some(prev) if prev.kind == kind && prev.end_byte as usize == start => {
                        prev.end_byte = end as u32;
                    }
                    _ => out.push(HighlightSpan {
                        start_byte: start as u32,
                        end_byte: end as u32,
                        kind,
                    }),
                }
            }
        }
    }
    Ok(out)
}

/// The token kind of each capture of the highlights query of a language (`None` for a capture
/// without a kind). The longest listed prefix of the capture name decides.
fn capture_kinds(language: Language) -> Result<&'static [Option<TokenKind>], Error> {
    static CELLS: [OnceLock<Vec<Option<TokenKind>>>; Language::ALL.len()] =
        [const { OnceLock::new() }; Language::ALL.len()];
    let config = highlight_config(language)?;
    Ok(CELLS[language as usize].get_or_init(|| {
        config
            .query
            .capture_names()
            .iter()
            .map(|name| {
                MAP.iter()
                    .filter(|(n, _)| {
                        name.strip_prefix(n)
                            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
                    })
                    .max_by_key(|(n, _)| n.len())
                    .map(|(_, kind)| *kind)
            })
            .collect()
    }))
}

/// Highlight only the bytes `range` of an already parsed tree. The cost depends on the size of
/// the range, not on the size of the file.
///
/// The spans follow the rules of [`highlight`], but they stop at the bounds of the range: a
/// token that crosses a bound is cut. When several captures cover one byte, the narrowest node
/// wins, and for equal nodes the last pattern of the query wins.
pub(crate) fn highlight_range(
    language: Language,
    tree: &Tree,
    source: &str,
    range: Range<usize>,
) -> Result<Vec<HighlightSpan>, Error> {
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    if start == end {
        return Ok(Vec::new());
    }
    let kinds = capture_kinds(language)?;
    let query = &highlight_config(language)?.query;
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(start..end);
    let mut captures = cursor.captures(query, tree.root_node(), source.as_bytes());
    // (start, end, pattern, kind) of every capture that has a kind.
    let mut found: Vec<(usize, usize, usize, TokenKind)> = Vec::new();
    while let Some((m, index)) = captures.next() {
        let capture = m.captures()[*index];
        if let Some(kind) = kinds[capture.index as usize] {
            let node = capture.node;
            found.push((node.start_byte(), node.end_byte(), m.pattern_index, kind));
        }
    }
    // Wide nodes first, so that a narrow node paints over them; the last pattern paints last.
    found.sort_by_key(|&(s, e, pattern, _)| (std::cmp::Reverse(e - s), pattern));
    let mut paint: Vec<Option<TokenKind>> = vec![None; end - start];
    for (s, e, _, kind) in found {
        let (s, e) = (s.max(start), e.min(end));
        if s < e {
            paint[s - start..e - start].fill(Some(kind));
        }
    }
    let mut out: Vec<HighlightSpan> = Vec::new();
    for (i, kind) in paint.iter().enumerate() {
        let Some(kind) = *kind else { continue };
        let at = (start + i) as u32;
        match out.last_mut() {
            Some(prev) if prev.kind == kind && prev.end_byte == at => prev.end_byte = at + 1,
            _ => out.push(HighlightSpan {
                start_byte: at,
                end_byte: at + 1,
                kind,
            }),
        }
    }
    Ok(out)
}

/// The byte range of the lines `first..end` (0-based, `end` excluded) of a text. A line
/// number past the last line gives the end of the text.
pub(crate) fn line_bytes(source: &str, first: usize, end: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let (mut start_byte, mut end_byte) = (None, None);
    let mut line = 0usize;
    let mut pos = 0usize;
    if first == 0 {
        start_byte = Some(0);
    }
    while pos < bytes.len() {
        let next = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| pos + i + 1);
        line += 1;
        pos = next;
        if line == first {
            start_byte = Some(pos);
        }
        if line == end {
            end_byte = Some(pos);
            break;
        }
    }
    let start = start_byte.unwrap_or(bytes.len());
    start..end_byte.unwrap_or(bytes.len()).max(start)
}

/// Move the bare leaf patterns (`(identifier) @variable`) to the front of a highlights query.
///
/// When several patterns capture the same node, `tree-sitter-highlight` takes the last one.
/// Some grammars list the bare patterns at the end (Go), so `(identifier) @variable` would
/// hide the function patterns. A bare pattern is the least specific one, so it goes first.
pub(crate) fn bare_patterns_first(query: &str) -> std::string::String {
    let (mut bare, mut rest) = (std::string::String::new(), std::string::String::new());
    let bytes = query.as_bytes();
    let (mut i, mut depth, mut start) = (0, 0usize, None::<usize>);
    let mut flush_from = 0;
    while i < bytes.len() {
        match bytes[i] {
            b';' if depth == 0 => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'(' | b'[' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b')' | b']' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        i += 1;
        if depth == 0 && start.is_some() {
            // The form ends here; the captures that follow belong to the same pattern.
            let mut end = i;
            loop {
                let rest_bytes = &bytes[end..];
                let ws = rest_bytes
                    .iter()
                    .take_while(|b| b.is_ascii_whitespace())
                    .count();
                if rest_bytes.get(ws) == Some(&b'@') {
                    let name = rest_bytes[ws..]
                        .iter()
                        .take_while(|b| !b.is_ascii_whitespace())
                        .count();
                    end += ws + name;
                } else {
                    break;
                }
            }
            let form = &query[start.unwrap()..end];
            let target = if is_bare(form) { &mut bare } else { &mut rest };
            target.push_str(&query[flush_from..end]);
            target.push('\n');
            flush_from = end;
            i = end;
            start = None;
        }
    }
    rest.push_str(&query[flush_from..]);
    bare.push_str(&rest);
    bare
}

/// True for `(node_name) @capture ...`: one node without fields, children, or predicates.
fn is_bare(form: &str) -> bool {
    let Some(inner) = form.strip_prefix('(') else {
        return false;
    };
    let Some((name, captures)) = inner.split_once(')') else {
        return false;
    };
    let name = name.trim();
    !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !captures.trim().is_empty()
        && captures.split_whitespace().all(|c| c.starts_with('@'))
}
