//! Live file sync of the machine mesh (ADR 0026, step M5).
//!
//! - One writer. The machine that shares a folder (`peers/share`) is the only writer. Each
//!   peer keeps a read-only mirror under `<data>/mirrors/<node8>/<name>/`.
//! - Live. Every [`SCAN_EVERY`] the sharer lists the folder (the VCS ignore rules apply; git
//!   uses `git ls-files`), hashes the changed files (SHA-256), and sends each new or changed
//!   file to each online peer in chunks of [`CHUNK`] (`mesh/put`). A deleted file goes as
//!   `mesh/del`. A peer that comes back gets the files it missed.
//! - Durable. Every [`WIP_EVERY`] a share in a repository gets a WIP commit in its own VCS and
//!   a push of the WIP ref (at most one push per minute, ADR 0013). Subversion WIP stays local.
//! - Safety. The receiver accepts only relative paths without `..`, writes to a `.part` file,
//!   checks the hash, and then renames the file into place.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use aivory_protocol::mesh::{MirrorInfo, ShareInfo, ShareParams, SharesView};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::mesh::{Mesh, MeshFiles};
use crate::remote::{now_ms, random_hex};

/// How often a share is scanned.
pub const SCAN_EVERY: Duration = Duration::from_secs(2);
/// How often a share gets a WIP commit and push.
pub const WIP_EVERY: Duration = Duration::from_secs(60);
/// The bytes of one `mesh/put` (base64 in one 8 MB frame).
pub const CHUNK: usize = 2 * 1024 * 1024;
/// Files above this size are not synced live (they still go into the WIP commit).
pub const MAX_FILE: u64 = 256 * 1024 * 1024;

/// Folders that are never synced.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    "target",
    "node_modules",
    ".aivory-mirror",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    mtime: u128,
}

#[derive(Debug, Clone)]
struct Entry {
    stamp: Stamp,
    sha: String,
}

struct Share {
    info: ShareInfo,
    /// The files of the last scan.
    files: BTreeMap<String, Entry>,
    /// Peer -> path -> sha that the peer has.
    sent: HashMap<String, HashMap<String, String>>,
    last_wip: Option<Instant>,
}

#[derive(Default)]
struct Mirror {
    files: u32,
    updated: Option<i64>,
}

/// The shares of this machine and the mirrors of the peers.
pub struct FileSync {
    mesh: Arc<Mesh>,
    root: PathBuf,
    shares: Mutex<HashMap<String, Share>>,
    mirrors: Mutex<HashMap<(String, String), Mirror>>,
}

impl std::fmt::Debug for FileSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSync")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl FileSync {
    /// `root`: the folder of the mirrors (`<data>/mirrors`). Start the scan loop with
    /// [`FileSync::start`].
    pub fn new(mesh: Arc<Mesh>, root: PathBuf) -> Arc<Self> {
        let sync = Arc::new(Self {
            mesh: Arc::clone(&mesh),
            root,
            shares: Mutex::new(HashMap::new()),
            mirrors: Mutex::new(HashMap::new()),
        });
        mesh.set_files(Arc::clone(&sync) as Arc<dyn MeshFiles>);
        sync
    }

    /// Run the scan loop on the runtime of the mesh. Call it after [`Mesh::attach`].
    pub fn start(self: &Arc<Self>) {
        let Some(handle) = self.mesh.runtime() else {
            return;
        };
        let sync = Arc::clone(self);
        handle.spawn(async move {
            loop {
                tokio::time::sleep(SCAN_EVERY).await;
                let ids: Vec<String> = sync.lock_shares().keys().cloned().collect();
                for id in ids {
                    sync.tick(&id).await;
                }
            }
        });
    }

    /// `peers/share`.
    pub fn share(&self, p: &ShareParams) -> Result<ShareInfo, String> {
        let path = PathBuf::from(&p.path);
        if !path.is_dir() {
            return Err(format!("'{}' is not a folder", p.path));
        }
        let name = p
            .name
            .clone()
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .ok_or("the folder has no name")?;
        if !safe_name(&name) {
            return Err(format!("'{name}' is not a valid share name"));
        }
        let to =
            p.to.iter()
                .map(|id| self.mesh.resolve_peer(id))
                .collect::<Result<Vec<_>, _>>()?;
        let info = ShareInfo {
            id: random_hex(4),
            name,
            path: path.to_string_lossy().into_owned(),
            to,
            ..ShareInfo::default()
        };
        self.lock_shares().insert(
            info.id.clone(),
            Share {
                info: info.clone(),
                files: BTreeMap::new(),
                sent: HashMap::new(),
                last_wip: None,
            },
        );
        Ok(info)
    }

    /// `peers/unshare`.
    pub fn unshare(&self, id: &str) -> Result<(), String> {
        self.lock_shares()
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| format!("no share '{id}'"))
    }

    /// `peers/shares`.
    pub fn view(&self) -> SharesView {
        let shares = self
            .lock_shares()
            .values()
            .map(|s| s.info.clone())
            .collect();
        let mirrors = self
            .mirrors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|((peer, name), m)| MirrorInfo {
                peer: peer.clone(),
                name: name.clone(),
                path: self.mirror_dir(peer, name).to_string_lossy().into_owned(),
                files: m.files,
                updated: m.updated,
            })
            .collect();
        SharesView { shares, mirrors }
    }

    /// One scan of a share: list, hash the changed files, send them, and make a WIP commit.
    async fn tick(&self, id: &str) {
        let Some((info, old)) = self
            .lock_shares()
            .get(id)
            .map(|s| (s.info.clone(), s.files.clone()))
        else {
            return;
        };
        let root = PathBuf::from(&info.path);
        let scan_root = root.clone();
        let files = tokio::task::spawn_blocking(move || scan(&scan_root, &old))
            .await
            .unwrap_or_default();
        let bytes = files.values().map(|e| e.stamp.size).sum();
        let targets: Vec<String> = {
            let online = self.mesh.online();
            if info.to.is_empty() {
                online
            } else {
                online.into_iter().filter(|p| info.to.contains(p)).collect()
            }
        };
        let sends: Vec<(String, Vec<String>, Vec<String>)> = {
            let mut shares = self.lock_shares();
            let Some(share) = shares.get_mut(id) else {
                return;
            };
            share.files = files.clone();
            share.info.files = u32::try_from(files.len()).unwrap_or(u32::MAX);
            share.info.bytes = bytes;
            share.info.last_scan = Some(now_ms());
            targets
                .iter()
                .map(|peer| {
                    let have = share.sent.entry(peer.clone()).or_default();
                    let put: Vec<String> = files
                        .iter()
                        .filter(|(p, e)| have.get(*p) != Some(&e.sha) && e.stamp.size <= MAX_FILE)
                        .map(|(p, _)| p.clone())
                        .collect();
                    let del: Vec<String> = have
                        .keys()
                        .filter(|p| !files.contains_key(*p))
                        .cloned()
                        .collect();
                    (peer.clone(), put, del)
                })
                .collect()
        };
        for (peer, put, del) in sends {
            for path in put {
                let entry = &files[&path];
                match self.send_file(&peer, &info.name, &root, &path, entry).await {
                    Ok(()) => self.mark(id, &peer, &path, Some(entry.sha.clone())),
                    Err(e) => {
                        self.set_error(id, format!("{}: {e}", short(&peer)));
                        break;
                    }
                }
            }
            for path in del {
                let params = json!({ "name": info.name, "path": path });
                if self.mesh.request(&peer, "mesh/del", params).await.is_ok() {
                    self.mark(id, &peer, &path, None);
                }
            }
        }
        self.wip(id, &root, &info.name).await;
    }

    async fn send_file(
        &self,
        peer: &str,
        name: &str,
        root: &Path,
        path: &str,
        entry: &Entry,
    ) -> Result<(), String> {
        let bytes = tokio::fs::read(root.join(path))
            .await
            .map_err(|e| format!("{path}: {e}"))?;
        let total = bytes.len();
        let mut offset = 0;
        loop {
            let end = (offset + CHUNK).min(total);
            let params = json!({
                "name": name, "path": path, "sha": entry.sha,
                "offset": offset, "total": total,
                "data": base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]),
            });
            self.mesh.request(peer, "mesh/put", params).await?;
            offset = end;
            if offset >= total {
                return Ok(());
            }
        }
    }

    /// A WIP commit and push of the share, at most every [`WIP_EVERY`].
    async fn wip(&self, id: &str, root: &Path, name: &str) {
        let due = self
            .lock_shares()
            .get(id)
            .is_some_and(|s| s.last_wip.is_none_or(|t| t.elapsed() >= WIP_EVERY));
        if !due {
            return;
        }
        if let Some(s) = self.lock_shares().get_mut(id) {
            s.last_wip = Some(Instant::now());
        }
        let node = self.mesh.node();
        let root = root.to_path_buf();
        let session = name.to_string();
        let result = tokio::task::spawn_blocking(move || commit_wip(&root, &node, &session))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let mut shares = self.lock_shares();
        if let Some(s) = shares.get_mut(id) {
            match result {
                Ok(Some(rev)) => s.info.last_wip = Some(rev),
                Ok(None) => {}
                Err(e) => s.info.last_error = Some(e),
            }
        }
    }

    fn mark(&self, id: &str, peer: &str, path: &str, sha: Option<String>) {
        if let Some(s) = self.lock_shares().get_mut(id) {
            let have = s.sent.entry(peer.to_string()).or_default();
            match sha {
                Some(sha) => have.insert(path.to_string(), sha),
                None => have.remove(path),
            };
            s.info.last_error = None;
        }
    }

    fn set_error(&self, id: &str, error: String) {
        if let Some(s) = self.lock_shares().get_mut(id) {
            s.info.last_error = Some(error);
        }
    }

    fn mirror_dir(&self, peer: &str, name: &str) -> PathBuf {
        self.root.join(short(peer)).join(name)
    }

    fn lock_shares(&self) -> std::sync::MutexGuard<'_, HashMap<String, Share>> {
        self.shares.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn put(&self, peer: &str, p: &Value) -> Result<Value, String> {
        let name = p["name"].as_str().unwrap_or_default();
        let path = p["path"].as_str().unwrap_or_default();
        let sha = p["sha"].as_str().unwrap_or_default();
        let offset = p["offset"].as_u64().unwrap_or(0);
        let total = p["total"].as_u64().unwrap_or(0);
        let data = base64::engine::general_purpose::STANDARD
            .decode(p["data"].as_str().unwrap_or_default())
            .map_err(|e| format!("bad data: {e}"))?;
        let target = self.target(peer, name, path)?;
        let part = part_file(&target);
        if let Some(dir) = part.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(offset > 0)
            .truncate(offset == 0)
            .open(&part)
            .map_err(|e| e.to_string())?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len != offset {
            return Err(format!(
                "{path}: chunk at {offset}, the file has {len} bytes"
            ));
        }
        file.write_all(&data).map_err(|e| e.to_string())?;
        drop(file);
        if offset + data.len() as u64 >= total {
            let got = sha_file(&part).map_err(|e| e.to_string())?;
            if got != sha {
                let _ = std::fs::remove_file(&part);
                return Err(format!("{path}: the hash does not match"));
            }
            std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
            self.touch_mirror(peer, name);
        }
        Ok(json!({ "ok": true }))
    }

    fn del(&self, peer: &str, p: &Value) -> Result<Value, String> {
        let name = p["name"].as_str().unwrap_or_default();
        let path = p["path"].as_str().unwrap_or_default();
        let target = self.target(peer, name, path)?;
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        self.touch_mirror(peer, name);
        Ok(json!({ "ok": true }))
    }

    /// The local file of a mirror path. Only relative paths without `..` pass.
    fn target(&self, peer: &str, name: &str, path: &str) -> Result<PathBuf, String> {
        if !safe_name(name) || !safe_rel(path) {
            return Err(format!("refused path '{name}/{path}'"));
        }
        Ok(self.mirror_dir(peer, name).join(path))
    }

    fn touch_mirror(&self, peer: &str, name: &str) {
        let dir = self.mirror_dir(peer, name);
        let files = u32::try_from(scan(&dir, &BTreeMap::new()).len()).unwrap_or(u32::MAX);
        let mut mirrors = self.mirrors.lock().unwrap_or_else(PoisonError::into_inner);
        let m = mirrors
            .entry((peer.to_string(), name.to_string()))
            .or_default();
        m.files = files;
        m.updated = Some(now_ms());
    }
}

impl MeshFiles for FileSync {
    fn handle(&self, peer: &str, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "mesh/put" => self.put(peer, &params),
            "mesh/del" => self.del(peer, &params),
            _ => Err(format!("'{method}' is not a file method")),
        }
    }
}

/// A WIP commit of the folder in its own VCS, then a push of the WIP ref. `None`: no VCS or
/// nothing changed.
fn commit_wip(root: &Path, node: &str, session: &str) -> Result<Option<String>, String> {
    if !own_vcs_root(root) {
        return Ok(None);
    }
    let Some(vcs) = aivory_vcs::detect(root) else {
        return Ok(None);
    };
    let wip = aivory_vcs::WipRef {
        node: short(node).to_string(),
        session: session.to_string(),
    };
    let rev = vcs
        .commit_wip(root, &wip, &format!("aivory wip {session}"))
        .map_err(|e| e.to_string())?;
    if rev.is_some() {
        vcs.push_wip(root, &wip, None).map_err(|e| e.to_string())?;
    }
    Ok(rev)
}

/// The files of a folder with their hashes. A file whose size and time did not change keeps
/// its old hash.
fn scan(root: &Path, old: &BTreeMap<String, Entry>) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    for rel in list_files(root) {
        let full = root.join(&rel);
        let Ok(meta) = std::fs::metadata(&full) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let stamp = Stamp {
            size: meta.len(),
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos()),
        };
        let sha = match old.get(&rel) {
            Some(e) if e.stamp == stamp => e.sha.clone(),
            _ => match sha_file(&full) {
                Ok(sha) => sha,
                Err(_) => continue,
            },
        };
        out.insert(rel, Entry { stamp, sha });
    }
    out
}

/// The relative paths (with `/`) of the files to sync. In a git checkout `git ls-files` gives
/// the tracked and the untracked files without the ignored ones.
fn list_files(root: &Path) -> Vec<String> {
    if !root.join(".git").exists() {
        let mut out = Vec::new();
        walk(root, root, &mut out);
        return out;
    }
    let git = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success());
    if let Some(out) = git {
        return out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .filter(|p| safe_rel(p) && !p.split('/').any(|part| SKIP_DIRS.contains(&part)))
            .collect();
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &path, out);
            }
        } else if kind.is_file()
            && !name.ends_with(".aivory-part")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(rel.join("/"));
        }
    }
}

fn sha_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn part_file(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".aivory-part");
    target.with_file_name(name)
}

/// `true` when the VCS metadata sits in `root` itself (`.git`, `.hg`, `.jj`, `.svn`, or a
/// Perforce `.p4config`). The mesh never uses a repository of a parent folder: a share or a
/// codebase inside a home folder that is a repository must not touch that repository.
pub fn own_vcs_root(root: &Path) -> bool {
    [".git", ".hg", ".jj", ".svn", ".p4config"]
        .iter()
        .any(|m| root.join(m).exists())
}

/// A relative path with normal parts only (no root, no `..`, no drive).
pub fn safe_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name != "."
        && name != ".."
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_pass() {
        assert!(safe_rel("src/main.rs"));
        assert!(safe_rel("a.txt"));
        for bad in [
            "",
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "a\\b",
            "./a/../..",
        ] {
            assert!(!safe_rel(bad), "{bad}");
        }
        assert!(safe_name("my-repo_1.0"));
        assert!(!safe_name(".."));
        assert!(!safe_name("a/b"));
    }

    #[test]
    fn a_parent_repository_is_never_used() {
        let dir = std::env::temp_dir().join(format!("aivory-vcsroot-{}", std::process::id()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        assert!(own_vcs_root(&dir));
        assert!(
            !own_vcs_root(&sub),
            "a folder inside a repository is not its root"
        );
        assert_eq!(commit_wip(&sub, "node", "s").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
