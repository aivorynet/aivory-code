use std::collections::HashSet;
use std::fs;
use std::path::Path;

use aivory_protocol::repos::MapFile;
use aivory_protocol::warroom::Part;

use crate::file::{assemble, codebase_id};
use crate::stamp;

/// The `author` key that `init` writes.
pub const INIT_AUTHOR: &str = "aivoryd map init";

/// Folders that never form a part.
const SKIPPED: &[&str] = &["target", "node_modules", "build", "dist", "vendor"];
/// Folders that group child projects: each child folder becomes a part.
const WORKSPACES: &[&str] = &["crates", "apps", "packages", "libs", "services", "modules"];
/// A file with one of these extensions counts as a source file.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "java", "kt", "kts", "scala", "groovy",
    "go", "py", "rb", "php", "swift", "m", "mm", "dart", "js", "jsx", "mjs", "cjs", "ts", "tsx",
    "vue", "svelte", "html", "css", "scss", "sass", "less", "sql", "sh", "bash", "ps1", "bat",
    "lua", "pl", "r", "ex", "exs", "erl", "hs", "ml", "clj", "jl", "zig", "proto", "graphql",
];
/// Deepest folder level that the source search visits.
const MAX_DEPTH: usize = 16;
/// The name of the part for files in the repository root.
const ROOT_PART: &str = "root";

/// Guess a first map from the top folders of a repository.
///
/// - One part per top-level folder that holds source files. The search skips
///   hidden folders, `target`, `node_modules`, `build`, `dist`, and `vendor`.
/// - A workspace folder (`crates`, `apps`, `packages`, `libs`, `services`,
///   `modules`) gives one part per child folder.
/// - Files in the repository root go in a last part named `root` (`*`).
///
/// The result has `version = 1` and `author = "aivoryd map init"`. Nothing is
/// written to disk. A folder that cannot be read counts as empty.
pub fn init(repo_root: &Path) -> MapFile {
    assemble(
        &codebase_id(repo_root),
        1,
        stamp::now_ms(),
        Some(INIT_AUTHOR),
        detect_parts(repo_root),
    )
}

fn detect_parts(repo_root: &Path) -> Vec<Part> {
    let (folders, has_root_file) = list(repo_root);
    let mut taken: HashSet<String> = HashSet::from([ROOT_PART.to_owned()]);
    let mut parts = Vec::new();
    for top in &folders {
        let top_dir = repo_root.join(top);
        if WORKSPACES.iter().any(|w| w.eq_ignore_ascii_case(top)) {
            let children: Vec<String> = list(&top_dir)
                .0
                .into_iter()
                .filter(|child| holds_source(&top_dir.join(child), 0))
                .collect();
            if !children.is_empty() {
                for child in children {
                    parts.push(Part {
                        name: unique_name(&mut taken, &child, top),
                        paths: vec![format!("{}/{}/**", escape(top), escape(&child))],
                    });
                }
                continue;
            }
        }
        if holds_source(&top_dir, 0) {
            parts.push(Part {
                name: unique_name(&mut taken, top, top),
                paths: vec![format!("{}/**", escape(top))],
            });
        }
    }
    if has_root_file {
        parts.push(Part {
            name: ROOT_PART.to_owned(),
            paths: vec!["*".to_owned()],
        });
    }
    parts
}

/// The usable child folders of `dir`, sorted, and whether `dir` has a file.
fn list(dir: &Path) -> (Vec<String>, bool) {
    let mut folders = Vec::new();
    let mut has_file = false;
    let Ok(entries) = fs::read_dir(dir) else {
        return (folders, has_file);
    };
    for entry in entries.flatten() {
        // `file_type` does not follow links, so a link never loops back.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() {
            has_file = true;
        } else if kind.is_dir()
            && let Some(name) = entry.file_name().to_str()
            && !is_skipped(name)
            && !name.contains(['\\', '/'])
        {
            folders.push(name.to_owned());
        }
    }
    folders.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
    (folders, has_file)
}

fn is_skipped(name: &str) -> bool {
    name.starts_with('.') || SKIPPED.iter().any(|s| name.eq_ignore_ascii_case(s))
}

/// True when `dir` or a folder below it holds a source file.
fn holds_source(dir: &Path, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut subfolders = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() {
            let name = entry.file_name();
            let is_source = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SOURCE_EXTENSIONS.iter().any(|s| s.eq_ignore_ascii_case(e)));
            if is_source {
                return true;
            }
        } else if kind.is_dir()
            && let Some(name) = entry.file_name().to_str()
            && !is_skipped(name)
        {
            subfolders.push(entry.path());
        }
    }
    subfolders.iter().any(|d| holds_source(d, depth + 1))
}

/// A part name that no other part uses: `name`, then `parent-name`, then `name-2`, ...
fn unique_name(taken: &mut HashSet<String>, name: &str, parent: &str) -> String {
    let mut candidate = name.to_owned();
    if taken.contains(&candidate) && parent != name {
        candidate = format!("{parent}-{name}");
    }
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = format!("{name}-{n}");
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// Make a folder name safe inside a glob.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_wraps_glob_characters() {
        assert_eq!(escape("plain-name_1"), "plain-name_1");
        assert_eq!(escape("a[1]{x}*?"), "a[[]1[]][{]x[}][*][?]");
    }

    #[test]
    fn unique_name_avoids_taken_names() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&mut taken, "core", "crates"), "core");
        assert_eq!(unique_name(&mut taken, "core", "apps"), "apps-core");
        assert_eq!(unique_name(&mut taken, "core", "apps"), "core-2");
    }
}
