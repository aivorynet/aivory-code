//! Syntax errors from the ERROR and MISSING nodes of a tree.

use tree_sitter::{Node, Tree};

use crate::{Range, SyntaxDiagnostic, SyntaxErrorKind};

/// The longest text that a message quotes, in bytes.
const QUOTE_MAX: usize = 24;
/// An ERROR node larger than this is reduced to its first token.
const WHOLE_NODE_MAX: usize = 40;

/// Collect the syntax errors of a tree, sorted by position. The walk skips every subtree
/// without an error, so a clean file costs almost nothing.
pub(crate) fn collect(tree: &Tree, source: &str) -> Vec<SyntaxDiagnostic> {
    let mut out = Vec::new();
    let root = tree.root_node();
    if !root.has_error() {
        return out;
    }
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let mut descend = false;
        if node.has_error() {
            if node.is_error() {
                out.push(unexpected(&node, source));
            } else if node.is_missing() {
                out.push(missing(&node));
            } else {
                descend = true;
            }
        }
        if descend && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                out.sort_by_key(|d| (d.range.start_byte, d.range.end_byte));
                return out;
            }
        }
    }
}

fn missing(node: &Node) -> SyntaxDiagnostic {
    let kind = node.kind();
    let message = if node.is_named() {
        format!("missing {}", kind.replace('_', " "))
    } else {
        format!("missing `{kind}`")
    };
    SyntaxDiagnostic {
        range: Range::from_node(node),
        kind: SyntaxErrorKind::Missing,
        message,
    }
}

fn unexpected(node: &Node, source: &str) -> SyntaxDiagnostic {
    let mut target = *node;
    let small = node.byte_range().len() <= WHOLE_NODE_MAX
        && node.start_position().row == node.end_position().row;
    if !small {
        // Point at the first token: the parser fails there.
        while let Some(child) = target.child(0) {
            target = child;
        }
    }
    let text = source.get(target.byte_range()).unwrap_or("").trim();
    let message = if text.is_empty() {
        "syntax error".to_owned()
    } else if small && text.contains(char::is_whitespace) {
        format!("unexpected input `{}`", quote(text))
    } else {
        format!("unexpected token `{}`", quote(text))
    };
    SyntaxDiagnostic {
        range: Range::from_node(&target),
        kind: SyntaxErrorKind::Unexpected,
        message,
    }
}

fn quote(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(QUOTE_MAX) {
        Some((i, _)) => format!("{}...", &line[..i]),
        None => line.to_owned(),
    }
}
