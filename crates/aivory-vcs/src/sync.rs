//! Backend-neutral sync layer of the machine mesh (ADR 0026, section "VCS layer").
//!
//! [`SyncVcs`] gives the daemon one view of git, Mercurial, Jujutsu, Subversion and Perforce:
//! repository identity, clone, fast-forward update, and WIP states that travel between nodes.
//! Each backend runs the CLI of its tool. [`detect`] picks the backend for a path.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VcsError};
use crate::git_sync::GitSync;
use crate::hg::HgSync;
use crate::jj::JjSync;
use crate::p4::P4Sync;
use crate::svn::SvnSync;

/// The kind of a version control system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VcsKind {
    /// Git.
    Git,
    /// Mercurial.
    Hg,
    /// Jujutsu.
    Jj,
    /// Subversion.
    Svn,
    /// Perforce.
    P4,
}

/// The identity of a repository. Two checkouts of one repository have the same `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoIdentity {
    /// The kind of VCS.
    pub kind: VcsKind,
    /// A stable id: git and jj root commit, hg revision 0, svn repository UUID, p4 server + view.
    pub id: String,
    /// The default remote, when there is one.
    pub remote: Option<String>,
}

/// The name of a WIP state: one per node and session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipRef {
    /// The node (machine) that wrote the state.
    pub node: String,
    /// The session that the state belongs to.
    pub session: String,
}

impl WipRef {
    /// Make a WIP name. A part has only ASCII letters, digits, `.`, `_` and `-`.
    pub fn new(node: impl Into<String>, session: impl Into<String>) -> Result<Self> {
        let wip = Self {
            node: node.into(),
            session: session.into(),
        };
        wip.validate()?;
        Ok(wip)
    }

    /// Check both parts.
    pub fn validate(&self) -> Result<()> {
        for (kind, v) in [("node", &self.node), ("session", &self.session)] {
            let good = !v.is_empty()
                && !v.starts_with(['-', '.'])
                && !v.contains("..")
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if !good {
                return Err(VcsError::Invalid(format!("bad WIP {kind} `{v}`")));
            }
        }
        Ok(())
    }

    /// The name in the backend: git ref, hg bookmark, jj bookmark, p4 shelf description, or
    /// for svn the path of the shadow folder, relative to the checkout.
    pub fn backend_name(&self, kind: VcsKind) -> String {
        let (n, s) = (&self.node, &self.session);
        match kind {
            VcsKind::Git => format!("refs/aivory/wip/{n}/{s}"),
            VcsKind::Hg => format!("aivory-wip/{n}/{s}"),
            VcsKind::Jj => format!("aivory/wip/{n}/{s}"),
            VcsKind::P4 => format!("aivory-wip {n} {s}"),
            VcsKind::Svn => format!(".svn/aivory-wip/{n}/{s}"),
        }
    }
}

/// Backend-neutral operations for the machine mesh. All paths are checkout roots (or a folder
/// inside one). A method never moves the user's branch or working copy, unless the text says so.
pub trait SyncVcs: Send + Sync + fmt::Debug {
    /// The kind of this backend.
    fn kind(&self) -> VcsKind;
    /// The identity of the repository of the checkout.
    fn identity(&self, workdir: &Path) -> Result<RepoIdentity>;
    /// Clone or check out `source` into `dest`, at `rev` when given.
    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()>;
    /// Pull and fast-forward. Refuses a dirty checkout. Returns whether anything changed.
    fn update(&self, workdir: &Path) -> Result<bool>;
    /// True when the working copy has no local change (new files that are not ignored count).
    fn is_clean(&self, workdir: &Path) -> Result<bool>;
    /// The id of the current revision.
    fn head(&self, workdir: &Path) -> Result<String>;
    /// Save the working copy as the WIP state. `None` when nothing changed since the last save.
    /// It does not move the user's branch, index, or working copy state.
    fn commit_wip(&self, workdir: &Path, wip: &WipRef, message: &str) -> Result<Option<String>>;
    /// Send the WIP state to the remote (default remote when `None`).
    fn push_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()>;
    /// Get the WIP state from the remote (default remote when `None`).
    fn fetch_wip(&self, workdir: &Path, wip: &WipRef, remote: Option<&str>) -> Result<()>;
    /// Make the working copy equal to the WIP state. The branch of the user stays.
    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()>;
    /// True when the VCS ignores `rel_path`.
    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool>;
    /// Make a second working copy of `repo` at `dest`, at `rev` when given.
    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()>;
    /// Remove a working copy that `create_workspace` made.
    fn remove_workspace(&self, repo: &Path, dest: &Path) -> Result<()>;
}

/// Find the backend for `path`. Walks up the folders. At each level `.jj` wins over `.git`,
/// then `.hg`, then `.svn`. Perforce comes last: a `P4CONFIG` file, or a client root of
/// `p4 -ztag info` that contains the path (only when `p4` is on `PATH`).
pub fn detect(path: &Path) -> Option<Box<dyn SyncVcs>> {
    let start = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut dir = Some(start.as_path());
    while let Some(d) = dir {
        if d.join(".jj").exists() {
            return Some(Box::new(JjSync::new()));
        }
        if d.join(".git").exists() {
            return Some(Box::new(GitSync::new()));
        }
        if d.join(".hg").exists() {
            return Some(Box::new(HgSync::new()));
        }
        if d.join(".svn").exists() {
            return Some(Box::new(SvnSync::new()));
        }
        dir = d.parent();
    }
    if P4Sync::covers(&start) {
        return Some(Box::new(P4Sync::new()));
    }
    None
}

// ---- Shared helpers of the backends -------------------------------------------------------

/// Run a CLI. Returns the raw output, also for a non-zero exit code.
pub(crate) fn exec(
    bin: &str,
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(data), Some(mut pipe)) => {
            let data = data.to_vec();
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

pub(crate) fn cmd_error(bin: &str, args: &[&str], out: &Output) -> VcsError {
    VcsError::Command {
        command: format!("{bin} {}", args.join(" ")),
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    }
}

/// Run a CLI and return stdout without the trailing newline. A non-zero exit code is an error.
pub(crate) fn run(
    bin: &str,
    cwd: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> Result<String> {
    let out = exec(bin, cwd, args, env, stdin)?;
    if !out.status.success() {
        return Err(cmd_error(bin, args, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

/// True when the tool starts with `--version` (or the given probe arguments).
pub(crate) fn tool_present(bin: &str, probe: &[&str]) -> bool {
    Command::new(bin)
        .args(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reject an argument that a CLI could read as an option.
pub(crate) fn check_arg(kind: &str, value: &str) -> Result<()> {
    crate::vcs::check_arg(kind, value)
}

/// Reject a relative path that leaves the checkout.
pub(crate) fn check_rel(rel: &str) -> Result<()> {
    check_arg("path", rel)?;
    let p = Path::new(rel);
    let bad = p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)));
    if bad {
        return Err(VcsError::Invalid(format!("bad relative path `{rel}`")));
    }
    Ok(())
}

pub(crate) fn dirty_error(workdir: &Path) -> VcsError {
    VcsError::Invalid(format!(
        "checkout {} has local changes; update refused",
        workdir.display()
    ))
}

/// FNV-1a, 64 bit. A fingerprint to detect "nothing changed", not a security hash.
#[derive(Debug)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub(crate) fn feed(&mut self, data: &[u8]) {
        for b in data {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // A separator, so that ("ab","c") differs from ("a","bc").
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }
    pub(crate) fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Join a relative path with `/` or `\` onto a folder.
pub(crate) fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, c| p.join(c))
}
