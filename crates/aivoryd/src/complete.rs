//! `compose/complete`: the suggestions for a chat field. `/` lists the commands of the agent
//! (the built-in commands that work in a headless run, the custom commands, prompts and skills
//! of the user and of the project); `@` lists the files of the codebase and the subagents.
//!
//! The native folders (`~/.claude`, `~/.codex`, `~/.gemini`) are only read: the file names
//! and the `description` of a command file. Session files are never read here.

use std::fs;
use std::path::{Path, PathBuf};

use aivory_protocol::complete::{
    COMPLETE_MAX, CompleteItem, CompleteItemKind, CompleteKind, CompleteList, CompleteParams, rank,
};

/// The native folders of the agents.
#[derive(Debug, Clone)]
pub struct Roots<'a> {
    pub claude: &'a Path,
    pub codex: &'a Path,
    pub gemini: &'a Path,
}

/// The built-in commands of Claude Code that work in a headless (`-p`) run.
const CLAUDE_BUILTIN: &[(&str, &str)] = &[
    ("compact", "Compact the conversation, keep a summary"),
    ("context", "Show the context use"),
    ("cost", "Show the cost of the session"),
    ("init", "Write a CLAUDE.md for the codebase"),
    ("review", "Review a pull request"),
    (
        "security-review",
        "Review the changes for security problems",
    ),
    ("pr-comments", "Show the comments of a pull request"),
    ("release-notes", "Show the release notes"),
    ("todos", "Show the todo list"),
];

/// The most files that one `@` search looks at.
const WALK_MAX: usize = 20_000;

/// The suggestions for `p`. `provider` and `cwd` come from the session (or from `p`).
pub fn list(p: &CompleteParams, provider: &str, cwd: Option<&Path>, roots: &Roots) -> CompleteList {
    let mut found: Vec<(u32, CompleteItem)> = Vec::new();
    match p.kind {
        CompleteKind::Command => commands(provider, cwd, roots, &mut found),
        CompleteKind::Mention => {
            if provider == "claude" {
                agents(cwd, roots, &mut found);
            }
            if let Some(cwd) = cwd {
                files(cwd, &p.query, &mut found);
            }
        }
    }
    let mut scored: Vec<(u32, CompleteItem)> = found
        .into_iter()
        .filter_map(|(base, item)| {
            let key = item.label.trim_start_matches(['/', '@']);
            rank(key, &p.query).map(|r| (r * 10 + base, item))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.label.len().cmp(&b.1.label.len()))
            .then(a.1.label.cmp(&b.1.label))
    });
    let mut items: Vec<CompleteItem> = Vec::new();
    for (_, item) in scored {
        if items.iter().any(|i| i.insert == item.insert) {
            continue;
        }
        items.push(item);
        if items.len() >= COMPLETE_MAX {
            break;
        }
    }
    CompleteList { items }
}

fn item(
    kind: CompleteItemKind,
    label: String,
    insert: String,
    detail: Option<String>,
) -> CompleteItem {
    CompleteItem {
        kind,
        label,
        insert,
        detail,
    }
}

fn commands(provider: &str, cwd: Option<&Path>, roots: &Roots, out: &mut Vec<(u32, CompleteItem)>) {
    match provider {
        "claude" => {
            let project = cwd.map(|c| c.join(".claude"));
            for dir in project.iter().map(PathBuf::as_path).chain([roots.claude]) {
                for (name, file) in files_with_ext(&dir.join("commands"), "md", 3) {
                    let name = name.rsplit('/').next().unwrap_or(&name).to_string();
                    let detail = md_description(&file);
                    out.push((
                        0,
                        item(
                            CompleteItemKind::Custom,
                            format!("/{name}"),
                            format!("/{name} "),
                            detail,
                        ),
                    ));
                }
                for (name, file) in skill_dirs(&dir.join("skills")) {
                    out.push((
                        1,
                        item(
                            CompleteItemKind::Skill,
                            format!("/{name}"),
                            format!("/{name} "),
                            md_description(&file),
                        ),
                    ));
                }
            }
            for (name, detail) in CLAUDE_BUILTIN {
                out.push((
                    2,
                    item(
                        CompleteItemKind::Command,
                        format!("/{name}"),
                        format!("/{name} "),
                        Some((*detail).to_string()),
                    ),
                ));
            }
        }
        "codex" => {
            // Custom prompts: `/prompts:<name>`. The daemon expands them when it sends.
            for (name, file) in files_with_ext(&roots.codex.join("prompts"), "md", 1) {
                out.push((
                    0,
                    item(
                        CompleteItemKind::Custom,
                        format!("/prompts:{name}"),
                        format!("/prompts:{name} "),
                        md_description(&file),
                    ),
                ));
            }
        }
        "gemini" => {
            let project = cwd.map(|c| c.join(".gemini"));
            for dir in project.iter().map(PathBuf::as_path).chain([roots.gemini]) {
                for (name, file) in files_with_ext(&dir.join("commands"), "toml", 3) {
                    let name = name.replace('/', ":");
                    out.push((
                        0,
                        item(
                            CompleteItemKind::Custom,
                            format!("/{name}"),
                            format!("/{name} "),
                            toml_description(&file),
                        ),
                    ));
                }
            }
        }
        _ => {}
    }
}

/// The subagents of Claude Code: `@agent-<name>`.
fn agents(cwd: Option<&Path>, roots: &Roots, out: &mut Vec<(u32, CompleteItem)>) {
    let project = cwd.map(|c| c.join(".claude"));
    for dir in project.iter().map(PathBuf::as_path).chain([roots.claude]) {
        for (name, file) in files_with_ext(&dir.join("agents"), "md", 1) {
            let name = frontmatter(&file, "name").unwrap_or(name);
            out.push((
                0,
                item(
                    CompleteItemKind::Agent,
                    format!("@agent-{name}"),
                    format!("@agent-{name} "),
                    md_description(&file),
                ),
            ));
        }
    }
}

/// The files and folders of the codebase (the ignore files apply).
fn files(cwd: &Path, query: &str, out: &mut Vec<(u32, CompleteItem)>) {
    let query = query.replace('\\', "/");
    let mut walker = ignore::WalkBuilder::new(cwd);
    walker.hidden(true).max_depth(Some(12)).follow_links(false);
    let mut seen = 0usize;
    let mut found: Vec<(u32, String, bool)> = Vec::new();
    for entry in walker.build().flatten() {
        seen += 1;
        if seen > WALK_MAX {
            break;
        }
        let Ok(rel) = entry.path().strip_prefix(cwd) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        let dir = entry.file_type().is_some_and(|t| t.is_dir());
        if let Some(r) = rank(&rel, &query) {
            // With an empty query, the top level first.
            let depth = rel.matches('/').count() as u32;
            let score = if query.is_empty() { depth } else { r };
            found.push((score, rel, dir));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.len().cmp(&b.1.len())));
    for (score, rel, dir) in found.into_iter().take(COMPLETE_MAX * 2) {
        let label = if dir { format!("{rel}/") } else { rel.clone() };
        let detail = dir.then(|| "folder".to_string());
        // The rank of the file is computed again by `list`; this keeps files after agents.
        let _ = score;
        out.push((
            1,
            item(
                CompleteItemKind::File,
                label.clone(),
                format!("@{label} "),
                detail,
            ),
        ));
    }
}

/// The files with extension `ext` under `dir`, to `depth` levels: the name relative to `dir`
/// without the extension (with `/`), and the path.
fn files_with_ext(dir: &Path, ext: &str, depth: usize) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    walk(dir, dir, ext, depth, &mut out);
    out.sort();
    out
}

fn walk(base: &Path, dir: &Path, ext: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 1 {
                walk(base, &path, ext, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e == ext)
            && let Ok(rel) = path.with_extension("").strip_prefix(base)
        {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

/// The skill folders under `dir` that hold a `SKILL.md`.
fn skill_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let file = e.path().join("SKILL.md");
            file.is_file()
                .then(|| (e.file_name().to_string_lossy().to_string(), file))
        })
        .collect();
    out.sort();
    out
}

/// The first 4 KB of a file as text.
fn head(file: &Path) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; 4096];
    let n = fs::File::open(file)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    buf.truncate(n);
    String::from_utf8_lossy(&buf).to_string()
}

/// A key of the YAML front matter of a Markdown file.
fn frontmatter(file: &Path, key: &str) -> Option<String> {
    let text = head(file);
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    rest[..end].lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// The description of a command, skill or agent file: the `description` of the front matter,
/// else the first text line. At most 90 characters.
fn md_description(file: &Path) -> Option<String> {
    let d = frontmatter(file, "description").or_else(|| {
        let text = head(file);
        let body = match text.strip_prefix("---") {
            Some(rest) => rest
                .find("\n---")
                .map_or("", |e| &rest[e + 4..])
                .to_string(),
            None => text,
        };
        body.lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .map(str::to_string)
    })?;
    Some(short(&d))
}

/// The `description` of a Gemini command file.
fn toml_description(file: &Path) -> Option<String> {
    let v: toml::Value = toml::from_str(&head(file)).ok()?;
    v.get("description")?.as_str().map(short)
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 90 {
        return s.to_string();
    }
    let mut t: String = s.chars().take(89).collect();
    t.push('…');
    t
}

/// A Codex custom prompt in a message (`/prompts:<name> args`): the text of the prompt file
/// with `$ARGUMENTS` (and `$1` … `$9`) put in. `None`: the message is not a known prompt.
pub fn expand_codex_prompt(text: &str, codex_root: &Path) -> Option<String> {
    let rest = text.trim_start().strip_prefix("/prompts:")?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return None;
    }
    let body = fs::read_to_string(codex_root.join("prompts").join(format!("{name}.md"))).ok()?;
    let body = match body.strip_prefix("---") {
        Some(r) => r
            .find("\n---")
            .map_or(body.as_str(), |e| r[e + 4..].trim_start()),
        None => body.as_str(),
    };
    let args = args.trim();
    let words: Vec<&str> = args.split_whitespace().collect();
    let mut out = body.replace("$ARGUMENTS", args);
    for i in (1..=9).rev() {
        out = out.replace(&format!("${i}"), words.get(i - 1).copied().unwrap_or(""));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (dir.join("claude"), dir.join("codex"), dir.join("gemini"))
    }

    fn params(kind: CompleteKind, query: &str) -> CompleteParams {
        CompleteParams {
            kind,
            query: query.into(),
            session: None,
            provider: None,
            cwd: None,
        }
    }

    #[test]
    fn claude_commands_hold_custom_skills_and_builtins() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (c, x, g) = roots(tmp.path());
        fs::create_dir_all(c.join("commands")).expect("dir");
        fs::write(
            c.join("commands/ship.md"),
            "---\ndescription: Ship the release\n---\nDo it.",
        )
        .expect("write");
        fs::create_dir_all(c.join("skills/tidy")).expect("dir");
        fs::write(
            c.join("skills/tidy/SKILL.md"),
            "---\nname: tidy\ndescription: Tidy up\n---\n",
        )
        .expect("write");
        let r = Roots {
            claude: &c,
            codex: &x,
            gemini: &g,
        };
        let all = list(&params(CompleteKind::Command, ""), "claude", None, &r);
        let labels: Vec<&str> = all.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"/ship"), "{labels:?}");
        assert!(labels.contains(&"/tidy"), "{labels:?}");
        assert!(labels.contains(&"/compact"), "{labels:?}");
        let ship = all.items.iter().find(|i| i.label == "/ship").expect("ship");
        assert_eq!(ship.detail.as_deref(), Some("Ship the release"));
        assert_eq!(ship.insert, "/ship ");
        let some = list(&params(CompleteKind::Command, "sec"), "claude", None, &r);
        assert_eq!(some.items[0].label, "/security-review");
    }

    #[test]
    fn codex_prompts_complete_and_expand() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (c, x, g) = roots(tmp.path());
        fs::create_dir_all(x.join("prompts")).expect("dir");
        fs::write(
            x.join("prompts/fix.md"),
            "---\ndescription: Fix it\n---\nFix $1 in $ARGUMENTS",
        )
        .expect("write");
        let r = Roots {
            claude: &c,
            codex: &x,
            gemini: &g,
        };
        let all = list(&params(CompleteKind::Command, "fi"), "codex", None, &r);
        assert_eq!(all.items[0].insert, "/prompts:fix ");
        assert_eq!(
            expand_codex_prompt("/prompts:fix bug.rs", &x).as_deref(),
            Some("Fix bug.rs in bug.rs")
        );
        assert!(expand_codex_prompt("/prompts:none", &x).is_none());
        assert!(expand_codex_prompt("hello", &x).is_none());
    }

    #[test]
    fn mentions_list_files_and_agents() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (c, x, g) = roots(tmp.path());
        let code = tmp.path().join("code");
        fs::create_dir_all(code.join("src")).expect("dir");
        fs::write(code.join("src/main.rs"), "fn main() {}").expect("write");
        fs::create_dir_all(c.join("agents")).expect("dir");
        fs::write(
            c.join("agents/tester.md"),
            "---\nname: tester\ndescription: Runs tests\n---\n",
        )
        .expect("write");
        let r = Roots {
            claude: &c,
            codex: &x,
            gemini: &g,
        };
        let all = list(
            &params(CompleteKind::Mention, "main"),
            "claude",
            Some(&code),
            &r,
        );
        assert_eq!(all.items[0].insert, "@src/main.rs ");
        let agents = list(
            &params(CompleteKind::Mention, "agent-t"),
            "claude",
            Some(&code),
            &r,
        );
        assert_eq!(agents.items[0].insert, "@agent-tester ");
        // Codex has no subagents.
        let codex = list(
            &params(CompleteKind::Mention, "agent"),
            "codex",
            Some(&code),
            &r,
        );
        assert!(
            codex
                .items
                .iter()
                .all(|i| i.kind != CompleteItemKind::Agent)
        );
    }
}
