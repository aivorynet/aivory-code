//! Mercurial backend of the sync layer.
//!
//! The WIP state is a draft changeset with the bookmark `aivory-wip/<node>/<session>`. The
//! changeset is made in a temporary `hg share` of the repository, so the working copy, the
//! parent, and the active bookmark of the user do not change.

use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::sync::{
    RepoIdentity, SyncVcs, VcsKind, WipRef, check_arg, check_rel, cmd_error, dirty_error, exec,
    join_rel, run,
};

const ENV: [(&str, &str); 2] = [
    ("HGPLAIN", "1"),
    ("HGUSER", "AIVory Code <wip@aivory.invalid>"),
];

/// The WIP commits (bookmarks `aivory-wip/...`). A checkout or an update never lands on one.
const NOT_WIP: &str = "- bookmark('re:^aivory-wip/')";

/// Every call turns on the `share` extension (`hg share` for workspaces); it ships with
/// Mercurial but is off by default.
fn with_ext<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut all = vec!["--config", "extensions.share="];
    all.extend_from_slice(args);
    all
}

/// The Mercurial backend of [`SyncVcs`].
#[derive(Debug, Clone, Default)]
pub struct HgSync;

impl HgSync {
    /// Use the `hg` binary from `PATH`.
    pub fn new() -> Self {
        Self
    }

    fn hg(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        run("hg", cwd, &with_ext(args), &ENV, None)
    }

    fn try_hg(&self, cwd: &Path, args: &[&str]) -> Option<String> {
        self.hg(cwd, args).ok().filter(|s| !s.is_empty())
    }

    fn node(&self, cwd: &Path, rev: &str) -> Result<String> {
        self.hg(cwd, &["log", "-r", rev, "--template", "{node}"])
    }

    /// The node of a bookmark, when it exists.
    fn bookmark_node(&self, cwd: &Path, name: &str) -> Option<String> {
        self.try_hg(
            cwd,
            &[
                "log",
                "-r",
                &format!("bookmark(\"{name}\")"),
                "--template",
                "{node}",
            ],
        )
    }

    /// Changed files as `(status letter, path)`.
    fn changes(&self, workdir: &Path) -> Result<Vec<(char, String)>> {
        let raw = self.hg(
            workdir,
            &[
                "--config",
                "ui.slash=True",
                "status",
                "-0",
                "-m",
                "-a",
                "-r",
                "-d",
                "-u",
            ],
        )?;
        Ok(raw
            .split('\0')
            .filter(|e| e.len() > 2)
            .filter_map(|e| Some((e.chars().next()?, e[2..].to_owned())))
            .collect())
    }
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| VcsError::Invalid(format!("path {} is not UTF-8", p.display())))
}

/// Run a command that may exit with 1 for "nothing to do".
fn run_allow_one(cwd: &Path, args: &[&str]) -> Result<()> {
    let out = exec("hg", cwd, &with_ext(args), &ENV, None)?;
    match out.status.code() {
        Some(0 | 1) => Ok(()),
        _ => Err(cmd_error("hg", args, &out)),
    }
}

impl SyncVcs for HgSync {
    fn kind(&self) -> VcsKind {
        VcsKind::Hg
    }

    fn identity(&self, workdir: &Path) -> Result<RepoIdentity> {
        let id = self.node(workdir, "0")?;
        let remote = self.try_hg(workdir, &["paths", "default"]);
        Ok(RepoIdentity {
            kind: VcsKind::Hg,
            id,
            remote,
        })
    }

    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
        check_arg("source", source)?;
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
        let cwd = parent.unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(cwd)?;
        let mut args = vec!["clone", "-q", "-U"];
        args.extend(["--", source, path_str(dest)?]);
        self.hg(cwd, &args)?;
        // Without a rev: the newest head of `default` that is not a WIP commit.
        let target = match rev {
            Some(rev) => {
                check_arg("rev", rev)?;
                rev.to_string()
            }
            None => format!("max(branch(default) {NOT_WIP})"),
        };
        if self
            .try_hg(dest, &["log", "-r", &target, "-T", "{node}"])
            .is_some()
        {
            self.hg(dest, &["update", "-q", "-r", &target])?;
        }
        Ok(())
    }

    fn update(&self, workdir: &Path) -> Result<bool> {
        if !self.is_clean(workdir)? {
            return Err(dirty_error(workdir));
        }
        let before = self.node(workdir, ".")?;
        if self.try_hg(workdir, &["paths", "default"]).is_none() {
            return Ok(false);
        }
        run_allow_one(workdir, &["pull", "-q"])?;
        // Fast-forward only: the target is a descendant of the working parent on its branch.
        let target = self.node(
            workdir,
            &format!("max(descendants(.) and branch(.) {NOT_WIP})"),
        )?;
        if target == before {
            return Ok(false);
        }
        self.hg(workdir, &["update", "-q", "-r", &target])?;
        Ok(true)
    }

    fn is_clean(&self, workdir: &Path) -> Result<bool> {
        Ok(self.hg(workdir, &["status"])?.is_empty())
    }

    fn head(&self, workdir: &Path) -> Result<String> {
        self.node(workdir, ".")
    }

    fn commit_wip(&self, workdir: &Path, wip: &WipRef, message: &str) -> Result<Option<String>> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Hg);
        let changes = self.changes(workdir)?;
        if changes.is_empty() {
            return Ok(None);
        }
        let base = self.node(workdir, ".")?;
        let tmp = tempfile::tempdir()?;
        let share = tmp.path().join("share");
        self.hg(
            workdir,
            &["share", "-U", "-B", path_str(workdir)?, path_str(&share)?],
        )?;
        self.hg(&share, &["update", "-q", "-r", &base])?;
        for (st, rel) in &changes {
            check_rel(rel)?;
            let dst = join_rel(&share, rel);
            match st {
                'R' | '!' => {
                    let _ = fs::remove_file(&dst);
                }
                _ => {
                    if let Some(p) = dst.parent() {
                        fs::create_dir_all(p)?;
                    }
                    fs::copy(join_rel(workdir, rel), &dst)?;
                }
            }
        }
        self.hg(&share, &["addremove", "-q"])?;
        if let Some(prev) = self.bookmark_node(workdir, &name)
            && self.hg(&share, &["diff", "-r", &prev])?.is_empty()
        {
            return Ok(None);
        }
        self.hg(&share, &["commit", "-m", message])?;
        let node = self.node(&share, ".")?;
        self.hg(&share, &["bookmark", "-f", "-r", &node, &name])?;
        Ok(Some(node))
    }

    fn push_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Hg);
        let mut args = vec!["push", "-q", "-B", name.as_str()];
        if let Some(r) = remote {
            check_arg("remote", r)?;
            args.push(r);
        }
        run_allow_one(workdir, &args)
    }

    fn fetch_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Hg);
        let mut args = vec!["pull", "-q", "-B", name.as_str()];
        if let Some(r) = remote {
            check_arg("remote", r)?;
            args.push(r);
        }
        self.hg(workdir, &args)?;
        Ok(())
    }

    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()> {
        wip.validate()?;
        let name = wip.backend_name(VcsKind::Hg);
        let node = self
            .bookmark_node(workdir, &name)
            .ok_or_else(|| VcsError::NotFound(format!("WIP bookmark {name}")))?;
        // Revert keeps the working parent. Files that the WIP state adds come back; files that
        // it lacks go away. Untracked files that the WIP state lacks stay.
        self.hg(workdir, &["revert", "--all", "--no-backup", "-r", &node])?;
        Ok(())
    }

    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool> {
        check_rel(rel_path)?;
        let out = self.hg(workdir, &["status", "-i", "-n", "--", rel_path])?;
        Ok(!out.is_empty())
    }

    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()> {
        self.hg(repo, &["share", "-U", path_str(repo)?, path_str(dest)?])?;
        let rev = match rev {
            Some(r) => {
                check_arg("rev", r)?;
                r.to_owned()
            }
            None => self.node(repo, ".")?,
        };
        self.hg(dest, &["update", "-q", "-r", &rev])?;
        Ok(())
    }

    fn remove_workspace(&self, _repo: &Path, dest: &Path) -> Result<()> {
        fs::remove_dir_all(dest)?;
        Ok(())
    }
}
