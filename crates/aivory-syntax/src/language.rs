//! The supported languages: detection, grammars, and the cached queries.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tree_sitter::{Language as TsLanguage, Query};
use tree_sitter_highlight::HighlightConfiguration;

use crate::Error;
use crate::highlight::{bare_patterns_first, names};

/// A language that the syntax layer can parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Json,
    Bash,
    Kotlin,
    Ruby,
    Php,
    Toml,
    Yaml,
    Html,
    Css,
}

impl Language {
    /// All languages.
    pub const ALL: [Language; 19] = [
        Self::Rust,
        Self::TypeScript,
        Self::Tsx,
        Self::JavaScript,
        Self::Python,
        Self::Go,
        Self::Java,
        Self::C,
        Self::Cpp,
        Self::CSharp,
        Self::Json,
        Self::Bash,
        Self::Kotlin,
        Self::Ruby,
        Self::Php,
        Self::Toml,
        Self::Yaml,
        Self::Html,
        Self::Css,
    ];

    /// The language of a file, from its file name first and from its extension second.
    /// The extension `.h` maps to C.
    pub fn detect(path: impl AsRef<Path>) -> Option<Language> {
        let path = path.as_ref();
        let name = path.file_name()?.to_str()?;
        if let Some(lang) = Self::from_file_name(name) {
            return Some(lang);
        }
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// The language of a well-known file name such as `Rakefile` or `.bashrc`.
    pub fn from_file_name(name: &str) -> Option<Language> {
        Some(match name {
            ".bashrc" | ".bash_profile" | ".bash_login" | ".bash_logout" | ".profile"
            | ".zshrc" | ".zshenv" | ".zprofile" | "PKGBUILD" => Self::Bash,
            "Rakefile" | "Gemfile" | "Guardfile" | "Podfile" | "Vagrantfile" | "Brewfile"
            | "Capfile" | "Fastfile" | "Berksfile" | "Dangerfile" => Self::Ruby,
            "SConstruct" | "SConscript" | "wscript" => Self::Python,
            "Pipfile" | "Cargo.lock" | "poetry.lock" | "uv.lock" => Self::Toml,
            ".babelrc" | ".eslintrc" | ".swcrc" | ".prettierrc" | "composer.lock" => Self::Json,
            ".clang-format" | ".clang-tidy" => Self::Yaml,
            _ => return None,
        })
    }

    /// The language of a file extension (without the dot, any case).
    pub fn from_extension(ext: &str) -> Option<Language> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "py" | "pyi" | "pyw" => Self::Python,
            "go" => Self::Go,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "inl" => {
                Self::Cpp
            }
            "cs" => Self::CSharp,
            "json" | "jsonc" => Self::Json,
            "sh" | "bash" | "zsh" | "ksh" => Self::Bash,
            "kt" | "kts" => Self::Kotlin,
            "rb" | "rake" | "gemspec" | "ru" => Self::Ruby,
            "php" | "phtml" => Self::Php,
            "toml" => Self::Toml,
            "yml" | "yaml" => Self::Yaml,
            "html" | "htm" | "xhtml" => Self::Html,
            "css" => Self::Css,
            _ => return None,
        })
    }

    /// The lowercase name, as used in the serialized form.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Json => "json",
            Self::Bash => "bash",
            Self::Kotlin => "kotlin",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Html => "html",
            Self::Css => "css",
        }
    }

    /// The versions of the grammar crates behind this language, for example
    /// `tree-sitter-rust 0.24.2`. The value comes from `Cargo.lock` at build time.
    pub fn grammar_version(self) -> String {
        let crates: &[&str] = match self {
            Self::Rust => &["tree-sitter-rust"],
            Self::TypeScript | Self::Tsx => &["tree-sitter-typescript", "tree-sitter-javascript"],
            Self::JavaScript => &["tree-sitter-javascript"],
            Self::Python => &["tree-sitter-python"],
            Self::Go => &["tree-sitter-go"],
            Self::Java => &["tree-sitter-java"],
            Self::C => &["tree-sitter-c"],
            Self::Cpp => &["tree-sitter-cpp", "tree-sitter-c"],
            Self::CSharp => &["tree-sitter-c-sharp"],
            Self::Json => &["tree-sitter-json"],
            Self::Bash => &["tree-sitter-bash"],
            Self::Kotlin => &["tree-sitter-kotlin-ng"],
            Self::Ruby => &["tree-sitter-ruby"],
            Self::Php => &["tree-sitter-php"],
            Self::Toml => &["tree-sitter-toml-ng"],
            Self::Yaml => &["tree-sitter-yaml"],
            Self::Html => &["tree-sitter-html"],
            Self::Css => &["tree-sitter-css"],
        };
        let all = env!("AIVORY_GRAMMAR_VERSIONS");
        crates
            .iter()
            .map(|c| {
                let version = all
                    .split(';')
                    .find_map(|l| l.strip_prefix(c).and_then(|r| r.strip_prefix('=')))
                    .unwrap_or("unknown");
                format!("{c} {version}")
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }

    pub(crate) fn ts_language(self) -> TsLanguage {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
        }
    }

    fn defs_source(self) -> &'static str {
        match self {
            Self::Rust => include_str!("../queries/rust.tags.scm"),
            Self::TypeScript | Self::Tsx => include_str!("../queries/typescript.tags.scm"),
            Self::JavaScript => include_str!("../queries/javascript.tags.scm"),
            Self::Python => include_str!("../queries/python.tags.scm"),
            Self::Go => include_str!("../queries/go.tags.scm"),
            Self::Java => include_str!("../queries/java.tags.scm"),
            Self::C => include_str!("../queries/c.tags.scm"),
            Self::Cpp => concat!(
                include_str!("../queries/c.tags.scm"),
                "\n",
                include_str!("../queries/cpp.tags.scm")
            ),
            Self::CSharp => include_str!("../queries/csharp.tags.scm"),
            Self::Json => include_str!("../queries/json.tags.scm"),
            Self::Bash => include_str!("../queries/bash.tags.scm"),
            Self::Kotlin => include_str!("../queries/kotlin.tags.scm"),
            Self::Ruby => include_str!("../queries/ruby.tags.scm"),
            Self::Php => include_str!("../queries/php.tags.scm"),
            Self::Toml => include_str!("../queries/toml.tags.scm"),
            Self::Yaml => include_str!("../queries/yaml.tags.scm"),
            Self::Html => include_str!("../queries/html.tags.scm"),
            Self::Css => include_str!("../queries/css.tags.scm"),
        }
    }

    fn imports_source(self) -> Option<&'static str> {
        Some(match self {
            Self::Rust => include_str!("../queries/rust.imports.scm"),
            Self::TypeScript | Self::Tsx | Self::JavaScript => {
                include_str!("../queries/javascript.imports.scm")
            }
            Self::Python => include_str!("../queries/python.imports.scm"),
            Self::Go => include_str!("../queries/go.imports.scm"),
            Self::Java => include_str!("../queries/java.imports.scm"),
            Self::C | Self::Cpp => include_str!("../queries/c.imports.scm"),
            Self::CSharp => include_str!("../queries/csharp.imports.scm"),
            Self::Bash => include_str!("../queries/bash.imports.scm"),
            Self::Kotlin => include_str!("../queries/kotlin.imports.scm"),
            Self::Ruby => include_str!("../queries/ruby.imports.scm"),
            Self::Php => include_str!("../queries/php.imports.scm"),
            Self::Css => include_str!("../queries/css.imports.scm"),
            Self::Json | Self::Toml | Self::Yaml | Self::Html => return None,
        })
    }

    /// The highlights query. Several grammars ship a query that extends the query of a base
    /// grammar, so the text joins the two.
    fn highlights_source(self) -> String {
        use tree_sitter_javascript as js;
        match self {
            // The Rust query marks numbers as `constant.builtin` and knows few operators.
            // The last pattern wins, so the additions go last.
            Self::Rust => format!(
                "{}
{}",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                r#"[(integer_literal) (float_literal)] @number
["+" "-" "/" "%" "=" "==" "!=" "<" ">" "<=" ">=" "&&" "||" "!" "+=" "-=" "*=" "/=" "->" "=>" ".." "..=" "?"] @operator"#
            ),
            Self::TypeScript => format!(
                "{}\n{}",
                js::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            Self::Tsx => format!(
                "{}\n{}\n{}",
                js::HIGHLIGHT_QUERY,
                js::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            Self::JavaScript => format!("{}\n{}", js::HIGHLIGHT_QUERY, js::JSX_HIGHLIGHT_QUERY),
            Self::Python => tree_sitter_python::HIGHLIGHTS_QUERY.into(),
            Self::Go => tree_sitter_go::HIGHLIGHTS_QUERY.into(),
            Self::Java => tree_sitter_java::HIGHLIGHTS_QUERY.into(),
            Self::C => tree_sitter_c::HIGHLIGHT_QUERY.into(),
            Self::Cpp => format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            ),
            Self::CSharp => tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
            Self::Json => tree_sitter_json::HIGHLIGHTS_QUERY.into(),
            Self::Bash => tree_sitter_bash::HIGHLIGHT_QUERY.into(),
            Self::Kotlin => include_str!("../queries/kotlin.highlights.scm").into(),
            Self::Ruby => tree_sitter_ruby::HIGHLIGHTS_QUERY.into(),
            Self::Php => tree_sitter_php::HIGHLIGHTS_QUERY.into(),
            Self::Toml => tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
            Self::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
            Self::Html => tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            Self::Css => tree_sitter_css::HIGHLIGHTS_QUERY.into(),
        }
    }

    fn locals_source(self) -> &'static str {
        match self {
            Self::JavaScript | Self::Tsx | Self::TypeScript => tree_sitter_javascript::LOCALS_QUERY,
            Self::Ruby => tree_sitter_ruby::LOCALS_QUERY,
            _ => "",
        }
    }
}

/// The compiled queries of one language.
pub(crate) struct LangData {
    pub ts: TsLanguage,
    pub defs: Query,
    pub imports: Option<Query>,
}

const N: usize = Language::ALL.len();

/// The compiled queries of a language. They compile once per process.
pub(crate) fn data(lang: Language) -> Result<&'static LangData, Error> {
    static CELLS: [OnceLock<Result<LangData, Error>>; N] = [const { OnceLock::new() }; N];
    CELLS[lang as usize]
        .get_or_init(|| {
            let ts = lang.ts_language();
            let compile = |which: &'static str, src: &str| {
                Query::new(&ts, src).map_err(|e| Error::Query {
                    lang,
                    which,
                    message: e.to_string(),
                })
            };
            let defs = compile("declarations", lang.defs_source())?;
            let imports = lang
                .imports_source()
                .map(|s| compile("imports", s))
                .transpose()?;
            Ok(LangData { ts, defs, imports })
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The highlight configuration of a language. It compiles once per process.
pub(crate) fn highlight_config(lang: Language) -> Result<&'static HighlightConfiguration, Error> {
    static CELLS: [OnceLock<Result<HighlightConfiguration, Error>>; N] =
        [const { OnceLock::new() }; N];
    CELLS[lang as usize]
        .get_or_init(|| {
            let mut config = HighlightConfiguration::new(
                lang.ts_language(),
                lang.name(),
                &bare_patterns_first(&lang.highlights_source()),
                "",
                lang.locals_source(),
            )
            .map_err(|e| Error::Query {
                lang,
                which: "highlights",
                message: e.to_string(),
            })?;
            config.configure(&names());
            Ok(config)
        })
        .as_ref()
        .map_err(Clone::clone)
}
