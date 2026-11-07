//! Tests on synthetic folders under the system temp directory. No real repository is read.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use aivory_map::{INIT_AUTHOR, MapError, PartMatcher, build, init, load, map_path, parse, write};
use aivory_protocol::warroom::Part;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aivory-map-test-{}-{label}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Create a file with its parent folders.
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

fn part(name: &str, paths: &[&str]) -> Part {
    Part {
        name: name.to_owned(),
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

const HEAD: &str = "version = 3\nupdated = \"2026-10-21T14:12:00Z\"\n";

// ---- parse and write ----

#[test]
fn parses_the_design_format() {
    let text = "# written by agents\nversion = 3\nupdated = \"2026-10-21T14:12:00Z\"\n\
                author = \"claude opus-5.5\"\n\n\
                [[part]]\nname  = \"payment\"\npaths = [\"crates/payment/**\", \"docs/pay*.md\"]\n\n\
                [[part]]\nname  = \"ui\"\npaths = [\"apps/desktop/**\"]\n";
    let file = parse("shop", text).unwrap();
    assert_eq!(file.codebase, "shop");
    assert_eq!(file.version, 3);
    assert_eq!(file.updated, 1_792_591_920_000);
    assert_eq!(file.author.as_deref(), Some("claude opus-5.5"));
    assert_eq!(
        file.parts,
        vec![
            part("payment", &["crates/payment/**", "docs/pay*.md"]),
            part("ui", &["apps/desktop/**"]),
        ]
    );
    assert_eq!(file.source, text);
}

#[test]
fn accepts_a_bare_toml_datetime_and_no_author() {
    let file = parse("x", "version = 1\nupdated = 2026-10-21T14:12:00Z\n").unwrap();
    assert_eq!(file.updated, 1_792_591_920_000);
    assert_eq!(file.author, None);
    assert!(file.parts.is_empty());
}

#[test]
fn build_then_parse_round_trips() {
    let parts = vec![
        part("payment", &["crates/payment/**", "docs/*.md"]),
        part("we\"ird", &["a b/**"]),
    ];
    let built = build("shop", 7, 1_792_591_920_000, Some("tester"), parts).unwrap();
    assert!(built.source.contains("[[part]]"));
    let again = parse("shop", &built.source).unwrap();
    assert_eq!(again, built);
}

#[test]
fn write_then_load_round_trips() {
    let dir = TempDir::new("roundtrip");
    let built = build("x", 1, 0, None, vec![part("all", &["**"])]).unwrap();
    let path = write(dir.path(), &built, false).unwrap();
    assert_eq!(path, map_path(dir.path()));
    let loaded = load(dir.path()).unwrap();
    assert_eq!(loaded.parts, built.parts);
    assert_eq!(loaded.source, built.source);
    assert_eq!(fs::read_to_string(path).unwrap(), built.source);
}

#[test]
fn write_refuses_to_overwrite_without_the_flag() {
    let dir = TempDir::new("overwrite");
    let first = build("x", 1, 0, None, vec![part("one", &["one/**"])]).unwrap();
    let second = build("x", 2, 0, None, vec![part("two", &["two/**"])]).unwrap();
    write(dir.path(), &first, false).unwrap();
    let err = write(dir.path(), &second, false).unwrap_err();
    assert!(matches!(err, MapError::Exists { .. }), "{err}");
    assert_eq!(load(dir.path()).unwrap().parts[0].name, "one");
    write(dir.path(), &second, true).unwrap();
    assert_eq!(load(dir.path()).unwrap().parts[0].name, "two");
}

#[test]
fn write_rejects_a_bad_source_and_a_missing_folder() {
    let dir = TempDir::new("badwrite");
    let mut file = build("x", 1, 0, None, vec![part("a", &["a/**"])]).unwrap();
    file.source = "version = ".to_owned();
    assert!(matches!(
        write(dir.path(), &file, true),
        Err(MapError::Syntax { .. })
    ));
    assert!(!map_path(dir.path()).exists());

    let good = build("x", 1, 0, None, vec![part("a", &["a/**"])]).unwrap();
    let missing = dir.path().join("does-not-exist");
    assert!(matches!(
        write(&missing, &good, true),
        Err(MapError::Io { .. })
    ));
    assert!(!missing.exists());
}

#[test]
fn load_reports_a_missing_file() {
    let dir = TempDir::new("nofile");
    assert!(matches!(load(dir.path()), Err(MapError::Io { .. })));
}

// ---- errors ----

#[test]
fn error_bad_toml_has_the_line() {
    let text = format!("{HEAD}\n[[part]\nname = \"a\"\n");
    let err = parse("x", &text).unwrap_err();
    assert!(matches!(err, MapError::Syntax { .. }), "{err}");
    assert_eq!(err.line(), Some(4), "{err}");

    let err = parse(
        "x",
        "version = 1\nupdated = \"2026-10-21T14:12:00Z\"\nname = = 2\n",
    )
    .unwrap_err();
    assert!(matches!(err, MapError::Syntax { line: 3, .. }), "{err}");
}

#[test]
fn error_missing_name_points_at_the_part_block() {
    let text = format!(
        "{HEAD}\n[[part]]\nname = \"a\"\npaths = [\"a/**\"]\n\n[[part]]\npaths = [\"b/**\"]\n"
    );
    let err = parse("x", &text).unwrap_err();
    assert!(
        matches!(err, MapError::MissingField { field: "name", .. }),
        "{err}"
    );
    assert_eq!(err.line(), Some(8), "{err}");
}

#[test]
fn error_missing_paths_points_at_the_part_block() {
    let text = format!("{HEAD}\n[[part]]\nname = \"a\"\n");
    let err = parse("x", &text).unwrap_err();
    assert!(
        matches!(err, MapError::MissingField { field: "paths", .. }),
        "{err}"
    );
    assert_eq!(err.line(), Some(4), "{err}");
}

#[test]
fn error_empty_paths_has_the_line() {
    let text = format!("{HEAD}\n[[part]]\nname = \"a\"\npaths = []\n");
    let err = parse("x", &text).unwrap_err();
    assert!(
        matches!(&err, MapError::EmptyPaths { part, .. } if part == "a"),
        "{err}"
    );
    assert_eq!(err.line(), Some(6), "{err}");
}

#[test]
fn error_bad_glob_has_the_line_of_the_glob() {
    let text =
        format!("{HEAD}\n[[part]]\nname = \"a\"\npaths = [\n  \"ok/**\",\n  \"crates/[abc\",\n]\n");
    let err = parse("x", &text).unwrap_err();
    assert!(
        matches!(&err, MapError::BadGlob { part, glob, .. } if part == "a" && glob == "crates/[abc"),
        "{err}"
    );
    assert_eq!(err.line(), Some(8), "{err}");
    assert!(err.to_string().contains("line 8"), "{err}");
}

#[test]
fn error_empty_glob_and_empty_name() {
    let text = format!("{HEAD}\n[[part]]\nname = \"a\"\npaths = [\"\"]\n");
    assert!(matches!(
        parse("x", &text).unwrap_err(),
        MapError::BadGlob { line: Some(6), .. }
    ));
    let text = format!("{HEAD}\n[[part]]\nname = \" \"\npaths = [\"a/**\"]\n");
    assert!(matches!(
        parse("x", &text).unwrap_err(),
        MapError::InvalidField {
            field: "name",
            line: 5,
            ..
        }
    ));
}

#[test]
fn error_header_keys() {
    let err = parse("x", "updated = \"2026-10-21T14:12:00Z\"\n").unwrap_err();
    assert!(matches!(
        err,
        MapError::MissingField {
            field: "version",
            ..
        }
    ));
    let err = parse("x", "version = 1\n").unwrap_err();
    assert!(matches!(
        err,
        MapError::MissingField {
            field: "updated",
            ..
        }
    ));
    let err = parse("x", "version = 1\nupdated = \"last week\"\n").unwrap_err();
    assert!(matches!(
        err,
        MapError::InvalidField {
            field: "updated",
            line: 2,
            ..
        }
    ));
    let err = parse(
        "x",
        "version = \"one\"\nupdated = \"2026-10-21T14:12:00Z\"\n",
    )
    .unwrap_err();
    assert!(matches!(
        err,
        MapError::InvalidField {
            field: "version",
            line: 1,
            ..
        }
    ));
    let err = parse("x", "version = -1\nupdated = \"2026-10-21T14:12:00Z\"\n").unwrap_err();
    assert!(matches!(
        err,
        MapError::InvalidField {
            field: "version",
            ..
        }
    ));
}

#[test]
fn matcher_new_rejects_a_bad_glob_without_a_line() {
    let err = PartMatcher::new(&[part("a", &["a/**"]), part("b", &["b/[x"])]).unwrap_err();
    assert!(
        matches!(&err, MapError::BadGlob { part, line: None, .. } if part == "b"),
        "{err}"
    );
    assert_eq!(err.line(), None);
}

#[test]
fn build_rejects_empty_paths_and_bad_globs() {
    assert!(matches!(
        build("x", 1, 0, None, vec![part("a", &[])]),
        Err(MapError::EmptyPaths { .. })
    ));
    assert!(matches!(
        build("x", 1, 0, None, vec![part("a", &["["])]),
        Err(MapError::BadGlob { .. })
    ));
}

// ---- matcher ----

#[test]
fn first_part_in_file_order_wins() {
    let m = PartMatcher::new(&[
        part("docs", &["crates/aivoryd/docs/**"]),
        part("daemon", &["crates/aivoryd/**"]),
        part("everything", &["**"]),
    ])
    .unwrap();
    assert_eq!(m.part_of("crates/aivoryd/docs/a.md"), Some("docs"));
    assert_eq!(m.part_of("crates/aivoryd/src/main.rs"), Some("daemon"));
    assert_eq!(m.part_of("README.md"), Some("everything"));

    // Swap the order: the broad glob now wins for every path.
    let m = PartMatcher::new(&[
        part("everything", &["**"]),
        part("daemon", &["crates/aivoryd/**"]),
    ])
    .unwrap();
    assert_eq!(m.part_of("crates/aivoryd/src/main.rs"), Some("everything"));
}

#[test]
fn a_part_matches_with_any_of_its_globs_and_unowned_files_have_no_part() {
    let m = PartMatcher::new(&[part("payment", &["crates/payment/**", "docs/pay*.md"])]).unwrap();
    assert_eq!(m.part_of("crates/payment/src/lib.rs"), Some("payment"));
    assert_eq!(m.part_of("docs/payments.md"), Some("payment"));
    assert_eq!(m.part_of("docs/other.md"), None);
    assert_eq!(m.part_of("crates/ui/src/lib.rs"), None);
    assert_eq!(m.part_of(""), None);
}

#[test]
fn star_stays_in_one_folder_and_double_star_crosses() {
    let m = PartMatcher::new(&[part("top", &["*"]), part("rs", &["src/**/*.rs"])]).unwrap();
    assert_eq!(m.part_of("Cargo.toml"), Some("top"));
    assert_eq!(m.part_of("src/main.rs"), Some("rs"));
    assert_eq!(m.part_of("src/a/b/c.rs"), Some("rs"));
    assert_eq!(m.part_of("src/a/b/c.txt"), None);
}

#[test]
fn windows_separators_work_in_paths_and_in_globs() {
    let m = PartMatcher::new(&[
        part("daemon", &["crates/aivoryd/**"]),
        part("win", &["apps\\desktop\\**"]),
    ])
    .unwrap();
    assert_eq!(m.part_of("crates\\aivoryd\\src\\main.rs"), Some("daemon"));
    assert_eq!(m.part_of("crates/aivoryd\\src/main.rs"), Some("daemon"));
    assert_eq!(m.part_of("apps/desktop/src/app.ts"), Some("win"));
    assert_eq!(m.part_of("apps\\desktop\\src\\app.ts"), Some("win"));
    assert_eq!(m.part_of(".\\crates\\aivoryd\\a.rs"), Some("daemon"));
    assert_eq!(m.part_of("./crates/aivoryd/a.rs"), Some("daemon"));
    assert_eq!(m.part_of("crates\\other\\a.rs"), None);
}

// ---- init ----

fn fake_workspace(dir: &TempDir) {
    for f in [
        "Cargo.toml",
        "README.md",
        "crates/alpha/src/lib.rs",
        "crates/beta/src/main.rs",
        "crates/gamma/README.md",
        "crates/notes.txt",
        "apps/web/src/index.ts",
        "apps/web/node_modules/dep/index.js",
        "packages/ui/button.tsx",
        "src/app.py",
        "scripts/build.sh",
        "docs/guide.md",
        "odd[1]/lib.rs",
        // Never form a part:
        ".github/hooks/check.sh",
        ".git/config",
        "target/debug/gen.rs",
        "node_modules/pkg/index.js",
        "vendor/lib/a.go",
        "build/out.js",
        "dist/bundle.js",
    ] {
        dir.touch(f);
    }
}

fn names(parts: &[Part]) -> Vec<&str> {
    parts.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn init_groups_a_workspace_layout() {
    let dir = TempDir::new("workspace");
    fake_workspace(&dir);
    let file = init(dir.path());

    assert_eq!(file.version, 1);
    assert_eq!(file.author.as_deref(), Some(INIT_AUTHOR));
    assert_eq!(
        names(&file.parts),
        [
            "web", "alpha", "beta", "odd[1]", "ui", "scripts", "src", "root"
        ]
    );
    let by_name = |n: &str| file.parts.iter().find(|p| p.name == n).unwrap();
    assert_eq!(by_name("web").paths, ["apps/web/**"]);
    assert_eq!(by_name("alpha").paths, ["crates/alpha/**"]);
    assert_eq!(by_name("ui").paths, ["packages/ui/**"]);
    assert_eq!(by_name("scripts").paths, ["scripts/**"]);
    assert_eq!(by_name("src").paths, ["src/**"]);
    assert_eq!(by_name("root").paths, ["*"]);
    assert_eq!(file.parts.last().unwrap().name, "root");

    // The generated text is a valid map and it says the same thing.
    let again = parse(&file.codebase, &file.source).unwrap();
    assert_eq!(again, file);
    assert!(file.source.contains("author = \"aivoryd map init\""));
    assert!(file.source.contains("version = 1"));
}

#[test]
fn init_parts_match_the_files_they_were_made_from() {
    let dir = TempDir::new("matchinit");
    fake_workspace(&dir);
    let file = init(dir.path());
    let m = PartMatcher::new(&file.parts).unwrap();
    assert_eq!(m.part_of("crates/alpha/src/lib.rs"), Some("alpha"));
    assert_eq!(m.part_of("crates\\beta\\src\\main.rs"), Some("beta"));
    assert_eq!(m.part_of("apps/web/src/index.ts"), Some("web"));
    assert_eq!(m.part_of("odd[1]/lib.rs"), Some("odd[1]"));
    assert_eq!(m.part_of("src/app.py"), Some("src"));
    assert_eq!(m.part_of("Cargo.toml"), Some("root"));
    assert_eq!(m.part_of(".gitignore"), Some("root"));
    // A folder with no source files, a loose file in a workspace folder, and
    // skipped folders belong to no part.
    assert_eq!(m.part_of("docs/guide.md"), None);
    assert_eq!(m.part_of("crates/gamma/README.md"), None);
    assert_eq!(m.part_of("crates/notes.txt"), None);
    assert_eq!(m.part_of("target/debug/gen.rs"), None);
}

#[test]
fn init_names_do_not_collide() {
    let dir = TempDir::new("collide");
    dir.touch("apps/core/a.rs");
    dir.touch("crates/core/b.rs");
    dir.touch("root/c.rs");
    dir.touch("main.rs");
    let file = init(dir.path());
    assert_eq!(
        names(&file.parts),
        ["core", "crates-core", "root-2", "root"]
    );
    assert_eq!(file.parts[1].paths, ["crates/core/**"]);
    assert_eq!(file.parts[2].paths, ["root/**"]);
}

#[test]
fn init_uses_a_workspace_folder_as_one_part_when_no_child_has_source() {
    let dir = TempDir::new("flatws");
    dir.touch("packages/index.ts");
    dir.touch("packages/docs/readme.md");
    let file = init(dir.path());
    assert_eq!(names(&file.parts), ["packages"]);
    assert_eq!(file.parts[0].paths, ["packages/**"]);
}

#[test]
fn init_skips_folders_that_hold_only_skipped_or_non_source_files() {
    let dir = TempDir::new("skips");
    dir.touch("app/node_modules/x.js");
    dir.touch("app/.cache/y.rs");
    dir.touch("app/readme.md");
    dir.touch("lib/deep/er/still/z.go");
    let file = init(dir.path());
    assert_eq!(names(&file.parts), ["lib"]);
}

#[test]
fn init_on_an_empty_or_missing_folder_gives_no_parts() {
    let dir = TempDir::new("empty");
    let file = init(dir.path());
    assert!(file.parts.is_empty());
    parse(&file.codebase, &file.source).unwrap();
    assert!(init(&dir.path().join("missing")).parts.is_empty());
}

#[test]
fn init_then_write_refuses_a_second_write() {
    let dir = TempDir::new("initwrite");
    dir.touch("src/main.rs");
    let file = init(dir.path());
    write(dir.path(), &file, false).unwrap();
    assert!(matches!(
        write(dir.path(), &init(dir.path()), false),
        Err(MapError::Exists { .. })
    ));
    let loaded = load(dir.path()).unwrap();
    assert_eq!(names(&loaded.parts), ["src"]);
    assert_eq!(
        loaded.codebase,
        dir.path().file_name().unwrap().to_string_lossy()
    );
}

#[test]
fn add_part_keeps_comments_and_other_parts() {
    let dir = TempDir::new("add-part");
    let text = "# my own comment\nversion = 1\nupdated = \"2026-01-01T00:00:00Z\"\n\n# the api\n[[part]]\nname  = \"api\"\npaths = [\"api/**\"]\n";
    fs::create_dir_all(dir.path().join(".aivory")).unwrap();
    fs::write(map_path(dir.path()), text).unwrap();
    let map = aivory_map::add_part(dir.path(), "web", &["web/**".into(), "*.html".into()]).unwrap();
    assert_eq!(map.parts.len(), 2);
    assert_eq!(map.parts[1].name, "web");
    assert!(map.updated > 1_767_225_600_000);
    let disk = fs::read_to_string(map_path(dir.path())).unwrap();
    assert!(disk.contains("# my own comment"));
    assert!(disk.contains("# the api"));
    assert!(disk.contains("name  = \"api\""));
    assert!(disk.contains("name  = \"web\""));
    assert_eq!(load(dir.path()).unwrap().parts, map.parts);
}

#[test]
fn add_part_creates_the_map_when_it_is_missing() {
    let dir = TempDir::new("add-part-new");
    dir.touch("src/main.rs");
    let map = aivory_map::add_part(dir.path(), "docs", &["docs/**".into()]).unwrap();
    assert!(map_path(dir.path()).is_file());
    assert!(map.parts.iter().any(|p| p.name == "docs"));
    assert!(map.parts.iter().any(|p| p.name == "src"));
}

#[test]
fn add_part_checks_its_input_and_writes_nothing_on_error() {
    let dir = TempDir::new("add-part-bad");
    assert!(matches!(
        aivory_map::add_part(dir.path(), "x", &["a[".into()]),
        Err(MapError::BadGlob { .. })
    ));
    assert!(matches!(
        aivory_map::add_part(dir.path(), " ", &["a/**".into()]),
        Err(MapError::InvalidField { .. })
    ));
    assert!(matches!(
        aivory_map::add_part(dir.path(), "x", &[]),
        Err(MapError::EmptyPaths { .. })
    ));
    assert!(!map_path(dir.path()).exists());
    aivory_map::add_part(dir.path(), "x", &["x/**".into()]).unwrap();
    assert!(matches!(
        aivory_map::add_part(dir.path(), "x", &["y/**".into()]),
        Err(MapError::DuplicatePart { .. })
    ));
}
