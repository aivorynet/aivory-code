//! The built-in quality rules that work on the tree: long function, deep nesting, high
//! cyclomatic complexity, and duplicated blocks.
//!
//! The rules read the kinds of the tree-sitter grammars. One list of kinds serves all languages,
//! because the grammars use the same names (`if_statement`, `for_statement`, ...).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use ast_grep_core::tree_sitter::StrDoc;

use crate::lang::SgLang;
use crate::sarif::{Level, SarifResult};
use crate::source::FileCtx;

pub const RULE_LONG_FUNCTION: &str = "aivory/long-function";
pub const RULE_DEEP_NESTING: &str = "aivory/deep-nesting";
pub const RULE_COMPLEXITY: &str = "aivory/high-complexity";
pub const RULE_DUPLICATE: &str = "aivory/duplicated-block";

pub(crate) type Doc = StrDoc<SgLang>;
pub(crate) type Root = ast_grep_core::AstGrep<Doc>;
pub(crate) type Node<'r> = ast_grep_core::Node<'r, Doc>;

/// The limits of the built-in rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// A function with more lines than this is too long.
    pub max_function_lines: u32,
    /// A function with a deeper nesting than this is too deep.
    pub max_nesting: u32,
    /// A function with a higher cyclomatic complexity than this is too complex.
    pub max_complexity: u32,
    /// A block with at least this many lines can be a duplicate.
    pub min_duplicate_lines: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_function_lines: 80,
            max_nesting: 4,
            max_complexity: 15,
            min_duplicate_lines: 12,
        }
    }
}

const FUNCTION_KINDS: &[&str] = &[
    "function_item",
    "function_definition",
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "function",
    "arrow_function",
    "method_definition",
    "method_declaration",
    "constructor_declaration",
    "local_function_statement",
];

const NESTING_KINDS: &[&str] = &[
    "if_expression",
    "if_statement",
    "for_expression",
    "for_statement",
    "for_in_statement",
    "for_each_statement",
    "foreach_statement",
    "enhanced_for_statement",
    "for_range_loop",
    "while_expression",
    "while_statement",
    "do_statement",
    "loop_expression",
    "match_expression",
    "match_statement",
    "switch_statement",
    "switch_expression",
    "expression_switch_statement",
    "type_switch_statement",
    "select_statement",
    "try_statement",
    "try_expression",
    "with_statement",
];

/// Every kind adds 1 to the cyclomatic complexity. The operators `&&`, `||`, `??`, `and`, `or`
/// are counted separately.
const DECISION_KINDS: &[&str] = &[
    "if_expression",
    "if_statement",
    "elif_clause",
    "for_expression",
    "for_statement",
    "for_in_statement",
    "for_each_statement",
    "foreach_statement",
    "enhanced_for_statement",
    "for_range_loop",
    "while_expression",
    "while_statement",
    "do_statement",
    "loop_expression",
    "match_arm",
    "case_clause",
    "switch_case",
    "switch_section",
    "switch_block_statement_group",
    "switch_rule",
    "case_statement",
    "expression_case",
    "type_case",
    "communication_case",
    "catch_clause",
    "except_clause",
    "conditional_expression",
    "ternary_expression",
    "for_in_clause",
    "if_clause",
    "boolean_operator",
];

fn is_function(kind: &str) -> bool {
    FUNCTION_KINDS.contains(&kind)
}

/// The name of a function node, or `<anonymous>`.
fn function_name(node: &Node<'_>) -> String {
    if let Some(n) = node.field("name") {
        return n.text().into_owned();
    }
    // A C declarator chain: `int *f(int a)` -> pointer_declarator -> function_declarator.
    let mut cur = node.field("declarator");
    while let Some(d) = cur {
        let kind = d.kind();
        if matches!(
            kind.as_ref(),
            "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name"
        ) {
            return d.text().into_owned();
        }
        cur = d.field("declarator");
    }
    // `const f = () => {}` and `{ f: function() {} }`.
    if let Some(p) = node.parent() {
        let pk = p.kind();
        if matches!(
            pk.as_ref(),
            "variable_declarator" | "pair" | "assignment_expression"
        ) && let Some(n) = p
            .field("name")
            .or_else(|| p.field("key"))
            .or_else(|| p.field("left"))
        {
            return n.text().into_owned();
        }
    }
    "<anonymous>".to_owned()
}

fn lines_of(node: &Node<'_>) -> u32 {
    (node.end_pos().line() - node.start_pos().line() + 1) as u32
}

/// True when `node` continues an `if` chain (`else if`): it does not nest deeper.
fn is_else_if(node: &Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let pk = parent.kind();
    if pk == "else_clause" {
        return true;
    }
    pk.starts_with("if_")
        && parent
            .field("alternative")
            .is_some_and(|alt| alt.node_id() == node.node_id())
}

/// All function nodes of a tree, outer functions first.
fn functions<'r>(root: &'r Root) -> Vec<Node<'r>> {
    root.root()
        .dfs()
        .filter(|n| n.is_named() && is_function(&n.kind()))
        .collect()
}

struct Metrics {
    max_depth: u32,
    deepest: Option<(usize, usize)>,
    complexity: u32,
}

/// Nesting and complexity of the body of one function. The walk skips nested functions.
fn metrics(function: &Node<'_>) -> Metrics {
    let mut m = Metrics {
        max_depth: 0,
        deepest: None,
        complexity: 1,
    };
    let mut stack: Vec<(Node<'_>, u32)> = Vec::new();
    for child in function.children().collect::<Vec<_>>().into_iter().rev() {
        stack.push((child, 0));
    }
    while let Some((node, depth)) = stack.pop() {
        let kind = node.kind();
        let kind = kind.as_ref();
        if is_function(kind) && node.is_named() {
            continue;
        }
        let mut child_depth = depth;
        if NESTING_KINDS.contains(&kind) && !is_else_if(&node) {
            child_depth = depth + 1;
            if child_depth > m.max_depth {
                m.max_depth = child_depth;
                m.deepest = Some((node.start_pos().line(), node.range().start));
            }
        }
        if DECISION_KINDS.contains(&kind) {
            m.complexity += 1;
        } else if matches!(kind, "binary_expression" | "binary_operator")
            && let Some(op) = node.field("operator")
            && matches!(op.text().as_ref(), "&&" | "||" | "??" | "and" | "or")
        {
            m.complexity += 1;
        }
        for child in node.children().collect::<Vec<_>>().into_iter().rev() {
            stack.push((child, child_depth));
        }
    }
    m
}

/// Run the per-file rules: long function, deep nesting, complexity.
pub fn scan_functions(
    ctx: &FileCtx,
    root: &Root,
    limits: &Thresholds,
    deadline: Instant,
    out: &mut Vec<SarifResult>,
) -> bool {
    for f in functions(root) {
        if Instant::now() >= deadline {
            return false;
        }
        let name = function_name(&f);
        let range = f.range();
        let lines = lines_of(&f);
        if lines > limits.max_function_lines {
            out.push(ctx.result_bytes(
                RULE_LONG_FUNCTION,
                Level::Warning,
                &format!(
                    "Function `{name}` has {lines} lines (limit {}).",
                    limits.max_function_lines
                ),
                range.start,
                f.field("name").map_or(range.start, |n| n.range().end),
            ));
        }
        let m = metrics(&f);
        if m.max_depth > limits.max_nesting
            && let Some((_, at)) = m.deepest
        {
            out.push(ctx.result_bytes(
                RULE_DEEP_NESTING,
                Level::Warning,
                &format!(
                    "Function `{name}` nests {} levels deep (limit {}).",
                    m.max_depth, limits.max_nesting
                ),
                at,
                at,
            ));
        }
        if m.complexity > limits.max_complexity {
            out.push(ctx.result_bytes(
                RULE_COMPLEXITY,
                Level::Warning,
                &format!(
                    "Function `{name}` has a cyclomatic complexity of {} (limit {}).",
                    m.complexity, limits.max_complexity
                ),
                range.start,
                f.field("name").map_or(range.start, |n| n.range().end),
            ));
        }
    }
    true
}

/// One parsed file for the duplicate scan.
pub struct DupInput<'a> {
    pub ctx: &'a FileCtx,
    pub root: &'a Root,
}

struct Cand {
    file: usize,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    lines: u32,
}

fn hash_of(parts: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts(&mut h);
    h.finish()
}

/// Find the blocks that occur at least twice (in one file or in several). Two blocks are equal
/// when their trees are equal, comments aside: the hash covers the kinds and the leaf text of the
/// subtree. Only the outermost block of a duplicate is reported.
pub fn scan_duplicates(
    inputs: &[DupInput<'_>],
    limits: &Thresholds,
    deadline: Instant,
    out: &mut Vec<SarifResult>,
) -> bool {
    // (hash) -> candidates; `skip[i]` marks nodes inside a duplicate.
    let mut groups: HashMap<u64, Vec<Cand>> = HashMap::new();
    let mut per_file_dup_ranges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); inputs.len()];
    let mut all: Vec<(u64, Cand)> = Vec::new();
    for (fi, input) in inputs.iter().enumerate() {
        if Instant::now() >= deadline {
            return false;
        }
        let nodes: Vec<Node<'_>> = input.root.root().dfs().collect();
        let mut index: HashMap<usize, usize> = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            index.insert(n.node_id(), i);
        }
        let parents: Vec<Option<usize>> = nodes
            .iter()
            .map(|n| n.parent().and_then(|p| index.get(&p.node_id()).copied()))
            .collect();
        // Children come after the parent in pre-order: walk backwards.
        let mut child_hashes: Vec<Vec<u64>> = vec![Vec::new(); nodes.len()];
        for i in (0..nodes.len()).rev() {
            let n = &nodes[i];
            let kind = n.kind();
            if kind.contains("comment") {
                continue;
            }
            let h = if n.is_leaf() {
                hash_of(|h| {
                    kind.hash(h);
                    n.text().hash(h);
                })
            } else {
                hash_of(|h| {
                    kind.hash(h);
                    child_hashes[i].hash(h);
                })
            };
            if let Some(p) = parents[i] {
                child_hashes[p].push(h);
            }
            // The children were pushed in reverse order: both blocks are reversed the same way.
            if i != 0 && n.is_named() && !n.is_leaf() {
                let lines = lines_of(n);
                if lines >= limits.min_duplicate_lines {
                    all.push((
                        h,
                        Cand {
                            file: fi,
                            start_byte: n.range().start,
                            end_byte: n.range().end,
                            start_line: n.start_pos().line(),
                            lines,
                        },
                    ));
                }
            }
        }
    }
    for (h, c) in all {
        groups.entry(h).or_default().push(c);
    }
    groups.retain(|_, v| v.len() >= 2);
    for v in groups.values() {
        for c in v {
            per_file_dup_ranges[c.file].push((c.start_byte, c.end_byte));
        }
    }
    // Keep the outermost blocks: drop a block that lies inside another duplicate block.
    let mut reports: Vec<Vec<&Cand>> = Vec::new();
    for v in groups.values() {
        let kept: Vec<&Cand> = v
            .iter()
            .filter(|c| {
                !per_file_dup_ranges[c.file].iter().any(|&(s, e)| {
                    s <= c.start_byte && c.end_byte <= e && (s, e) != (c.start_byte, c.end_byte)
                })
            })
            .collect();
        if kept.len() >= 2 {
            reports.push(kept);
        }
    }
    let mut results = Vec::new();
    for kept in reports {
        for c in &kept {
            let others: Vec<String> = kept
                .iter()
                .filter(|o| !std::ptr::eq(**o, *c))
                .take(3)
                .map(|o| format!("{}:{}", inputs[o.file].ctx.path, o.start_line + 1))
                .collect();
            results.push(inputs[c.file].ctx.result_bytes(
                RULE_DUPLICATE,
                Level::Note,
                &format!(
                    "Duplicated block of {} lines, also at {}.",
                    c.lines,
                    others.join(", ")
                ),
                c.start_byte,
                c.start_byte,
            ));
        }
    }
    results.sort_by(|a, b| (a.file(), a.start_line()).cmp(&(b.file(), b.start_line())));
    out.extend(results);
    true
}
