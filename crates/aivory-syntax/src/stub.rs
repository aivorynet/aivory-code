//! The L1 stub: a compact summary of the visible declarations of one file.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, QueryCursor, Tree};

use crate::decls::last_row;
use crate::language::data;
use crate::{
    Declaration, Error, FoldKind, FoldingRange, Import, Language, OutlineItem, Range, Reference,
    decls,
};

/// The version of the extractor: the queries and the code that build a stub. Raise it when a
/// change alters the stub of any file. The stub key contains it.
pub const EXTRACTOR_VERSION: u32 = 1;

/// The blake3 hash of a source text as 64 hex characters.
pub(crate) fn hash_source(source: &str) -> String {
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

/// A compact summary of one file. The index stores it under the key
/// ([`Stub::hash`], [`Stub::grammar_version`], [`Stub::extractor_version`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stub {
    /// The blake3 hash of the source, 64 hex characters.
    pub hash: String,
    pub language: Language,
    /// The grammar crates and their versions, for example `tree-sitter-rust 0.24.2`.
    pub grammar_version: String,
    pub extractor_version: u32,
    /// The declarations, sorted by position. A container comes before its members.
    pub declarations: Vec<Declaration>,
    pub imports: Vec<Import>,
    /// The identifiers that are not declaration names and not part of an import.
    pub references: Vec<Reference>,
    /// The structure view: the declarations as a tree.
    pub outline: Vec<OutlineItem>,
    pub folding: Vec<FoldingRange>,
}

impl Stub {
    /// Parse `source` and build its stub.
    pub fn parse(language: Language, source: &str) -> Result<Stub, Error> {
        if source.len() > u32::MAX as usize {
            return Err(Error::TooLarge);
        }
        let mut parser = Parser::new();
        parser
            .set_language(&data(language)?.ts)
            .map_err(|e| Error::Grammar(language, e.to_string()))?;
        let tree = parser.parse(source, None).ok_or(Error::Parse)?;
        Self::from_tree(language, &tree, source)
    }

    pub(crate) fn from_tree(language: Language, tree: &Tree, source: &str) -> Result<Stub, Error> {
        let declarations = decls::extract(language, tree, source)?;
        let imports = extract_imports(language, tree, source)?;
        let references = extract_references(language, tree, source, &declarations, &imports);
        let outline = build_outline(&declarations);
        let folding = extract_folding(language, tree, source, &declarations, &imports);
        Ok(Stub {
            hash: hash_source(source),
            language,
            grammar_version: language.grammar_version(),
            extractor_version: EXTRACTOR_VERSION,
            declarations,
            imports,
            references,
            outline,
            folding,
        })
    }

    /// The cache key: the same key means the same stub.
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.hash, self.grammar_version, self.extractor_version
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Imports

fn extract_imports(lang: Language, tree: &Tree, source: &str) -> Result<Vec<Import>, Error> {
    let Some(query) = data(lang)?.imports.as_ref() else {
        return Ok(Vec::new());
    };
    let names = query.capture_names();
    let idx = |n: &str| names.iter().position(|x| *x == n);
    let (import_idx, path_idx, alias_idx) = (idx("import"), idx("path"), idx("alias"));
    let mut by_node: BTreeMap<(usize, usize), Import> = BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let mut import = None;
        let mut path = None;
        let mut alias = None;
        for c in m.captures() {
            let i = Some(c.index as usize);
            if i == import_idx {
                import = Some(c.node);
            } else if i == path_idx {
                path.get_or_insert(c.node);
            } else if i == alias_idx {
                alias.get_or_insert(c.node);
            }
        }
        let (Some(import), Some(path)) = (import, path) else {
            continue;
        };
        let entry = by_node
            .entry((import.start_byte(), import.end_byte()))
            .or_insert_with(|| Import {
                path: clean_path(source.get(path.byte_range()).unwrap_or("")),
                alias: None,
                range: Range::from_node(&import),
            });
        if entry.alias.is_none() {
            entry.alias = alias
                .and_then(|a| source.get(a.byte_range()))
                .map(str::to_owned);
        }
    }
    Ok(by_node
        .into_values()
        .filter(|i| !i.path.is_empty())
        .collect())
}

/// Remove quotes, angle brackets, and white space from an import path.
fn clean_path(raw: &str) -> String {
    let t = raw.trim();
    let t = t.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>'));
    t.split_whitespace().collect()
}

// ---------------------------------------------------------------------------------------------
// References

fn is_reference_kind(lang: Language, kind: &str) -> bool {
    if kind.ends_with("identifier") {
        return true;
    }
    match lang {
        Language::Ruby => kind == "constant",
        Language::Php => kind == "name",
        Language::Bash => kind == "variable_name",
        Language::Css => matches!(kind, "class_name" | "id_name"),
        _ => false,
    }
}

fn extract_references(
    lang: Language,
    tree: &Tree,
    source: &str,
    decls: &[Declaration],
    imports: &[Import],
) -> Vec<Reference> {
    let declared: HashSet<(u32, u32)> = decls
        .iter()
        .map(|d| (d.name_range.start_byte, d.name_range.end_byte))
        .collect();
    let imported: HashSet<(u32, u32)> = imports
        .iter()
        .map(|i| (i.range.start_byte, i.range.end_byte))
        .collect();
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let key = (node.start_byte() as u32, node.end_byte() as u32);
        let skip = imported.contains(&key);
        if !skip {
            if node.child_count() == 0 {
                if node.is_named()
                    && is_reference_kind(lang, node.kind())
                    && !declared.contains(&key)
                    && let Some(name) = source.get(node.byte_range())
                {
                    out.push(Reference {
                        name: name.to_owned(),
                        range: Range::from_node(&node),
                    });
                }
            } else if cursor.goto_first_child() {
                continue;
            }
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Outline

fn build_outline(decls: &[Declaration]) -> Vec<OutlineItem> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); decls.len()];
    let mut roots = Vec::new();
    for (i, d) in decls.iter().enumerate() {
        match d.parent {
            Some(p) => children[p as usize].push(i),
            None => roots.push(i),
        }
    }
    fn item(i: usize, decls: &[Declaration], children: &[Vec<usize>]) -> OutlineItem {
        let d = &decls[i];
        OutlineItem {
            name: d.name.clone(),
            kind: d.kind,
            range: d.range,
            name_range: d.name_range,
            children: children[i]
                .iter()
                .map(|&c| item(c, decls, children))
                .collect(),
        }
    }
    roots.iter().map(|&r| item(r, decls, &children)).collect()
}

// ---------------------------------------------------------------------------------------------
// Folding

fn is_fold_kind(kind: &str) -> bool {
    // The list starts after the opening line, so a fold of it would start one line late.
    if kind == "statement_list" {
        return false;
    }
    kind.ends_with("_body")
        || kind.ends_with("block")
        || kind.ends_with("_list")
        || matches!(
            kind,
            "object"
                | "array"
                | "arguments"
                | "parameters"
                | "compound_statement"
                | "declaration_list"
                | "table"
                | "table_array_element"
                | "inline_table"
                | "dictionary"
                | "list"
                | "set"
                | "tuple"
                | "hash"
                | "element"
                | "script_element"
                | "style_element"
                | "block_mapping_pair"
                | "block_sequence"
                | "rule_set"
                | "at_rule"
                | "initializer_list"
                | "object_type"
                | "named_imports"
        )
}

fn extract_folding(
    lang: Language,
    tree: &Tree,
    source: &str,
    decls: &[Declaration],
    imports: &[Import],
) -> Vec<FoldingRange> {
    let lines: Vec<&str> = source.lines().collect();
    // A node can end at column 0 of the next line (after a newline token), and a table or a
    // block can end with blank lines. A fold stops at the last line with text.
    let end_line = |start: usize, mut end: usize| {
        while end > start && lines.get(end).is_none_or(|l| l.trim().is_empty()) {
            end -= 1;
        }
        end
    };
    let mut out: Vec<FoldingRange> = Vec::new();
    let mut line_comments: Vec<u32> = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        let (sr, er) = (node.start_position().row, last_row(&node));
        let mut descend = false;
        if node.kind().contains("comment") {
            if er > sr {
                out.push(fold(sr, er, FoldKind::Comment));
            } else {
                line_comments.push(sr as u32);
            }
        } else if er > sr {
            if is_fold_kind(node.kind()) {
                let mut start = sr;
                // Indentation languages: the fold starts at the header line of the block.
                if node.kind() == "block"
                    && matches!(lang, Language::Python)
                    && let Some(p) = node.parent()
                {
                    start = p.start_position().row;
                }
                let er = end_line(start, er);
                if er > start {
                    out.push(fold(start, er, FoldKind::Block));
                }
            }
            descend = true;
        }
        if descend && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    for d in decls {
        let start = d.range.start_line as usize;
        let mut end = d.range.end_line as usize;
        if d.range.end_col == 0 && end > start {
            end -= 1;
        }
        let end = end_line(start, end);
        if end > start {
            out.push(fold(start, end, FoldKind::Block));
        }
    }
    // Consecutive line comments fold as one region.
    let mut i = 0;
    while i < line_comments.len() {
        let mut j = i;
        while j + 1 < line_comments.len() && line_comments[j + 1] == line_comments[j] + 1 {
            j += 1;
        }
        if j > i {
            out.push(fold(
                line_comments[i] as usize,
                line_comments[j] as usize,
                FoldKind::Comment,
            ));
        }
        i = j + 1;
    }
    // Consecutive imports (one blank line allowed between them) fold as one region.
    let mut i = 0;
    while i < imports.len() {
        let mut j = i;
        while j + 1 < imports.len()
            && imports[j + 1].range.start_line <= imports[j].range.end_line + 2
        {
            j += 1;
        }
        if imports[j].range.end_line > imports[i].range.start_line {
            out.push(fold(
                imports[i].range.start_line as usize,
                imports[j].range.end_line as usize,
                FoldKind::Imports,
            ));
        }
        i = j + 1;
    }
    out.sort_by_key(|f| (f.start_line, f.end_line, f.kind as u8));
    out.dedup_by_key(|f| (f.start_line, f.end_line));
    out
}

fn fold(start: usize, end: usize, kind: FoldKind) -> FoldingRange {
    FoldingRange {
        start_line: start as u32,
        end_line: end as u32,
        kind,
    }
}
