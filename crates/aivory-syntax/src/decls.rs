//! Declaration extraction. The queries in `queries/*.tags.scm` mark each declaration with
//! `@definition.<kind>` and its name with `@name`. The code here adds the signature, the doc
//! comment, the visibility, and the container.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor, Tree};

use crate::language::data;
use crate::{DeclKind, Declaration, Error, Language, Range, Visibility};

/// The longest signature, in bytes.
const SIGNATURE_MAX: usize = 200;

struct Raw<'t> {
    pattern: usize,
    def: Node<'t>,
    name: Node<'t>,
    kind: DeclKind,
}

fn kind_from_capture(name: &str) -> Option<DeclKind> {
    Some(match name.strip_prefix("definition.")? {
        "module" => DeclKind::Module,
        "namespace" => DeclKind::Namespace,
        "class" => DeclKind::Class,
        "struct" => DeclKind::Struct,
        "enum" => DeclKind::Enum,
        "variant" => DeclKind::Variant,
        "interface" => DeclKind::Interface,
        "trait" => DeclKind::Trait,
        "impl" => DeclKind::Impl,
        "function" => DeclKind::Function,
        "method" => DeclKind::Method,
        "constructor" => DeclKind::Constructor,
        "field" => DeclKind::Field,
        "property" => DeclKind::Property,
        "variable" => DeclKind::Variable,
        "constant" => DeclKind::Constant,
        "type" => DeclKind::TypeAlias,
        "macro" => DeclKind::Macro,
        "section" => DeclKind::Section,
        _ => return None,
    })
}

/// The deepest nesting of declarations that the stub keeps. Data files (JSON, YAML, ...) can
/// hold many keys, so the stub keeps only the upper levels.
fn max_depth(lang: Language) -> Option<usize> {
    match lang {
        Language::Json | Language::Yaml | Language::Toml | Language::Css => Some(2),
        Language::Html => Some(3),
        _ => None,
    }
}

pub(crate) fn extract(
    lang: Language,
    tree: &Tree,
    source: &str,
) -> Result<Vec<Declaration>, Error> {
    let data = data(lang)?;
    let query = &data.defs;
    let names = query.capture_names();
    let name_idx = names.iter().position(|n| *n == "name");
    let kinds: Vec<Option<DeclKind>> = names.iter().map(|n| kind_from_capture(n)).collect();

    // 1. Collect the matches. One name node yields one declaration: the first pattern wins.
    let mut by_name: HashMap<usize, Raw> = HashMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let mut def = None;
        let mut name = None;
        for c in m.captures() {
            if Some(c.index as usize) == name_idx {
                name = Some(c.node);
            } else if let Some(kind) = kinds[c.index as usize] {
                def = Some((c.node, kind));
            }
        }
        let (Some(name), Some((def, kind))) = (name, def) else {
            continue;
        };
        let raw = Raw {
            pattern: m.pattern_index,
            def,
            name,
            kind,
        };
        match by_name.entry(name.id()) {
            Entry::Occupied(mut e) => {
                if raw.pattern < e.get().pattern {
                    e.insert(raw);
                }
            }
            Entry::Vacant(e) => {
                e.insert(raw);
            }
        }
    }
    let mut raws: Vec<Raw> = by_name.into_values().collect();
    raws.sort_by_key(|r| {
        (
            r.def.start_byte(),
            std::cmp::Reverse(r.def.end_byte()),
            r.name.start_byte(),
        )
    });

    // 2. Parent by containment (the raws are sorted, so a stack finds the nearest container).
    let mut parents: Vec<Option<usize>> = Vec::with_capacity(raws.len());
    let mut stack: Vec<usize> = Vec::new();
    for (i, raw) in raws.iter().enumerate() {
        while let Some(&top) = stack.last() {
            let t = &raws[top].def;
            let contains = t.start_byte() <= raw.def.start_byte()
                && raw.def.end_byte() <= t.end_byte()
                && t.id() != raw.def.id();
            if contains {
                break;
            }
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(i);
    }

    // 3. Build the declarations.
    let mut decls: Vec<Declaration> = Vec::with_capacity(raws.len());
    for (i, raw) in raws.iter().enumerate() {
        let parent_raw = parents[i].map(|p| &raws[p]);
        let name_node = normalize_name(lang, raw);
        let name = text(source, &name_node)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut name = name;
        if name.len() > 80 {
            name = truncate(&name, 80);
        }
        let kind = refine_kind(lang, raw, parent_raw, &name);
        let (outer, wrapped_in_export) = outer_node(raw.def);
        decls.push(Declaration {
            signature: signature(&raw.def, source),
            doc: doc_range(lang, &raw.def, &outer, source),
            visibility: visibility(lang, kind, &raw.def, wrapped_in_export, &name, source),
            container: parent_raw.map(|p| {
                let n = normalize_name(lang, p);
                text(source, &n).to_owned()
            }),
            parent: parents[i].map(|p| p as u32),
            range: Range::from_node(&raw.def),
            name_range: Range::from_node(&name_node),
            kind,
            name,
        });
    }

    // 4. Go: a method belongs to its receiver type.
    if lang == Language::Go {
        for (i, raw) in raws.iter().enumerate() {
            if raw.def.kind() != "method_declaration" {
                continue;
            }
            if let Some(recv) = go_receiver(&raw.def, source) {
                decls[i].container = Some(recv.to_owned());
                decls[i].parent = decls
                    .iter()
                    .position(|d| {
                        d.name == recv
                            && matches!(
                                d.kind,
                                DeclKind::Struct
                                    | DeclKind::Interface
                                    | DeclKind::TypeAlias
                                    | DeclKind::Class
                            )
                    })
                    .map(|p| p as u32);
            }
        }
    }

    // 4b. C++: `void Shape::area() {}` belongs to `Shape`, and a method with the name of its
    // class is a constructor.
    if lang == Language::Cpp {
        for (i, raw) in raws.iter().enumerate() {
            if let Some(q) = raw
                .name
                .parent()
                .filter(|q| q.kind() == "qualified_identifier")
                && let Some(scope) = q.child_by_field_name("scope")
            {
                let scope = text(source, &scope).to_owned();
                let base = scope.rsplit("::").next().unwrap_or(&scope).to_owned();
                decls[i].parent = decls
                    .iter()
                    .position(|d| d.name == base && d.kind.is_type_like())
                    .map(|p| p as u32);
                decls[i].container = Some(base);
                if decls[i].kind == DeclKind::Function {
                    decls[i].kind = DeclKind::Method;
                }
            }
            let d = &mut decls[i];
            if d.kind == DeclKind::Method && d.container.as_deref() == Some(d.name.as_str()) {
                d.kind = DeclKind::Constructor;
            }
        }
    }

    // 5. Data files: keep the upper levels only.
    if let Some(limit) = max_depth(lang) {
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(p) = decls[i].parent {
                d += 1;
                i = p as usize;
            }
            d
        };
        let keep: Vec<bool> = (0..decls.len()).map(|i| depth(i) < limit).collect();
        let mut remap = vec![None; decls.len()];
        let mut next = 0u32;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                remap[i] = Some(next);
                next += 1;
            }
        }
        let mut kept = Vec::with_capacity(next as usize);
        for (i, mut d) in decls.into_iter().enumerate() {
            if keep[i] {
                d.parent = d.parent.and_then(|p| remap[p as usize]);
                kept.push(d);
            }
        }
        decls = kept;
    }
    Ok(decls)
}

fn text<'s>(source: &'s str, node: &Node) -> &'s str {
    source.get(node.byte_range()).unwrap_or("")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// The node whose text is the name. Rust `impl` blocks use the self type: `Foo<T>` gives `Foo`.
fn normalize_name<'t>(lang: Language, raw: &Raw<'t>) -> Node<'t> {
    let mut n = raw.name;
    if lang == Language::Rust && raw.kind == DeclKind::Impl {
        loop {
            let next = match n.kind() {
                "generic_type" => n.child_by_field_name("type"),
                "scoped_type_identifier" => n.child_by_field_name("name"),
                "reference_type" => n.child_by_field_name("type"),
                _ => None,
            };
            match next {
                Some(x) => n = x,
                None => break,
            }
        }
    }
    n
}

fn refine_kind(lang: Language, raw: &Raw, parent: Option<&Raw>, name: &str) -> DeclKind {
    let mut kind = raw.kind;
    let in_type = parent.is_some_and(|p| p.kind.is_type_like());
    if kind == DeclKind::Function && in_type {
        kind = DeclKind::Method;
    }
    if matches!(kind, DeclKind::Function | DeclKind::Method) {
        let ctor = match lang {
            Language::JavaScript | Language::TypeScript | Language::Tsx => "constructor",
            Language::Python => "__init__",
            Language::Ruby => "initialize",
            Language::Php => "__construct",
            _ => "",
        };
        if !ctor.is_empty() && name == ctor {
            kind = DeclKind::Constructor;
        }
    }
    if lang == Language::Python
        && kind == DeclKind::Variable
        && name.chars().any(|c| c.is_ascii_uppercase())
        && !name.chars().any(|c| c.is_ascii_lowercase())
    {
        kind = DeclKind::Constant;
    }
    kind
}

/// The declaration with the wrappers that carry the doc comment and the modifiers (`export`,
/// decorators, `template<...>`, the `const` statement of a variable). The flag tells whether
/// one of the wrappers is an `export`.
fn outer_node(def: Node) -> (Node, bool) {
    let mut node = def;
    let mut exported = false;
    while let Some(p) = node.parent() {
        match p.kind() {
            "export_statement" => exported = true,
            "decorated_definition"
            | "ambient_declaration"
            | "lexical_declaration"
            | "variable_declaration"
            | "template_declaration"
            | "type_declaration"
            | "const_declaration"
            | "var_declaration" => {}
            _ => break,
        }
        node = p;
    }
    (node, exported)
}

/// The header of a declaration: the text up to the body, on one line.
fn signature(def: &Node, source: &str) -> String {
    let start = def.start_byte();
    let body = body_start(def);
    let end = match body {
        Some(b) if b >= start => b,
        _ => {
            // No body: the first line only.
            let slice = source.get(start..def.end_byte()).unwrap_or("");
            start + slice.find('\n').unwrap_or(slice.len())
        }
    };
    let raw = source.get(start..end).unwrap_or("");
    let mut sig = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    while sig.ends_with('{') || sig.ends_with(' ') {
        sig.pop();
    }
    truncate(&sig, SIGNATURE_MAX)
}

fn body_start(def: &Node) -> Option<usize> {
    if let Some(b) = def.child_by_field_name("body") {
        return Some(b.start_byte());
    }
    let mut cursor = def.walk();
    for child in def.children(&mut cursor) {
        let k = child.kind();
        if k.ends_with("_body")
            || k.ends_with("_list") && k != "parameter_list" && k != "type_parameter_list"
            || matches!(
                k,
                "block"
                    | "compound_statement"
                    | "statement_block"
                    | "declaration_list"
                    | "class_body"
                    | "enum_body"
                    | "interface_body"
                    | "body_statement"
                    | "do_block"
                    | "block_mapping"
                    | "switch_body"
            )
        {
            return Some(child.start_byte());
        }
    }
    None
}

/// The last row with text of a node. A line comment ends after its newline, at column 0 of the
/// next row; that row does not count.
pub(crate) fn last_row(node: &Node) -> usize {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row - 1
    } else {
        end.row
    }
}

fn is_comment(node: &Node) -> bool {
    node.kind().contains("comment")
}

/// The doc comment: the adjacent comments before the declaration, or the Python docstring.
fn doc_range(lang: Language, def: &Node, outer: &Node, source: &str) -> Option<Range> {
    if lang == Language::Python {
        return python_docstring(def);
    }
    if matches!(
        lang,
        Language::Json | Language::Html | Language::Css | Language::Toml | Language::Yaml
    ) {
        return None;
    }
    let mut prev = outer.prev_sibling();
    let mut expected_row = outer.start_position().row;
    // Rust attributes sit between the doc comment and the item.
    while let Some(p) = prev {
        if matches!(p.kind(), "attribute_item" | "attribute_list" | "decorator") {
            expected_row = p.start_position().row;
            prev = p.prev_sibling();
        } else {
            break;
        }
    }
    let mut top: Option<Node> = None;
    let mut near: Option<Node> = None;
    while let Some(c) = prev {
        if !is_comment(&c) || last_row(&c) + 1 < expected_row {
            break;
        }
        // A trailing comment of the line before belongs to that line.
        let alone = c
            .prev_sibling()
            .is_none_or(|b| last_row(&b) < c.start_position().row);
        if !alone {
            break;
        }
        if lang == Language::Rust {
            let t = text(source, &c);
            let is_doc = (t.starts_with("///") && !t.starts_with("////")) || t.starts_with("/**");
            if !is_doc {
                break;
            }
        }
        near.get_or_insert(c);
        top = Some(c);
        expected_row = c.start_position().row;
        prev = c.prev_sibling();
    }
    let (top, near) = (top?, near?);
    // A line comment ends after its newline. The range stops before the newline.
    let mut a = Range::from_node(&top);
    let mut end = near.end_byte();
    while end > near.start_byte() && matches!(source.as_bytes()[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let line_start = source[..end].rfind('\n').map_or(0, |i| i + 1);
    a.end_byte = end as u32;
    a.end_line = last_row(&near) as u32;
    a.end_col = (end - line_start) as u32;
    Some(a)
}

fn python_docstring(def: &Node) -> Option<Range> {
    if !matches!(def.kind(), "function_definition" | "class_definition") {
        return None;
    }
    let body = def.child_by_field_name("body")?;
    let first = body.named_child(0)?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let s = first.named_child(0)?;
    (s.kind() == "string").then(|| Range::from_node(&s))
}

fn modifier_text(def: &Node, source: &str) -> String {
    let mut out = String::new();
    let mut cursor = def.walk();
    for child in def.children(&mut cursor) {
        if matches!(
            child.kind(),
            "modifiers"
                | "modifier"
                | "visibility_modifier"
                | "accessibility_modifier"
                | "storage_class_specifier"
        ) {
            out.push_str(text(source, &child));
            out.push(' ');
        }
    }
    out
}

fn has_word(modifiers: &str, word: &str) -> bool {
    modifiers
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| w == word)
}

fn visibility(
    lang: Language,
    kind: DeclKind,
    def: &Node,
    exported: bool,
    name: &str,
    source: &str,
) -> Visibility {
    use Visibility::*;
    let parent = def.parent();
    if matches!(
        kind,
        DeclKind::Module | DeclKind::Namespace | DeclKind::Impl
    ) && !matches!(lang, Language::Rust)
    {
        return Unknown;
    }
    let mods = modifier_text(def, source);
    let parent_kind = parent.map_or("", |p| p.kind());
    let grandparent_kind = parent.and_then(|p| p.parent()).map_or("", |g| g.kind());
    let access_word = |default: Visibility| {
        if has_word(&mods, "private") {
            Private
        } else if has_word(&mods, "protected") {
            Protected
        } else if has_word(&mods, "internal") {
            Internal
        } else if has_word(&mods, "public") {
            Public
        } else {
            default
        }
    };
    match lang {
        Language::Rust => {
            if kind == DeclKind::Impl {
                return Unknown;
            }
            let vis = {
                let mut cursor = def.walk();
                def.children(&mut cursor)
                    .find(|c| c.kind() == "visibility_modifier")
                    .map(|c| text(source, &c).to_owned())
            };
            match vis.as_deref() {
                Some("pub") => Public,
                Some(v) if v.starts_with("pub(self") => Private,
                Some(_) => Internal,
                None => {
                    // Variants, trait items, and trait impl items are as visible as their parent.
                    let trait_member = parent.is_some_and(|p| {
                        p.kind() == "declaration_list"
                            && p.parent().is_some_and(|g| {
                                g.kind() == "trait_item"
                                    || g.kind() == "impl_item"
                                        && g.child_by_field_name("trait").is_some()
                            })
                    });
                    if kind == DeclKind::Variant || trait_member {
                        Public
                    } else {
                        Private
                    }
                }
            }
        }
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            if name.starts_with('#') {
                return Private;
            }
            let mut cursor = def.walk();
            let acc = def
                .children(&mut cursor)
                .find(|c| c.kind() == "accessibility_modifier")
                .map(|c| text(source, &c).to_owned());
            match acc.as_deref() {
                Some("private") => Private,
                Some("protected") => Protected,
                Some(_) => Public,
                None => {
                    if exported
                        || kind == DeclKind::Variant
                        || matches!(
                            parent_kind,
                            "class_body" | "interface_body" | "object_type" | "enum_body"
                        )
                    {
                        Public
                    } else {
                        Internal
                    }
                }
            }
        }
        Language::Python => {
            if name.starts_with("__") && name.ends_with("__") || !name.starts_with('_') {
                Public
            } else {
                Private
            }
        }
        Language::Go => {
            if kind == DeclKind::Module {
                Unknown
            } else if name.chars().next().is_some_and(char::is_uppercase) {
                Public
            } else {
                Private
            }
        }
        Language::Java => {
            if kind == DeclKind::Variant || parent_kind == "interface_body" {
                Public
            } else {
                access_word(Internal)
            }
        }
        Language::CSharp => {
            if has_word(&mods, "private") && has_word(&mods, "protected") {
                return Private;
            }
            if kind == DeclKind::Variant
                || parent_kind == "declaration_list" && grandparent_kind == "interface_declaration"
            {
                return Public;
            }
            let top_level = kind.is_type_like()
                && (matches!(
                    parent_kind,
                    "compilation_unit" | "file_scoped_namespace_declaration"
                ) || parent_kind == "declaration_list"
                    && grandparent_kind == "namespace_declaration");
            access_word(if top_level { Internal } else { Private })
        }
        Language::Kotlin => access_word(Public),
        Language::Php => {
            let mut cursor = def.walk();
            let vis = def
                .children(&mut cursor)
                .find(|c| c.kind() == "visibility_modifier")
                .map(|c| text(source, &c).to_owned());
            match vis.as_deref() {
                Some("private") => Private,
                Some("protected") => Protected,
                _ => Public,
            }
        }
        Language::C | Language::Cpp => {
            if has_word(&mods, "static") {
                return Private;
            }
            if lang == Language::Cpp && parent_kind == "field_declaration_list" {
                let mut prev = def.prev_sibling();
                while let Some(p) = prev {
                    if p.kind() == "access_specifier" {
                        return match text(source, &p) {
                            "private" => Private,
                            "protected" => Protected,
                            _ => Public,
                        };
                    }
                    prev = p.prev_sibling();
                }
                let is_class = parent
                    .and_then(|p| p.parent())
                    .is_some_and(|g| g.kind() == "class_specifier");
                return if is_class { Private } else { Public };
            }
            Public
        }
        Language::Ruby
        | Language::Bash
        | Language::Json
        | Language::Toml
        | Language::Yaml
        | Language::Html
        | Language::Css => Unknown,
    }
}

/// The receiver type name of a Go method (`func (s *Server) Run()` gives `Server`).
fn go_receiver<'s>(def: &Node, source: &'s str) -> Option<&'s str> {
    let recv = def.child_by_field_name("receiver")?;
    let mut stack = vec![recv];
    while let Some(n) = stack.pop() {
        if n.kind() == "type_identifier" {
            return source.get(n.byte_range());
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}
