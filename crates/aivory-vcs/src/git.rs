//! The Git backend. It runs the `git` command line (no `gix`).
//!
//! Safety rules of this module:
//! - A snapshot uses a temporary index file (`GIT_INDEX_FILE`). The index of the user, the
//!   working tree, `HEAD`, and the branches stay as they are.
//! - Snapshot refs live only under `refs/aivory/turns/`.
//! - Every command runs with `--no-optional-locks`, so a read does not refresh the index.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::error::{Result, VcsError};
use crate::types::{
    BASELINE_TURN, BlameLine, ChangeStatus, CommitInfo, FileChange, LogOptions, MergeMessage,
    MergeOutcome, PublishRequest, PublishedCommit, RefInfo, Snapshot, SnapshotRequest, StatusEntry,
    TRAILER_AGENTS, TRAILER_TASK, TRAILER_TURNS, TURN_REF_PREFIX, TurnRef, Workspace,
    WorkspaceRequest, trailer_value,
};
use crate::vcs::{Vcs, check_arg};

/// Author and committer of snapshot commits. Snapshots are hidden, so they do not use the
/// identity of the user.
const SNAPSHOT_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "AIVory Code"),
    ("GIT_AUTHOR_EMAIL", "snapshot@aivory.invalid"),
    ("GIT_COMMITTER_NAME", "AIVory Code"),
    ("GIT_COMMITTER_EMAIL", "snapshot@aivory.invalid"),
];

const ZERO_ID_PREFIX: &str = "0000000000";

/// The Git backend.
#[derive(Debug, Clone)]
pub struct GitVcs {
    binary: PathBuf,
}

impl Default for GitVcs {
    fn default() -> Self {
        Self::new()
    }
}

impl GitVcs {
    /// Use the `git` binary from `PATH`.
    pub fn new() -> Self {
        Self {
            binary: PathBuf::from("git"),
        }
    }

    /// Use a given `git` binary.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// Run `git` and return the raw output, also for a non-zero exit code.
    fn exec(
        &self,
        cwd: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Output> {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(cwd)
            .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        // A stray variable of the parent process must not redirect the command.
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            cmd.env_remove(var);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes when the daemon runs git.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn()?;
        let writer = match (stdin, child.stdin.take()) {
            (Some(data), Some(mut pipe)) => {
                let data = data.to_vec();
                // A thread writes, so a large input cannot block against a full output pipe.
                Some(thread::spawn(move || {
                    let _ = pipe.write_all(&data);
                }))
            }
            _ => None,
        };
        let out = child.wait_with_output()?;
        if let Some(w) = writer {
            let _ = w.join();
        }
        Ok(out)
    }

    fn command_error(args: &[&str], out: &Output) -> VcsError {
        VcsError::Command {
            command: format!("git {}", args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        }
    }

    /// Run `git` and return stdout. A non-zero exit code is an error.
    fn run_env(
        &self,
        cwd: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let out = self.exec(cwd, args, env, stdin)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(Self::command_error(args, &out))
        }
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
        self.run_env(cwd, args, &[], None)
    }

    /// Run `git` and return trimmed stdout as text.
    fn text(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let out = self.run(cwd, args)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// Resolve a revision to an object of a type. `None` when it does not exist.
    fn try_resolve(&self, cwd: &Path, rev: &str, kind: &str) -> Result<Option<String>> {
        check_arg("revision", rev)?;
        let spec = format!("{rev}^{{{kind}}}");
        let out = self.exec(cwd, &["rev-parse", "--verify", "--quiet", &spec], &[], None)?;
        if out.status.success() {
            Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()))
        } else {
            Ok(None)
        }
    }

    fn tree_of(&self, cwd: &Path, rev: &str) -> Result<String> {
        self.try_resolve(cwd, rev, "tree")?
            .ok_or_else(|| VcsError::NotFound(format!("revision `{rev}`")))
    }

    fn empty_tree(&self, cwd: &Path) -> Result<String> {
        let out = self.run_env(
            cwd,
            &["hash-object", "-t", "tree", "--stdin"],
            &[],
            Some(b""),
        )?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// Copy the index of the user into `dest`, so that `git add` finds the stat data and does
    /// not hash every file again. The copy keeps the modification time (racy-git check).
    fn seed_index(&self, workdir: &Path, dest: &Path) -> Result<bool> {
        let rel = self.text(workdir, &["rev-parse", "--git-path", "index"])?;
        let src = workdir.join(rel);
        let Ok(meta) = fs::metadata(&src) else {
            return Ok(false);
        };
        fs::copy(&src, dest)?;
        if let Ok(modified) = meta.modified() {
            let file = fs::OpenOptions::new().write(true).open(dest)?;
            file.set_modified(modified)?;
        }
        Ok(true)
    }

    /// Write the working tree into the object store and return the tree id. The index of the
    /// user does not change: the work happens in a temporary index file.
    fn workdir_tree(&self, workdir: &Path) -> Result<String> {
        let tmp = tempfile::tempdir()?;
        let idx = tmp.path().join("index");
        let idx_str = idx.to_string_lossy().into_owned();
        let env = [("GIT_INDEX_FILE", idx_str.as_str())];
        let add = |this: &Self| {
            this.run_env(workdir, &["add", "-A", "--", ":/"], &env, None)
                .map(|_| ())
        };
        let seeded = self.seed_index(workdir, &idx).unwrap_or(false);
        if let Err(e) = add(self) {
            if !seeded {
                return Err(e);
            }
            // The copied index may not fit (for example a split index). Start empty.
            let _ = fs::remove_file(&idx);
            add(self)?;
        }
        let out = self.run_env(workdir, &["write-tree"], &env, None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    fn snapshot_message(req: &SnapshotRequest) -> String {
        format!(
            "aivory turn {s}/{n}\n\nAIVory-Session: {s}\nAIVory-Turn: {n}\nAIVory-Provider: {p}\n\
             AIVory-Model: {m}\n",
            s = req.session,
            n = req.n,
            p = req.provider,
            m = req.model
        )
    }
}

/// A session id becomes a ref component, so only safe characters pass.
fn check_session(session: &str) -> Result<()> {
    let ok = !session.is_empty()
        && !session.starts_with(['.', '-'])
        && !session.ends_with(['.', '-'])
        && !session.contains("..")
        && !session.ends_with(".lock")
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(VcsError::Invalid(format!("bad session id `{session}`")))
    }
}

fn snapshot_ref(session: &str, n: u32) -> String {
    format!("{TURN_REF_PREFIX}/{session}/{n}")
}

fn is_zero_id(id: &str) -> bool {
    id.starts_with(ZERO_ID_PREFIX) && id.bytes().all(|b| b == b'0')
}

// ---- Parsers (pure functions, unit tested) ------------------------------------------------

fn status_from_letter(c: &str) -> Option<ChangeStatus> {
    match c.chars().next()? {
        'A' => Some(ChangeStatus::Added),
        'M' => Some(ChangeStatus::Modified),
        'D' => Some(ChangeStatus::Deleted),
        'T' => Some(ChangeStatus::TypeChanged),
        _ => None,
    }
}

/// Parse `git diff-tree --raw -z --no-renames` and `--numstat -z --no-renames` output.
pub(crate) fn parse_changes(raw: &[u8], numstat: &[u8]) -> Result<Vec<FileChange>> {
    let raw = String::from_utf8_lossy(raw);
    let mut out: Vec<FileChange> = Vec::new();
    let mut tokens = raw.split('\0').filter(|t| !t.is_empty());
    while let Some(head) = tokens.next() {
        let head = head
            .strip_prefix(':')
            .ok_or_else(|| VcsError::Parse(format!("diff-tree header `{head}`")))?;
        let path = tokens
            .next()
            .ok_or_else(|| VcsError::Parse("diff-tree path missing".into()))?;
        let fields: Vec<&str> = head.split(' ').collect();
        if fields.len() < 5 {
            return Err(VcsError::Parse(format!("diff-tree header `{head}`")));
        }
        let Some(status) = status_from_letter(fields[4]) else {
            continue;
        };
        let new_id = fields[3];
        out.push(FileChange {
            path: path.to_owned(),
            status,
            added: 0,
            removed: 0,
            binary: false,
            blob_after: (status != ChangeStatus::Deleted && !is_zero_id(new_id))
                .then(|| new_id.to_owned()),
        });
    }
    let by_path: HashMap<String, usize> = out
        .iter()
        .enumerate()
        .map(|(i, c)| (c.path.clone(), i))
        .collect();
    let numstat = String::from_utf8_lossy(numstat);
    for entry in numstat.split('\0').filter(|t| !t.is_empty()) {
        let mut parts = entry.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Some(&i) = by_path.get(path) else {
            continue;
        };
        if a == "-" || r == "-" {
            out[i].binary = true;
        } else {
            out[i].added = a.parse().unwrap_or(0);
            out[i].removed = r.parse().unwrap_or(0);
        }
    }
    Ok(out)
}

/// Parse `git status --porcelain=v1 -z`.
pub(crate) fn parse_status(raw: &[u8]) -> Vec<StatusEntry> {
    let text = String::from_utf8_lossy(raw);
    let mut tokens = text.split('\0').filter(|t| !t.is_empty());
    let mut out = Vec::new();
    while let Some(tok) = tokens.next() {
        let mut chars = tok.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = tok.get(3..).unwrap_or_default().to_owned();
        let orig_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            tokens.next().map(str::to_owned)
        } else {
            None
        };
        out.push(StatusEntry {
            index: x,
            worktree: y,
            path,
            orig_path,
        });
    }
    out
}

/// Parse `git blame --porcelain`.
pub(crate) fn parse_blame(raw: &[u8]) -> Result<Vec<BlameLine>> {
    #[derive(Default, Clone)]
    struct Meta {
        time: i64,
        summary: String,
        filename: String,
    }
    let text = String::from_utf8_lossy(raw);
    let mut metas: HashMap<String, Meta> = HashMap::new();
    let mut lines = Vec::new();
    // (commit, orig_line, final_line) of the header that waits for its content line.
    let mut head: Option<(String, u32, u32)> = None;
    for line in text.split('\n') {
        if let Some(content) = line.strip_prefix('\t') {
            let Some((commit, orig_line, final_line)) = head.take() else {
                continue;
            };
            let meta = metas.get(&commit).cloned().unwrap_or_default();
            lines.push(BlameLine {
                line: final_line,
                orig_line,
                uncommitted: is_zero_id(&commit),
                commit,
                orig_path: meta.filename,
                time: meta.time,
                summary: meta.summary,
                content: content.trim_end_matches('\r').to_owned(),
            });
            continue;
        }
        let mut parts = line.split(' ');
        let first = parts.next().unwrap_or_default();
        let is_header = first.len() >= 40 && first.bytes().all(|b| b.is_ascii_hexdigit());
        if is_header {
            let orig = parts.next().and_then(|v| v.parse().ok());
            let fin = parts.next().and_then(|v| v.parse().ok());
            let (Some(orig), Some(fin)) = (orig, fin) else {
                return Err(VcsError::Parse(format!("blame header `{line}`")));
            };
            metas.entry(first.to_owned()).or_default();
            head = Some((first.to_owned(), orig, fin));
        } else if let Some((commit, _, _)) = &head {
            let meta = metas.entry(commit.clone()).or_default();
            if let Some(v) = line.strip_prefix("author-time ") {
                meta.time = v.parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("summary ") {
                meta.summary = v.to_owned();
            } else if let Some(v) = line.strip_prefix("filename ") {
                meta.filename = v.to_owned();
            }
        }
    }
    Ok(lines)
}

/// Parse `git merge-tree --write-tree -z --name-only`.
pub(crate) fn parse_merge_tree(raw: &[u8], clean: bool) -> Result<MergeOutcome> {
    let text = String::from_utf8_lossy(raw);
    let mut tokens = text.split('\0');
    let tree = tokens
        .next()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| VcsError::Parse("merge-tree gave no tree id".into()))?
        .to_owned();
    // Conflicted paths, up to an empty token.
    let mut conflicts = Vec::new();
    for tok in tokens.by_ref() {
        if tok.is_empty() {
            break;
        }
        conflicts.push(tok.to_owned());
    }
    // Messages: `<count> <paths...> <type> <message>`, each field ends with NUL.
    let mut messages = Vec::new();
    while let Some(count) = tokens.next() {
        let Ok(count) = count.parse::<usize>() else {
            break;
        };
        let paths: Vec<String> = tokens.by_ref().take(count).map(str::to_owned).collect();
        let kind = tokens.next().unwrap_or_default().to_owned();
        let text = tokens.next().unwrap_or_default().trim_end().to_owned();
        messages.push(MergeMessage { paths, kind, text });
    }
    Ok(MergeOutcome {
        clean,
        tree,
        conflicts,
        messages,
    })
}

/// Parse `git log` output of the format that [`GitVcs::log`] asks for.
pub(crate) fn parse_log(raw: &[u8]) -> Vec<CommitInfo> {
    let text = String::from_utf8_lossy(raw);
    text.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            if rec.is_empty() {
                return None;
            }
            let mut f = rec.splitn(7, '\x1f');
            let id = f.next()?.to_owned();
            let parents = f.next()?.split_whitespace().map(str::to_owned).collect();
            let author_name = f.next()?.to_owned();
            let author_email = f.next()?.to_owned();
            let time = f.next()?.parse().ok()?;
            let subject = f.next()?.to_owned();
            let body = f.next().unwrap_or_default().trim().to_owned();
            let turns = trailer_value(&body, TRAILER_TURNS)
                .map(TurnRef::parse_list)
                .unwrap_or_default();
            Some(CommitInfo {
                id,
                parents,
                author_name,
                author_email,
                time,
                subject,
                body,
                turns,
            })
        })
        .collect()
}

// ---- The trait implementation -------------------------------------------------------------

impl Vcs for GitVcs {
    fn name(&self) -> &'static str {
        "git"
    }

    fn resolve(&self, repo: &Path, rev: &str) -> Result<String> {
        self.try_resolve(repo, rev, "commit")?
            .ok_or_else(|| VcsError::NotFound(format!("revision `{rev}`")))
    }

    fn status(&self, repo: &Path) -> Result<Vec<StatusEntry>> {
        let out = self.run(
            repo,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        Ok(parse_status(&out))
    }

    fn diff_trees(&self, repo: &Path, from: Option<&str>, to: &str) -> Result<Vec<FileChange>> {
        check_arg("revision", to)?;
        let from = match from {
            Some(f) => {
                check_arg("revision", f)?;
                f.to_owned()
            }
            None => self.empty_tree(repo)?,
        };
        let base = ["diff-tree", "-r", "-z", "--no-renames", "--no-abbrev"];
        let raw = self.run(repo, &[&base[..], &["--raw", &from, to]].concat())?;
        let num = self.run(repo, &[&base[..], &["--numstat", &from, to]].concat())?;
        parse_changes(&raw, &num)
    }

    fn diff_workdir(&self, workdir: &Path, from: &str) -> Result<Vec<FileChange>> {
        let tree = self.workdir_tree(workdir)?;
        self.diff_trees(workdir, Some(from), &tree)
    }

    fn diff_patch(
        &self,
        repo: &Path,
        from: Option<&str>,
        to: &str,
        path: Option<&str>,
    ) -> Result<String> {
        check_arg("revision", to)?;
        let from = match from {
            Some(f) => {
                check_arg("revision", f)?;
                f.to_owned()
            }
            None => self.empty_tree(repo)?,
        };
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            &from,
            to,
        ];
        if let Some(p) = path {
            check_arg("path", p)?;
            args.push("--");
            args.push(p);
        }
        let out = self.run(repo, &args)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn log(&self, repo: &Path, opts: &LogOptions) -> Result<Vec<CommitInfo>> {
        let rev = opts.rev.as_deref().unwrap_or("HEAD");
        check_arg("revision", rev)?;
        let limit = opts.max_count.map(|n| format!("--max-count={n}"));
        let mut args = vec![
            "log",
            "--no-color",
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%b%x1e",
        ];
        if let Some(l) = &limit {
            args.push(l);
        }
        args.push(rev);
        if let Some(p) = &opts.path {
            check_arg("path", p)?;
            args.push("--");
            args.push(p);
        }
        let out = self.exec(repo, &args, &[], None)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            // A new repository has no commit yet: the log is empty.
            let unborn = rev == "HEAD"
                && (err.contains("does not have any commits yet")
                    || err.contains("unknown revision"));
            if unborn {
                return Ok(Vec::new());
            }
            return Err(Self::command_error(&args, &out));
        }
        Ok(parse_log(&out.stdout))
    }

    fn blame(
        &self,
        repo: &Path,
        rev: Option<&str>,
        path: &str,
        range: Option<(u32, u32)>,
    ) -> Result<Vec<BlameLine>> {
        check_arg("path", path)?;
        let range = range.map(|(a, b)| format!("{a},{b}"));
        let mut args = vec!["blame", "--porcelain"];
        if let Some(r) = &range {
            args.push("-L");
            args.push(r);
        }
        if let Some(rev) = rev {
            check_arg("revision", rev)?;
            args.push(rev);
        }
        args.push("--");
        args.push(path);
        let out = self.exec(repo, &args, &[], None)?;
        if out.status.success() {
            return parse_blame(&out.stdout);
        }
        // A new file that Git does not know has no history: every line is uncommitted.
        let err = String::from_utf8_lossy(&out.stderr);
        let unknown = err.contains("no such path") || err.contains("no such ref");
        if rev.is_none()
            && unknown
            && let Ok(bytes) = fs::read(repo.join(path))
        {
            return Ok(uncommitted_lines(&bytes, path, range.as_deref()));
        }
        Err(Self::command_error(&args, &out))
    }

    fn blame_contents(
        &self,
        repo: &Path,
        base_rev: &str,
        path: &str,
        contents: &[u8],
    ) -> Result<Vec<BlameLine>> {
        check_arg("revision", base_rev)?;
        check_arg("path", path)?;
        let tmp = tempfile::tempdir()?;
        let file = tmp.path().join("contents");
        fs::write(&file, contents)?;
        let file = file.to_string_lossy();
        let out = self.run(
            repo,
            &[
                "blame",
                "--porcelain",
                "--contents",
                &file,
                base_rev,
                "--",
                path,
            ],
        )?;
        parse_blame(&out)
    }

    fn show_file(&self, repo: &Path, rev: &str, path: &str) -> Result<Vec<u8>> {
        check_arg("revision", rev)?;
        check_arg("path", path)?;
        self.run(repo, &["cat-file", "blob", &format!("{rev}:{path}")])
    }

    fn refs(&self, repo: &Path, prefix: Option<&str>) -> Result<Vec<RefInfo>> {
        let mut args = vec!["for-each-ref", "--format=%(refname)%00%(objectname)"];
        if let Some(p) = prefix {
            check_arg("ref prefix", p)?;
            args.push(p);
        }
        let out = self.run(repo, &args)?;
        Ok(parse_refs(&out))
    }

    fn create_workspace(&self, repo: &Path, req: &WorkspaceRequest) -> Result<Workspace> {
        check_arg("workspace name", &req.name)?;
        if req.name.contains(['/', '\\']) {
            return Err(VcsError::Invalid(format!(
                "bad workspace name `{}`",
                req.name
            )));
        }
        fs::create_dir_all(&req.root)?;
        let path = req.root.join(&req.name);
        let path_str = path.to_string_lossy().into_owned();
        let mut args: Vec<&str> = vec!["worktree", "add"];
        if !req.sparse.is_empty() {
            args.push("--no-checkout");
        }
        if let Some(start) = &req.start_point {
            check_arg("revision", start)?;
        }
        match (&req.branch, req.create_branch) {
            (Some(b), true) => {
                check_arg("branch", b)?;
                args.extend(["-b", b.as_str(), path_str.as_str()]);
                if let Some(start) = &req.start_point {
                    args.push(start);
                }
            }
            (Some(b), false) => {
                check_arg("branch", b)?;
                args.extend([path_str.as_str(), b.as_str()]);
            }
            (None, _) => {
                args.extend(["--detach", path_str.as_str()]);
                if let Some(start) = &req.start_point {
                    args.push(start);
                }
            }
        }
        self.run(repo, &args)?;
        if !req.sparse.is_empty() {
            let mut sparse: Vec<&str> = vec!["sparse-checkout", "set", "--cone", "--"];
            for dir in &req.sparse {
                check_arg("sparse directory", dir)?;
                sparse.push(dir);
            }
            self.run(&path, &sparse)?;
            // Fill the empty index and the folder; the sparse rules limit the files.
            self.run(&path, &["read-tree", "-m", "-u", "HEAD"])?;
        }
        let head = self.text(&path, &["rev-parse", "HEAD"])?;
        let branch = self
            .exec(&path, &["symbolic-ref", "--short", "-q", "HEAD"], &[], None)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        Ok(Workspace { path, branch, head })
    }

    fn remove_workspace(&self, repo: &Path, workspace: &Path, force: bool) -> Result<()> {
        let path = workspace.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path);
        self.run(repo, &args)?;
        Ok(())
    }

    fn snapshot(&self, workdir: &Path, req: &SnapshotRequest) -> Result<Option<Snapshot>> {
        check_session(&req.session)?;
        let ref_name = snapshot_ref(&req.session, req.n);
        if self.try_resolve(workdir, &ref_name, "commit")?.is_some() {
            return Err(VcsError::Invalid(format!(
                "snapshot ref `{ref_name}` exists"
            )));
        }
        let parent = match &req.parent {
            Some(p) => Some(self.resolve(workdir, p)?),
            None => self
                .snapshot_before(workdir, &req.session, req.n)?
                .map(|s| s.commit),
        };
        let tree = self.workdir_tree(workdir)?;
        // The baseline is the start state of a session: it exists also when the tree equals its
        // parent (the base commit), so that a turn has a snapshot to return to.
        if req.n != BASELINE_TURN
            && let Some(p) = &parent
            && self.tree_of(workdir, p)? == tree
        {
            return Ok(None);
        }
        let date = req.time.map(|t| format!("@{t} +0000"));
        let mut env: Vec<(&str, &str)> = SNAPSHOT_IDENTITY.to_vec();
        if let Some(d) = &date {
            env.push(("GIT_AUTHOR_DATE", d));
            env.push(("GIT_COMMITTER_DATE", d));
        }
        let message = Self::snapshot_message(req);
        let mut args = vec!["-c", "commit.gpgsign=false", "commit-tree", &tree];
        if let Some(p) = &parent {
            args.extend(["-p", p]);
        }
        args.extend(["-F", "-"]);
        let out = self.run_env(workdir, &args, &env, Some(message.as_bytes()))?;
        let commit = String::from_utf8_lossy(&out).trim().to_owned();
        // The empty old value makes the update fail when the ref exists.
        self.run(
            workdir,
            &[
                "update-ref",
                "-m",
                "aivory turn snapshot",
                &ref_name,
                &commit,
                "",
            ],
        )?;
        Ok(Some(Snapshot {
            session: req.session.clone(),
            n: req.n,
            commit,
            tree,
            parent,
            ref_name,
        }))
    }

    fn restore(&self, workdir: &Path, rev: &str) -> Result<()> {
        let target = self.tree_of(workdir, rev)?;
        let current = self.workdir_tree(workdir)?;
        // Changes from the target to the current state.
        let changes = self.diff_trees(workdir, Some(&target), &current)?;
        // Files that exist only now go away. A type change goes away first, too.
        for c in &changes {
            if matches!(c.status, ChangeStatus::Added | ChangeStatus::TypeChanged) {
                remove_file_and_empty_dirs(workdir, &c.path)?;
            }
        }
        let paths: Vec<&str> = changes
            .iter()
            .filter(|c| c.status != ChangeStatus::Added)
            .map(|c| c.path.as_str())
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        let tmp = tempfile::tempdir()?;
        let idx = tmp.path().join("index");
        let idx_str = idx.to_string_lossy().into_owned();
        let env = [("GIT_INDEX_FILE", idx_str.as_str())];
        self.run_env(workdir, &["read-tree", &target], &env, None)?;
        let input = paths.join("\0");
        self.run_env(
            workdir,
            &["checkout-index", "-f", "-z", "--stdin"],
            &env,
            Some(input.as_bytes()),
        )?;
        Ok(())
    }

    fn list_snapshots(&self, repo: &Path, session: &str) -> Result<Vec<Snapshot>> {
        check_session(session)?;
        let prefix = format!("{TURN_REF_PREFIX}/{session}/");
        let out = self.run(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(tree)%00%(parent)",
                &prefix,
            ],
        )?;
        let text = String::from_utf8_lossy(&out);
        let mut snaps: Vec<Snapshot> = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\0');
                let name = f.next()?;
                let n = name.strip_prefix(&prefix)?.parse().ok()?;
                let commit = f.next()?.to_owned();
                let tree = f.next()?.to_owned();
                let parent = f
                    .next()
                    .and_then(|p| p.split_whitespace().next())
                    .map(str::to_owned);
                Some(Snapshot {
                    session: session.to_owned(),
                    n,
                    commit,
                    tree,
                    parent,
                    ref_name: name.to_owned(),
                })
            })
            .collect();
        snaps.sort_by_key(|s| s.n);
        Ok(snaps)
    }

    fn trial_merge(&self, repo: &Path, ours: &str, theirs: &str) -> Result<MergeOutcome> {
        check_arg("revision", ours)?;
        check_arg("revision", theirs)?;
        let args = [
            "merge-tree",
            "--write-tree",
            "-z",
            "--name-only",
            ours,
            theirs,
        ];
        let out = self.exec(repo, &args, &[], None)?;
        match out.status.code() {
            Some(0) => parse_merge_tree(&out.stdout, true),
            Some(1) => parse_merge_tree(&out.stdout, false),
            _ => Err(Self::command_error(&args, &out)),
        }
    }

    fn fetch(&self, repo: &Path, remote: &str) -> Result<()> {
        check_arg("remote", remote)?;
        self.run(repo, &["fetch", "--prune", remote])?;
        Ok(())
    }

    fn push_ref(&self, repo: &Path, remote: &str, refspec: &str) -> Result<()> {
        check_arg("remote", remote)?;
        check_arg("ref spec", refspec)?;
        self.run(repo, &["push", remote, refspec])?;
        Ok(())
    }

    fn list_remote_refs(&self, repo: &Path, remote: &str) -> Result<Vec<RefInfo>> {
        check_arg("remote", remote)?;
        let out = self.run(repo, &["ls-remote", remote])?;
        let text = String::from_utf8_lossy(&out);
        Ok(text
            .lines()
            .filter_map(|l| {
                let (id, name) = l.split_once('\t')?;
                Some(RefInfo {
                    name: name.to_owned(),
                    id: id.to_owned(),
                })
            })
            .collect())
    }

    fn publish_turns(&self, repo: &Path, req: &PublishRequest) -> Result<Option<PublishedCommit>> {
        check_arg("branch", &req.branch)?;
        let ref_name = format!("refs/heads/{}", req.branch);
        self.run(repo, &["check-ref-format", &ref_name])?;
        let snapshot = req
            .turns
            .iter()
            .rev()
            .find_map(|t| t.snapshot_ref.as_deref())
            .ok_or_else(|| VcsError::Invalid("no turn has a snapshot".into()))?;
        let tree = self.tree_of(repo, snapshot)?;
        let tip = self.try_resolve(repo, &ref_name, "commit")?;
        let parent = match (&tip, &req.base) {
            (Some(t), _) => Some(t.clone()),
            (None, Some(base)) => Some(self.resolve(repo, base)?),
            (None, None) => self.try_resolve(repo, "HEAD", "commit")?,
        };
        if let Some(p) = &parent
            && self.tree_of(repo, p)? == tree
        {
            return Ok(None);
        }
        let message = publish_message(req);
        let mut env: Vec<(&str, &str)> = Vec::new();
        if let Some((name, email)) = &req.author {
            env.extend([
                ("GIT_AUTHOR_NAME", name.as_str()),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_COMMITTER_NAME", name.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
            ]);
        }
        let mut args = vec!["commit-tree", &tree];
        if let Some(p) = &parent {
            args.extend(["-p", p]);
        }
        args.extend(["-F", "-"]);
        let out = self.run_env(repo, &args, &env, Some(message.as_bytes()))?;
        let commit = String::from_utf8_lossy(&out).trim().to_owned();
        // Compare-and-swap: the update fails when somebody moved the branch.
        let old = tip.as_deref().unwrap_or("");
        self.run(
            repo,
            &[
                "update-ref",
                "-m",
                "aivory publish",
                &ref_name,
                &commit,
                old,
            ],
        )?;
        if req.notes {
            let note = notes_text(req);
            self.run_env(
                repo,
                &["notes", "--ref=aivory", "add", "-f", "-F", "-", &commit],
                &env,
                Some(note.as_bytes()),
            )?;
        }
        if let Some(checkout) = &req.sync_checkout {
            let head = self.exec(checkout, &["symbolic-ref", "-q", "HEAD"], &[], None)?;
            if head.status.success() && String::from_utf8_lossy(&head.stdout).trim() == ref_name {
                // The branch moved under this checkout. Reset only the index to the new commit.
                // The files stay as they are.
                self.run(checkout, &["reset", "-q", "--mixed"])?;
            }
        }
        Ok(Some(PublishedCommit {
            commit,
            tree,
            parent,
            ref_name,
            message,
        }))
    }
}

fn parse_refs(raw: &[u8]) -> Vec<RefInfo> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|l| {
            let (name, id) = l.split_once('\0')?;
            Some(RefInfo {
                name: name.to_owned(),
                id: id.to_owned(),
            })
        })
        .collect()
}

fn uncommitted_lines(bytes: &[u8], path: &str, range: Option<&str>) -> Vec<BlameLine> {
    let (from, to) = range
        .and_then(|r| r.split_once(','))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .unwrap_or((1, u32::MAX));
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .enumerate()
        .filter_map(|(i, content)| {
            let line = u32::try_from(i + 1).ok()?;
            (line >= from && line <= to).then(|| BlameLine {
                line,
                orig_line: line,
                commit: "0".repeat(40),
                uncommitted: true,
                orig_path: path.to_owned(),
                time: 0,
                summary: String::new(),
                content: content.trim_end_matches('\r').to_owned(),
            })
        })
        .collect()
}

fn publish_message(req: &PublishRequest) -> String {
    let mut msg = req.subject.trim().to_owned();
    if let Some(body) = req.body.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        msg.push_str("\n\n");
        msg.push_str(body);
    }
    msg.push_str("\n\n");
    if let Some(task) = &req.task_id {
        msg.push_str(&format!("{TRAILER_TASK}: {task}\n"));
    }
    let turns: Vec<String> = req
        .turns
        .iter()
        .map(|t| format!("{}/{}", t.session_id, t.n))
        .collect();
    msg.push_str(&format!("{TRAILER_TURNS}: {}\n", turns.join(",")));
    let mut agents: Vec<String> = Vec::new();
    for t in &req.turns {
        let agent = match &t.model {
            Some(m) if !m.is_empty() => format!("{}/{m}", t.provider),
            _ => t.provider.clone(),
        };
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    msg.push_str(&format!("{TRAILER_AGENTS}: {}\n", agents.join(", ")));
    msg
}

/// The text of the Git note: one line per turn. It holds no prompt text.
fn notes_text(req: &PublishRequest) -> String {
    let mut text = String::new();
    for t in &req.turns {
        text.push_str(&format!(
            "{}/{} {} {}\n",
            t.session_id,
            t.n,
            t.provider,
            t.model.as_deref().unwrap_or("-")
        ));
    }
    text
}

/// Remove a file below `root`, then its parent folders when they are empty.
fn remove_file_and_empty_dirs(root: &Path, rel: &str) -> Result<()> {
    let path = root.join(rel);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // A read-only file blocks removal on Windows.
            if let Ok(meta) = fs::metadata(&path) {
                let mut perm = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                fs::set_permissions(&path, perm)?;
            }
            fs::remove_file(&path)?;
        }
        Err(e) => return Err(e.into()),
    }
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_check_accepts_normal_ids() {
        assert!(check_session("S-88").is_ok());
        assert!(check_session("a_b.c-1").is_ok());
        for bad in ["", "a/b", "..", "a..b", "-x", "x.lock", "a b", ".x"] {
            assert!(check_session(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn merge_tree_output_parses() {
        let raw = b"abc123\0f.txt\0g.txt\0\0\x31\0f.txt\0Auto-merging\0Auto-merging f.txt\n\0\
                    1\0f.txt\0CONFLICT (contents)\0CONFLICT (content): Merge conflict in f.txt\n\0";
        let m = parse_merge_tree(raw, false).unwrap();
        assert_eq!(m.tree, "abc123");
        assert_eq!(m.conflicts, ["f.txt", "g.txt"]);
        assert_eq!(m.messages.len(), 2);
        assert_eq!(m.messages[1].kind, "CONFLICT (contents)");
        assert_eq!(m.messages[1].paths, ["f.txt"]);
        let clean = parse_merge_tree(b"abc123\0\0", true).unwrap();
        assert!(clean.clean && clean.conflicts.is_empty() && clean.messages.is_empty());
    }

    #[test]
    fn status_output_parses_renames() {
        let raw = b"R  new.txt\0old.txt\0 M a.txt\0?? b.txt\0";
        let s = parse_status(raw);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].path, "new.txt");
        assert_eq!(s[0].orig_path.as_deref(), Some("old.txt"));
        assert_eq!((s[1].index, s[1].worktree), (' ', 'M'));
        assert_eq!(s[2].path, "b.txt");
    }

    #[test]
    fn changes_join_raw_and_numstat() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let z = "0".repeat(40);
        let raw = format!(
            ":000000 100644 {z} {a} A\0new.txt\0:100644 000000 {b} {z} D\0gone.txt\0\
             :100644 100644 {a} {b} M\0bin.dat\0"
        );
        let num = "3\t0\tnew.txt\0".to_owned() + "0\t2\tgone.txt\0-\t-\tbin.dat\0";
        let c = parse_changes(raw.as_bytes(), num.as_bytes()).unwrap();
        assert_eq!(c[0].added, 3);
        assert_eq!(c[0].blob_after.as_deref(), Some(a.as_str()));
        assert_eq!(c[1].status, ChangeStatus::Deleted);
        assert_eq!(c[1].blob_after, None);
        assert_eq!(c[1].removed, 2);
        assert!(c[2].binary);
    }

    #[test]
    fn log_output_parses_turn_trailer() {
        let rec = "id1\x1fp1 p2\x1fAda\x1fa@x\x1f1700000000\x1fsubject\x1fbody\n\n\
                   AIVory-Turns: S-88/7,S-88/8\n\x1e";
        let c = parse_log(rec.as_bytes());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].parents, ["p1", "p2"]);
        assert_eq!(c[0].turns.len(), 2);
        assert_eq!(c[0].turns[1].to_string(), "S-88/8");
    }
}
