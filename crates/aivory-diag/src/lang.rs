//! The languages of the rule engine: an `aivory_syntax::Language` that ast-grep can parse.
//!
//! `ast-grep-core` and `aivory-syntax` use the same tree-sitter (0.27), so one tree-sitter links.
//! `aivory-syntax` does not expose its grammars, so this crate depends on the grammar crates
//! itself (same versions). The rule engine supports the languages of [`SgLang::ALL`].

use std::borrow::Cow;

use aivory_syntax::Language;
use ast_grep_core::language::Language as SgLanguage;
use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{Pattern, PatternError};
use serde::{Deserialize, Deserializer};

/// A language that the rule engine supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SgLang(Language);

impl SgLang {
    /// The supported languages.
    pub const ALL: [Language; 10] = [
        Language::Rust,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Python,
        Language::Go,
        Language::Java,
        Language::C,
        Language::Cpp,
        Language::CSharp,
    ];

    /// The rule language of a syntax language, or `None` when the rule engine lacks it.
    pub fn new(language: Language) -> Option<Self> {
        Self::ALL.contains(&language).then_some(Self(language))
    }

    pub fn language(self) -> Language {
        self.0
    }

    /// The meta variable character that the grammar accepts in an identifier.
    fn expando(self) -> char {
        match self.0 {
            Language::TypeScript | Language::Tsx | Language::JavaScript => '$',
            _ => '\u{b5}',
        }
    }

    /// Parse the name in the `language:` field of a rule.
    pub fn from_name(name: &str) -> Option<Self> {
        let language = match name.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "typescript" | "ts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "javascript" | "js" | "jsx" => Language::JavaScript,
            "python" | "py" => Language::Python,
            "go" | "golang" => Language::Go,
            "java" => Language::Java,
            "c" => Language::C,
            "cpp" | "c++" | "cc" => Language::Cpp,
            "csharp" | "c#" | "cs" => Language::CSharp,
            _ => return None,
        };
        Self::new(language)
    }
}

impl<'de> Deserialize<'de> for SgLang {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Self::from_name(&name).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "language `{name}` is not supported by the rule engine"
            ))
        })
    }
}

/// Replace `$NAME`, `$_`, and `$$$` by the expando character, as the other ast-grep languages do.
fn expand(expando: char, query: &str) -> Cow<'_, str> {
    if expando == '$' {
        return Cow::Borrowed(query);
    }
    let mut out = String::with_capacity(query.len());
    let mut dollars = 0usize;
    for c in query.chars() {
        if c == '$' {
            dollars += 1;
            continue;
        }
        let replace = matches!(c, 'A'..='Z' | '_') || dollars == 3;
        let sigil = if replace { expando } else { '$' };
        out.extend(std::iter::repeat_n(sigil, dollars));
        dollars = 0;
        out.push(c);
    }
    let sigil = if dollars == 3 { expando } else { '$' };
    out.extend(std::iter::repeat_n(sigil, dollars));
    Cow::Owned(out)
}

impl SgLanguage for SgLang {
    fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
        expand(self.expando(), query)
    }

    fn meta_var_char(&self) -> char {
        '$'
    }

    fn expando_char(&self) -> char {
        self.expando()
    }

    fn from_path<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        Self::new(Language::detect(path)?)
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        self.get_ts_language().id_for_node_kind(kind, true)
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        self.get_ts_language()
            .field_id_for_name(field)
            .map(|f| f.get())
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, *self))
    }
}

impl LanguageExt for SgLang {
    fn get_ts_language(&self) -> TSLanguage {
        match self.0 {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::Java => tree_sitter_java::LANGUAGE.into(),
            Language::C => tree_sitter_c::LANGUAGE.into(),
            Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            // `SgLang::new` never builds another language.
            _ => unreachable!("unsupported rule language"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_replaces_meta_variables_only() {
        assert_eq!(expand('\u{b5}', "$X.unwrap()"), "\u{b5}X.unwrap()");
        assert_eq!(expand('\u{b5}', "f($$$)"), "f(\u{b5}\u{b5}\u{b5})");
        assert_eq!(expand('\u{b5}', "a$b"), "a$b");
        assert_eq!(expand('$', "$X"), "$X");
    }

    #[test]
    fn names_map_to_languages() {
        assert_eq!(
            SgLang::from_name("Rust").unwrap().language(),
            Language::Rust
        );
        assert!(SgLang::from_name("cobol").is_none());
        assert!(SgLang::new(Language::Toml).is_none());
    }
}
