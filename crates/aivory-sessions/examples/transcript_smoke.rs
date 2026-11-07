//! Smoke test on real sessions. Prints counts only: no message text.
//!
//! A second table shows the span trace of each session: the number of spans of each kind, the
//! number of DAG nodes, and the time of `trace`.
//!
//! Usage: `cargo run --release -p aivory-sessions --example transcript_smoke [N] [provider]`
//! (default: the 20 newest sessions of `~/.claude`; provider is `claude`, `codex` or `gemini`).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aivory_protocol::{LineKind, SessionState, SpanKind};
use aivory_sessions::transcript::{derive_state, normalize, subagents, trace};
use aivory_sessions::{
    ClaudeStore, CodexStore, FileRole, GeminiStore, Provider, SessionStore, StoreRoots,
};

const KINDS: [(LineKind, char); 9] = [
    (LineKind::Meta, 'x'),
    (LineKind::User, 'u'),
    (LineKind::Agent, 'a'),
    (LineKind::Tool, 't'),
    (LineKind::Output, 'o'),
    (LineKind::Added, 'p'),
    (LineKind::Removed, 'm'),
    (LineKind::Subagent, 's'),
    (LineKind::Error, 'e'),
];

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);
    let provider: Provider = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(Provider::Claude);
    let Some(roots) = StoreRoots::for_current_user() else {
        eprintln!("home directory unknown");
        return;
    };
    let store: Box<dyn SessionStore> = match provider {
        Provider::Claude => Box::new(ClaudeStore::new(roots.claude)),
        Provider::Codex => Box::new(CodexStore::new(roots.codex)),
        Provider::Gemini => Box::new(GeminiStore::new(roots.gemini)),
    };
    let mut files: Vec<_> = store
        .enumerate()
        .into_iter()
        .filter(|f| f.role == FileRole::Session)
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.mtime_ms));
    files.truncate(count);
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(0);

    let (mut total_ms, mut total_bytes) = (0.0, 0u64);
    let mut states = std::collections::BTreeMap::<char, usize>::new();
    println!("#  size_kb  lines  x u a t o p m s e  state ask subs  norm_ms state_ms");
    for (i, f) in files.iter().enumerate() {
        let t0 = Instant::now();
        let lines = match normalize(provider, &f.path) {
            Ok(l) => l,
            Err(e) => {
                println!("{i:>2} normalize failed: {}", e.to_string().len());
                continue;
            }
        };
        let norm_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let derived = derive_state(provider, &f.path, now, f.mtime_ms);
        let state_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let counts: Vec<String> = KINDS
            .iter()
            .map(|(k, _)| lines.iter().filter(|l| l.kind == *k).count().to_string())
            .collect();
        let (code, ask) = match &derived {
            Ok(d) => (d.state.code(), d.ask.as_ref().map_or("-", |a| a.kind.tag())),
            Err(_) => ('?', "-"),
        };
        *states.entry(code).or_default() += 1;
        let subs = if provider == Provider::Claude {
            subagents(&f.path, now)
        } else {
            Vec::new()
        };
        let done = subs
            .iter()
            .filter(|s| s.state == SessionState::Done)
            .count();
        println!(
            "{i:>2} {:>8} {:>6}  {}  {code} {ask} {}/{}  {norm_ms:.1} {state_ms:.1}",
            f.size / 1024,
            lines.len(),
            counts.join(" "),
            done,
            subs.len(),
        );
        total_ms += norm_ms;
        total_bytes += f.size;
    }
    println!(
        "sessions {} bytes {} normalize_total_ms {:.1} states {:?}",
        files.len(),
        total_bytes,
        total_ms,
        states
    );

    println!(
        "
#  size_kb  run turn llm tool agent  nodes edges  trace_ms"
    );
    let (mut trace_ms, mut slowest) = (0.0, 0.0f64);
    for (i, f) in files.iter().enumerate() {
        let id = f
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let t0 = Instant::now();
        let result = trace(provider, &f.path, &id, now);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(tr) => {
                let n = |k: SpanKind| tr.spans.iter().filter(|s| s.kind == k).count();
                println!(
                    "{i:>2} {:>8}  {:>3} {:>4} {:>3} {:>4} {:>5}  {:>5} {:>5}  {ms:.1}",
                    f.size / 1024,
                    n(SpanKind::Run),
                    n(SpanKind::Turn),
                    n(SpanKind::Llm),
                    n(SpanKind::Tool),
                    n(SpanKind::Agent),
                    tr.nodes.len(),
                    tr.edges.len(),
                );
            }
            Err(_) => println!("{i:>2} trace failed"),
        }
        trace_ms += ms;
        slowest = slowest.max(ms);
    }
    println!("trace_total_ms {trace_ms:.1} slowest_ms {slowest:.1}");
}
