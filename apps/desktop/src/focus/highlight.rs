//! A small original lexer for the Focus editor: Rust, TOML, Java, Kotlin, JavaScript and
//! TypeScript, Markdown. It colors one file in one pass and keeps the state that crosses lines
//! (a block comment, a text block, a Markdown fence). It is not a parser; a wrong color is
//! cheaper than a wrong layout, so a line never fails.

/// The language of a file, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Toml,
    Java,
    Kotlin,
    Script,
    Markdown,
    Plain,
}

impl Lang {
    /// The name in the status bar.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Toml => "TOML",
            Self::Java => "Java",
            Self::Kotlin => "Kotlin",
            Self::Script => "TypeScript",
            Self::Markdown => "Markdown",
            Self::Plain => "Plain text",
        }
    }
}

/// The language of a path.
pub fn lang_of(path: &str) -> Lang {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Lang::Rust,
        "toml" => Lang::Toml,
        "java" => Lang::Java,
        "kt" | "kts" => Lang::Kotlin,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => Lang::Script,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::Plain,
    }
}

/// The kind of a colored span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Type,
    Function,
    Str,
    Number,
    Comment,
    Macro,
    Attr,
    Heading,
}

/// The color of a token kind (the artboard classes `.kw .fn .st .co .nu .ty .mc .at .sec`).
pub fn color(tok: Tok) -> u32 {
    match tok {
        Tok::Keyword => 0xCF222E,
        Tok::Function => 0x8250DF,
        Tok::Str => 0x0A3069,
        Tok::Comment => 0x6E7781,
        Tok::Number => 0x0550AE,
        Tok::Type => 0x953800,
        Tok::Macro => 0x8250DF,
        Tok::Attr => 0x116329,
        Tok::Heading => 0x953800,
    }
}

/// A colored byte range of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub tok: Tok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Carry {
    #[default]
    None,
    /// Inside a `/* */` comment.
    Block,
    /// Inside a `"""` text block.
    Text,
    /// Inside a Markdown code fence.
    Fence,
}

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];
const JAVA: &[&str] = &[
    "abstract",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "throws",
    "true",
    "try",
    "var",
    "void",
    "while",
    "record",
];
const KOTLIN: &[&str] = &[
    "as",
    "break",
    "by",
    "class",
    "companion",
    "const",
    "continue",
    "data",
    "do",
    "else",
    "enum",
    "false",
    "for",
    "fun",
    "if",
    "import",
    "in",
    "interface",
    "is",
    "lateinit",
    "null",
    "object",
    "override",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "sealed",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "val",
    "var",
    "when",
    "while",
    "suspend",
    "open",
];
const SCRIPT: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "private",
    "public",
    "readonly",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];
const TOML: &[&str] = &["true", "false"];

fn keywords(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => RUST,
        Lang::Java => JAVA,
        Lang::Kotlin => KOTLIN,
        Lang::Script => SCRIPT,
        Lang::Toml => TOML,
        _ => &[],
    }
}

/// Color a whole text. One entry per line, in line order.
pub fn lex(lang: Lang, text: &str) -> Vec<Vec<Span>> {
    let mut carry = Carry::None;
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            lex_line(lang, line, &mut carry)
        })
        .collect()
}

fn lex_line(lang: Lang, line: &str, carry: &mut Carry) -> Vec<Span> {
    match lang {
        Lang::Plain => Vec::new(),
        Lang::Markdown => markdown_line(line, carry),
        Lang::Toml => toml_line(line),
        _ => code_line(lang, line, carry),
    }
}

fn span(start: usize, end: usize, tok: Tok) -> Span {
    Span { start, end, tok }
}

fn markdown_line(line: &str, carry: &mut Carry) -> Vec<Span> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        *carry = if *carry == Carry::Fence {
            Carry::None
        } else {
            Carry::Fence
        };
        return vec![span(0, line.len(), Tok::Comment)];
    }
    if *carry == Carry::Fence {
        return if line.is_empty() {
            Vec::new()
        } else {
            vec![span(0, line.len(), Tok::Str)]
        };
    }
    if trimmed.starts_with('#') {
        return vec![span(0, line.len(), Tok::Heading)];
    }
    if trimmed.starts_with('>') {
        return vec![span(0, line.len(), Tok::Comment)];
    }
    let mut out = Vec::new();
    let pad = line.len() - trimmed.len();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
        out.push(span(pad, pad + 1, Tok::Keyword));
    } else {
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            out.push(span(pad, pad + digits + 1, Tok::Keyword));
        }
    }
    // `inline code`.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`'
            && let Some(close) = line[i + 1..].find('`')
        {
            out.push(span(i, i + close + 2, Tok::Str));
            i += close + 2;
        } else {
            i += 1;
        }
    }
    out.sort_by_key(|s| s.start);
    out
}

fn toml_line(line: &str) -> Vec<Span> {
    let trimmed = line.trim_start();
    let pad = line.len() - trimmed.len();
    if trimmed.starts_with('#') {
        return vec![span(pad, line.len(), Tok::Comment)];
    }
    if trimmed.starts_with('[') {
        return vec![span(pad, line.len(), Tok::Heading)];
    }
    let mut out = Vec::new();
    let mut rest_from = 0;
    if let Some(eq) = line.find('=') {
        let key = line[..eq].trim_end();
        if !key.trim().is_empty() && !key.contains('"') {
            out.push(span(pad, key.len(), Tok::Attr));
            rest_from = eq + 1;
        }
    }
    scan(Lang::Toml, line, rest_from, '#', &mut out);
    out
}

/// Scan strings, numbers, words and a line comment from `from` on.
fn scan(lang: Lang, line: &str, from: usize, comment: char, out: &mut Vec<Span>) {
    let kws = keywords(lang);
    let chars: Vec<(usize, char)> = line.char_indices().filter(|(i, _)| *i >= from).collect();
    let mut k = 0;
    while k < chars.len() {
        let (i, c) = chars[k];
        if c == comment {
            out.push(span(i, line.len(), Tok::Comment));
            return;
        }
        if c == '"' {
            let mut j = k + 1;
            while j < chars.len() && chars[j].1 != '"' {
                j += if chars[j].1 == '\\' { 2 } else { 1 };
            }
            let end = chars.get(j).map_or(line.len(), |(e, _)| e + 1);
            out.push(span(i, end, Tok::Str));
            k = j + 1;
        } else if c.is_ascii_digit() {
            let mut j = k;
            while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '_') {
                j += 1;
            }
            let end = chars.get(j).map_or(line.len(), |(e, _)| *e);
            out.push(span(i, end, Tok::Number));
            k = j;
        } else if c.is_alphabetic() || c == '_' {
            let mut j = k;
            while j < chars.len() && (chars[j].1.is_alphanumeric() || chars[j].1 == '_') {
                j += 1;
            }
            let end = chars.get(j).map_or(line.len(), |(e, _)| *e);
            let word = &line[i..end];
            if kws.contains(&word) {
                out.push(span(i, end, Tok::Keyword));
            }
            k = j;
        } else {
            k += 1;
        }
    }
}

fn code_line(lang: Lang, line: &str, carry: &mut Carry) -> Vec<Span> {
    let mut out = Vec::new();
    let kws = keywords(lang);
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let at = |k: usize| chars.get(k).map_or('\0', |(_, c)| *c);
    let end_of = |k: usize| chars.get(k).map_or(line.len(), |(i, _)| *i);
    let mut k = 0;
    // The line starts inside a comment or a text block.
    match *carry {
        Carry::Block => {
            let close = line.find("*/").map(|p| p + 2);
            out.push(span(0, close.unwrap_or(line.len()), Tok::Comment));
            match close {
                Some(p) => {
                    *carry = Carry::None;
                    k = chars
                        .iter()
                        .position(|(i, _)| *i >= p)
                        .unwrap_or(chars.len());
                }
                None => return out,
            }
        }
        Carry::Text => {
            let close = line.find("\"\"\"").map(|p| p + 3);
            out.push(span(0, close.unwrap_or(line.len()), Tok::Str));
            match close {
                Some(p) => {
                    *carry = Carry::None;
                    k = chars
                        .iter()
                        .position(|(i, _)| *i >= p)
                        .unwrap_or(chars.len());
                }
                None => return out,
            }
        }
        _ => {}
    }
    while k < chars.len() {
        let (i, c) = chars[k];
        let next = at(k + 1);
        if c == '/' && next == '/' {
            out.push(span(i, line.len(), Tok::Comment));
            return out;
        }
        if c == '/' && next == '*' {
            match line[i + 2..].find("*/") {
                Some(p) => {
                    let end = i + 2 + p + 2;
                    out.push(span(i, end, Tok::Comment));
                    k = chars
                        .iter()
                        .position(|(e, _)| *e >= end)
                        .unwrap_or(chars.len());
                    continue;
                }
                None => {
                    out.push(span(i, line.len(), Tok::Comment));
                    *carry = Carry::Block;
                    return out;
                }
            }
        }
        if c == '"' && next == '"' && at(k + 2) == '"' && matches!(lang, Lang::Java | Lang::Kotlin)
        {
            match line[i + 3..].find("\"\"\"") {
                Some(p) => {
                    let end = i + 3 + p + 3;
                    out.push(span(i, end, Tok::Str));
                    k = chars
                        .iter()
                        .position(|(e, _)| *e >= end)
                        .unwrap_or(chars.len());
                    continue;
                }
                None => {
                    out.push(span(i, line.len(), Tok::Str));
                    *carry = Carry::Text;
                    return out;
                }
            }
        }
        if c == '"' || (c == '`' && lang == Lang::Script) || (c == '\'' && lang != Lang::Rust) {
            let mut j = k + 1;
            while j < chars.len() && chars[j].1 != c {
                j += if chars[j].1 == '\\' { 2 } else { 1 };
            }
            out.push(span(i, end_of(j + 1), Tok::Str));
            k = j + 1;
            continue;
        }
        if c == '\'' {
            // A Rust char (`'a'`, `'\n'`) or a lifetime (`'a`).
            if at(k + 2) == '\'' || (next == '\\' && at(k + 3) == '\'') {
                let j = if next == '\\' { k + 3 } else { k + 2 };
                out.push(span(i, end_of(j + 1), Tok::Str));
                k = j + 1;
            } else {
                let mut j = k + 1;
                while at(j).is_alphanumeric() || at(j) == '_' {
                    j += 1;
                }
                out.push(span(i, end_of(j), Tok::Type));
                k = j.max(k + 1);
            }
            continue;
        }
        if (c == '#' && lang == Lang::Rust && (next == '[' || next == '!'))
            || (c == '@' && matches!(lang, Lang::Java | Lang::Kotlin | Lang::Script))
        {
            let mut j = k + 1;
            if lang == Lang::Rust {
                while j < chars.len() && at(j) != ']' {
                    j += 1;
                }
                j += 1;
            } else {
                while at(j).is_alphanumeric() || at(j) == '_' || at(j) == '.' {
                    j += 1;
                }
            }
            out.push(span(i, end_of(j), Tok::Attr));
            k = j.max(k + 1);
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = k;
            while at(j).is_ascii_alphanumeric() || at(j) == '_' || at(j) == '.' {
                // `1..2` is a range, not a float.
                if at(j) == '.' && at(j + 1) == '.' {
                    break;
                }
                j += 1;
            }
            out.push(span(i, end_of(j), Tok::Number));
            k = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let mut j = k;
            while at(j).is_alphanumeric() || at(j) == '_' || at(j) == '$' {
                j += 1;
            }
            let end = end_of(j);
            let word = &line[i..end];
            let tok = if kws.contains(&word) {
                Some(Tok::Keyword)
            } else if lang == Lang::Rust && at(j) == '!' && at(j + 1) != '=' {
                Some(Tok::Macro)
            } else if at(j) == '(' {
                Some(Tok::Function)
            } else if word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().any(char::is_lowercase)
            {
                Some(Tok::Type)
            } else {
                None
            };
            if let Some(tok) = tok {
                out.push(span(i, end, tok));
            }
            k = j;
            continue;
        }
        k += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(lang: Lang, line: &str) -> Vec<(&str, Tok)> {
        lex(lang, line)[0]
            .iter()
            .map(|s| (&line[s.start..s.end], s.tok))
            .collect()
    }

    #[test]
    fn the_extension_picks_the_language() {
        assert_eq!(lang_of("a/b/lib.rs"), Lang::Rust);
        assert_eq!(lang_of("Cargo.toml"), Lang::Toml);
        assert_eq!(lang_of("A.java"), Lang::Java);
        assert_eq!(lang_of("A.kt"), Lang::Kotlin);
        assert_eq!(lang_of("x.tsx"), Lang::Script);
        assert_eq!(lang_of("README.md"), Lang::Markdown);
        assert_eq!(lang_of("Makefile"), Lang::Plain);
    }

    #[test]
    fn rust_has_keywords_types_macros_and_comments() {
        let t = toks(
            Lang::Rust,
            "pub fn run(x: Budget) { println!(\"hi\"); } // end",
        );
        assert!(t.contains(&("pub", Tok::Keyword)));
        assert!(t.contains(&("run", Tok::Function)));
        assert!(t.contains(&("Budget", Tok::Type)));
        assert!(t.contains(&("println", Tok::Macro)));
        assert!(t.contains(&("\"hi\"", Tok::Str)));
        assert!(t.contains(&("// end", Tok::Comment)));
    }

    #[test]
    fn a_rust_lifetime_is_no_char_literal() {
        let t = toks(Lang::Rust, "fn f<'a>(x: &'a str) -> char { 'x' }");
        assert!(t.contains(&("'a", Tok::Type)));
        assert!(t.contains(&("'x'", Tok::Str)));
    }

    #[test]
    fn a_block_comment_crosses_lines() {
        let l = lex(Lang::Java, "int a; /* one\ntwo\nthree */ int b;");
        assert_eq!(l[1], vec![span(0, 3, Tok::Comment)]);
        assert_eq!(l[2][0], span(0, 8, Tok::Comment));
        assert!(l[2].iter().any(|s| s.tok == Tok::Keyword));
    }

    #[test]
    fn toml_has_sections_keys_and_values() {
        let l = lex(
            Lang::Toml,
            "[theme]\nname = \"operator\" # c\nsize = 12\nok = true",
        );
        assert_eq!(l[0][0].tok, Tok::Heading);
        assert_eq!(l[1][0].tok, Tok::Attr);
        assert!(l[1].iter().any(|s| s.tok == Tok::Str));
        assert!(l[1].iter().any(|s| s.tok == Tok::Comment));
        assert!(l[2].iter().any(|s| s.tok == Tok::Number));
        assert!(l[3].iter().any(|s| s.tok == Tok::Keyword));
    }

    #[test]
    fn markdown_has_headings_fences_and_code() {
        let l = lex(
            Lang::Markdown,
            "# Title\n- item `x`\n```rs\nlet a;\n```\ntext",
        );
        assert_eq!(l[0][0].tok, Tok::Heading);
        assert_eq!(l[1][0].tok, Tok::Keyword);
        assert!(l[1].iter().any(|s| s.tok == Tok::Str));
        assert_eq!(l[3][0].tok, Tok::Str);
        assert!(l[5].is_empty());
    }

    #[test]
    fn kotlin_and_script_have_annotations_and_strings() {
        let k = toks(Lang::Kotlin, "@Test fun go() = \"a\"");
        assert!(k.contains(&("@Test", Tok::Attr)));
        assert!(k.contains(&("fun", Tok::Keyword)));
        let s = toks(Lang::Script, "const a = `x` + 'y' + 1_0;");
        assert!(s.contains(&("`x`", Tok::Str)));
        assert!(s.contains(&("'y'", Tok::Str)));
    }

    #[test]
    fn non_ascii_text_never_panics() {
        for lang in [
            Lang::Rust,
            Lang::Toml,
            Lang::Markdown,
            Lang::Kotlin,
            Lang::Script,
        ] {
            let _ = lex(
                lang,
                "let ä = \"ö\"; // ü\n/* é\n   ü */ 'é' \"unterminated",
            );
        }
    }
}
