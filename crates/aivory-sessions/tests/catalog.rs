//! Fixture tests for the three stores. The fixtures contain synthetic data only.

use std::path::PathBuf;

use aivory_sessions::{
    FindError, Provider, SessionCatalog, SessionFilter, SessionSummary, StoreRoots,
};

fn fixture_roots() -> StoreRoots {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    StoreRoots {
        claude: base.join("claude"),
        codex: base.join("codex"),
        gemini: base.join("gemini"),
    }
}

fn by_id<'a>(all: &'a [SessionSummary], id: &str) -> &'a SessionSummary {
    all.iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("session {id} missing"))
}

#[test]
fn lists_all_three_providers() {
    let report = SessionCatalog::new(&fixture_roots()).scan();
    let count = |p: Provider| report.sessions.iter().filter(|s| s.provider == p).count();
    assert_eq!(
        count(Provider::Claude),
        2,
        "the file without messages is skipped"
    );
    assert_eq!(count(Provider::Codex), 1);
    assert_eq!(count(Provider::Gemini), 2);
}

#[test]
fn claude_fields_and_sidechains() {
    let all = SessionCatalog::new(&fixture_roots()).scan().sessions;
    let s = by_id(&all, "11111111-1111-4111-8111-111111111111");
    assert_eq!(s.cwd.as_deref(), Some("C:\\synthetic\\app"));
    assert_eq!(
        s.title.as_deref(),
        Some("Synthetic fixture title"),
        "ai-title beats last-prompt and first prompt"
    );
    assert_eq!(
        s.git_branch.as_deref(),
        Some("feature/x"),
        "latest branch wins"
    );
    assert_eq!(s.cli_version.as_deref(), Some("2.0.0-test"));
    assert_eq!(s.model.as_deref(), Some("claude-test-1"));
    assert!(s.created.is_some());
    assert_eq!(
        s.sidechains.len(),
        2,
        "one sub-agent file and one old-style agent file"
    );

    let custom = by_id(&all, "55555555-5555-4555-8555-555555555555");
    assert_eq!(custom.title.as_deref(), Some("Synthetic custom title"));
    assert_eq!(custom.model, None, "the <synthetic> model is ignored");
    assert!(custom.sidechains.is_empty());
}

#[test]
fn codex_fields() {
    let all = SessionCatalog::new(&fixture_roots()).scan().sessions;
    let s = by_id(&all, "22222222-2222-4222-8222-222222222222");
    assert_eq!(s.cwd.as_deref(), Some("C:\\synthetic\\codex-app"));
    assert_eq!(s.title.as_deref(), Some("Synthetic codex prompt"));
    assert_eq!(s.git_branch.as_deref(), Some("develop"));
    assert_eq!(s.cli_version.as_deref(), Some("0.0.1-test"));
    assert_eq!(s.model.as_deref(), Some("gpt-test"));
}

#[test]
fn gemini_json_and_jsonl_formats() {
    let all = SessionCatalog::new(&fixture_roots()).scan().sessions;
    let old = by_id(&all, "33333333-3333-4333-8333-333333333333");
    assert_eq!(
        old.cwd.as_deref(),
        Some("c:\\synthetic\\gemini-a"),
        "cwd from .project_root"
    );
    assert_eq!(old.title.as_deref(), Some("Synthetic gemini prompt"));
    assert_eq!(old.model.as_deref(), Some("gemini-test"));

    let new = by_id(&all, "44444444-4444-4444-8444-444444444444");
    assert_eq!(
        new.cwd.as_deref(),
        Some("c:\\synthetic\\gemini-b"),
        "cwd from projects.json"
    );
    assert_eq!(new.title.as_deref(), Some("Synthetic gemini summary"));
    assert_eq!(new.model.as_deref(), Some("gemini-test-2"));
}

#[test]
fn filter_and_find() {
    let catalog = SessionCatalog::new(&fixture_roots());
    let only_codex = catalog.list(&SessionFilter {
        provider: Some(Provider::Codex),
        ..SessionFilter::default()
    });
    assert_eq!(only_codex.len(), 1);

    let below_app = catalog.list(&SessionFilter {
        cwd: Some("C:/synthetic/app/".into()),
        ..SessionFilter::default()
    });
    assert_eq!(
        below_app.len(),
        2,
        "cwd filter matches the directory and its children, in any slash style"
    );

    assert_eq!(
        catalog.find("2222", None).unwrap().provider,
        Provider::Codex
    );
    assert!(matches!(
        catalog.find("nope", None),
        Err(FindError::NotFound(_))
    ));
    assert!(matches!(
        catalog.find("", None),
        Err(FindError::NotFound(_))
    ));
}

#[test]
fn cache_is_used_on_second_scan() {
    let dir = std::env::temp_dir().join(format!("aivory-sessions-test-{}", std::process::id()));
    let cache = dir.join("cache.json");
    let catalog = SessionCatalog::new(&fixture_roots()).with_cache_file(Some(cache.clone()));

    let cold = catalog.scan();
    assert!(cold.stats.parsed >= 5);
    assert_eq!(cold.stats.cache_hits, 0);

    let warm = catalog.scan();
    assert_eq!(
        warm.stats.parsed, 0,
        "nothing re-parsed when no file changed"
    );
    assert_eq!(warm.sessions, cold.sessions);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn missing_roots_give_an_empty_list() {
    let nowhere = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("does-not-exist");
    let roots = StoreRoots {
        claude: nowhere.clone(),
        codex: nowhere.clone(),
        gemini: nowhere,
    };
    assert!(SessionCatalog::new(&roots).scan().sessions.is_empty());
}
