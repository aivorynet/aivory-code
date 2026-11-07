//! The language servers that the pool knows.

use std::ffi::OsStr;
use std::path::PathBuf;

use aivory_syntax::Language;
use serde_json::Value;

use crate::linters::which_in;

/// How to start the server of a language.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    /// The name of the server, for example `rust-analyzer`.
    pub id: String,
    /// The binary name that is searched on `PATH`.
    pub command: String,
    /// The arguments. `{data_dir}` becomes a folder of the pool for this lineage and server.
    pub args: Vec<String>,
    /// The languages that the server serves.
    pub languages: Vec<Language>,
    pub init_options: Option<Value>,
    /// A fixed binary. When set, `command` is not searched on `PATH`.
    pub exe: Option<PathBuf>,
}

impl ServerSpec {
    pub fn new(id: &str, command: &str, args: &[&str], languages: &[Language]) -> Self {
        Self {
            id: id.to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            languages: languages.to_vec(),
            init_options: None,
            exe: None,
        }
    }

    /// The binary of the server, when it is installed.
    pub fn find(&self, path_var: &OsStr) -> Option<PathBuf> {
        match &self.exe {
            Some(exe) => exe.is_file().then(|| exe.clone()),
            None => which_in(&self.command, path_var),
        }
    }

    pub fn serves(&self, language: Language) -> bool {
        self.languages.contains(&language)
    }
}

/// The servers of the design: rust-analyzer, typescript-language-server, pyright-langserver,
/// gopls, clangd, and jdtls.
pub fn known_servers() -> Vec<ServerSpec> {
    use Language::*;
    vec![
        ServerSpec::new("rust-analyzer", "rust-analyzer", &[], &[Rust]),
        ServerSpec::new(
            "typescript-language-server",
            "typescript-language-server",
            &["--stdio"],
            &[TypeScript, Tsx, JavaScript],
        ),
        ServerSpec::new("pyright", "pyright-langserver", &["--stdio"], &[Python]),
        ServerSpec::new("gopls", "gopls", &[], &[Go]),
        ServerSpec::new("clangd", "clangd", &[], &[C, Cpp]),
        ServerSpec::new("jdtls", "jdtls", &["-data", "{data_dir}"], &[Java]),
    ]
}

/// The `languageId` of the LSP for a language.
pub fn language_id(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::Tsx => "typescriptreact",
        Language::JavaScript => "javascript",
        Language::Python => "python",
        Language::Go => "go",
        Language::Java => "java",
        Language::C => "c",
        Language::Cpp => "cpp",
        Language::CSharp => "csharp",
        Language::Kotlin => "kotlin",
        Language::Ruby => "ruby",
        Language::Php => "php",
        Language::Bash => "shellscript",
        Language::Json => "json",
        Language::Toml => "toml",
        Language::Yaml => "yaml",
        Language::Html => "html",
        Language::Css => "css",
    }
}
