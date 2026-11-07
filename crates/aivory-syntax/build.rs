//! Reads the grammar crate versions from `Cargo.lock` and passes them to the crate as the
//! environment variable `AIVORY_GRAMMAR_VERSIONS` (`name=version` entries, separated by `;`).
//! The stub key uses them, so a grammar update invalidates the cached stubs.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let mut out = String::new();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mut dir = Some(PathBuf::from(manifest));
    while let Some(d) = dir {
        let lock = d.join("Cargo.lock");
        if lock.is_file() {
            println!("cargo:rerun-if-changed={}", lock.display());
            if let Ok(text) = std::fs::read_to_string(&lock) {
                let mut name: Option<String> = None;
                for line in text.lines() {
                    if let Some(v) = line.strip_prefix("name = \"") {
                        name = v.strip_suffix('"').map(str::to_owned);
                    } else if let Some(v) = line.strip_prefix("version = \"")
                        && let (Some(n), Some(v)) = (name.take(), v.strip_suffix('"'))
                        && n.starts_with("tree-sitter")
                    {
                        let _ = write!(out, "{n}={v};");
                    }
                }
            }
            break;
        }
        dir = d.parent().map(PathBuf::from);
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=AIVORY_GRAMMAR_VERSIONS={out}");
}
