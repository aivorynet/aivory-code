//! Git backend of the sync layer.
//!
//! The WIP state is a hidden commit on `refs/aivory/wip/<node>/<session>`. It comes from a
//! temporary index file, so the index, `HEAD`, and the branch of the user do not change.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::sync::{
    RepoIdentity, SyncVcs, VcsKind, WipRef, check_arg, check_rel, cmd_error, dirty_error, exec, run,
};

const AUTHOR_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "AIVory Code"),
    ("GIT_AUTHOR_EMAIL", "wip@aivory.invalid"),
    ("GIT_COMMITTER_NAME", "AIVory Code"),
    ("GIT_COMMITTER_EMAIL", "wip@aivory.invalid"),
];

/// The git backend of [`SyncVcs`].
#[derive(Debug, Clone, Default)]
pub struct GitSync;

impl GitSync {
    /// Use the `git` binary from `PATH`.
    pub fn new() -> Self {
        Self
    }

    fn git(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
        let mut all = vec![("GIT_TERMINAL_PROMPT", "0")];
        all.extend_from_slice(env);
        run("git", cwd, args, &all, None)
    }

    /// stdout, or `None` when git exits with an error.
    fn try_git(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        self.git(cwd, args, &[]).ok().filter(|s| !s.is_empty())
    }

    fn toplevel(&self, workdir: &Path) -> Result<std::path::PathBuf> {
        Ok(self
            .git(workdir, &["rev-parse", "--show-toplevel"], &[])?
            .into())
    }
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| VcsError::Invalid(format!("path {} is not UTF-8", p.display())))
}

fn remove_with_empty_dirs(root: &Path, rel: &str) {
    let mut p = root.join(rel);
    let _ = fs::remove_file(&p);
    while p.pop() && p != root {
        if fs::remove_dir(&p).is_err() {
            break;
        }
    }
}

impl SyncVcs for GitSync {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn identity(&self, workdir: &Path) -> Result<RepoIdentity> {
        let roots = self.git(workdir, &["rev-list", "--max-parents=0", "HEAD"], &[])?;
        let id = roots
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .min()
            .ok_or_else(|| VcsError::NotFound("repository has no commit".into()))?
            .to_owned();
        let remote = self.try_git(workdir, &["config", "--get", "remote.origin.url"]);
        Ok(RepoIdentity {
            kind: VcsKind::Git,
            id,
            remote,
        })
    }

    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
        check_arg("source", source)?;
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
        let cwd = parent.unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(cwd)?;
        self.git(
            cwd,
            &["clone", "--quiet", "--", source, path_str(dest)?],
            &[],
        )?;
        if let Some(rev) = rev {
            check_arg("rev", rev)?;
            self.git(dest, &["checkout", "--quiet", "--detach", rev], &[])?;
        }
        Ok(())
    }

    fn update(&self, workdir: &Path) -> Result<bool> {
        if !self.is_clean(workdir)? {
            return Err(dirty_error(workdir));
        }
        if self.git(workdir, &["remote"], &[])?.is_empty() {
            return Ok(false);
        }
        let before = self.head(workdir)?;
        self.git(workdir, &["fetch", "--quiet"], &[])?;
        if self
            .try_git(workdir, &["rev-parse", "--verify", "-q", "@{u}^{commit}"])
            .is_none()
        {
            return Ok(false);
        }
        self.git(workdir, &["merge", "--ff-only", "--quiet", "@{u}"], &[])?;
        Ok(self.head(workdir)? != before)
    }

    fn is_clean(&self, workdir: &Path) -> Result<bool> {
        Ok(self
            .git(workdir, &["status", "--porcelain"], &[])?
            .is_empty())
    }

    fn head(&self, workdir: &Path) -> Result<String> {
        self.git(workdir, &["rev-parse", "--verify", "HEAD"], &[])
    }

    fn commit_wip(&self, workdir: &Path, wip: &WipRef, message: &str) -> Result<Option<String>> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Git);
        let head = self.try_git(workdir, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]);
        let tmp = tempfile::tempdir()?;
        let idx = tmp.path().join("index");
        let idx_s = path_str(&idx)?;
        let mut env: Vec<(&str, &str)> = AUTHOR_ENV.to_vec();
        env.push(("GIT_INDEX_FILE", idx_s));
        if let Some(h) = &head {
            self.git(workdir, &["read-tree", h], &env)?;
        }
        self.git(workdir, &["add", "-A"], &env)?;
        let tree = self.git(workdir, &["write-tree"], &env)?;
        // Compare with the last WIP state, or else with HEAD, or else with the empty tree.
        let prev = self.try_git(
            workdir,
            &["rev-parse", "--verify", "-q", &format!("{name}^{{tree}}")],
        );
        let head_tree = head
            .as_ref()
            .and_then(|_| self.try_git(workdir, &["rev-parse", "--verify", "-q", "HEAD^{tree}"]));
        let reference = match prev.or(head_tree) {
            Some(t) => t,
            None => run("git", workdir, &["mktree"], &[], Some(b""))?,
        };
        if tree == reference {
            return Ok(None);
        }
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(h) = &head {
            args.extend(["-p", h.as_str()]);
        }
        let commit = self.git(workdir, &args, &env)?;
        self.git(workdir, &["update-ref", &name, &commit], &[])?;
        Ok(Some(commit))
    }

    fn push_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let remote = remote.unwrap_or("origin");
        check_arg("remote", remote)?;
        let name = wip.backend_name(VcsKind::Git);
        self.git(
            workdir,
            &["push", "--quiet", remote, &format!("+{name}:{name}")],
            &[],
        )?;
        Ok(())
    }

    fn fetch_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let remote = remote.unwrap_or("origin");
        check_arg("remote", remote)?;
        let name = wip.backend_name(VcsKind::Git);
        self.git(
            workdir,
            &["fetch", "--quiet", remote, &format!("+{name}:{name}")],
            &[],
        )?;
        Ok(())
    }

    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Git);
        let commit = self
            .try_git(
                workdir,
                &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")],
            )
            .ok_or_else(|| VcsError::NotFound(format!("WIP ref {name}")))?;
        let top = self.toplevel(workdir)?;
        let tmp = tempfile::tempdir()?;
        let idx = tmp.path().join("index");
        let env = [("GIT_INDEX_FILE", path_str(&idx)?)];
        self.git(&top, &["read-tree", &commit], &env)?;
        let wanted: HashSet<String> = self
            .git(&top, &["ls-tree", "-r", "-z", "--name-only", &commit], &[])?
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        // Files that exist now but not in the WIP state go away.
        let current = self.git(
            &top,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
            &[],
        )?;
        for f in current.split('\0').filter(|s| !s.is_empty()) {
            if !wanted.contains(f) {
                remove_with_empty_dirs(&top, f);
            }
        }
        self.git(&top, &["checkout-index", "-a", "-f"], &env)?;
        // The real index returns to HEAD; the WIP files show as changes.
        if self
            .try_git(&top, &["rev-parse", "--verify", "-q", "HEAD^{commit}"])
            .is_some()
        {
            self.git(&top, &["reset", "--quiet"], &[])?;
        } else {
            self.git(&top, &["read-tree", "--empty"], &[])?;
        }
        Ok(())
    }

    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool> {
        check_rel(rel_path)?;
        let args = ["check-ignore", "-q", "--", rel_path];
        let out = exec("git", workdir, &args, &[], None)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(cmd_error("git", &args, &out)),
        }
    }

    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()> {
        let rev = rev.unwrap_or("HEAD");
        check_arg("rev", rev)?;
        self.git(
            repo,
            &["worktree", "add", "--detach", path_str(dest)?, rev],
            &[],
        )?;
        Ok(())
    }

    fn remove_workspace(&self, repo: &Path, dest: &Path) -> Result<()> {
        self.git(
            repo,
            &["worktree", "remove", "--force", path_str(dest)?],
            &[],
        )?;
        Ok(())
    }
}
