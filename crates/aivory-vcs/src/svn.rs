//! Subversion backend of the sync layer.
//!
//! Subversion has no hidden refs. The WIP state is a shadow store in
//! `<checkout>/.svn/aivory-wip/<node>/<session>/`:
//!
//! - `base`: the base revision,
//! - `patch.diff`: the output of `svn diff`,
//! - `manifest`: one `A|M|D <path>` line for each file that the patch cannot carry,
//! - `files/<path>`: copies of the added, unversioned, and binary files,
//! - `id`: a fingerprint of all of the above.

use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::sync::{
    Fnv, RepoIdentity, SyncVcs, VcsKind, WipRef, check_arg, check_rel, dirty_error, join_rel, run,
};

/// The Subversion backend of [`SyncVcs`].
#[derive(Debug, Clone, Default)]
pub struct SvnSync;

impl SvnSync {
    /// Use the `svn` binary from `PATH`.
    pub fn new() -> Self {
        Self
    }

    fn svn(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut all = vec!["--non-interactive"];
        all.extend_from_slice(args);
        run("svn", cwd, &all, &[], None)
    }

    fn info(&self, cwd: &Path, item: &str) -> Result<String> {
        Ok(self
            .svn(cwd, &["info", "--show-item", item])?
            .trim()
            .to_owned())
    }

    /// `(status letter, path)` of each changed or unversioned item.
    fn status(&self, workdir: &Path) -> Result<Vec<(char, String)>> {
        let raw = self.svn(workdir, &["status"])?;
        let mut v: Vec<(char, String)> = raw
            .lines()
            .filter(|l| l.len() > 8)
            .filter_map(|l| {
                let c = l.chars().next()?;
                "MAD?!R~"
                    .contains(c)
                    .then(|| (c, l[8..].trim().replace('\\', "/")))
            })
            .collect();
        v.sort();
        Ok(v)
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// All files below a path, as paths relative to the checkout.
fn files_below(root: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let p = join_rel(root, rel);
    if p.is_dir() {
        for e in fs::read_dir(&p)? {
            let name = e?.file_name();
            let name = name.to_string_lossy();
            if name != ".svn" {
                files_below(root, &format!("{rel}/{name}"), out)?;
            }
        }
    } else if p.exists() {
        out.push(rel.to_owned());
    }
    Ok(())
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| VcsError::Invalid(format!("path {} is not UTF-8", p.display())))
}

impl SyncVcs for SvnSync {
    fn kind(&self) -> VcsKind {
        VcsKind::Svn
    }

    fn identity(&self, workdir: &Path) -> Result<RepoIdentity> {
        Ok(RepoIdentity {
            kind: VcsKind::Svn,
            id: self.info(workdir, "repos-uuid")?,
            remote: self.info(workdir, "repos-root-url").ok(),
        })
    }

    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
        check_arg("source", source)?;
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
        let cwd = parent.unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(cwd)?;
        let mut args = vec!["checkout", "--quiet"];
        if let Some(rev) = rev {
            check_arg("rev", rev)?;
            args.extend(["-r", rev]);
        }
        args.extend([source, path_str(dest)?]);
        self.svn(cwd, &args)?;
        Ok(())
    }

    fn update(&self, workdir: &Path) -> Result<bool> {
        if !self.is_clean(workdir)? {
            return Err(dirty_error(workdir));
        }
        let before = self.head(workdir)?;
        self.svn(workdir, &["update", "--quiet"])?;
        Ok(self.head(workdir)? != before)
    }

    fn is_clean(&self, workdir: &Path) -> Result<bool> {
        Ok(self.status(workdir)?.is_empty())
    }

    fn head(&self, workdir: &Path) -> Result<String> {
        self.info(workdir, "revision")
    }

    fn commit_wip(&self, workdir: &Path, wip: &WipRef, _message: &str) -> Result<Option<String>> {
        wip.validate()?;
        let entries = self.status(workdir)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let base = self.head(workdir)?;
        let patch = self.svn(workdir, &["diff"])?;
        let mut manifest: Vec<String> = Vec::new();
        let mut copies: Vec<String> = Vec::new();
        for (st, rel) in &entries {
            check_rel(rel)?;
            match st {
                'D' => {}
                '!' => manifest.push(format!("D {rel}")),
                'M' => {
                    let p = join_rel(workdir, rel);
                    if p.is_file() && is_binary(&fs::read(p)?) {
                        manifest.push(format!("M {rel}"));
                        copies.push(rel.clone());
                    }
                }
                _ => {
                    let mut found = Vec::new();
                    files_below(workdir, rel, &mut found)?;
                    for f in found {
                        manifest.push(format!("A {f}"));
                        copies.push(f);
                    }
                }
            }
        }
        manifest.sort();
        copies.sort();
        copies.dedup();
        let mut fnv = Fnv::new();
        fnv.feed(base.as_bytes());
        fnv.feed(patch.as_bytes());
        for m in &manifest {
            fnv.feed(m.as_bytes());
        }
        for c in &copies {
            fnv.feed(c.as_bytes());
            fnv.feed(&fs::read(join_rel(workdir, c))?);
        }
        let id = fnv.hex();
        let store = join_rel(workdir, &wip.backend_name(VcsKind::Svn));
        if fs::read_to_string(store.join("id")).is_ok_and(|old| old.trim() == id) {
            return Ok(None);
        }
        if store.exists() {
            fs::remove_dir_all(&store)?;
        }
        fs::create_dir_all(store.join("files"))?;
        fs::write(store.join("base"), &base)?;
        fs::write(store.join("patch.diff"), &patch)?;
        fs::write(store.join("manifest"), manifest.join("\n"))?;
        for c in &copies {
            let dst = join_rel(&store.join("files"), c);
            if let Some(p) = dst.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(join_rel(workdir, c), dst)?;
        }
        fs::write(store.join("id"), &id)?;
        Ok(Some(id))
    }

    /// No-op. Subversion WIP travels peer to peer only: a peer reads the shadow store of the
    /// checkout directly. The Subversion server has no place for hidden WIP data.
    fn push_wip(&self, _workdir: &Path, _wip: &WipRef, _remote: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// No-op. See [`SvnSync::push_wip`](SyncVcs::push_wip).
    fn fetch_wip(&self, _workdir: &Path, _wip: &WipRef, _remote: Option<&str>) -> Result<()> {
        Ok(())
    }

    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()> {
        wip.validate()?;
        let store = join_rel(workdir, &wip.backend_name(VcsKind::Svn));
        let base = fs::read_to_string(store.join("base"))
            .map_err(|_| VcsError::NotFound(format!("WIP store {}", store.display())))?;
        let base = base.trim();
        // A clean checkout at the base revision first.
        self.svn(workdir, &["revert", "--recursive", "--quiet", "."])?;
        for (st, rel) in self.status(workdir)? {
            if st == '?' {
                check_rel(&rel)?;
                let p = join_rel(workdir, &rel);
                if p.is_dir() {
                    fs::remove_dir_all(p)?;
                } else {
                    fs::remove_file(p)?;
                }
            }
        }
        self.svn(workdir, &["update", "--quiet", "-r", base])?;
        let patch = store.join("patch.diff");
        if fs::metadata(&patch)?.len() > 0 {
            self.svn(workdir, &["patch", "--quiet", path_str(&patch)?])?;
        }
        let manifest = fs::read_to_string(store.join("manifest"))?;
        for line in manifest.lines() {
            let Some((op, rel)) = line.split_once(' ') else {
                continue;
            };
            check_rel(rel)?;
            match op {
                "D" => {
                    self.svn(workdir, &["rm", "--force", "--quiet", rel])?;
                }
                _ => {
                    let dst = join_rel(workdir, rel);
                    if let Some(p) = dst.parent() {
                        fs::create_dir_all(p)?;
                    }
                    fs::copy(join_rel(&store.join("files"), rel), &dst)?;
                    if op == "A" {
                        self.svn(workdir, &["add", "--force", "--parents", "--quiet", rel])?;
                    }
                }
            }
        }
        Ok(())
    }

    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool> {
        check_rel(rel_path)?;
        let out = self.svn(workdir, &["status", "--no-ignore", rel_path])?;
        Ok(out.lines().any(|l| l.starts_with('I')))
    }

    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()> {
        let url = self.info(repo, "url")?;
        self.clone_or_checkout(&url, dest, rev)
    }

    fn remove_workspace(&self, _repo: &Path, dest: &Path) -> Result<()> {
        fs::remove_dir_all(dest)?;
        Ok(())
    }
}
