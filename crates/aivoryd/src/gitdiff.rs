//! `session/diff` for sessions that aivoryd sees: the working-tree diff of the session folder
//! (`git diff`, plus untracked files as added files). The repository stays untouched.

use std::path::Path;
use std::process::{Command, Stdio};

use aivory_protocol::{DiffFile, DiffLine, DiffLineKind};

/// Untracked files above this size show as one line.
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024;
const MAX_UNTRACKED_FILES: usize = 50;

/// The diff of the working tree against HEAD. An empty list for a folder outside git.
pub fn working_tree(cwd: &Path) -> Vec<DiffFile> {
    let mut files = git(cwd, &["diff", "--no-color", "--no-ext-diff", "-U3", "HEAD"])
        .map(|text| parse(&text))
        .unwrap_or_default();
    if let Some(list) = git(cwd, &["ls-files", "--others", "--exclude-standard"]) {
        for name in list
            .lines()
            .filter(|l| !l.is_empty())
            .take(MAX_UNTRACKED_FILES)
        {
            files.push(untracked(cwd, name));
        }
    }
    files
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn untracked(cwd: &Path, name: &str) -> DiffFile {
    let path = cwd.join(name);
    let size = path.metadata().map_or(0, |m| m.len());
    let text = if size <= MAX_UNTRACKED_BYTES {
        std::fs::read_to_string(&path).ok()
    } else {
        None
    };
    let lines: Vec<DiffLine> = match &text {
        Some(text) => text
            .lines()
            .enumerate()
            .map(|(i, l)| DiffLine {
                kind: DiffLineKind::Added,
                old: None,
                new: Some(i as u32 + 1),
                text: l.to_string(),
                blame: None,
            })
            .collect(),
        None => vec![DiffLine {
            kind: DiffLineKind::Hunk,
            old: None,
            new: None,
            text: format!("new file, {size} bytes (binary or large)"),
            blame: None,
        }],
    };
    DiffFile {
        path: name.to_string(),
        added: lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Added)
            .count() as u32,
        removed: 0,
        lines,
    }
}

/// Parse `git diff` output (unified format) into files with numbered lines.
pub fn parse(text: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest.rsplit_once(" b/").map_or(rest, |(_, b)| b).to_string();
            files.push(DiffFile {
                path,
                added: 0,
                removed: 0,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            let (o, n) = hunk_start(line);
            old = o;
            new = n;
            file.lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                old: None,
                new: None,
                text: line.to_string(),
                blame: None,
            });
        } else if file.lines.is_empty() {
            // Header lines (index, ---, +++, mode) come before the first hunk.
            continue;
        } else if let Some(t) = line.strip_prefix('+') {
            file.added += 1;
            file.lines
                .push(numbered(DiffLineKind::Added, None, Some(new), t));
            new += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            file.removed += 1;
            file.lines
                .push(numbered(DiffLineKind::Removed, Some(old), None, t));
            old += 1;
        } else if let Some(t) = line.strip_prefix(' ') {
            file.lines
                .push(numbered(DiffLineKind::Context, Some(old), Some(new), t));
            old += 1;
            new += 1;
        }
    }
    files
}

fn numbered(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old,
        new,
        text: text.to_string(),
        blame: None,
    }
}

/// `@@ -12,7 +12,9 @@` -> (12, 12).
fn hunk_start(line: &str) -> (u32, u32) {
    let mut parts = line.split_whitespace().skip(1);
    let num = |p: Option<&str>| {
        p.and_then(|s| s.get(1..))
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
    };
    (num(parts.next()), num(parts.next()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -2,3 +2,4 @@ fn main() {
     let a = 1;
-    let b = 2;
+    let b = 3;
+    let c = 4;
     println!(\"{a}\");
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-old
+new
";

    #[test]
    fn parses_files_counts_and_numbers() {
        let files = parse(DIFF);
        assert_eq!(files.len(), 2);
        let a = &files[0];
        assert_eq!((a.path.as_str(), a.added, a.removed), ("src/a.rs", 2, 1));
        let kinds: Vec<DiffLineKind> = a.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                DiffLineKind::Hunk,
                DiffLineKind::Context,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Added,
                DiffLineKind::Context
            ]
        );
        assert_eq!((a.lines[1].old, a.lines[1].new), (Some(2), Some(2)));
        assert_eq!(a.lines[2].old, Some(3));
        assert_eq!((a.lines[3].new, a.lines[4].new), (Some(3), Some(4)));
        assert_eq!((a.lines[5].old, a.lines[5].new), (Some(4), Some(5)));
        assert_eq!((files[1].added, files[1].removed), (1, 1));
    }

    #[test]
    fn hunk_without_counts() {
        assert_eq!(hunk_start("@@ -1 +1 @@"), (1, 1));
        assert_eq!(hunk_start("@@ -0,0 +1,3 @@"), (0, 1));
    }
}
