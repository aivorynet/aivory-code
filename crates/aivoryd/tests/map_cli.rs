//! Runs `aivoryd map init` and `aivoryd map show` on a synthetic folder in the temp directory.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("aivoryd-map-cli-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn touch(&self, rel: &str) {
        let file = self.0.join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "x").unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn aivoryd(args: &[&str], repo: &TempDir) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aivoryd"))
        .args(args)
        .arg(&repo.0)
        .output()
        .expect("run aivoryd")
}

#[test]
fn map_init_writes_a_file_and_refuses_a_second_write() {
    let repo = TempDir::new("init");
    repo.touch("crates/alpha/src/lib.rs");
    repo.touch("apps/web/index.ts");
    repo.touch("Cargo.toml");

    let out = aivoryd(&["map", "init"], &repo);
    assert!(out.status.success(), "{out:?}");
    let text = fs::read_to_string(repo.0.join(".aivory").join("map.toml")).unwrap();
    assert!(text.contains("version = 1"), "{text}");
    assert!(text.contains("author = \"aivoryd map init\""), "{text}");

    // A second init keeps the file and exits with an error.
    fs::write(
        repo.0.join(".aivory").join("map.toml"),
        format!("{text}# edited\n"),
    )
    .unwrap();
    let again = aivoryd(&["map", "init"], &repo);
    assert_eq!(again.status.code(), Some(2), "{again:?}");
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));
    let kept = fs::read_to_string(repo.0.join(".aivory").join("map.toml")).unwrap();
    assert!(kept.ends_with("# edited\n"));

    // --force replaces it.
    let forced = aivoryd(&["map", "init", "--force"], &repo);
    assert!(forced.status.success(), "{forced:?}");
    let fresh = fs::read_to_string(repo.0.join(".aivory").join("map.toml")).unwrap();
    assert!(!fresh.contains("# edited"));
}

#[test]
fn map_show_prints_the_parts_as_json() {
    let repo = TempDir::new("show");
    repo.touch("crates/alpha/src/lib.rs");
    repo.touch("README.md");
    assert!(aivoryd(&["map", "init"], &repo).status.success());

    let out = aivoryd(&["map", "show"], &repo);
    assert!(out.status.success(), "{out:?}");
    let parts: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parts[0]["name"], "alpha");
    assert_eq!(parts[0]["paths"][0], "crates/alpha/**");
    assert_eq!(parts[1]["name"], "root");
}

#[test]
fn map_show_reports_a_missing_or_bad_file() {
    let repo = TempDir::new("bad");
    let missing = aivoryd(&["map", "show"], &repo);
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");

    repo.touch(".aivory/map.toml");
    let bad = aivoryd(&["map", "show"], &repo);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("line"));
}
