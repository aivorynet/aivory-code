//! `repos/get` and `repos/map` for the live war room: parts with their live sessions, the
//! file tree of the tracked files, and recent activity. Read-only.

use std::path::Path;
use std::process::{Command, Stdio};

use aivory_protocol::{
    ActivityRow, Codebase, FileNode, LineKind, MapFile, Pane, PartStats, RepoDetail,
};

/// Nodes of the file tree at most (folders first, two levels deep).
const MAX_NODES: usize = 400;
const MAX_DEPTH: usize = 2;

/// A file that a live session claims (an editing or intent claim on its path key). The caller
/// lists the editing claims first: the first claim of a file shows its pill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileClaim {
    /// Relative to the repository root, with `/`.
    pub path: String,
    /// The session id of the war room.
    pub session: String,
}

pub fn detail(codebase: &Codebase, sessions: &[Pane], claims: &[FileClaim]) -> RepoDetail {
    let mine: Vec<&Pane> = sessions
        .iter()
        .filter(|s| s.codebase == codebase.id)
        .collect();
    let parts = codebase
        .parts
        .iter()
        .map(|p| {
            let in_part: Vec<&&Pane> = mine.iter().filter(|s| s.part == p.name).collect();
            let roots: Vec<&str> = p.paths.iter().map(|g| g.trim_end_matches("/**")).collect();
            let claimed = claims
                .iter()
                .filter(|c| mine.iter().any(|s| s.id == c.session))
                .filter(|c| roots.iter().any(|r| under(&c.path, r)))
                .count();
            PartStats {
                name: p.name.clone(),
                paths: p.paths.clone(),
                sessions: in_part.iter().map(|s| s.id.clone()).collect(),
                claims: claimed as u32,
                last_change: in_part.iter().map(|s| s.since).max(),
                lines: 0,
            }
        })
        .collect();
    let mut activity: Vec<ActivityRow> = mine
        .iter()
        .map(|s| ActivityRow {
            at: s.since,
            provider: s.provider.clone(),
            session: s.id.clone(),
            what: s
                .tail
                .iter()
                .rev()
                .find(|l| l.kind == LineKind::Tool)
                .map(|l| format!("{} {}", l.label.as_deref().unwrap_or(""), l.text))
                .unwrap_or_else(|| s.title.clone()),
            added: s.tail.iter().filter(|l| l.kind == LineKind::Added).count() as u32,
            removed: s
                .tail
                .iter()
                .filter(|l| l.kind == LineKind::Removed)
                .count() as u32,
            cost: s.cost,
        })
        .collect();
    activity.sort_by_key(|a| std::cmp::Reverse(a.at));
    RepoDetail {
        codebase: codebase.clone(),
        parts,
        files: tree(Path::new(&codebase.path), codebase, &mine, claims),
        activity,
    }
}

/// The map of the codebase: `.aivory/map.toml`, or the heuristic of `aivoryd map init`.
pub fn map(codebase: &Codebase) -> MapFile {
    let root = Path::new(&codebase.path);
    aivory_map::load(root).unwrap_or_else(|_| aivory_map::init(root))
}

/// Folders and files of the tracked files, two levels deep. A folder that is the root of a
/// part (its glob `dir/**`) carries the part tag.
fn tree(root: &Path, codebase: &Codebase, mine: &[&Pane], claims: &[FileClaim]) -> Vec<FileNode> {
    let Some(list) = git_files(root) else {
        return Vec::new();
    };
    let part_of_dir = |dir: &str| {
        codebase
            .parts
            .iter()
            .find(|p| {
                p.paths
                    .iter()
                    .any(|g| g.trim_end_matches("/**") == dir.trim_end_matches('/'))
            })
            .map(|p| p.name.clone())
    };
    let mut seen = std::collections::HashSet::new();
    let mut paths: Vec<String> = Vec::new();
    let mut push = |path: String| {
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    };
    for file in list.lines().filter(|l| !l.is_empty()) {
        let parts: Vec<&str> = file.split('/').collect();
        let n = parts.len();
        // Folders at depth 0..MAX_DEPTH, files down to the same depth.
        for k in 1..n.min(MAX_DEPTH + 1) {
            push(format!("{}/", parts[..k].join("/")));
        }
        if n <= MAX_DEPTH {
            push(file.to_string());
        }
    }
    // Folders before files at each level, then by name.
    paths.sort_by_key(|p| sort_key(p));
    paths
        .into_iter()
        .take(MAX_NODES)
        .map(|path| {
            let dir = path.ends_with('/');
            let trimmed = path.trim_end_matches('/');
            let depth = trimmed.matches('/').count() as u8;
            // The first claim on the file (or, for a folder, below it) shows the pill.
            let claimer = claims
                .iter()
                .find(|c| {
                    if dir {
                        under(&c.path, trimmed)
                    } else {
                        c.path == trimmed
                    }
                })
                .and_then(|c| mine.iter().find(|s| s.id == c.session));
            FileNode {
                name: trimmed.rsplit('/').next().unwrap_or(trimmed).to_string(),
                part: if dir { part_of_dir(trimmed) } else { None },
                path,
                dir,
                depth,
                claimed_by: claimer.map(|s| s.id.clone()),
                claim_state: claimer.map(|s| s.state),
            }
        })
        .collect()
}

/// True when `path` is `folder` or lies below it. `.` or an empty folder is the root.
fn under(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches('/');
    folder.is_empty()
        || folder == "."
        || path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|r| r.starts_with('/'))
}

/// Sort a path as its parent chain, with folders first on each level.
fn sort_key(path: &str) -> Vec<(u8, String)> {
    let dir = path.ends_with('/');
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_last = i + 1 == parts.len();
            (u8::from(is_last && !dir), p.to_lowercase())
        })
        .collect()
}

fn git_files(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_lies_under_its_folders() {
        assert!(under("src/a.rs", "src"));
        assert!(under("src/a/b.rs", "src/"));
        assert!(under("a.rs", "."));
        assert!(!under("srcx/a.rs", "src"));
    }

    #[test]
    fn folders_come_before_files() {
        let mut paths = vec!["b.rs", "src/", "a.md", "src/x.rs", "src/core/"];
        paths.sort_by_key(|p| sort_key(p));
        assert_eq!(paths, ["src/", "src/core/", "src/x.rs", "a.md", "b.rs"]);
    }
}
