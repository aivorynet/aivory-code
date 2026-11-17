//! Jujutsu backend of the sync layer. It uses `jj` commands only, for colocated and for
//! plain jj repositories.
//!
//! The WIP state is a duplicate of the working-copy commit. A bookmark
//! `aivory/wip/<node>/<session>` names it. The duplicate is a sibling commit, so `@` and the
//! bookmarks of the user stay.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::sync::{RepoIdentity, SyncVcs, VcsKind, WipRef, check_arg, check_rel, dirty_error, run};

const ENV: [(&str, &str); 2] = [
    ("JJ_USER", "AIVory Code"),
    ("JJ_EMAIL", "wip@aivory.invalid"),
];
const COMMON: [&str; 3] = ["--no-pager", "--color", "never"];
const ID_LINES: &str = "commit_id ++ \"\\n\"";

/// The Jujutsu backend of [`SyncVcs`].
#[derive(Debug, Clone, Default)]
pub struct JjSync;

impl JjSync {
    /// Use the `jj` binary from `PATH`.
    pub fn new() -> Self {
        Self
    }

    fn jj(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut all: Vec<&str> = COMMON.to_vec();
        all.extend_from_slice(args);
        run("jj", cwd, &all, &ENV, None)
    }

    fn try_jj(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        self.jj(cwd, args).ok().filter(|s| !s.is_empty())
    }

    /// Lines of a template over a revset.
    fn log_lines(&self, cwd: &Path, revset: &str, template: &str) -> Result<Vec<String>> {
        let out = self.jj(cwd, &["log", "--no-graph", "-r", revset, "-T", template])?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Resolve the WIP bookmark: the local one, or the one that a fetch made.
    fn resolve(&self, cwd: &Path, name: &str) -> Option<String> {
        for revset in [format!("\"{name}\""), format!("\"{name}\"@origin")] {
            if let Ok(mut ids) = self.log_lines(cwd, &revset, ID_LINES)
                && ids.len() == 1
            {
                return ids.pop();
            }
        }
        None
    }

    fn remote_args<'a>(args: &mut Vec<&'a str>, remote: Option<&'a str>) -> Result<()> {
        if let Some(r) = remote {
            check_arg("remote", r)?;
            args.extend(["--remote", r]);
        }
        Ok(())
    }
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| VcsError::Invalid(format!("path {} is not UTF-8", p.display())))
}

impl SyncVcs for JjSync {
    fn kind(&self) -> VcsKind {
        VcsKind::Jj
    }

    fn identity(&self, workdir: &Path) -> Result<RepoIdentity> {
        let mut roots = self.log_lines(workdir, "roots((::@-) ~ root())", ID_LINES)?;
        roots.sort();
        let id = roots
            .into_iter()
            .next()
            .ok_or_else(|| VcsError::NotFound("repository has no commit".into()))?;
        let remote = self
            .try_jj(workdir, &["git", "remote", "list"])
            .and_then(|l| {
                l.lines()
                    .next()
                    .and_then(|first| first.split_whitespace().nth(1).map(str::to_owned))
            });
        Ok(RepoIdentity {
            kind: VcsKind::Jj,
            id,
            remote,
        })
    }

    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
        check_arg("source", source)?;
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
        let cwd = parent.unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(cwd)?;
        self.jj(cwd, &["git", "clone", "--", source, path_str(dest)?])?;
        if let Some(rev) = rev {
            check_arg("rev", rev)?;
            self.jj(dest, &["new", rev])?;
        }
        Ok(())
    }

    fn update(&self, workdir: &Path) -> Result<bool> {
        if !self.is_clean(workdir)? {
            return Err(dirty_error(workdir));
        }
        if self.jj(workdir, &["git", "remote", "list"])?.is_empty() {
            return Ok(false);
        }
        let before = self.head(workdir)?;
        self.jj(workdir, &["git", "fetch"])?;
        // Fast-forward only: a bookmarked descendant of the parent of `@`.
        let targets = self.log_lines(
            workdir,
            "heads((descendants(@-) & bookmarks()) ~ @)",
            ID_LINES,
        )?;
        match targets.as_slice() {
            [] => Ok(false),
            [t] if *t == before => Ok(false),
            [t] => {
                self.jj(workdir, &["new", t])?;
                Ok(true)
            }
            _ => Err(VcsError::Invalid(
                "bookmarks diverge; no fast-forward".into(),
            )),
        }
    }

    fn is_clean(&self, workdir: &Path) -> Result<bool> {
        // Any jj command snapshots the working copy first.
        let v = self.jj(workdir, &["log", "--no-graph", "-r", "@", "-T", "empty"])?;
        Ok(v.trim() == "true")
    }

    fn head(&self, workdir: &Path) -> Result<String> {
        self.log_lines(workdir, "@-", ID_LINES)?
            .into_iter()
            .next()
            .ok_or_else(|| VcsError::NotFound("no parent of the working copy".into()))
    }

    fn commit_wip(&self, workdir: &Path, wip: &WipRef, message: &str) -> Result<Option<String>> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Jj);
        let unchanged = match self.resolve(workdir, &name) {
            Some(prev) => self
                .jj(
                    workdir,
                    &["diff", "--summary", "--from", &prev, "--to", "@"],
                )?
                .is_empty(),
            None => self.is_clean(workdir)?,
        };
        if unchanged {
            return Ok(None);
        }
        let template = "change_id ++ \"\\n\"";
        let before: HashSet<String> = self
            .log_lines(workdir, "children(@-)", template)?
            .into_iter()
            .collect();
        self.jj(workdir, &["duplicate", "@"])?;
        let fresh = self
            .log_lines(workdir, "children(@-)", template)?
            .into_iter()
            .find(|c| !before.contains(c))
            .ok_or_else(|| VcsError::Parse("jj duplicate made no commit".into()))?;
        self.jj(workdir, &["describe", "-r", &fresh, "-m", message])?;
        let commit = self
            .log_lines(workdir, &fresh, ID_LINES)?
            .into_iter()
            .next()
            .ok_or_else(|| VcsError::Parse("WIP commit not found".into()))?;
        self.jj(
            workdir,
            &["bookmark", "set", &name, "--allow-backwards", "-r", &commit],
        )?;
        Ok(Some(commit))
    }

    fn push_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Jj);
        let mut args = vec!["git", "push", "-b", name.as_str()];
        Self::remote_args(&mut args, remote)?;
        let mut with_new = args.clone();
        with_new.push("--allow-new");
        match self.jj(workdir, &with_new) {
            Err(VcsError::Command { stderr, .. }) if stderr.contains("unexpected argument") => {
                // A newer jj has no `--allow-new`.
                self.jj(workdir, &args)?;
                Ok(())
            }
            other => other.map(|_| ()),
        }
    }

    fn fetch_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Jj);
        let mut args = vec!["git", "fetch", "-b", name.as_str()];
        Self::remote_args(&mut args, remote)?;
        self.jj(workdir, &args)?;
        Ok(())
    }

    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Jj);
        let commit = self
            .resolve(workdir, &name)
            .ok_or_else(|| VcsError::NotFound(format!("WIP bookmark {name}")))?;
        self.jj(workdir, &["restore", "--from", &commit])?;
        Ok(())
    }

    /// jj has no ignore query. A file that exists, and that the snapshot did not track, is
    /// ignored (new files are tracked by default).
    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool> {
        check_rel(rel_path)?;
        if !workdir.join(rel_path).exists() {
            return Ok(false);
        }
        let fileset = format!("root-file:\"{rel_path}\"");
        let listed = self.jj(workdir, &["file", "list", "-r", "@", &fileset])?;
        Ok(listed.is_empty())
    }

    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()> {
        let mut args = vec!["workspace", "add"];
        if let Some(r) = rev {
            check_arg("rev", r)?;
            args.extend(["-r", r]);
        }
        args.push(path_str(dest)?);
        self.jj(repo, &args)?;
        Ok(())
    }

    fn remove_workspace(&self, repo: &Path, dest: &Path) -> Result<()> {
        let name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| VcsError::Invalid("bad workspace path".into()))?;
        self.jj(repo, &["workspace", "forget", name])?;
        fs::remove_dir_all(dest)?;
        Ok(())
    }
}
