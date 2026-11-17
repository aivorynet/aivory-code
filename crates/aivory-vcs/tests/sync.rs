//! Tests of the sync layer against temporary repositories (synthetic data only).
//!
//! Each backend has a module. A test returns early when its tools are missing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aivory_vcs::{SyncVcs, VcsKind, WipRef, detect};
use tempfile::TempDir;

// ---- Helpers ------------------------------------------------------------------------------

fn have(bin: &str, probe: &[&str]) -> bool {
    Command::new(bin)
        .args(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a tool and return stdout. Panics on failure.
fn tool(bin: &str, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("run {bin}: {e}"));
    assert!(
        out.status.success(),
        "{bin} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(p, text).expect("write");
}

fn read(dir: &Path, rel: &str) -> String {
    // git may turn LF into CRLF (core.autocrlf on Windows).
    fs::read_to_string(dir.join(rel))
        .unwrap_or_default()
        .replace("\r\n", "\n")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir");
    for e in fs::read_dir(src).expect("read_dir") {
        let e = e.expect("entry");
        let to = dst.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            fs::copy(e.path(), to).expect("copy");
        }
    }
}

/// How a test makes the repository and moves it forward.
struct Fixture {
    /// The `source` argument of `clone_or_checkout`.
    source: String,
    /// Make a new upstream revision that adds `b.txt`.
    advance: Box<dyn Fn()>,
    /// Move the WIP state from checkout 1 to checkout 2, for a backend with no remote store.
    transport: Option<fn(&Path, &Path)>,
}

/// The common scenario. `dir` is a scratch folder.
fn scenario(sync: &dyn SyncVcs, fx: &Fixture, kind: VcsKind, dir: &Path) {
    assert_eq!(sync.kind(), kind);
    let (c1, c2, c3, c4) = (
        dir.join("c1"),
        dir.join("c2"),
        dir.join("c3"),
        dir.join("c4"),
    );

    // clone_or_checkout, identity, head
    sync.clone_or_checkout(&fx.source, &c1, None)
        .expect("clone c1");
    sync.clone_or_checkout(&fx.source, &c2, None)
        .expect("clone c2");
    let id1 = sync.identity(&c1).expect("identity c1");
    let id2 = sync.identity(&c2).expect("identity c2");
    assert_eq!(id1.kind, kind);
    assert!(!id1.id.is_empty());
    assert_eq!(id1, id2, "two checkouts share one identity");
    assert!(sync.is_clean(&c1).expect("clean"));
    let head1 = sync.head(&c1).expect("head");
    assert!(!head1.is_empty());

    // commit_wip: a modified file, a new file, and an ignored file
    write(&c1, "a.txt", "one\nmodified\n");
    write(&c1, "new.txt", "brand new\n");
    write(&c1, "x.log", "ignored\n");
    assert!(!sync.is_clean(&c1).expect("dirty"));
    let wip = WipRef::new("node-a", "s1").expect("wip name");
    let first = sync.commit_wip(&c1, &wip, "wip: test").expect("commit_wip");
    assert!(first.is_some(), "a change gives a WIP state");
    let again = sync
        .commit_wip(&c1, &wip, "wip: test")
        .expect("commit_wip 2");
    assert_eq!(again, None, "nothing changed gives None");
    // The user's state stays.
    assert_eq!(sync.head(&c1).expect("head"), head1);
    assert_eq!(read(&c1, "a.txt"), "one\nmodified\n");
    assert_eq!(read(&c1, "new.txt"), "brand new\n");
    assert!(!sync.is_clean(&c1).expect("still dirty"));

    // ignored
    assert!(sync.ignored(&c1, "x.log").expect("ignored x.log"));
    assert!(!sync.ignored(&c1, "a.txt").expect("ignored a.txt"));

    // update refuses a dirty checkout
    let err = sync.update(&c1).expect_err("dirty update");
    assert!(err.to_string().contains("local changes"), "{err}");

    // push, fetch, and apply in a second checkout
    sync.push_wip(&c1, &wip, None).expect("push_wip");
    if let Some(transport) = fx.transport {
        transport(&c1, &c2);
    }
    sync.fetch_wip(&c2, &wip, None).expect("fetch_wip");
    let head2 = sync.head(&c2).expect("head c2");
    sync.apply_wip(&c2, &wip).expect("apply_wip");
    assert_eq!(read(&c2, "a.txt"), "one\nmodified\n");
    assert_eq!(read(&c2, "new.txt"), "brand new\n");
    assert!(
        !c2.join("x.log").exists(),
        "an ignored file does not travel"
    );
    assert_eq!(
        sync.head(&c2).expect("head c2"),
        head2,
        "apply keeps the branch"
    );

    // update: fast-forward of a clean checkout
    sync.clone_or_checkout(&fx.source, &c3, None)
        .expect("clone c3");
    assert!(!sync.update(&c3).expect("update no news"));
    let head3 = sync.head(&c3).expect("head c3");
    (fx.advance)();
    assert!(
        sync.update(&c3).expect("update ff"),
        "new upstream revision"
    );
    assert_ne!(sync.head(&c3).expect("head c3"), head3);
    assert!(c3.join("b.txt").exists());
    assert!(!sync.update(&c3).expect("update again"));

    // workspaces
    sync.create_workspace(&c3, &c4, None)
        .expect("create_workspace");
    assert!(c4.join("a.txt").exists());
    sync.remove_workspace(&c3, &c4).expect("remove_workspace");
    assert!(!c4.exists());

    // detect
    let found = detect(&c1).expect("detect");
    assert_eq!(found.kind(), kind);
    let sub = c1.join("deep");
    fs::create_dir_all(&sub).expect("mkdir");
    assert_eq!(detect(&sub).expect("detect sub").kind(), kind);
}

fn url_of(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

// ---- Shared types -------------------------------------------------------------------------

mod types {
    use super::*;

    #[test]
    fn wip_names_per_backend() {
        let w = WipRef::new("laptop", "s-1").expect("name");
        assert_eq!(w.backend_name(VcsKind::Git), "refs/aivory/wip/laptop/s-1");
        assert_eq!(w.backend_name(VcsKind::Hg), "aivory-wip/laptop/s-1");
        assert_eq!(w.backend_name(VcsKind::Jj), "aivory/wip/laptop/s-1");
        assert_eq!(w.backend_name(VcsKind::P4), "aivory-wip laptop s-1");
        assert_eq!(w.backend_name(VcsKind::Svn), ".svn/aivory-wip/laptop/s-1");
    }

    #[test]
    fn wip_name_validation() {
        for bad in ["", "-x", "a/b", "a b", "..", "a..b", ".hidden"] {
            assert!(WipRef::new(bad, "s").is_err(), "node `{bad}`");
            assert!(WipRef::new("n", bad).is_err(), "session `{bad}`");
        }
    }

    #[test]
    fn serde_is_camel_case() {
        let id = aivory_vcs::RepoIdentity {
            kind: VcsKind::P4,
            id: "x".into(),
            remote: None,
        };
        let json = serde_json_like(&id);
        assert!(json.contains("\"kind\":\"p4\""), "{json}");
        let kinds = [VcsKind::Git, VcsKind::Hg, VcsKind::Jj, VcsKind::Svn];
        let names: Vec<String> = kinds.iter().map(serde_json_like).collect();
        assert_eq!(names, ["\"git\"", "\"hg\"", "\"jj\"", "\"svn\""]);
    }

    /// `serde_json` is not a dependency of this crate; a tiny serializer is enough here.
    fn serde_json_like<T: serde::Serialize>(v: &T) -> String {
        let mut out = String::new();
        v.serialize(&mut MiniJson(&mut out)).expect("serialize");
        out
    }

    struct MiniJson<'a>(&'a mut String);
    #[derive(Debug)]
    struct MiniErr;
    impl std::fmt::Display for MiniErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("mini json error")
        }
    }
    impl std::error::Error for MiniErr {}
    impl serde::ser::Error for MiniErr {
        fn custom<T: std::fmt::Display>(_: T) -> Self {
            MiniErr
        }
    }
    impl<'a, 'b> serde::Serializer for &'a mut MiniJson<'b> {
        type Ok = ();
        type Error = MiniErr;
        type SerializeSeq = serde::ser::Impossible<(), MiniErr>;
        type SerializeTuple = serde::ser::Impossible<(), MiniErr>;
        type SerializeTupleStruct = serde::ser::Impossible<(), MiniErr>;
        type SerializeTupleVariant = serde::ser::Impossible<(), MiniErr>;
        type SerializeMap = serde::ser::Impossible<(), MiniErr>;
        type SerializeStruct = Self;
        type SerializeStructVariant = serde::ser::Impossible<(), MiniErr>;
        fn serialize_bool(self, _: bool) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_i8(self, _: i8) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_i16(self, _: i16) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_i32(self, _: i32) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_i64(self, _: i64) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_u8(self, _: u8) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_u16(self, _: u16) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_u32(self, _: u32) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_u64(self, _: u64) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_f32(self, _: f32) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_f64(self, _: f64) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_char(self, _: char) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_str(self, v: &str) -> Result<(), MiniErr> {
            self.0.push_str(&format!("\"{v}\""));
            Ok(())
        }
        fn serialize_bytes(self, _: &[u8]) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_none(self) -> Result<(), MiniErr> {
            self.0.push_str("null");
            Ok(())
        }
        fn serialize_some<T: ?Sized + serde::Serialize>(self, v: &T) -> Result<(), MiniErr> {
            v.serialize(self)
        }
        fn serialize_unit(self) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_unit_struct(self, _: &'static str) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_unit_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
        ) -> Result<(), MiniErr> {
            self.serialize_str(variant)
        }
        fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(
            self,
            _: &'static str,
            _: &T,
        ) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: &T,
        ) -> Result<(), MiniErr> {
            Err(MiniErr)
        }
        fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, MiniErr> {
            Err(MiniErr)
        }
        fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, MiniErr> {
            Err(MiniErr)
        }
        fn serialize_tuple_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleStruct, MiniErr> {
            Err(MiniErr)
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeTupleVariant, MiniErr> {
            Err(MiniErr)
        }
        fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, MiniErr> {
            Err(MiniErr)
        }
        fn serialize_struct(
            self,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStruct, MiniErr> {
            self.0.push('{');
            Ok(self)
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
            _: usize,
        ) -> Result<Self::SerializeStructVariant, MiniErr> {
            Err(MiniErr)
        }
    }
    impl serde::ser::SerializeStruct for &mut MiniJson<'_> {
        type Ok = ();
        type Error = MiniErr;
        fn serialize_field<T: ?Sized + serde::Serialize>(
            &mut self,
            key: &'static str,
            value: &T,
        ) -> Result<(), MiniErr> {
            if !self.0.ends_with('{') {
                self.0.push(',');
            }
            self.0.push_str(&format!("\"{key}\":"));
            value.serialize(&mut **self)
        }
        fn end(self) -> Result<(), MiniErr> {
            self.0.push('}');
            Ok(())
        }
    }
}

// ---- Git ----------------------------------------------------------------------------------

/// A plain git repository on `main` with `a.txt` and an ignore rule for `*.log`.
fn git_origin(root: &Path) -> PathBuf {
    let o = root.join("origin");
    fs::create_dir_all(&o).expect("mkdir");
    let env = [
        ("GIT_AUTHOR_NAME", "T"),
        ("GIT_AUTHOR_EMAIL", "t@example.invalid"),
        ("GIT_COMMITTER_NAME", "T"),
        ("GIT_COMMITTER_EMAIL", "t@example.invalid"),
    ];
    tool("git", &o, &["init", "-q", "-b", "main", "."], &env);
    write(&o, "a.txt", "one\n");
    write(&o, ".gitignore", "*.log\n");
    tool("git", &o, &["add", "-A"], &env);
    tool("git", &o, &["commit", "-q", "-m", "init"], &env);
    o
}

fn git_advance(origin: &Path) {
    let env = [
        ("GIT_AUTHOR_NAME", "T"),
        ("GIT_AUTHOR_EMAIL", "t@example.invalid"),
        ("GIT_COMMITTER_NAME", "T"),
        ("GIT_COMMITTER_EMAIL", "t@example.invalid"),
    ];
    write(origin, "b.txt", "two\n");
    tool("git", origin, &["add", "-A"], &env);
    tool("git", origin, &["commit", "-q", "-m", "two"], &env);
}

mod git {
    use super::*;
    use aivory_vcs::GitSync;

    #[test]
    fn full_scenario() {
        if !have("git", &["--version"]) {
            eprintln!("skip: git not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        let origin = git_origin(tmp.path());
        let o2 = origin.clone();
        let fx = Fixture {
            source: origin.to_string_lossy().into_owned(),
            advance: Box::new(move || git_advance(&o2)),
            transport: None,
        };
        scenario(&GitSync::new(), &fx, VcsKind::Git, tmp.path());
    }

    #[test]
    fn wip_ref_and_branch_stay() {
        if !have("git", &["--version"]) {
            eprintln!("skip: git not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        let origin = git_origin(tmp.path());
        let sync = GitSync::new();
        let wip = WipRef::new("n1", "s1").expect("wip");
        write(&origin, "a.txt", "changed\n");
        write(&origin, "gone-soon.txt", "x\n");
        let commit = sync
            .commit_wip(&origin, &wip, "wip")
            .expect("commit_wip")
            .expect("some");
        let reff = tool("git", &origin, &["rev-parse", "refs/aivory/wip/n1/s1"], &[]);
        assert_eq!(reff, commit);
        // The index stays empty of changes: git sees an unstaged edit and an untracked file.
        let st = tool("git", &origin, &["status", "--porcelain"], &[]);
        assert!(st.contains("M a.txt"), "{st}");
        assert!(st.contains("?? gone-soon.txt"), "{st}");
        // The WIP commit holds the untracked file.
        let files = tool(
            "git",
            &origin,
            &["ls-tree", "-r", "--name-only", &commit],
            &[],
        );
        assert!(files.contains("gone-soon.txt"), "{files}");
        // A deleted file goes away with apply_wip.
        let second = tmp.path().join("second");
        sync.clone_or_checkout(&origin.to_string_lossy(), &second, None)
            .expect("clone");
        write(&second, "extra.txt", "local only\n");
        tool(
            "git",
            &second,
            &[
                "fetch",
                "-q",
                &origin.to_string_lossy(),
                "+refs/aivory/wip/n1/s1:refs/aivory/wip/n1/s1",
            ],
            &[],
        );
        sync.apply_wip(&second, &wip).expect("apply");
        assert!(!second.join("extra.txt").exists());
        assert_eq!(read(&second, "gone-soon.txt"), "x\n");
    }
}

// ---- Mercurial ----------------------------------------------------------------------------

mod hg {
    use super::*;
    use aivory_vcs::HgSync;

    const ENV: [(&str, &str); 2] = [("HGPLAIN", "1"), ("HGUSER", "T <t@example.invalid>")];

    fn advance(origin: &Path) {
        write(origin, "b.txt", "two\n");
        tool("hg", origin, &["add", "b.txt"], &ENV);
        tool("hg", origin, &["commit", "-m", "two"], &ENV);
    }

    #[test]
    fn full_scenario() {
        if !have("hg", &["--version"]) {
            eprintln!("skip: hg not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        let o = tmp.path().join("origin");
        fs::create_dir_all(&o).expect("mkdir");
        tool("hg", &o, &["init"], &ENV);
        write(&o, "a.txt", "one\n");
        write(&o, ".hgignore", "syntax: glob\n*.log\n");
        tool("hg", &o, &["add", "a.txt", ".hgignore"], &ENV);
        tool("hg", &o, &["commit", "-m", "init"], &ENV);
        let o2 = o.clone();
        let fx = Fixture {
            source: o.to_string_lossy().into_owned(),
            advance: Box::new(move || advance(&o2)),
            transport: None,
        };
        scenario(&HgSync::new(), &fx, VcsKind::Hg, tmp.path());
    }
}

// ---- Jujutsu ------------------------------------------------------------------------------

mod jj {
    use super::*;
    use aivory_vcs::JjSync;

    #[test]
    fn full_scenario() {
        if !have("jj", &["--version"]) || !have("git", &["--version"]) {
            eprintln!("skip: jj not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        // The remote is a plain git repository; jj clones and fetches from it.
        let origin = git_origin(tmp.path());
        let o2 = origin.clone();
        let fx = Fixture {
            source: origin.to_string_lossy().into_owned(),
            advance: Box::new(move || git_advance(&o2)),
            transport: None,
        };
        scenario(&JjSync::new(), &fx, VcsKind::Jj, tmp.path());
    }
}

// ---- Subversion ---------------------------------------------------------------------------

mod svn {
    use super::*;
    use aivory_vcs::SvnSync;

    fn svn(dir: &Path, args: &[&str]) -> String {
        tool("svn", dir, args, &[])
    }

    #[test]
    fn full_scenario() {
        if !have("svn", &["--version", "--quiet"]) || !have("svnadmin", &["--version", "--quiet"]) {
            eprintln!("skip: svn not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        let repo = tmp.path().join("repo");
        tool(
            "svnadmin",
            tmp.path(),
            &["create", &repo.to_string_lossy()],
            &[],
        );
        let url = url_of(&repo);
        let author = tmp.path().join("author");
        svn(
            tmp.path(),
            &["checkout", "-q", &url, &author.to_string_lossy()],
        );
        write(&author, "a.txt", "one\n");
        svn(&author, &["add", "-q", "a.txt"]);
        svn(&author, &["propset", "-q", "svn:ignore", "*.log", "."]);
        svn(&author, &["commit", "-q", "-m", "init"]);
        let a2 = author.clone();
        let fx = Fixture {
            source: url,
            advance: Box::new(move || {
                svn(&a2, &["update", "-q"]);
                write(&a2, "b.txt", "two\n");
                svn(&a2, &["add", "-q", "b.txt"]);
                svn(&a2, &["commit", "-q", "-m", "two"]);
            }),
            // Subversion WIP travels peer to peer: copy the shadow store.
            transport: Some(|from, to| {
                copy_dir(&from.join(".svn/aivory-wip"), &to.join(".svn/aivory-wip"));
            }),
        };
        scenario(&SvnSync::new(), &fx, VcsKind::Svn, tmp.path());
    }
}

// ---- Perforce -----------------------------------------------------------------------------

mod p4 {
    use super::*;
    use aivory_vcs::P4Sync;

    #[test]
    fn full_scenario() {
        if !have("p4", &["-V"]) || !have("p4d", &["-V"]) {
            eprintln!("skip: p4 or p4d not installed");
            return;
        }
        let tmp = TempDir::new().expect("tmp");
        let server = tmp.path().join("server");
        fs::create_dir_all(&server).expect("mkdir");
        // `rsh:` starts a private p4d for each command: no free port needed.
        let port = format!("rsh:p4d -r {} -i -q", server.to_string_lossy());
        let seed = tmp.path().join("seed");
        fs::create_dir_all(&seed).expect("mkdir");
        let env = [
            ("P4PORT", port.as_str()),
            ("P4USER", "aivory"),
            ("P4CLIENT", "seed"),
            ("P4IGNORE", ".p4ignore"),
        ];
        let p4 = |args: &[&str]| tool("p4", &seed, args, &env);
        let spec = format!(
            "Client: seed\nRoot: {}\nOptions: allwrite noclobber nocompress unlocked nomodtime normdir\nView:\n\t//depot/... //seed/...\n",
            seed.to_string_lossy()
        );
        let mut child = Command::new("p4")
            .current_dir(&seed)
            .args(["client", "-i"])
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("p4 client -i");
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(spec.as_bytes())
                .expect("spec");
        }
        assert!(child.wait().expect("wait").success());
        write(&seed, "a.txt", "one\n");
        write(&seed, ".p4ignore", "*.log\n.p4config\n");
        p4(&["add", "a.txt", ".p4ignore"]);
        p4(&["submit", "-d", "init"]);

        let sp = seed.clone();
        let port2 = port.clone();
        let fx = Fixture {
            source: "//depot/...".into(),
            advance: Box::new(move || {
                write(&sp, "b.txt", "two\n");
                let env = [
                    ("P4PORT", port2.as_str()),
                    ("P4USER", "aivory"),
                    ("P4CLIENT", "seed"),
                ];
                tool("p4", &sp, &["add", "b.txt"], &env);
                tool("p4", &sp, &["submit", "-d", "two"], &env);
            }),
            transport: None,
        };
        let sync = P4Sync::new().with_port(port).with_user("aivory");
        scenario(&sync, &fx, VcsKind::P4, tmp.path());
    }
}
