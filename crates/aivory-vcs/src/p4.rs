//! Perforce backend of the sync layer.
//!
//! The WIP state is a shelved pending changelist. Its description is
//! `aivory-wip <node> <session>`; a second line `fp:<hex>` holds a fingerprint of the files,
//! so an unchanged working copy gives `None`. A shelf lives on the server, so `push_wip` and
//! `fetch_wip` need no work.
//!
//! A checkout is a client with a config file (`P4CONFIG`, default `.p4config`) in its root. The
//! `P4PORT` and `P4USER` of the process, or of [`P4Sync::with_port`] and [`P4Sync::with_user`],
//! go into that file when `clone_or_checkout` makes the client.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::error::{Result, VcsError};
use crate::sync::{
    Fnv, RepoIdentity, SyncVcs, VcsKind, WipRef, check_arg, check_rel, cmd_error, dirty_error,
    exec, run, tool_present,
};

const DEFAULT_CONFIG: &str = ".p4config";

/// The Perforce backend of [`SyncVcs`].
#[derive(Debug, Clone, Default)]
pub struct P4Sync {
    port: Option<String>,
    user: Option<String>,
}

type Record = Vec<(String, String)>;

fn config_name() -> String {
    std::env::var("P4CONFIG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_CONFIG.to_owned())
}

/// Parse `p4 -ztag` output: `... key value` lines; a blank line ends a record.
fn parse_ztag(text: &str) -> Vec<Record> {
    let mut out = Vec::new();
    let mut cur: Record = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("... ") {
            let (k, v) = rest.split_once(' ').unwrap_or((rest, ""));
            cur.push((k.to_owned(), v.to_owned()));
        } else if line.trim().is_empty() && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn field<'a>(rec: &'a Record, key: &str) -> Option<&'a str> {
    rec.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn is_config_file(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    name == config_name()
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| VcsError::Invalid(format!("path {} is not UTF-8", p.display())))
}

fn client_name_for(dest: &Path) -> String {
    let mut f = Fnv::new();
    f.feed(dest.to_string_lossy().as_bytes());
    format!("aivory-{}", f.hex())
}

impl P4Sync {
    /// Use the `p4` binary from `PATH` and the environment of the process.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a `P4PORT` for new clients.
    pub fn with_port(mut self, port: impl Into<String>) -> Self {
        self.port = Some(port.into());
        self
    }

    /// Use a `P4USER` for new clients.
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// True when `path` belongs to a Perforce client: a config file above it, or a client root
    /// of `p4 -ztag info` that contains it. Needs `p4` on `PATH` for the second test.
    pub(crate) fn covers(path: &Path) -> bool {
        let name = config_name();
        let mut dir = Some(path);
        while let Some(d) = dir {
            if d.join(&name).is_file() {
                return true;
            }
            dir = d.parent();
        }
        if !tool_present("p4", &["-V"]) {
            return false;
        }
        let cwd = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let Ok(out) = run("p4", cwd, &["-ztag", "info"], &[], None) else {
            return false;
        };
        parse_ztag(&out)
            .first()
            .and_then(|r| field(r, "clientRoot"))
            .is_some_and(|root| {
                let root = fs::canonicalize(root).unwrap_or_else(|_| root.into());
                path.starts_with(root)
            })
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut v = vec![("P4CONFIG".to_owned(), config_name())];
        if let Some(p) = &self.port {
            v.push(("P4PORT".into(), p.clone()));
        }
        if let Some(u) = &self.user {
            v.push(("P4USER".into(), u.clone()));
        }
        v
    }

    fn p4_in(&self, cwd: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        let env = self.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        run("p4", cwd, args, &env, stdin)
    }

    fn p4(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        self.p4_in(cwd, args, None)
    }

    /// Files that `p4 reconcile` would open: `(depot file, local file, action)`.
    fn pending(&self, workdir: &Path) -> Result<Vec<(String, String, String)>> {
        let env = self.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let args = ["-ztag", "reconcile", "-n"];
        let out = exec("p4", workdir, &args, &env, None)?;
        let text = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() && !stderr.contains("no file(s) to reconcile") {
            return Err(cmd_error("p4", &args, &out));
        }
        let mut v: Vec<(String, String, String)> = parse_ztag(&text)
            .iter()
            .filter_map(|r| {
                Some((
                    field(r, "depotFile")?.to_owned(),
                    field(r, "clientFile")?.to_owned(),
                    field(r, "action")?.to_owned(),
                ))
            })
            .filter(|(_, local, _)| !is_config_file(local))
            .collect();
        v.sort();
        Ok(v)
    }

    fn opened(&self, workdir: &Path) -> Result<HashSet<String>> {
        let out = exec(
            "p4",
            workdir,
            &["-ztag", "opened"],
            &self
                .env()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            None,
        )?;
        // "File(s) not opened" is an error text on stderr; stdout is empty then.
        Ok(parse_ztag(&String::from_utf8_lossy(&out.stdout))
            .iter()
            .filter_map(|r| field(r, "depotFile").map(str::to_owned))
            .collect())
    }

    /// The pending changelist that holds the shelf of a WIP state, with its fingerprint.
    fn find_shelf(&self, workdir: &Path, desc: &str) -> Result<Option<(String, String)>> {
        let out = self.p4(workdir, &["-ztag", "changes", "-s", "pending", "-l"])?;
        for r in parse_ztag(&out) {
            let text = field(&r, "desc").unwrap_or("");
            // A multi-line value: the first line is in `desc`; the rest follows as plain lines.
            if text.trim() == desc {
                let cl = field(&r, "change").unwrap_or("").to_owned();
                let fp = self.shelf_fingerprint(workdir, &cl).unwrap_or_default();
                return Ok(Some((cl, fp)));
            }
        }
        Ok(None)
    }

    fn shelf_fingerprint(&self, workdir: &Path, cl: &str) -> Option<String> {
        let spec = self.p4(workdir, &["change", "-o", cl]).ok()?;
        spec.lines()
            .find_map(|l| l.trim().strip_prefix("fp:").map(str::to_owned))
    }

    fn write_config(&self, dest: &Path, client: &str) -> Result<()> {
        let mut text = format!("P4CLIENT={client}\n");
        let port = self.port.clone().or_else(|| std::env::var("P4PORT").ok());
        let user = self.user.clone().or_else(|| std::env::var("P4USER").ok());
        if let Some(p) = port {
            text.push_str(&format!("P4PORT={p}\n"));
        }
        if let Some(u) = user {
            text.push_str(&format!("P4USER={u}\n"));
        }
        fs::write(dest.join(config_name()), text)?;
        Ok(())
    }

    fn make_client(
        &self,
        dest: &Path,
        client: &str,
        view: &[String],
        stream: Option<&str>,
    ) -> Result<()> {
        let mut spec = format!(
            "Client:\t{client}\nRoot:\t{}\nOptions:\tallwrite noclobber nocompress unlocked nomodtime normdir\nSubmitOptions:\tsubmitunchanged\nLineEnd:\tlocal\n",
            path_str(dest)?
        );
        if let Some(s) = stream {
            spec.push_str(&format!("Stream:\t{s}\n"));
        } else {
            spec.push_str("View:\n");
            for line in view {
                spec.push_str(&format!("\t{line}\n"));
            }
        }
        self.p4_in(dest, &["client", "-i"], Some(spec.as_bytes()))?;
        Ok(())
    }
}

impl SyncVcs for P4Sync {
    fn kind(&self) -> VcsKind {
        VcsKind::P4
    }

    /// The id is `<serverAddress>|<stream>` for a stream client, else
    /// `<serverAddress>|<depot side of the view lines>`. The client root is not part of it:
    /// it differs on each machine.
    fn identity(&self, workdir: &Path) -> Result<RepoIdentity> {
        let info = parse_ztag(&self.p4(workdir, &["-ztag", "info"])?);
        let server = info
            .first()
            .and_then(|r| field(r, "serverAddress"))
            .ok_or_else(|| VcsError::Parse("no serverAddress in p4 info".into()))?
            .to_owned();
        let client = parse_ztag(&self.p4(workdir, &["-ztag", "client", "-o"])?);
        let rec = client.first().cloned().unwrap_or_default();
        let scope = match field(&rec, "Stream") {
            Some(s) => s.to_owned(),
            None => rec
                .iter()
                .filter(|(k, _)| k.starts_with("View"))
                .filter_map(|(_, v)| v.split_whitespace().next().map(str::to_owned))
                .collect::<Vec<_>>()
                .join(","),
        };
        Ok(RepoIdentity {
            kind: VcsKind::P4,
            id: format!("{server}|{scope}"),
            remote: Some(server),
        })
    }

    /// `source` is a depot path such as `//depot/proj/...`. `rev` is a changelist number.
    fn clone_or_checkout(&self, source: &str, dest: &Path, rev: Option<&str>) -> Result<()> {
        check_arg("source", source)?;
        if !source.starts_with("//") {
            return Err(VcsError::Invalid(format!("not a depot path `{source}`")));
        }
        fs::create_dir_all(dest)?;
        let client = client_name_for(dest);
        self.write_config(dest, &client)?;
        let depot = if source.ends_with("...") {
            source.to_owned()
        } else {
            format!("{}/...", source.trim_end_matches('/'))
        };
        let view = vec![format!("{depot} //{client}/...")];
        self.make_client(dest, &client, &view, None)?;
        let target = match rev {
            Some(r) => {
                check_arg("rev", r)?;
                format!("//{client}/...@{r}")
            }
            None => format!("//{client}/..."),
        };
        self.p4(dest, &["sync", "-q", &target])?;
        Ok(())
    }

    fn update(&self, workdir: &Path) -> Result<bool> {
        if !self.is_clean(workdir)? {
            return Err(dirty_error(workdir));
        }
        let before = self.head(workdir)?;
        self.p4(workdir, &["sync", "-q"])?;
        Ok(self.head(workdir)? != before)
    }

    fn is_clean(&self, workdir: &Path) -> Result<bool> {
        Ok(self.opened(workdir)?.is_empty() && self.pending(workdir)?.is_empty())
    }

    fn head(&self, workdir: &Path) -> Result<String> {
        let out = self.p4(workdir, &["-ztag", "changes", "-m1", "...#have"])?;
        parse_ztag(&out)
            .first()
            .and_then(|r| field(r, "change"))
            .map(str::to_owned)
            .ok_or_else(|| VcsError::NotFound("no synced changelist".into()))
    }

    fn commit_wip(&self, workdir: &Path, wip: &WipRef, _message: &str) -> Result<Option<String>> {
        wip.validate()?;
        let desc = wip.backend_name(VcsKind::P4);
        let was_open = self.opened(workdir)?;
        let files: Vec<(String, String, String)> = self
            .pending(workdir)?
            .into_iter()
            .filter(|(depot, _, _)| !was_open.contains(depot))
            .collect();
        if files.is_empty() {
            return Ok(None);
        }
        let mut fnv = Fnv::new();
        for (depot, local, action) in &files {
            fnv.feed(depot.as_bytes());
            fnv.feed(action.as_bytes());
            fnv.feed(&fs::read(local).unwrap_or_default());
        }
        let fp = fnv.hex();
        let existing = self.find_shelf(workdir, &desc)?;
        if existing.as_ref().is_some_and(|(_, old)| *old == fp) {
            return Ok(None);
        }
        if let Some((cl, _)) = &existing {
            // Remove the old shelf; a failure here means there was no shelf.
            let _ = self.p4(workdir, &["shelve", "-d", "-c", cl]);
            let _ = self.p4(workdir, &["change", "-d", cl]);
        }
        let locals: Vec<&str> = files.iter().map(|(_, l, _)| l.as_str()).collect();
        let with_files = |head: &[&str]| -> Vec<String> {
            head.iter()
                .map(|s| (*s).to_owned())
                .chain(locals.iter().map(|s| (*s).to_owned()))
                .collect()
        };
        let run_args = |args: Vec<String>| -> Result<String> {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            self.p4(workdir, &refs)
        };
        run_args(with_files(&["reconcile"]))?;
        let spec = format!("Change:\tnew\nDescription:\n\t{desc}\n\tfp:{fp}\n");
        let created = self.p4_in(workdir, &["change", "-i"], Some(spec.as_bytes()))?;
        let cl = created
            .split_whitespace()
            .nth(1)
            .filter(|n| n.chars().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| VcsError::Parse(format!("unexpected `p4 change -i` output: {created}")))?
            .to_owned();
        run_args(with_files(&["reopen", "-c", &cl]))?;
        self.p4(workdir, &["shelve", "-f", "-c", &cl])?;
        // The files were not open before: the working copy keeps its content, p4 forgets
        // the open state.
        run_args(with_files(&["revert", "-k"]))?;
        Ok(Some(cl))
    }

    /// No-op. A shelf lives on the server already.
    fn push_wip(&self, _workdir: &Path, _wip: &WipRef, _remote: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// No-op. A shelf lives on the server already.
    fn fetch_wip(&self, _workdir: &Path, _wip: &WipRef, _remote: Option<&str>) -> Result<()> {
        Ok(())
    }

    fn apply_wip(&self, workdir: &Path, wip: &WipRef) -> Result<()> {
        wip.validate()?;
        let desc = wip.backend_name(VcsKind::P4);
        let (cl, _) = self
            .find_shelf(workdir, &desc)?
            .ok_or_else(|| VcsError::NotFound(format!("WIP shelf `{desc}`")))?;
        // Drop local opened files first; revert restores the depot content.
        if !self.opened(workdir)?.is_empty() {
            self.p4(workdir, &["revert", "..."])?;
        }
        self.p4(workdir, &["unshelve", "-s", &cl, "-f"])?;
        Ok(())
    }

    /// Uses `p4 ignores -i`, which tests a file against `P4IGNORE`.
    fn ignored(&self, workdir: &Path, rel_path: &str) -> Result<bool> {
        check_rel(rel_path)?;
        let env = self.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let out = exec("p4", workdir, &["ignores", "-i", rel_path], &env, None)?;
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        Ok(out.status.success() && text.contains("ignored") && !text.contains("not ignored"))
    }

    fn create_workspace(&self, repo: &Path, dest: &Path, rev: Option<&str>) -> Result<()> {
        let spec = self.p4(repo, &["client", "-o"])?;
        let mut old = String::new();
        let mut stream = None;
        let mut view_lines = Vec::new();
        let mut in_view = false;
        for line in spec.lines() {
            if let Some(v) = line.strip_prefix("Client:") {
                old = v.trim().to_owned();
            } else if let Some(v) = line.strip_prefix("Stream:") {
                stream = Some(v.trim().to_owned());
            }
            if line.starts_with("View:") {
                in_view = true;
            } else if in_view && line.starts_with(['\t', ' ']) {
                view_lines.push(line.trim().to_owned());
            } else if !line.trim().is_empty() {
                in_view = false;
            }
        }
        if old.is_empty() {
            return Err(VcsError::Parse("no client name in `p4 client -o`".into()));
        }
        let client = client_name_for(dest);
        let view: Vec<String> = view_lines
            .iter()
            .map(|l| l.replace(&format!("//{old}/"), &format!("//{client}/")))
            .collect();
        fs::create_dir_all(dest)?;
        self.write_config(dest, &client)?;
        self.make_client(dest, &client, &view, stream.as_deref())?;
        let target = match rev {
            Some(r) => {
                check_arg("rev", r)?;
                format!("//{client}/...@{r}")
            }
            None => format!("//{client}/..."),
        };
        self.p4(dest, &["sync", "-q", &target])?;
        Ok(())
    }

    fn remove_workspace(&self, repo: &Path, dest: &Path) -> Result<()> {
        let config = fs::read_to_string(dest.join(config_name()))?;
        let client = config
            .lines()
            .find_map(|l| l.strip_prefix("P4CLIENT="))
            .map(|s| s.trim().to_owned())
            .ok_or_else(|| VcsError::Parse("no P4CLIENT in config".into()))?;
        self.p4(repo, &["client", "-d", "-f", &client])?;
        fs::remove_dir_all(dest)?;
        Ok(())
    }
}
