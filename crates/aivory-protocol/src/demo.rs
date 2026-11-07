//! The sample data of the accepted design (`docs/design/screens/*.dc.html`), ported from the
//! script blocks of the artboards. `aivoryd serve --demo` serves it, so that both UIs run
//! without real sessions, and `proto/fixtures/` holds it as JSON for the client tests.
//!
//! The demo clock stands at [`DEMO_NOW`] (2026-10-21 14:40 UTC, the clock of the artboards).
//! Where the artboards disagree (the queue count, a context percentage), the demo keeps one
//! value for all screens.

use std::collections::BTreeMap;

use crate::common::*;
use crate::inbox::*;
use crate::repos::*;
use crate::session::*;
use crate::settings::*;
use crate::tasks::*;
use crate::warroom::*;

/// 2026-10-21T14:40:00Z.
pub const DEMO_NOW: UnixMs = 1_792_593_600_000;
const DAY_MS: i64 = 86_400_000;
/// Days from the epoch to 2026-10-21.
const DEMO_DAY: i64 = 20_747;

fn ago(minutes: f64) -> UnixMs {
    DEMO_NOW - (minutes * MINUTE_MS as f64).round() as i64
}

/// Midnight UTC of a day of October 2026 (0 = September 30).
fn october(day: i64) -> UnixMs {
    (DEMO_DAY - 21 + day) * DAY_MS
}

fn s(v: &str) -> String {
    v.to_string()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn state(code: char) -> SessionState {
    SessionState::from_code(code).unwrap_or(SessionState::Working)
}

/// (id, parts, path, worktrees, daemon, languages). The branch is `main` everywhere.
type CodebaseSeed = (
    &'static str,
    &'static [&'static str],
    &'static str,
    u32,
    &'static str,
    &'static [&'static str],
);

#[rustfmt::skip]
const CODEBASES: [CodebaseSeed; 10] = [
    ("aivory-code", &["ui", "orchestrator", "ledger", "sync"], "~/work/aivory-code", 4, "desktop", &["Rust"]),
    ("aivory-guard", &["scanner", "api", "rules", "dashboard"], "~/work/aivory-guard", 3, "desktop", &["Python", "TS"]),
    ("billing", &["payment", "invoices", "database"], "~/work/billing", 2, "build-01 (ssh)", &["Rust", "SQL"]),
    ("smart-inference", &["router", "cache", "metering"], "~/work/smart-inference", 3, "build-01 (ssh)", &["Go"]),
    ("kronk", &["git", "runner", "mcp"], "~/work/kronk", 2, "desktop", &["Rust"]),
    ("slaide", &["editor", "render", "export"], "~/work/slaide", 3, "runner-eu", &["Python", "TS"]),
    ("aivory-hugo", &["pages", "theme", "i18n"], "~/work/aivory-hugo", 1, "desktop", &["Hugo"]),
    ("mobile", &["ui", "sync", "push"], "~/work/mobile", 1, "desktop", &["Kotlin"]),
    ("docs", &["guides", "api-ref"], "~/work/docs", 0, "desktop", &["Markdown"]),
    ("infra", &["k8s", "ci"], "~/work/infra", 0, "runner-eu", &["YAML"]),
];

fn part_paths(codebase: &str, part: &str) -> Vec<String> {
    let known: &[&str] = match (codebase, part) {
        ("aivory-code", "ui") => &["crates/ui/**", "assets/**"],
        ("aivory-code", "orchestrator") => &["crates/orch/**"],
        ("aivory-code", "ledger") => &["crates/ledger/**"],
        ("aivory-code", "sync") => &["crates/sync/**", ".aivory/map.toml"],
        ("aivory-guard", "scanner") => &["guard/scanner/**"],
        ("aivory-guard", "api") => &["guard/api/**"],
        ("aivory-guard", "rules") => &["rules/**", "guard/rules/**"],
        ("aivory-guard", "dashboard") => &["web/**"],
        ("billing", "payment") => &["src/payment/**"],
        ("billing", "invoices") => &["billing/invoices/**"],
        ("billing", "database") => &["billing/db/**", "migrations/**"],
        _ => return vec![format!("{part}/**")],
    };
    strings(known)
}

pub fn codebases() -> Vec<Codebase> {
    CODEBASES
        .iter()
        .map(|(id, parts, path, worktrees, daemon, langs)| Codebase {
            id: s(id),
            path: s(path),
            branch: s("main"),
            worktrees: *worktrees,
            daemon: s(daemon),
            languages: strings(langs),
            parts: parts
                .iter()
                .map(|p| Part {
                    name: s(p),
                    paths: part_paths(id, p),
                })
                .collect(),
        })
        .collect()
}

type Ask0 = (
    AskKind,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// A session of the war room (`S` in `Main.dc.html`).
struct Seed {
    id: &'static str,
    cb: &'static str,
    pt: &'static str,
    ag: &'static str,
    m: &'static str,
    t: &'static str,
    st: char,
    mn: f64,
    c: f64,
    pr: &'static str,
    f: &'static [&'static str],
    tail: &'static [(char, &'static str)],
    ask: Option<Ask0>,
    task: Option<&'static str>,
}

#[rustfmt::skip]
const SEEDS: [Seed; 24] = [
    Seed { id: "7f3a", cb: "billing", pt: "payment", ag: "claude", m: "opus-5.5", t: "Stripe webhook retries", st: 'q', mn: 6.0, c: 2.41, pr: "add retry with exponential backoff to the stripe webhook handler", f: &["src/payment/webhook.rs", "src/payment/retry.rs"], tail: &[('t', "Edit  src/payment/retry.rs"), ('p', "+ let delay = BASE_MS * 2u64.pow(attempt).min(30_000);"), ('m', "- thread::sleep(Duration::from_secs(1));"), ('s', "↳ test-writer  4 cases for retry::backoff  ✓"), ('s', "↳ migrator  wrote 0042_webhook_attempts.sql"), ('a', "The retry table needs a new column. I want to apply the migration to staging.")], ask: Some((AskKind::Permission, "sqlx migrate run --database-url $STAGING_DB", &["Allow", "Always allow", "Deny"], &[])), task: Some("t8") },
    Seed { id: "c218", cb: "aivory-code", pt: "ui", ag: "codex", m: "gpt-5.2-codex", t: "Virtualize the session grid", st: 'q', mn: 3.0, c: 0.88, pr: "the session grid lags with 40 tiles. virtualize it", f: &["crates/ui/src/grid.rs", "crates/ui/src/tile.rs"], tail: &[('t', "Bash  cargo bench -p ui grid_render"), ('o', "grid_render/40  time: [18.2 ms 18.6 ms 19.1 ms]"), ('a', "40 tiles cost 18 ms per frame. Most of it is text layout of hidden rows."), ('a', "The plan is ready.")], ask: Some((AskKind::Plan, "Virtualize the grid in 4 steps.", &["Approve", "Edit plan", "Reject"], &["Measure tile rows and keep a row index", "Lay out only the rows in the viewport, plus one", "Cache shaped text per session line", "Bench again. Target: under 4 ms"])), task: Some("t9") },
    Seed { id: "9b0e", cb: "aivory-guard", pt: "rules", ag: "gemini", m: "3-pro", t: "OWASP rule pack v2", st: 'q', mn: 2.0, c: 0.52, pr: "port the OWASP top-10 rules to the v2 rule format", f: &["rules/owasp/a03_injection.yaml", "guard/rules/loader.py"], tail: &[('t', "Write  rules/v2/a03_injection.toml"), ('p', "+ [[match]] pattern = \"cursor.execute(f-string)\""), ('s', "↳ rule-tester  38 of 41 fixtures pass"), ('a', "Three fixtures only match with the legacy regex engine.")], ask: Some((AskKind::Question, "Keep the legacy regex rules as a fallback, or drop them in v2?", &["Keep as fallback", "Drop them"], &[])), task: Some("t10") },
    Seed { id: "a01c", cb: "aivory-code", pt: "orchestrator", ag: "claude", m: "opus-5.5", t: "Subagent fan-out limits", st: 'w', mn: 23.0, c: 3.12, pr: "cap subagent fan-out per session and per codebase", f: &["crates/orch/src/spawn.rs", "crates/orch/src/budget.rs"], tail: &[('t', "Edit  crates/orch/src/budget.rs"), ('p', "+ pub max_children: u8,      // default 4"), ('p', "+ pub max_per_codebase: u8, // default 12"), ('s', "↳ explorer  mapped 14 spawn call sites"), ('s', "↳ test-writer  writing budget::exceeds"), ('s', "↳ reviewer  waits for the diff"), ('t', "Bash  cargo test -p orch budget"), ('o', "test result: ok. 22 passed; 0 failed"), ('a', "Now I wire the budget into the spawn path.")], ask: None, task: Some("t4") },
    Seed { id: "a7e2", cb: "aivory-code", pt: "ledger", ag: "claude", m: "sonnet-5.5", t: "Ledger compaction", st: 'w', mn: 9.0, c: 0.74, pr: "compact ledger segments older than 7 days", f: &["crates/ledger/src/segment.rs", "crates/ledger/src/compact.rs"], tail: &[('t', "Edit  crates/ledger/src/compact.rs"), ('p', "+ fn merge(a: &Segment, b: &Segment) -> Segment {"), ('p', "+     let mut out = Segment::with_capacity(a.len() + b.len());"), ('p', "+     out.extend(a.newest_per_key(b));"), ('a', "Merge keeps the newest entry per key. Next: tombstones.")], ask: None, task: Some("t11") },
    Seed { id: "d3f0", cb: "aivory-code", pt: "sync", ag: "goose", m: "goose-1.9", t: "CRDT merge for map.toml", st: 'i', mn: 12.0, c: 0.40, pr: "merge concurrent edits of .aivory/map.toml from two agents", f: &["crates/sync/src/map_merge.rs", ".aivory/map.toml"], tail: &[('t', "Edit  crates/sync/src/map_merge.rs"), ('p', "+ parts.sort_by(|a, b| a.path.cmp(&b.path));"), ('t', "Bash  cargo test -p sync map_merge"), ('o', "test result: ok. 9 passed; 0 failed"), ('a', "Done. The merge does not depend on the order now."), ('x', "— idle · waits for a next task —")], ask: None, task: Some("t13") },
    Seed { id: "5e81", cb: "aivory-guard", pt: "scanner", ag: "claude", m: "opus-5.5", t: "Incremental scan cache", st: 'w', mn: 31.0, c: 4.05, pr: "only rescan files whose hash changed since the last scan", f: &["guard/scanner/walker.py", "guard/scanner/cache.py"], tail: &[('t', "Edit  guard/scanner/cache.py"), ('p', "+ if self.index.get(path) == digest:"), ('p', "+     return CachedResult(path)"), ('s', "↳ bench  full scan 41.2s → incremental 3.8s"), ('s', "↳ test-writer  cache invalidation on rename"), ('a', "Rename handling needs the inode. I add it to the index.")], ask: None, task: Some("t5") },
    Seed { id: "6c40", cb: "aivory-guard", pt: "rules", ag: "claude", m: "sonnet-5.5", t: "Secrets rule tuning", st: 'w', mn: 15.0, c: 0.95, pr: "cut the false positives in the secrets rules", f: &["rules/secrets.toml", "guard/rules/entropy.py"], tail: &[('t', "Bash  aivory-guard eval rules/secrets --corpus fp-2026"), ('o', "false positives 212 → 37"), ('s', "↳ corpus-builder  added 18 negative samples"), ('a', "An entropy threshold of 4.2 removes most of them.")], ask: None, task: None },
    Seed { id: "6f19", cb: "aivory-guard", pt: "dashboard", ag: "gemini", m: "3-flash", t: "Findings heatmap", st: 'i', mn: 4.0, c: 0.22, pr: "show a heatmap of findings per directory", f: &["web/src/Heatmap.tsx", "web/src/api.ts"], tail: &[('t', "Write  web/src/Heatmap.tsx"), ('p', "+ const cell = scale(count) // 0..4"), ('a', "The heatmap renders. The task is complete."), ('x', "— idle —")], ask: None, task: None },
    Seed { id: "8a22", cb: "billing", pt: "invoices", ag: "gemini", m: "3-pro", t: "Invoice PDF layout", st: 'w', mn: 11.0, c: 0.58, pr: "match the new invoice PDF layout from the design file", f: &["billing/invoices/pdf.ts", "billing/invoices/template.hbs"], tail: &[('t', "Edit  billing/invoices/template.hbs"), ('p', "+ <td class=\"num\">fmt(total)</td>"), ('t', "Bash  npm run render:sample"), ('o', "wrote out/invoice-sample.pdf (2 pages)"), ('a', "Page 2 breaks inside the table. I add a keep-together rule.")], ask: None, task: None },
    Seed { id: "8d07", cb: "billing", pt: "database", ag: "claude", m: "sonnet-5.5", t: "Invoice lookup index", st: 'w', mn: 5.0, c: 0.33, pr: "speed up the invoice lookup by customer and month", f: &["billing/db/schema.sql", "billing/db/queries.sql"], tail: &[('t', "Bash  psql -c \"EXPLAIN ANALYZE select ...\""), ('o', "Seq Scan on invoices  (actual time=412.8 ms)"), ('p', "+ CREATE INDEX invoices_cust_month ON invoices (customer_id, month);"), ('t', "Bash  psql -c \"EXPLAIN ANALYZE select ...\""), ('o', "Index Scan using invoices_cust_month  (actual time=0.9 ms)")], ask: None, task: None },
    Seed { id: "41dd", cb: "smart-inference", pt: "cache", ag: "opencode", m: "kimi-k2", t: "Redis failover", st: 'e', mn: 1.0, c: 0.31, pr: "make the response cache survive a redis primary failover", f: &["cache/redis_pool.go", "cache/failover_test.go"], tail: &[('t', "Edit  cache/redis_pool.go"), ('p', "+ opts.MaxRetries = 3"), ('t', "Bash  go test ./cache -run Failover"), ('e', "--- FAIL: TestFailover (5.02s)"), ('e', "    dial tcp 127.0.0.1:6380: connect: connection refused"), ('a', "The test needs a sentinel on :6380. It does not run.")], ask: Some((AskKind::Error, "Test infra is missing: no sentinel on :6380.", &["Start sentinel", "Retry", "Open log"], &[])), task: None },
    Seed { id: "90c4", cb: "smart-inference", pt: "router", ag: "claude", m: "opus-5.5", t: "Model routing table", st: 'w', mn: 18.0, c: 2.20, pr: "route by task class with a fallback chain per provider", f: &["router/table.go", "router/classify.go"], tail: &[('t', "Edit  router/table.go"), ('p', "+ {Class: \"refactor\", Chain: []string{\"opus\", \"gpt-5.2\", \"gemini-3\"}},"), ('s', "↳ eval-runner  312 prompts, 94.1% routed as expected"), ('a', "Most misroutes are short questions. I add a length feature.")], ask: None, task: Some("t6") },
    Seed { id: "93e8", cb: "smart-inference", pt: "metering", ag: "codex", m: "gpt-5.2-codex", t: "Token metering", st: 'w', mn: 6.0, c: 0.47, pr: "meter tokens per org and push them to billing every minute", f: &["metering/counter.go", "metering/push.go"], tail: &[('t', "Edit  metering/push.go"), ('p', "+ ticker := time.NewTicker(time.Minute)"), ('t', "Bash  go test ./metering"), ('o', "ok  metering  0.84s")], ask: None, task: None },
    Seed { id: "k1b7", cb: "kronk", pt: "mcp", ag: "claude", m: "sonnet-5.5", t: "MCP tool timeouts", st: 'w', mn: 8.0, c: 0.51, pr: "long git ops over MCP time out. make the timeouts per tool", f: &["kronk/mcp/server.rs", "kronk/mcp/tools.rs"], tail: &[('t', "Edit  kronk/mcp/tools.rs"), ('p', "+ \"git.clone\" => Duration::from_secs(120),"), ('p', "+ _ => Duration::from_secs(30),"), ('a', "Clone gets 120 s. All other tools keep 30 s.")], ask: None, task: None },
    Seed { id: "k4c2", cb: "kronk", pt: "runner", ag: "claude", m: "opus-5.5", t: "Runner sandbox seccomp", st: 'w', mn: 26.0, c: 2.88, pr: "add a seccomp profile to the runner sandbox", f: &["kronk/runner/sandbox.rs", "kronk/runner/seccomp.json"], tail: &[('t', "Write  kronk/runner/seccomp.json"), ('p', "+ { \"names\": [\"ptrace\", \"mount\"], \"action\": \"SCMP_ACT_ERRNO\" }"), ('s', "↳ fuzzer  2,000 syscalls, 0 escapes"), ('t', "Bash  cargo test -p runner sandbox"), ('o', "test result: ok. 17 passed; 0 failed")], ask: None, task: Some("t12") },
    Seed { id: "s3d1", cb: "slaide", pt: "render", ag: "claude", m: "opus-5.5", t: "PPTX chart export", st: 'w', mn: 14.0, c: 1.66, pr: "export charts as native pptx charts, not as images", f: &["slaide/export/pptx/chart.py", "slaide/export/pptx/xml.py"], tail: &[('t', "Edit  slaide/export/pptx/chart.py"), ('p', "+ ser = c_ser(idx=i, tx=name, val=num_ref(sheet, col))"), ('s', "↳ fixture-maker  bar, line and pie decks"), ('s', "↳ validator  opens the decks in LibreOffice"), ('a', "Bar and line export as native charts. Pie is next.")], ask: None, task: Some("t7") },
    Seed { id: "s7a0", cb: "slaide", pt: "editor", ag: "opencode", m: "kimi-k2", t: "Undo stack", st: 'i', mn: 18.0, c: 0.29, pr: "group text edits into one undo step per word", f: &["web/editor/history.ts"], tail: &[('t', "Edit  web/editor/history.ts"), ('p', "+ if (isWordBoundary(prev, next)) this.commit()"), ('a', "Undo groups by word now. The task is complete."), ('x', "— idle —")], ask: None, task: None },
    Seed { id: "s9e2", cb: "slaide", pt: "export", ag: "codex", m: "gpt-5.2", t: "Embed fonts in PDF export", st: 'w', mn: 7.0, c: 0.34, pr: "embed the deck fonts in the pdf export", f: &["slaide/export/pdf/fonts.py", "slaide/export/pdf/writer.py"], tail: &[('t', "Edit  slaide/export/pdf/fonts.py"), ('p', "+ subset = font.subset(used_glyphs(deck))"), ('t', "Bash  pytest tests/export -k fonts"), ('o', "5 passed in 2.10s")], ask: None, task: None },
    Seed { id: "h8f3", cb: "aivory-hugo", pt: "i18n", ag: "gemini", m: "3-flash", t: "German translations", st: 'w', mn: 13.0, c: 0.37, pr: "translate the new docs pages to german", f: &["i18n/de.toml", "content/de/docs/war-room.md"], tail: &[('t', "Write  content/de/docs/war-room.md"), ('p', "+ title = \"Kommandozentrale\""), ('s', "↳ glossary-check  4 terms differ from the glossary"), ('a', "I align the 4 terms with the glossary.")], ask: None, task: None },
    Seed { id: "m6b4", cb: "mobile", pt: "sync", ag: "claude", m: "sonnet-5.5", t: "Offline answer queue", st: 'w', mn: 9.0, c: 0.62, pr: "queue answers offline and send them on reconnect", f: &["app/sync/Queue.kt", "app/sync/Net.kt"], tail: &[('t', "Edit  app/sync/Queue.kt"), ('p', "+ suspend fun flush() = pending.forEach { send(it) }"), ('t', "Bash  ./gradlew testDebug --tests \"*Queue*\""), ('o', "BUILD SUCCESSFUL in 38s")], ask: None, task: None },
    Seed { id: "o2c4", cb: "docs", pt: "guides", ag: "gemini", m: "3-flash", t: "War room guide", st: 'p', mn: 30.0, c: 0.21, pr: "write the getting started guide for the war room", f: &["guides/start.md", "guides/war-room.md"], tail: &[('t', "Write  guides/war-room.md"), ('p', "+ ## Answer an agent"), ('p', "+ Hover a pane. The input folds up from the bottom."), ('a', "Next: one screenshot for each step.")], ask: None, task: None },
    Seed { id: "i0a8", cb: "infra", pt: "ci", ag: "claude", m: "haiku-5.5", t: "Cache cargo builds in CI", st: 'd', mn: 40.0, c: 0.18, pr: "cache cargo builds in ci", f: &[".github/workflows/ci.yml"], tail: &[], ask: None, task: Some("t14") },
    Seed { id: "i3f2", cb: "infra", pt: "k8s", ag: "codex", m: "gpt-5.2-codex", t: "Node pool resize", st: 'p', mn: 55.0, c: 0.35, pr: "resize the node pool", f: &["k8s/pool.yaml"], tail: &[], ask: None, task: None },
];

/// The session of the conflict in the inbox. It is not in the war room of the artboard.
#[rustfmt::skip]
const CONFLICT_SEED: Seed = Seed { id: "b5c9", cb: "aivory-code", pt: "ui", ag: "claude", m: "haiku-5.5", t: "Tile skeleton states", st: 'w', mn: 2.0, c: 0.97, pr: "add skeleton states for tiles while a session attaches", f: &["crates/ui/src/tile.rs"], tail: &[('t', "Edit  crates/ui/src/tile.rs"), ('p', "+ TileState::Attaching => self.skeleton(cx),"), ('x', "— aivoryd: trial merge with session c218 fails —")], ask: None, task: None };

/// (name, state, task, command, output) per session (`SUBS` in `Main.dc.html`).
type SubSeed = (&'static str, char, &'static str, &'static str, &'static str);

#[rustfmt::skip]
fn sub_seeds(id: &str) -> &'static [SubSeed] {
    match id {
        "7f3a" => &[("test-writer", 'd', "write 4 cases for retry::backoff", "cargo test -p payment retry", "test result: ok. 4 passed; 0 failed"), ("migrator", 'd', "write a migration for webhook attempts", "sqlx migrate add webhook_attempts", "wrote migrations/0042_webhook_attempts.sql")],
        "9b0e" => &[("rule-tester", 'w', "run all 41 fixtures against the v2 rules", "aivory-guard test rules/v2 --fixtures owasp", "38 of 41 fixtures pass")],
        "a01c" => &[("explorer", 'd', "map every spawn call site", "rg -n \"spawn_child\" crates/", "14 call sites in 6 files"), ("test-writer", 'w', "write tests for budget::exceeds", "cargo test -p orch budget::exceeds", "running 5 tests"), ("reviewer", 'i', "review the budget diff when it is ready", "git diff --stat", "waits for the main agent")],
        "5e81" => &[("bench", 'd', "compare a full scan with an incremental scan", "aivory-guard bench scan --repo fixtures/large", "full 41.2s · incremental 3.8s"), ("test-writer", 'w', "cover cache invalidation on rename", "pytest tests/scanner -k rename", "2 passed, 1 failed")],
        "6c40" => &[("corpus-builder", 'w', "collect negative samples for the secrets rules", "aivory-guard corpus add --negative", "added 18 samples")],
        "90c4" => &[("eval-runner", 'w', "run the 312 routing prompts", "go run ./cmd/eval --set routing-312", "94.1% routed as expected")],
        "k4c2" => &[("fuzzer", 'w', "fuzz the sandbox with 2,000 syscalls", "cargo run -p runner --bin fuzz -- --n 2000", "0 escapes so far")],
        "s3d1" => &[("fixture-maker", 'd', "make bar, line and pie test decks", "python -m slaide.fixtures charts", "wrote 3 decks"), ("validator", 'w', "open each deck in LibreOffice", "soffice --headless --convert-to pdf out/*.pptx", "bar.pptx ok · line.pptx ok")],
        "h8f3" => &[("glossary-check", 'w', "compare the new terms with the glossary", "aivory i18n check --lang de", "4 terms differ")],
        _ => &[],
    }
}

fn utf16_len(v: &str) -> usize {
    v.encode_utf16().count()
}

fn parent_dir(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir.to_string(),
        _ => s("."),
    }
}

/// A design line `[kind, body]` as a protocol line.
pub(crate) fn design_line(kind: char, body: &str) -> Line {
    match kind {
        't' => {
            let (name, rest) = body.split_once(' ').unwrap_or((body, ""));
            Line::tool(name, rest.trim_start())
        }
        'u' => Line::user(body),
        'o' => Line::output(body),
        'p' => Line::added(body.strip_prefix("+ ").unwrap_or(body)),
        'm' => Line::removed(body.strip_prefix("- ").unwrap_or(body)),
        's' => {
            let body = body.strip_prefix("↳ ").unwrap_or(body);
            let (name, rest) = body.split_once("  ").unwrap_or((body, ""));
            Line::subagent(name, rest)
        }
        'e' => Line::error(body),
        'x' => Line::meta(body),
        'h' => {
            let (label, rest) = body.split_once(" · ").unwrap_or((body, ""));
            Line::turn(label, rest)
        }
        _ => Line::agent(body),
    }
}

/// The full main transcript of a session (`lines()` of `Main.dc.html`).
fn main_lines(seed: &Seed) -> Vec<Line> {
    let h = (utf16_len(seed.t) * 31 + utf16_len(seed.pr) * 7) % 97;
    let dir = parent_dir(seed.f[0]);
    let pt = seed.pt;
    let mut l = vec![
        Line::meta(format!(
            "{} {} · {} · session {}",
            seed.ag, seed.m, seed.cb, seed.id
        )),
        Line::user(format!(
            "read .aivory/map.toml and stay inside part \"{pt}\""
        )),
        Line::agent(format!(
            "Part \"{pt}\" maps to {dir}/. I keep my edits inside it."
        )),
        Line::tool("Read", "README.md"),
        Line::output(format!("{} lines", 60 + h % 40)),
        Line::tool("Glob", format!("\"{dir}/**\"")),
        Line::output(format!("{} files", 8 + h % 30)),
        Line::agent("I know the layout of the part. Ready for the task."),
        Line::user(seed.pr),
        Line::agent(format!("I start with the code in part \"{pt}\".")),
        Line::tool("Read", seed.f[0]),
        Line::output(format!("{} lines", 120 + (h * 37) % 280)),
    ];
    if let Some(f1) = seed.f.get(1) {
        l.push(Line::tool("Grep", format!("\"{pt}\" {dir}")));
        l.push(Line::output(format!(
            "{} matches in {} files",
            3 + h % 9,
            2 + h % 4
        )));
        l.push(Line::tool("Read", *f1));
        l.push(Line::output(format!("{} lines", 60 + (h * 13) % 200)));
    }
    l.extend(seed.tail.iter().map(|(k, b)| design_line(*k, b)));
    l
}

/// The transcript of a subagent (`sublines()` of `Main.dc.html`).
fn sub_lines(seed: &Seed, sub: &SubSeed) -> Vec<Line> {
    let (name, st, task, cmd, out) = *sub;
    let dir = parent_dir(seed.f[0]);
    let n = utf16_len(name);
    let mut l = vec![
        Line::meta(format!("subagent {name} · spawned by session {}", seed.id)),
        Line::user(task),
        Line::agent(format!(
            "I work inside part \"{}\" and report back to the main agent.",
            seed.pt
        )),
        Line::tool("Read", *seed.f.get(1).unwrap_or(&seed.f[0])),
        Line::output(format!("{} lines", 40 + n * 17)),
        Line::tool("Grep", format!("\"{}\" {dir}", seed.pt)),
        Line::output(format!("{} matches", 2 + n % 7)),
        Line::agent("The context is clear. I run the check."),
        Line::tool("Bash", cmd),
        Line::output(out),
    ];
    match st {
        'd' => {
            l.push(Line::agent("Done. I return the result to the main agent."));
            l.push(Line::meta("— returned to main —"));
        }
        'i' => l.push(Line::meta("— waits for the main agent —")),
        _ => {}
    }
    l
}

fn tail_of(lines: &[Line]) -> Vec<Line> {
    lines[lines.len().saturating_sub(PANE_TAIL_LINES)..].to_vec()
}

fn context_pct(seed: &Seed) -> u8 {
    (30 + (utf16_len(seed.t) * 13) % 50) as u8
}

fn ask_reason(id: &str) -> Option<String> {
    (id == "7f3a").then(|| {
        s("The retry table needs a new column. The migrator subagent wrote \
           0042_webhook_attempts.sql. I want to apply it to staging and then run the webhook tests.")
    })
}

fn pane(seed: &Seed) -> (Pane, Vec<Line>) {
    let lines = main_lines(seed);
    let pane = Pane {
        id: s(seed.id),
        title: s(seed.t),
        codebase: s(seed.cb),
        part: s(seed.pt),
        provider: s(seed.ag),
        model: s(seed.m),
        state: state(seed.st),
        since: ago(seed.mn),
        cost: seed.c,
        // The same rule as the demo trace: 120k tokens per dollar.
        tokens: (seed.c * 120_000.0).round() as u64,
        // The demo backend answers every ask.
        controlled: true,
        context_pct: context_pct(seed),
        prompt: s(seed.pr),
        ask: seed.ask.map(|(kind, text, options, steps)| Ask {
            id: format!("{}-ask-1", seed.id),
            kind,
            text: s(text),
            options: strings(options),
            steps: strings(steps),
            reason: ask_reason(seed.id),
            risky: kind == AskKind::Permission,
        }),
        subagents: sub_seeds(seed.id)
            .iter()
            .map(|(name, st, task, _, _)| Subagent {
                id: format!("{}-{name}", seed.id),
                name: s(name),
                state: state(*st),
                task: s(task),
            })
            .collect(),
        tail: tail_of(&lines),
        lines_total: lines.len() as u32,
        composer: ComposerPrefs {
            mode: Mode::Edit,
            provider: s(seed.ag),
            model: s(seed.m),
            effort: Effort::High,
        },
        task: seed.task.map(s),
    };
    (pane, lines)
}

/// The pulse chart: 30 design points stretched to 60 minutes (`lerp` of `Main.dc.html`).
#[rustfmt::skip]
pub fn activity() -> Vec<ActivitySample> {
    const W: [u32; 30] = [12, 13, 13, 14, 15, 15, 16, 17, 17, 18, 18, 19, 19, 18, 17, 17, 16, 16, 17, 18, 19, 19, 18, 18, 17, 17, 16, 15, 15, 14];
    const Q: [u32; 30] = [1, 1, 2, 2, 1, 1, 2, 3, 3, 2, 1, 1, 2, 2, 3, 4, 4, 3, 2, 2, 1, 1, 2, 3, 3, 4, 4, 3, 4, 4];
    const I: [u32; 30] = [8, 7, 7, 6, 6, 6, 5, 4, 4, 4, 5, 4, 3, 4, 4, 3, 4, 5, 5, 4, 4, 4, 4, 3, 4, 3, 4, 4, 3, 3];
    let n = ACTIVITY_SAMPLES;
    let lerp = |a: &[u32; 30], j: usize| -> u32 {
        let t = j as f64 / (n - 1) as f64 * 29.0;
        let k = t.floor() as usize;
        let f = t - k as f64;
        let lo = f64::from(a[k]);
        let hi = f64::from(*a.get(k + 1).unwrap_or(&a[k]));
        (lo + (hi - lo) * f + 0.5).floor() as u32
    };
    (0..n)
        .map(|j| ActivitySample {
            at: DEMO_NOW - (n - 1 - j) as i64 * MINUTE_MS,
            active: lerp(&W, j),
            waiting: lerp(&Q, j),
            idle: lerp(&I, j),
        })
        .collect()
}

fn usage() -> Vec<UsageWindow> {
    [
        // (provider, label, pct, tokens used, minutes to the reset)
        ("claude", "5 h window", 62, 1_860_000, 132),
        ("claude", "Week", 41, 12_300_000, 3 * 24 * 60 + 410),
        ("codex", "5 h window", 31, 620_000, 47),
        ("codex", "Week", 22, 4_400_000, 5 * 24 * 60 + 75),
        ("gemini", "Daily quota", 18, 0, 9 * 60 + 20),
        ("gemini", "Monthly budget $60", 30, 0, 18 * 24 * 60),
    ]
    .into_iter()
    .map(|(provider, label, pct, used, minutes)| UsageWindow {
        provider: s(provider),
        label: s(label),
        pct,
        used,
        resets_at: Some(DEMO_NOW + minutes * crate::common::MINUTE_MS),
        estimate: false,
    })
    .collect()
}

/// The span tree of session a01c (`SP` of `Session.dc.html`).
#[rustfmt::skip]
fn trace_a01c(pane: &Pane, tool_calls: u32) -> SessionTrace {
    // (id, parent, kind, name, agent, start, end, state, tokens in, tokens out, cost, tool, input, output)
    type SpanSeed = (&'static str, &'static str, SpanKind, &'static str, &'static str, f64, f64, char, u64, u64, f64, &'static str, &'static str, &'static str);
    const NOW: f64 = 23.4;
    const SP: [SpanSeed; 21] = [
        ("s1", "", SpanKind::Run, "session a01c", "main", 0.0, NOW, 'w', 412_380, 38_912, 3.12, "", "cap subagent fan-out per session and per codebase", "3 turns · 47 tool calls · 3 subagents"),
        ("s2", "s1", SpanKind::Turn, "turn 1 · cap subagent fan-out", "main", 0.0, 6.2, 'd', 96_210, 8_140, 0.71, "", "cap subagent fan-out per session and per codebase", "Plan: 1. map spawn sites  2. budget struct  3. wire into spawn  4. tests"),
        ("s3", "s2", SpanKind::Llm, "chat", "main", 0.0, 0.6, 'd', 18_420, 1_210, 0.12, "", "system + user prompt + .aivory/map.toml", "I start with the spawn code in part \"orchestrator\"."),
        ("s4", "s2", SpanKind::Tool, "Read crates/orch/src/spawn.rs", "main", 0.6, 0.62, 'd', 0, 0, 0.0, "file_read", "crates/orch/src/spawn.rs", "218 lines"),
        ("s5", "s2", SpanKind::Agent, "invoke explorer", "explorer", 0.7, 4.9, 'd', 41_200, 3_020, 0.29, "", "map every spawn call site", "14 call sites in 6 files"),
        ("s6", "s5", SpanKind::Llm, "chat", "explorer", 0.7, 1.4, 'd', 12_800, 640, 0.08, "", "map every spawn call site", "I search for spawn_child."),
        ("s7", "s5", SpanKind::Tool, "Bash rg -n \"spawn_child\" crates/", "explorer", 1.4, 1.5, 'd', 0, 0, 0.0, "bash", "rg -n \"spawn_child\" crates/", "14 matches in 6 files"),
        ("s8", "s5", SpanKind::Llm, "chat · summarize", "explorer", 1.5, 4.8, 'd', 28_400, 2_380, 0.21, "", "14 matches", "Call sites grouped by crate: orch 8, sessions 4, agents 2."),
        ("s9", "s2", SpanKind::Llm, "chat · plan", "main", 4.9, 6.2, 'd', 23_400, 3_870, 0.30, "", "explorer result", "4-step plan"),
        ("s10", "s1", SpanKind::Turn, "turn 2 · write the budget struct", "main", 6.2, 14.8, 'd', 148_900, 14_400, 1.18, "", "go ahead with step 2", "Budget struct in place. 22 tests pass."),
        ("s11", "s10", SpanKind::Tool, "Edit crates/orch/src/budget.rs", "main", 6.5, 6.6, 'd', 0, 0, 0.0, "file_edit", "crates/orch/src/budget.rs", "+38 −4"),
        ("s12", "s10", SpanKind::Agent, "invoke test-writer", "test-writer", 7.0, NOW, 'w', 61_300, 5_980, 0.44, "", "write tests for budget::exceeds", "running 5 tests"),
        ("s13", "s12", SpanKind::Llm, "chat", "test-writer", 7.0, 9.1, 'd', 20_100, 2_240, 0.15, "", "write tests for budget::exceeds", "Five cases: zero, at limit, over limit, nested, per codebase."),
        ("s14", "s12", SpanKind::Tool, "Write tests/budget_exceeds.rs", "test-writer", 9.1, 9.2, 'd', 0, 0, 0.0, "file_write", "tests/budget_exceeds.rs", "96 lines"),
        ("s15", "s12", SpanKind::Tool, "Bash cargo test -p orch budget::exceeds", "test-writer", 21.9, NOW, 'w', 0, 0, 0.0, "bash", "cargo test -p orch budget::exceeds", "running 5 tests …"),
        ("s16", "s10", SpanKind::Tool, "Bash cargo test -p orch budget", "main", 12.0, 14.1, 'd', 0, 0, 0.0, "bash", "cargo test -p orch budget", "test result: ok. 22 passed; 0 failed"),
        ("s17", "s1", SpanKind::Turn, "turn 3 · wire budget into spawn", "main", 14.8, NOW, 'w', 167_270, 16_372, 1.23, "", "now wire the budget into the spawn path", "in progress"),
        ("s18", "s17", SpanKind::Llm, "chat", "main", 14.8, 16.9, 'd', 31_000, 2_900, 0.24, "", "wire the budget into spawn", "I change spawn_child to check the budget first."),
        ("s19", "s17", SpanKind::Agent, "invoke reviewer", "reviewer", 15.2, NOW, 'q', 4_200, 310, 0.03, "", "review the budget diff when it is ready", "waits for the main agent"),
        ("s20", "s17", SpanKind::Tool, "Edit crates/orch/src/spawn.rs", "main", 17.0, 17.1, 'd', 0, 0, 0.0, "file_edit", "crates/orch/src/spawn.rs", "+21 −6"),
        ("s21", "s17", SpanKind::Llm, "chat", "main", 17.1, NOW, 'w', 52_000, 4_100, 0.38, "", "spawn.rs diff + test status", "streaming…"),
    ];
    let start = ago(NOW);
    let at = |m: f64| start + (m * MINUTE_MS as f64).round() as i64;
    let spans = SP
        .iter()
        .map(|&(id, parent, kind, name, agent, s0, e, st, tin, tout, cost, tool, input, output)| Span {
            id: s(id),
            parent: (!parent.is_empty()).then(|| s(parent)),
            kind,
            name: s(name),
            agent: s(agent),
            model: s(if agent == "main" { "opus-5.5" } else { "sonnet-5.5" }),
            start: at(s0),
            end: (e < NOW).then(|| at(e)),
            state: state(st),
            tokens_in: tin,
            tokens_out: tout,
            tokens_cache_read: None,
            tokens_cache_write: None,
            cost,
            tool: (!tool.is_empty()).then(|| s(tool)),
            input: s(input),
            output: s(output),
        })
        .collect();
    let node = |id: &str, title: &str, agent: &str, minutes: Option<f64>, st: char, span: &str| DagNode {
        id: s(id),
        title: s(title),
        agent: s(agent),
        state: state(st),
        duration_ms: minutes.map(|m| (m * MINUTE_MS as f64).round() as u64),
        span: (!span.is_empty()).then(|| s(span)),
    };
    let edge = |a: &str, b: &str| DagEdge { from: s(a), to: s(b) };
    SessionTrace {
        session: pane.id.clone(),
        stats: SessionStats {
            subagents: pane.subagents.len() as u32,
            turns: 3,
            tool_calls: tool_calls.max(47),
            cost: pane.cost,
            context_pct: pane.context_pct,
        },
        spans,
        nodes: vec![
            node("n1", "Map spawn call sites", "explorer", Some(4.2), 'd', "s5"),
            node("n2", "Write the budget struct", "main", Some(8.6), 'd', "s10"),
            node("n3", "Write budget tests", "test-writer", Some(16.4), 'w', "s12"),
            node("n4", "Wire budget into spawn", "main", Some(8.6), 'w', "s17"),
            node("n5", "Review the budget diff", "reviewer", None, 'q', "s19"),
            node("n6", "Run the full test suite", "main", None, 'p', ""),
        ],
        edges: vec![
            edge("n1", "n2"),
            edge("n2", "n3"),
            edge("n2", "n4"),
            edge("n3", "n5"),
            edge("n4", "n5"),
            edge("n5", "n6"),
        ],
    }
}

/// A small trace for the sessions that the artboard does not show: the run, the setup turn,
/// the task turn, and one agent span per subagent.
pub fn generic_trace(pane: &Pane, tool_calls: u32) -> SessionTrace {
    let minutes = pane.minutes(DEMO_NOW);
    let running = matches!(pane.state, SessionState::Working | SessionState::NeedsYou);
    let start = if pane.state == SessionState::Working {
        pane.since
    } else {
        pane.since - 8 * MINUTE_MS
    };
    let end = (!running).then_some(pane.since);
    let turn_state = match pane.state {
        SessionState::Working | SessionState::NeedsYou | SessionState::Error => pane.state,
        _ => SessionState::Done,
    };
    let tokens = (pane.cost * 120_000.0).round() as u64;
    let span = |id: &str,
                parent: Option<&str>,
                kind,
                name: String,
                agent: &str,
                start,
                end,
                state,
                share: f64,
                input: &str,
                output: &str| Span {
        id: s(id),
        parent: parent.map(s),
        kind,
        name,
        agent: s(agent),
        model: pane.model.clone(),
        start,
        end,
        state,
        tokens_in: (tokens as f64 * share).round() as u64,
        tokens_out: (tokens as f64 * share / 10.0).round() as u64,
        tokens_cache_read: None,
        tokens_cache_write: None,
        cost: (pane.cost * share * 100.0).round() / 100.0,
        tool: None,
        input: s(input),
        output: s(output),
    };
    let setup_end = start + MINUTE_MS;
    let mut spans = vec![
        span(
            "s1",
            None,
            SpanKind::Run,
            format!("session {}", pane.id),
            "main",
            start,
            end,
            turn_state,
            1.0,
            &pane.prompt,
            &format!("{minutes} min"),
        ),
        span(
            "s2",
            Some("s1"),
            SpanKind::Turn,
            s("turn 1 · read the map"),
            "main",
            start,
            Some(setup_end),
            SessionState::Done,
            0.1,
            "read .aivory/map.toml",
            "Ready for the task.",
        ),
        span(
            "s3",
            Some("s1"),
            SpanKind::Turn,
            format!("turn 2 · {}", pane.title.to_lowercase()),
            "main",
            setup_end,
            end,
            turn_state,
            0.9,
            &pane.prompt,
            if running { "in progress" } else { "done" },
        ),
    ];
    let mut nodes = vec![DagNode {
        id: s("n1"),
        title: pane.title.clone(),
        agent: s("main"),
        state: turn_state,
        duration_ms: end.map(|e| (e - setup_end) as u64),
        span: Some(s("s3")),
    }];
    let mut edges = Vec::new();
    for (i, sub) in pane.subagents.iter().enumerate() {
        let id = format!("s{}", 4 + i);
        let sub_end = (sub.state == SessionState::Done).then_some(setup_end + MINUTE_MS);
        spans.push(span(
            &id,
            Some("s3"),
            SpanKind::Agent,
            format!("invoke {}", sub.name),
            &sub.name,
            setup_end,
            sub_end,
            sub.state,
            0.0,
            &sub.task,
            sub.state.label(),
        ));
        let node_id = format!("n{}", 2 + i);
        nodes.push(DagNode {
            id: node_id.clone(),
            title: sub.task.clone(),
            agent: sub.name.clone(),
            state: sub.state,
            duration_ms: sub_end.map(|e| (e - setup_end) as u64),
            span: Some(id),
        });
        edges.push(DagEdge {
            from: s("n1"),
            to: node_id,
        });
    }
    SessionTrace {
        session: pane.id.clone(),
        stats: SessionStats {
            subagents: pane.subagents.len() as u32,
            turns: 2,
            tool_calls,
            cost: pane.cost,
            context_pct: pane.context_pct,
        },
        spans,
        nodes,
        edges,
    }
}

/// Numbers for diff lines, from the hunk headers: context lines get both, added lines the
/// new number, removed lines the old number.
fn number_diff(lines: &[(char, &str)], blame: impl Fn(usize) -> Option<Blame>) -> Vec<DiffLine> {
    let (mut old, mut new) = (0u32, 0u32);
    lines
        .iter()
        .enumerate()
        .map(|(i, &(k, text))| {
            let (kind, o, n) = match k {
                'h' => {
                    // @@ -a,b +c,d @@
                    let mut it = text.split_whitespace().skip(1);
                    let num = |v: Option<&str>| {
                        v.and_then(|v| v[1..].split(',').next()?.parse::<u32>().ok())
                            .unwrap_or(1)
                    };
                    old = num(it.next());
                    new = num(it.next());
                    (DiffLineKind::Hunk, None, None)
                }
                'a' => {
                    new += 1;
                    (DiffLineKind::Added, None, Some(new - 1))
                }
                'r' => {
                    old += 1;
                    (DiffLineKind::Removed, Some(old - 1), None)
                }
                _ => {
                    old += 1;
                    new += 1;
                    (DiffLineKind::Context, Some(old - 1), Some(new - 1))
                }
            };
            DiffLine {
                kind,
                old: o,
                new: n,
                text: s(text),
                blame: if kind == DiffLineKind::Added {
                    blame(i)
                } else {
                    None
                },
            }
        })
        .collect()
}

fn diff_file(path: &str, added: u32, removed: u32, lines: Vec<DiffLine>) -> DiffFile {
    DiffFile {
        path: s(path),
        added,
        removed,
        lines,
    }
}

/// The diff of a01c (`FILES` of `Session.dc.html`).
#[rustfmt::skip]
fn diff_a01c() -> Vec<DiffFile> {
    let none = |_| None;
    vec![
        diff_file("crates/orch/src/budget.rs", 38, 4, number_diff(&[
            ('h', "@@ -1,12 +1,46 @@"), (' ', "use crate::session::Codebase;"), ('a', "use std::collections::HashMap;"), (' ', ""),
            ('a', "pub struct Budget {"), ('a', "    pub max_children: u8,      // default 4"), ('a', "    pub max_per_codebase: u8, // default 12"),
            ('a', "    running: HashMap<Codebase, u8>,"), ('a', "}"), (' ', ""), ('r', "pub fn budget_ok() -> bool { true }"),
            ('a', "impl Budget {"), ('a', "    pub fn check(&self, cb: &Codebase) -> Result<(), BudgetError> {"),
            ('a', "        let n = self.running.get(cb).copied().unwrap_or(0);"),
            ('a', "        if n >= self.max_per_codebase { return Err(BudgetError::Codebase(n)); }"),
            ('a', "        Ok(())"), ('a', "    }"), ('a', "}"),
        ], none)),
        diff_file("crates/orch/src/spawn.rs", 21, 6, number_diff(&[
            ('h', "@@ -88,9 +88,24 @@ impl Orchestrator"), (' ', "    pub fn spawn_child(&mut self, cfg: ChildCfg) -> Result<SessionId> {"),
            ('r', "        let child = Session::spawn(cfg)?;"), ('a', "        self.budget.check(&cfg.codebase)?;"),
            ('a', "        let child = Session::spawn(cfg)?;"), (' ', "        self.children.push(child.id);"),
        ], none)),
        diff_file("tests/budget_exceeds.rs", 96, 0, number_diff(&[
            ('h', "@@ -0,0 +1,96 @@"), ('a', "#[test]"), ('a', "fn rejects_the_13th_session_in_a_codebase() {"),
            ('a', "    let b = Budget::new(4, 12);"), ('a', "    assert!(b.with_running(\"code\", 12).check(&\"code\".into()).is_err());"), ('a', "}"),
        ], none)),
        diff_file("crates/orch/src/lib.rs", 1, 0, number_diff(&[
            ('h', "@@ -3,6 +3,7 @@"), (' ', "pub mod spawn;"), ('a', "pub mod budget;"),
        ], none)),
    ]
}

/// The diff of a7e2 with blame by prompt (`FILES` of `mobile/Diff.dc.html`).
#[rustfmt::skip]
fn diff_a7e2() -> Vec<DiffFile> {
    let blame = |i: usize| {
        Some(Blame {
            turn: 2 + (i % 3) as u32,
            prompt: s(if i % 2 == 1 {
                "compact ledger segments older than 7 days"
            } else {
                "keep the newest entry per key and drop old tombstones"
            }),
            model: s("sonnet-5.5"),
            session: s("a7e2"),
            at: DEMO_DAY * DAY_MS + (14 * 60 + 10 + i as i64) * MINUTE_MS,
            cost: (10 + i * 3) as f64 / 100.0,
        })
    };
    vec![
        diff_file("crates/ledger/src/compact.rs", 88, 12, number_diff(&[
            ('h', "@@ -40,10 +40,52 @@"), (' ', "impl Ledger {"), ('a', "    pub fn compact(&mut self, older_than: Days) -> Stats {"),
            ('a', "        let old = self.segments_older_than(older_than);"),
            ('a', "        let merged = old.chunks(2).map(|p| merge(&p[0], p.get(1)));"),
            ('a', "        self.replace(old, merged.collect())"), ('a', "    }"), (' ', ""),
            ('a', "fn merge(a: &Segment, b: Option<&Segment>) -> Segment {"),
            ('a', "    let mut out = Segment::with_capacity(a.len() + b.map_or(0, |b| b.len()));"),
            ('a', "    out.extend(a.newest_per_key(b));"), ('a', "    out.entries.sort_unstable_by_key(|e| e.key);"),
            ('a', "    out.drop_tombstones_older_than(Days(7));"), ('r', "    // TODO: compaction"), (' ', "    out"), (' ', "}"),
        ], blame)),
        diff_file("crates/ledger/src/segment.rs", 34, 22, number_diff(&[
            ('h', "@@ -12,8 +12,20 @@"), (' ', "pub struct Segment {"), (' ', "    pub entries: Vec<Entry>,"), ('a', "    pub sealed_at: Timestamp,"),
            (' ', "}"), ('r', "impl Segment { fn len(&self) -> usize { 0 } }"), ('a', "impl Segment {"),
            ('a', "    pub fn len(&self) -> usize { self.entries.len() }"), ('a', "}"),
        ], blame)),
        diff_file("crates/ledger/tests/compact.rs", 20, 4, number_diff(&[
            ('h', "@@ -1,4 +1,20 @@"), ('a', "#[test]"), ('a', "fn drops_old_tombstones() {"), ('a', "    let mut l = Ledger::fixture(30);"),
            ('a', "    assert_eq!(l.compact(Days(7)).dropped, 12);"), ('a', "}"),
        ], blame)),
    ]
}

/// One inbox item (`IT` of `Inbox.dc.html`).
struct InboxSeed {
    session: &'static str,
    kind: InboxKind,
    agents: &'static [(&'static str, &'static str)],
    mn: f64,
    preview: &'static str,
    text: &'static str,
    options: &'static [&'static str],
    steps: &'static [&'static str],
    gates: &'static [(&'static str, GateState)],
    sides: &'static [(&'static str, &'static [(char, &'static str)])],
}

#[rustfmt::skip]
const INBOX: [InboxSeed; 6] = [
    InboxSeed { session: "7f3a", kind: InboxKind::Permission, agents: &[("claude", "opus-5.5")], mn: 6.0, preview: "sqlx migrate run --database-url $STAGING_DB", text: "sqlx migrate run --database-url $STAGING_DB", options: &["Allow", "Always allow", "Deny"], steps: &[], gates: &[], sides: &[] },
    InboxSeed { session: "c218", kind: InboxKind::Plan, agents: &[("codex", "gpt-5.2-codex")], mn: 3.0, preview: "Virtualize the grid in 4 steps. Target: under 4 ms per frame.", text: "Virtualize the grid in 4 steps.", options: &["Approve", "Edit plan", "Reject"], steps: &["Measure tile rows and keep a row index", "Lay out only the rows in the viewport, plus one", "Cache shaped text per session line", "Bench again. Target: under 4 ms"], gates: &[], sides: &[] },
    InboxSeed { session: "9b0e", kind: InboxKind::Question, agents: &[("gemini", "3-pro")], mn: 2.0, preview: "Keep the legacy regex rules as a fallback, or drop them in v2?", text: "Keep the legacy regex rules as a fallback, or drop them in v2?", options: &["Keep as fallback", "Drop them"], steps: &[], gates: &[], sides: &[] },
    InboxSeed { session: "41dd", kind: InboxKind::Error, agents: &[("opencode", "kimi-k2")], mn: 1.0, preview: "dial tcp 127.0.0.1:6380: connect: connection refused", text: "Test infra is missing: no sentinel on :6380.", options: &["Start sentinel", "Retry", "Open log"], steps: &[], gates: &[], sides: &[] },
    InboxSeed { session: "a7e2", kind: InboxKind::Review, agents: &[("claude", "sonnet-5.5")], mn: 12.0, preview: "Done. 3 files, +142 −38. Tests and Guard pass.", text: "The session is done: 3 files changed, +142 −38. Review the diff before the merge.", options: &["Approve and merge", "Request changes", "Open the diff"], steps: &[], gates: &[("tests 31/31", GateState::Ok), ("Guard clean", GateState::Ok), ("review open", GateState::Pending)], sides: &[] },
    InboxSeed { session: "b5c9", kind: InboxKind::Conflict, agents: &[("claude", "haiku-5.5"), ("codex", "gpt-5.2-codex")], mn: 2.0, preview: "Tile skeleton states and Virtualize the grid change Tile::render.", text: "The trial merge fails in 2 hunks of Tile::render.", options: &["Ping both", "Pause the skeleton session", "Queue the merge"], steps: &[], gates: &[], sides: &[("Tile skeleton states · claude", &[('p', "+ TileState::Attaching => self.skeleton(cx),"), ('o', "  render() line 88")]), ("Virtualize the session grid · codex", &[('p', "+ if !self.visible(row) { return Empty; }"), ('o', "  render() line 86")])] },
];

/// The inbox result of an answer (`RES` of `Inbox.dc.html`).
pub fn inbox_result(option: &str) -> String {
    s(match option {
        "Allow" | "Always allow" => "allowed",
        "Deny" => "denied",
        "Approve" => "approved",
        "Edit plan" => "edit plan",
        "Reject" => "rejected",
        "Keep as fallback" | "Drop them" => "answered",
        "Start sentinel" => "fixed",
        "Retry" => "retried",
        "Open log" | "Open the diff" => "opened",
        "Approve and merge" => "merged",
        "Request changes" => "changes",
        "Ping both" => "pinged",
        "Pause the skeleton session" => "paused",
        "Queue the merge" => "queued",
        _ => "answered",
    })
}

/// The agent reply after an answer (`live()` of `Main.dc.html`).
pub fn answer_reply(answer: &str) -> String {
    if answer == "Resume" {
        s("I resume where I stopped.")
    } else {
        format!("OK. I continue with: {}.", answer.to_lowercase())
    }
}

/// One task (`TK` of `Tasks.dc.html`).
struct TaskSeed {
    id: &'static str,
    column: TaskColumn,
    title: &'static str,
    cb: &'static str,
    pt: &'static str,
    playbook: Playbook,
    step: u8,
    wait: Option<AskKind>,
    agents: &'static [&'static str],
    prompt: &'static str,
    /// Minutes ago; `None` = queued.
    started: Option<f64>,
    cost: f64,
    session: Option<&'static str>,
    sessions: &'static [(char, &'static str, &'static str)],
    gates: &'static [(&'static str, GateState)],
}

#[rustfmt::skip]
const TASKS: [TaskSeed; 14] = {
    use GateState::{Failed, Ok, Pending};
    use Playbook::{Feature, Fix, Port};
    use TaskColumn::*;
    [
        TaskSeed { id: "t1", column: Planned, title: "Rate limits per org", cb: "aivory-guard", pt: "api", playbook: Feature, step: 0, wait: None, agents: &[], prompt: "Add rate limits per organisation to the public api, with headers and a 429 body.", started: None, cost: 0.0, session: None, sessions: &[], gates: &[] },
        TaskSeed { id: "t2", column: Planned, title: "Dark theme tokens", cb: "aivory-code", pt: "ui", playbook: Feature, step: 0, wait: None, agents: &[], prompt: "Derive the dark theme from the light tokens. Keep the session panes square.", started: None, cost: 0.0, session: None, sessions: &[], gates: &[] },
        TaskSeed { id: "t3", column: Planned, title: "Invoice e-mail templates", cb: "billing", pt: "invoices", playbook: Feature, step: 0, wait: None, agents: &[], prompt: "Send the invoice PDF with a plain text e-mail in English and German.", started: None, cost: 0.0, session: None, sessions: &[], gates: &[] },
        TaskSeed { id: "t4", column: Working, title: "Subagent fan-out limits", cb: "aivory-code", pt: "orchestrator", playbook: Feature, step: 2, wait: None, agents: &["claude", "claude", "claude", "claude"], prompt: "Cap subagent fan-out per session and per codebase. Default 4 children, 12 per codebase.", started: Some(23.0), cost: 3.12, session: Some("a01c"), sessions: &[('w', "Subagent fan-out limits", "main · opus-5.5"), ('w', "test-writer", "sub · running"), ('i', "reviewer", "sub · waits"), ('d', "explorer", "sub · done")], gates: &[("tests 22/22", Ok), ("Guard pending", Pending), ("review open", Pending)] },
        TaskSeed { id: "t5", column: Working, title: "Incremental scan cache", cb: "aivory-guard", pt: "scanner", playbook: Feature, step: 2, wait: None, agents: &["claude", "claude", "claude"], prompt: "Only rescan files whose hash changed since the last scan.", started: Some(31.0), cost: 4.05, session: Some("5e81"), sessions: &[], gates: &[] },
        TaskSeed { id: "t6", column: Working, title: "Model routing table", cb: "smart-inference", pt: "router", playbook: Feature, step: 1, wait: None, agents: &["claude", "claude"], prompt: "Route by task class with a fallback chain per provider.", started: Some(18.0), cost: 2.20, session: Some("90c4"), sessions: &[], gates: &[] },
        TaskSeed { id: "t7", column: Working, title: "PPTX chart export", cb: "slaide", pt: "render", playbook: Feature, step: 1, wait: None, agents: &["claude", "claude", "claude"], prompt: "Export charts as native pptx charts, not as images.", started: Some(14.0), cost: 1.66, session: Some("s3d1"), sessions: &[], gates: &[] },
        TaskSeed { id: "t8", column: Waiting, title: "Stripe webhook retries", cb: "billing", pt: "payment", playbook: Fix, step: 1, wait: Some(AskKind::Permission), agents: &["claude", "claude", "claude"], prompt: "Add retry with exponential backoff to the stripe webhook handler.", started: Some(40.0), cost: 2.41, session: Some("7f3a"), sessions: &[], gates: &[] },
        TaskSeed { id: "t9", column: Waiting, title: "Virtualize the session grid", cb: "aivory-code", pt: "ui", playbook: Feature, step: 0, wait: Some(AskKind::Plan), agents: &["codex"], prompt: "The session grid lags with 40 tiles. Virtualize it.", started: Some(9.0), cost: 0.88, session: Some("c218"), sessions: &[], gates: &[] },
        TaskSeed { id: "t10", column: Waiting, title: "OWASP rule pack v2", cb: "aivory-guard", pt: "rules", playbook: Port, step: 1, wait: Some(AskKind::Question), agents: &["gemini", "gemini"], prompt: "Port the OWASP top-10 rules to the v2 rule format.", started: Some(27.0), cost: 0.52, session: Some("9b0e"), sessions: &[], gates: &[] },
        TaskSeed { id: "t11", column: Review, title: "Ledger compaction", cb: "aivory-code", pt: "ledger", playbook: Feature, step: 3, wait: None, agents: &["claude"], prompt: "Compact ledger segments older than 7 days.", started: Some(60.0), cost: 0.74, session: Some("a7e2"), sessions: &[], gates: &[("tests 31/31", Ok), ("Guard clean", Ok), ("review open", Pending)] },
        TaskSeed { id: "t12", column: Review, title: "Runner sandbox seccomp", cb: "kronk", pt: "runner", playbook: Feature, step: 3, wait: None, agents: &["claude", "claude"], prompt: "Add a seccomp profile to the runner sandbox.", started: Some(120.0), cost: 2.88, session: Some("k4c2"), sessions: &[], gates: &[("tests 17/17", Ok), ("Guard 1 finding", Failed), ("review open", Pending)] },
        TaskSeed { id: "t13", column: Done, title: "CRDT merge for map.toml", cb: "aivory-code", pt: "sync", playbook: Feature, step: 5, wait: None, agents: &["goose"], prompt: "Merge concurrent edits of .aivory/map.toml from two agents.", started: Some(218.0), cost: 0.40, session: Some("d3f0"), sessions: &[], gates: &[("tests 9/9", Ok), ("Guard clean", Ok), ("reviewed", Ok)] },
        TaskSeed { id: "t14", column: Done, title: "Cache cargo builds in CI", cb: "infra", pt: "ci", playbook: Fix, step: 5, wait: None, agents: &["claude"], prompt: "Cache cargo builds in CI.", started: Some(300.0), cost: 0.18, session: Some("i0a8"), sessions: &[], gates: &[("tests ok", Ok), ("Guard clean", Ok), ("reviewed", Ok)] },
    ]
};

/// Step states, actors and durations (`stepStates()` and `WHO` of `Tasks.dc.html`).
pub fn task_steps(playbook: Playbook, column: TaskColumn, step: u8, waiting: bool) -> Vec<Step> {
    const WHO: [&str; 3] = ["main · opus-5.5", "main + 2 sub", "test-writer"];
    playbook
        .steps()
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let idx = i as u8;
            let state = if idx < step {
                StepState::Done
            } else if idx == step {
                match column {
                    TaskColumn::Done => StepState::Done,
                    TaskColumn::Planned => StepState::NotStarted,
                    _ if waiting => StepState::Waiting,
                    _ => StepState::Working,
                }
            } else {
                StepState::NotStarted
            };
            let actor = match state {
                StepState::NotStarted => None,
                _ if i == 3 => Some(s("reviewer · you")),
                _ => Some(s(WHO[i.min(2)])),
            };
            Step {
                name: s(name),
                state,
                actor,
                duration_ms: (state == StepState::Done)
                    .then(|| (2 + i as u64 * 3) * MINUTE_MS as u64),
            }
        })
        .collect()
}

fn default_gates() -> Vec<Gate> {
    vec![
        Gate::new("tests", GateState::Pending),
        Gate::new("Guard", GateState::Pending),
        Gate::new("review", GateState::Pending),
    ]
}

fn tasks(panes: &[Pane]) -> Vec<Task> {
    TASKS
        .iter()
        .enumerate()
        .map(|(index, t)| {
            let pane = t.session.and_then(|id| panes.iter().find(|p| p.id == id));
            let sessions = match (t.session, pane) {
                (Some(id), _) if !t.sessions.is_empty() => t
                    .sessions
                    .iter()
                    .map(|(st, title, role)| TaskSession {
                        session: s(id),
                        title: s(title),
                        role: s(role),
                        state: state(*st),
                    })
                    .collect(),
                (Some(id), Some(p)) => vec![TaskSession {
                    session: s(id),
                    title: s(t.title),
                    role: format!("main · {}", p.model),
                    state: if t.wait.is_some() {
                        SessionState::NeedsYou
                    } else if t.column == TaskColumn::Done {
                        SessionState::Done
                    } else {
                        SessionState::Working
                    },
                }],
                _ => Vec::new(),
            };
            Task {
                id: s(t.id),
                column: t.column,
                title: s(t.title),
                codebase: s(t.cb),
                part: s(t.pt),
                playbook: t.playbook,
                step: t.step,
                waiting_for: t.wait,
                agents: strings(t.agents),
                prompt: s(t.prompt),
                priority: match t.playbook {
                    Playbook::Fix => TaskPriority::High,
                    Playbook::Port => TaskPriority::Low,
                    Playbook::Feature => TaskPriority::Normal,
                },
                // A started task was created 5 minutes before it started. Queued tasks are
                // 30, 25, 20 ... minutes old by their position in the table.
                created_at: ago(t.started.map_or(30.0 - index as f64 * 5.0, |m| m + 5.0)),
                started: t.started.map(ago),
                cost: t.cost,
                sessions,
                gates: if t.gates.is_empty() {
                    default_gates()
                } else {
                    t.gates.iter().map(|(l, g)| Gate::new(*l, *g)).collect()
                },
                steps: task_steps(t.playbook, t.column, t.step, t.wait.is_some()),
            }
        })
        .collect()
}

/// The `hash()` of `Repos.dc.html`: the sum of the character codes.
fn char_sum(v: &str) -> usize {
    v.encode_utf16().map(usize::from).sum()
}

fn repo_detail(cb: &Codebase, panes: &[Pane]) -> RepoDetail {
    let live: Vec<&Pane> = panes
        .iter()
        .filter(|p| p.codebase == cb.id && p.state.is_live())
        .collect();
    let parts = cb
        .parts
        .iter()
        .map(|part| {
            let ps: Vec<&&Pane> = live.iter().filter(|p| p.part == part.name).collect();
            let h = char_sum(&format!("{}{}", cb.id, part.name));
            PartStats {
                name: part.name.clone(),
                paths: part.paths.clone(),
                sessions: ps.iter().map(|p| p.id.clone()).collect(),
                claims: if ps.is_empty() {
                    0
                } else {
                    (ps.len() + h % 3) as u32
                },
                last_change: Some(if ps.is_empty() {
                    DEMO_NOW - (1 + h as i64 % 9) * DAY_MS
                } else {
                    ago((2 + h % 20) as f64)
                }),
                lines: (1200 + (h * 37) % 9000) as u32,
            }
        })
        .collect();
    let mut files = Vec::new();
    for part in &cb.parts {
        let root = part.paths[0].trim_end_matches("/**").to_string();
        files.push(FileNode {
            path: format!("{root}/"),
            name: format!("{root}/"),
            dir: true,
            depth: 0,
            part: Some(part.name.clone()),
            claimed_by: None,
            claim_state: None,
        });
        let claimer = live.iter().find(|p| p.part == part.name);
        let own = format!("{}.rs", part.name);
        for (i, name) in ["mod.rs", own.as_str(), "tests.rs"].into_iter().enumerate() {
            let claimed = claimer.filter(|_| i == 1);
            files.push(FileNode {
                path: format!("{root}/{name}"),
                name: s(name),
                dir: false,
                depth: 1,
                part: None,
                claimed_by: claimed.map(|p| p.id.clone()),
                claim_state: claimed.map(|p| p.state),
            });
        }
    }
    const WHAT: [&str; 5] = ["tests pass", "new module", "refactor", "fix", "docs"];
    let activity = live
        .iter()
        .flat_map(|p| [(*p, 0usize), (*p, 1usize)])
        .take(8)
        .enumerate()
        .map(|(i, (p, j))| {
            let h = char_sum(&format!("{}{j}", p.id));
            let what = if j == 1 {
                format!("Turn {} in {}: {}", 2 + h % 3, p.part, WHAT[h % 5])
            } else {
                format!("Edit in {}: {}", p.part, WHAT[h % 5])
            };
            ActivityRow {
                at: ago((2 + i * 4) as f64),
                provider: p.provider.clone(),
                session: p.id.clone(),
                what,
                added: (4 + h % 60) as u32,
                removed: (h % 18) as u32,
                cost: (5 + h % 90) as f64 / 100.0,
            }
        })
        .collect();
    RepoDetail {
        codebase: cb.clone(),
        parts,
        files,
        activity,
    }
}

/// `.aivory/map.toml` of a codebase, as the "View map.toml" panel shows it.
pub fn map_file(cb: &Codebase) -> MapFile {
    let mut source = String::from(
        "# written by agents · reviewed by you\nversion = 3\nupdated = \"2026-10-21T14:12:00Z\"\n",
    );
    for p in &cb.parts {
        let paths: Vec<String> = p.paths.iter().map(|x| format!("\"{x}\"")).collect();
        source.push_str(&format!(
            "\n[[part]]\nname  = \"{}\"\npaths = [{}]\n",
            p.name,
            paths.join(", ")
        ));
    }
    MapFile {
        codebase: cb.id.clone(),
        version: 3,
        updated: ago(28.0),
        author: Some(s("claude opus-5.5")),
        parts: cb.parts.clone(),
        source,
    }
}

#[rustfmt::skip]
fn settings_view() -> SettingsView {
    let prefs = |enabled: bool, model: &str| ProviderPrefs {
        enabled,
        model: s(model),
        effort: Effort::High,
        mode: Mode::Edit,
    };
    let ch = |desktop, phone, email| Channels {
        desktop,
        phone,
        email,
    };
    let usage = usage();
    let usage_of = |p: &str| -> Vec<UsageWindow> {
        usage.iter().filter(|u| u.provider == p).cloned().collect()
    };
    let provider = |id: &str, name: &str, monogram: &str, auth: &str, status, models: &[&str]| ProviderInfo {
        id: s(id),
        name: s(name),
        monogram: s(monogram),
        auth: s(auth),
        status,
        models: strings(models),
        usage: usage_of(id),
    };
    SettingsView {
        settings: Settings {
            providers: BTreeMap::from([
                (s("claude"), prefs(true, "opus-5.5")),
                (s("codex"), prefs(true, "gpt-5.2-codex")),
                (s("gemini"), prefs(true, "3-pro")),
                (s("opencode"), prefs(true, "kimi-k2")),
                (s("goose"), prefs(false, "goose-1.9")),
            ]),
            autopilot: Autopilot::default(),
            display: Display::default(),
            notify: Notify {
                needs_answer: ch(true, true, false),
                error: ch(true, true, true),
                review: ch(true, true, false),
                conflict: ch(true, true, false),
                autopilot: ch(true, false, false),
            },
            quiet_hours: QuietHours {
                on: true,
                from: s("22:00"),
                to: s("07:00"),
            },
            remote: Remote::default(),
            ledger: LedgerPrefs::default(),
            acp: Vec::new(),
            pinned: Vec::new(),
            coffee_reminder: true,
        },
        providers: vec![
            provider("claude", "Claude Code", "C", "subscription · Max · signed in", ProviderStatus::Connected, &["opus-5.5", "sonnet-5.5", "haiku-5.5"]),
            provider("codex", "Codex", "X", "ChatGPT Pro · signed in", ProviderStatus::Connected, &["gpt-5.2-codex", "gpt-5.2"]),
            provider("gemini", "Gemini CLI", "G", "API key · $18.20 this month", ProviderStatus::Connected, &["3-pro", "3-flash"]),
            provider("opencode", "OpenCode", "O", "ACP agent · local · kimi-k2 through OpenRouter", ProviderStatus::Acp, &["kimi-k2", "qwen3-coder"]),
            provider("goose", "Goose", "G", "ACP agent · local", ProviderStatus::Acp, &["goose-1.9"]),
        ],
        daemons: vec![
            Daemon { id: s("desktop"), name: s("desktop · this machine"), kind: DaemonKind::Local, detail: s("aivoryd 0.9.2 · Windows 11"), sessions: 14, running: true },
            Daemon { id: s("build-01"), name: s("build-01"), kind: DaemonKind::Ssh, detail: s("ssh pp@build-01 · aivoryd 0.9.2"), sessions: 5, running: true },
            Daemon { id: s("runner-eu"), name: s("runner-eu"), kind: DaemonKind::Cloud, detail: s("cloud runner · eu-central"), sessions: 2, running: true },
        ],
        devices: vec![
            Device { id: s("pixel-9-pro"), name: s("Pixel 9 Pro"), os: s("Android 16"), paired: october(0) + 12 * 3_600_000, last_seen: ago(2.0), online: true },
            Device { id: s("iphone-16"), name: s("iPhone 16"), os: s("iOS 26"), paired: october(2) + 12 * 3_600_000, last_seen: DEMO_NOW - DAY_MS, online: false },
        ],
        account: Account {
            team: String::new(),
            seats: 1,
            license: s("MIT"),
            renews: String::new(),
            hub: String::new(),
            relay: String::new(),
            hub_state: LinkState::Off,
        },
        budget: Budget {
            month: s("2026-10"),
            spent: 412.0,
            limit: 600.0,
            warn_pct: 80,
        },
    }
}

/// All demo data. The fields are public, so that a demo server can change them.
#[derive(Debug, Clone)]
pub struct Demo {
    pub war_room: WarRoom,
    /// Full transcripts. Key: session id, or `session/subagent` for a subagent.
    pub histories: BTreeMap<String, Vec<Line>>,
    pub traces: BTreeMap<String, SessionTrace>,
    pub diffs: BTreeMap<String, SessionDiff>,
    pub inbox: InboxList,
    pub tasks: Vec<Task>,
    pub repos: Vec<RepoDetail>,
    pub maps: Vec<MapFile>,
    pub settings: SettingsView,
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo {
    pub fn new() -> Self {
        let codebases = codebases();
        let mut histories = BTreeMap::new();
        let mut panes = Vec::new();
        for seed in &SEEDS {
            let (pane, lines) = pane(seed);
            for sub in sub_seeds(seed.id) {
                histories.insert(format!("{}/{}", seed.id, sub.0), sub_lines(seed, sub));
            }
            histories.insert(pane.id.clone(), lines);
            panes.push(pane);
        }

        let mut traces = BTreeMap::new();
        let mut diffs = BTreeMap::new();
        for (seed, pane) in SEEDS.iter().zip(&panes) {
            let tools = histories[&pane.id]
                .iter()
                .filter(|l| l.kind == LineKind::Tool)
                .count() as u32;
            let trace = if pane.id == "a01c" {
                trace_a01c(pane, tools)
            } else {
                generic_trace(pane, tools)
            };
            traces.insert(pane.id.clone(), trace);
            let files = match seed.id {
                "a01c" => diff_a01c(),
                "a7e2" => diff_a7e2(),
                _ => Vec::new(),
            };
            diffs.insert(
                pane.id.clone(),
                SessionDiff {
                    session: pane.id.clone(),
                    files,
                },
            );
        }

        let conflict_lines = main_lines(&CONFLICT_SEED);
        let items = INBOX
            .iter()
            .map(|it| {
                let pane = panes.iter().find(|p| p.id == it.session);
                let (title, cb, pt, prompt, files, tail, ctx, cost) = match pane {
                    Some(p) => {
                        let seed = SEEDS
                            .iter()
                            .find(|x| x.id == p.id)
                            .unwrap_or(&CONFLICT_SEED);
                        (
                            p.title.clone(),
                            p.codebase.clone(),
                            p.part.clone(),
                            p.prompt.clone(),
                            strings(seed.f),
                            p.tail.clone(),
                            p.context_pct,
                            p.cost,
                        )
                    }
                    None => {
                        let c = &CONFLICT_SEED;
                        (
                            s("Both edit crates/ui/src/tile.rs"),
                            s(c.cb),
                            s(c.pt),
                            s(c.pr),
                            strings(c.f),
                            tail_of(&conflict_lines),
                            context_pct(c),
                            c.c,
                        )
                    }
                };
                InboxItem {
                    id: format!("in-{}", it.session),
                    kind: it.kind,
                    session: s(it.session),
                    ask: pane.and_then(|p| p.ask.as_ref()).map(|a| a.id.clone()),
                    title,
                    codebase: cb,
                    part: pt,
                    agents: it
                        .agents
                        .iter()
                        .map(|(provider, model)| AgentRef {
                            provider: s(provider),
                            model: s(model),
                        })
                        .collect(),
                    since: ago(it.mn),
                    cost,
                    context_pct: ctx,
                    preview: s(it.preview),
                    text: s(it.text),
                    options: strings(it.options),
                    steps: strings(it.steps),
                    gates: it.gates.iter().map(|(l, g)| Gate::new(*l, *g)).collect(),
                    sides: it
                        .sides
                        .iter()
                        .map(|(title, lines)| ConflictSide {
                            title: s(title),
                            lines: lines.iter().map(|(k, b)| design_line(*k, b)).collect(),
                        })
                        .collect(),
                    prompt,
                    files,
                    tail,
                    result: None,
                }
            })
            .collect();
        histories.insert(s(CONFLICT_SEED.id), conflict_lines);

        let tasks = tasks(&panes);
        let repos = codebases.iter().map(|c| repo_detail(c, &panes)).collect();
        let maps = codebases.iter().map(map_file).collect();
        let settings = settings_view();
        let inbox = InboxList {
            items,
            done: Vec::new(),
        };
        let mut war_room = WarRoom {
            epoch: 0,
            cursor: 0,
            now: DEMO_NOW,
            codebases,
            sessions: panes,
            activity: activity(),
            autopilot: settings.settings.autopilot.clone(),
            queued: tasks
                .iter()
                .filter(|t| t.column == TaskColumn::Planned)
                .count() as u32,
            inbox_open: inbox.items.len() as u32,
            usage: usage(),
            hub: LinkState::Connected,
            relay: LinkState::Connected,
            board: Default::default(),
        };
        war_room.board = war_room.compute_board();
        Self {
            war_room,
            histories,
            traces,
            diffs,
            inbox,
            tasks,
            repos,
            maps,
            settings,
        }
    }

    /// The full transcript of a session or of one of its subagents (by name or id).
    pub fn history(&self, id: &str, subagent: Option<&str>) -> Option<&Vec<Line>> {
        match subagent {
            None => self.histories.get(id),
            Some(sub) => {
                let name = sub.strip_prefix(&format!("{id}-")).unwrap_or(sub);
                self.histories.get(&format!("{id}/{name}"))
            }
        }
    }

    pub fn history_mut(&mut self, id: &str) -> Option<&mut Vec<Line>> {
        self.histories.get_mut(id)
    }
}

// ---------------------------------------------------------------------------------------------
// Code intelligence and the ledger (`code/*`, `ledger/*`)

/// A file of the demo codebases. The text matches the diffs of the demo sessions.
#[derive(Debug, Clone, Copy)]
pub struct DemoFile {
    pub codebase: &'static str,
    pub path: &'static str,
    pub text: &'static str,
}

const BUDGET_RS: &str = "\
use std::collections::HashMap;

use crate::session::Codebase;

/// Limits for the children that one session may start.
pub struct Budget {
    pub max_children: u8,
    pub max_per_codebase: u8,
    running: HashMap<Codebase, u8>,
}

impl Budget {
    pub fn new(max_children: u8, max_per_codebase: u8) -> Self {
        Self { max_children, max_per_codebase, running: HashMap::new() }
    }

    /// Fails when `cb` has too many children.
    pub fn check(&self, cb: &Codebase) -> Result<(), BudgetError> {
        let n = self.running.get(cb).copied().unwrap_or(0);
        if n >= self.max_per_codebase {
            return Err(BudgetError::Codebase(n));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum BudgetError {
    Codebase(u8),
}
";

const SPAWN_RS: &str = "\
use crate::budget::{Budget, BudgetError};
use crate::session::{Codebase, Session};

pub struct Orchestrator {
    budget: Budget,
    children: Vec<u64>,
}

impl Orchestrator {
    pub fn spawn_child(&mut self, cb: &Codebase) -> Result<u64, BudgetError> {
        self.budget.check(cb)?;
        let child = Session::spawn(cb);
        self.children.push(child.id);
        Ok(child.id)
    }
}
";

const SESSION_RS: &str = "\
pub type Codebase = String;

pub struct Session {
    pub id: u64,
}

impl Session {
    pub fn spawn(cb: &Codebase) -> Session {
        Session { id: cb.len() as u64 }
    }
}
";

const FINDINGS_TS: &str = "\
export interface Finding {
  id: string;
  path: string;
  severity: \"low\" | \"high\";
}

export function countByDirectory(findings: Finding[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const f of findings) {
    const dir = f.path.split(\"/\").slice(0, -1).join(\"/\");
    out.set(dir, (out.get(dir) ?? 0) + 1);
  }
  return out;
}
";

const HEATMAP_TS: &str = "\
import { countByDirectory, Finding } from \"./findings\";

export function cells(findings: Finding[]): number[] {
  const counts = countByDirectory(findings);
  return [...counts.values()].map((n) => Math.min(n, 4));
}
";

/// The files of the demo codebases `aivory-code` (Rust) and `aivory-guard` (TypeScript).
pub fn code_files() -> Vec<DemoFile> {
    let f = |codebase, path, text| DemoFile {
        codebase,
        path,
        text,
    };
    vec![
        f("aivory-code", "crates/orch/src/budget.rs", BUDGET_RS),
        f("aivory-code", "crates/orch/src/spawn.rs", SPAWN_RS),
        f("aivory-code", "crates/orch/src/session.rs", SESSION_RS),
        f("aivory-guard", "web/src/findings.ts", FINDINGS_TS),
        f("aivory-guard", "web/src/heatmap.ts", HEATMAP_TS),
    ]
}

/// The session of the demo ledger.
pub const LEDGER_SESSION: &str = "a01c";

fn ledger_turn(
    n: u32,
    prompt: &str,
    minutes_ago: f64,
    cost: f64,
    files: &[(&str, u32, u32)],
) -> crate::code::LedgerTurn {
    crate::code::LedgerTurn {
        n,
        prompt: s(prompt),
        provider: s("claude"),
        model: Some(s("opus-5.5")),
        started_at: ago(minutes_ago + 5.0),
        ended_at: ago(minutes_ago),
        cost: (n > 0).then_some(cost),
        tokens_in: (n > 0).then_some(40_000 * u64::from(n)),
        tokens_out: (n > 0).then_some(3_000 * u64::from(n)),
        snapshot: true,
        files: files
            .iter()
            .map(|(path, added, removed)| crate::code::TurnFileStat {
                path: s(path),
                added: *added,
                removed: *removed,
            })
            .collect(),
        diagnostics_added: (n > 0).then_some(u32::from(n == 2)),
        diagnostics_removed: (n > 0).then_some(u32::from(n == 3)),
    }
}

const DEMO_TURN_PROMPTS: [&str; 3] = [
    "cap subagent fan-out per session and per codebase",
    "check the limit before a child starts",
    "return a typed error when the limit is hit",
];

/// The turns of the demo session `a01c`.
pub fn ledger_turns() -> crate::code::TurnList {
    crate::code::TurnList {
        session: s(LEDGER_SESSION),
        turns: vec![
            ledger_turn(0, "", 30.0, 0.0, &[]),
            ledger_turn(
                1,
                DEMO_TURN_PROMPTS[0],
                18.0,
                0.71,
                &[("crates/orch/src/budget.rs", 16, 1)],
            ),
            ledger_turn(
                2,
                DEMO_TURN_PROMPTS[1],
                11.0,
                0.88,
                &[
                    ("crates/orch/src/budget.rs", 9, 0),
                    ("crates/orch/src/spawn.rs", 1, 0),
                ],
            ),
            ledger_turn(
                3,
                DEMO_TURN_PROMPTS[2],
                4.0,
                0.42,
                &[("crates/orch/src/budget.rs", 5, 0)],
            ),
        ],
    }
}

fn blame_turn(n: u32) -> crate::code::BlameTurn {
    let ends = [0.0, 18.0, 11.0, 4.0];
    let costs = [0.0, 0.71, 0.88, 0.42];
    crate::code::BlameTurn {
        session: s(LEDGER_SESSION),
        n,
        prompt: s(DEMO_TURN_PROMPTS[n as usize - 1]),
        model: s("opus-5.5"),
        time: ago(ends[n as usize]),
        cost: Some(costs[n as usize]),
    }
}

/// The commit that holds the first four lines of `budget.rs`.
fn demo_commit() -> crate::code::CommitRef {
    crate::code::CommitRef {
        id: s("3f2a91c0d4b8"),
        summary: s("Add the orchestrator skeleton"),
        time: ago(3.0 * 24.0 * 60.0),
    }
}

/// The blame by prompt of `crates/orch/src/budget.rs` in the session `a01c`. The first four
/// lines are committed, the others are the work of the turns 1, 2, and 3.
pub fn ledger_blame() -> crate::code::FileBlame {
    let hunk = |line, count, commit, turn| crate::code::BlameHunk {
        line,
        count,
        commit,
        turn,
    };
    crate::code::FileBlame {
        path: s("crates/orch/src/budget.rs"),
        hunks: vec![
            hunk(1, 4, Some(demo_commit()), None),
            hunk(5, 12, None, Some(blame_turn(1))),
            hunk(17, 9, None, Some(blame_turn(2))),
            hunk(26, 5, None, Some(blame_turn(3))),
        ],
    }
}

/// The history of `crates/orch/src/budget.rs`: one commit and the three turns.
pub fn ledger_timeline() -> crate::code::Timeline {
    let turns = ledger_turns();
    let commit = demo_commit();
    let mut events = vec![crate::code::TimelineEvent::Commit {
        id: commit.id,
        summary: commit.summary,
        author: s("Dana Lee"),
        time: commit.time,
        turns: Vec::new(),
    }];
    for t in turns.turns.iter().skip(1) {
        let stat = t
            .files
            .iter()
            .find(|f| f.path == "crates/orch/src/budget.rs");
        if let Some(stat) = stat {
            events.push(crate::code::TimelineEvent::Turn {
                session: s(LEDGER_SESSION),
                n: t.n,
                prompt: t.prompt.clone(),
                model: s("opus-5.5"),
                time: t.ended_at,
                cost: t.cost,
                added: stat.added,
                removed: stat.removed,
            });
        }
    }
    crate::code::Timeline {
        path: s("crates/orch/src/budget.rs"),
        events,
    }
}

fn finding(
    rule: &str,
    level: crate::code::FindingLevel,
    message: &str,
    line: u32,
    tool: &str,
) -> crate::code::Finding {
    crate::code::Finding {
        rule: s(rule),
        level,
        message: s(message),
        path: s("crates/orch/src/budget.rs"),
        line,
        col: 8,
        end_line: Some(line),
        end_col: Some(20),
        tool: s(tool),
        fingerprint: format!("demo-{rule}-{line}"),
    }
}

/// The findings of turn 3 of the demo session: one warning of the rule pack stays, one warning
/// of turn 2 is gone.
pub fn ledger_diagnostics() -> crate::code::DiagnosticsResult {
    use crate::code::FindingLevel::{Note, Warning};
    crate::code::DiagnosticsResult {
        turn: Some(3),
        rows: vec![
            finding(
                "clippy::new_without_default",
                Note,
                "`Budget::new` takes arguments; add a builder if more limits come",
                13,
                "clippy",
            ),
            finding(
                "aivory/risky-unwrap",
                Warning,
                "unwrap_or hides a missing key; check the map first",
                19,
                "aivory-rules",
            ),
        ],
        added: Vec::new(),
        removed: vec![finding(
            "aivory/deep-nesting",
            Warning,
            "nesting is deeper than 4 levels",
            21,
            "aivory-rules",
        )],
        notes: vec![s(
            "stages: syntax, rules, linters (clippy); lsp is not running",
        )],
    }
}

// ---------------------------------------------------------------------------------------------
// Coordination (`claims/*`, `radar/*`, `mail/*`): the sessions, claims, and pairs of the Radar
// frame of `Lens.dc.html` (sessions 7f3a, 8a22, k9d4, m2e5, 8d07; scores 1.0, 0.7, 0.4, 0.1).

/// The session of the sample with a block in its mailbox (`block/override` frees it).
pub const BLOCKED_SESSION: &str = "5e81";

/// The claims of the radar sample, ordered by key and session.
pub fn claims() -> crate::coord::ClaimList {
    use crate::coord::{Claim, ClaimRange, ClaimState::*};
    let claim = |session: &str,
                 key: &str,
                 state,
                 path: Option<&str>,
                 range: Option<(u32, u32)>,
                 lease_s: u32| Claim {
        session: s(session),
        codebase: s("billing"),
        key: s(key),
        state,
        path: path.map(s),
        range: range.map(|(from, to)| ClaimRange { from, to }),
        lease_s,
    };
    let charge = "src/payment/charge.rs";
    let invoice = "src/billing/invoice.rs";
    let money = "src/billing/money.rs";
    let schema = "billing/db/schema.rs";
    crate::coord::ClaimList {
        claims: vec![
            claim(
                "8d07",
                "path:billing/db/schema.rs",
                Editing,
                Some(schema),
                None,
                541,
            ),
            claim(
                "m2e5",
                "path:billing/db/schema.rs",
                Editing,
                Some(schema),
                None,
                318,
            ),
            claim(
                "7f3a",
                "path:src/payment/charge.rs",
                Editing,
                Some(charge),
                None,
                505,
            ),
            claim(
                "k9d4",
                "path:src/payment/charge.rs",
                Editing,
                Some(charge),
                None,
                420,
            ),
            claim(
                "8a22",
                "sym:rust:billing::Invoice::total",
                Editing,
                Some(invoice),
                Some((40, 52)),
                380,
            ),
            claim(
                "7f3a",
                "sym:rust:billing::Invoice::total",
                Intent,
                Some(invoice),
                None,
                505,
            ),
            claim(
                "8a22",
                "sym:rust:billing::Money::round",
                Editing,
                Some(money),
                Some((12, 30)),
                380,
            ),
            claim(
                "m2e5",
                "sym:rust:billing::Money::round",
                Editing,
                Some(money),
                Some((12, 30)),
                318,
            ),
            claim(
                "7f3a",
                "test:payment::charge::tests::pays_once",
                Editing,
                None,
                None,
                505,
            ),
            claim(
                "k9d4",
                "test:payment::charge::tests::pays_once",
                Editing,
                None,
                None,
                420,
            ),
        ],
    }
}

/// The radar of the sample: five sessions and four pairs, highest risk first.
pub fn radar() -> crate::coord::RadarList {
    use crate::coord::{RadarHit, RadarNode};
    let node = |id: &str, provider: &str, part: &str, owner: &str, state| RadarNode {
        id: s(id),
        provider: s(provider),
        part: s(part),
        owner: Some(s(owner)),
        state,
    };
    let hit =
        |a: &str, b: &str, tier, keys: &[&str], score, files: &[&str], trial, minutes| RadarHit {
            a: s(a),
            b: s(b),
            codebase: s("billing"),
            tier,
            keys: keys.iter().map(|k| s(k)).collect(),
            score,
            conflict_files: files.iter().map(|f| s(f)).collect(),
            trial,
            trial_error: None,
            at: ago(minutes),
            status: None,
        };
    crate::coord::RadarList {
        sessions: vec![
            node("7f3a", "claude", "payment", "you", SessionState::NeedsYou),
            node(
                "k9d4",
                "opencode",
                "payment",
                "Jonas",
                SessionState::Working,
            ),
            node("8a22", "gemini", "invoices", "you", SessionState::Working),
            node("8d07", "claude", "database", "you", SessionState::Working),
            node("m2e5", "codex", "invoices", "Mara", SessionState::Working),
        ],
        hits: vec![
            hit(
                "7f3a",
                "k9d4",
                1,
                &[
                    "path:src/payment/charge.rs",
                    "test:payment::charge::tests::pays_once",
                ],
                1.0,
                &["src/payment/charge.rs"],
                true,
                3.0,
            ),
            hit(
                "7f3a",
                "8a22",
                3,
                &["sym:rust:billing::Invoice::total"],
                0.7,
                &[],
                true,
                5.0,
            ),
            hit(
                "8a22",
                "m2e5",
                2,
                &["sym:rust:billing::Money::round"],
                0.4,
                &[],
                true,
                9.0,
            ),
            hit(
                "8d07",
                "m2e5",
                1,
                &["mod:billing/db", "path:billing/db/schema.rs"],
                0.1,
                &[],
                true,
                14.0,
            ),
        ],
    }
}

/// The mailbox of a session of the sample. 7f3a has two pings, and 5e81 has a block that the
/// human set: the human can free it with `block/override`.
pub fn mail(session: &str) -> crate::coord::MailList {
    use crate::coord::{MailItem, PingKind};
    let items = if session == "7f3a" {
        vec![
            MailItem {
                id: 1,
                from: s("8a22"),
                kind: PingKind::Fyi,
                key: s("sym:rust:billing::Invoice::total"),
                text: s(
                    "8a22 also changes sym:rust:billing::Invoice::total (radar tier 3, risk 0.7)",
                ),
                created_at: ago(5.0),
                delivered_at: Some(ago(4.0)),
                resolved: false,
                count: 1,
                digest: false,
            },
            MailItem {
                id: 2,
                from: s("k9d4"),
                kind: PingKind::Fyi,
                key: s("path:src/payment/charge.rs"),
                text: s(
                    "k9d4 also changes path:src/payment/charge.rs (radar tier 1, risk 1.0; text conflict in src/payment/charge.rs)",
                ),
                created_at: ago(3.0),
                delivered_at: None,
                resolved: false,
                count: 1,
                digest: false,
            },
        ]
    } else if session == BLOCKED_SESSION {
        vec![MailItem {
            id: 1,
            from: s("you"),
            kind: PingKind::Block,
            key: s("path:guard/scanner/cache.py"),
            text: s("Stop changing guard/scanner/cache.py until 6c40 lands."),
            created_at: ago(7.0),
            delivered_at: Some(ago(6.0)),
            resolved: false,
            count: 1,
            digest: false,
        }]
    } else {
        Vec::new()
    };
    crate::coord::MailList {
        session: s(session),
        items,
    }
}
