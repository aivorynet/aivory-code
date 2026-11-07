//! The pure parts of the session deep dive: span rows with collapse, the minute axis, the kind
//! filter, the stats strip, the evidence fields, the transcript lookup and the DAG edge style.

use std::collections::{HashMap, HashSet};

use aivory_protocol::{
    DagNode, DiffFile, Line, LineKind, MINUTE_MS, Pane, SessionState, Span, SpanKind, UnixMs,
};

/// The bar and swatch color of a span kind.
pub fn kind_color(kind: SpanKind) -> u32 {
    match kind {
        SpanKind::Run => 0x1A1A2E,
        SpanKind::Turn => 0x4B5BDB,
        SpanKind::Llm => 0x8B5CF6,
        SpanKind::Tool => 0xE8A23B,
        SpanKind::Agent => 0x0EA5A4,
    }
}

/// The small tag after a row name, and the head of the evidence panel.
pub fn kind_tag(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Run => "RUN",
        SpanKind::Turn => "TURN",
        SpanKind::Llm => "LLM",
        SpanKind::Tool => "TOOL",
        SpanKind::Agent => "AGENT",
    }
}

/// The value of the `Kind` field.
pub fn kind_name(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Run => "run",
        SpanKind::Turn => "turn",
        SpanKind::Llm => "llm",
        SpanKind::Tool => "tool",
        SpanKind::Agent => "agent",
    }
}

/// The kind chips: `All` shows every row at full strength, a kind dims the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFilter {
    All,
    Only(SpanKind),
}

pub const CHIPS: [(KindFilter, &str); 5] = [
    (KindFilter::All, "All"),
    (KindFilter::Only(SpanKind::Llm), "LLM"),
    (KindFilter::Only(SpanKind::Tool), "Tool"),
    (KindFilter::Only(SpanKind::Agent), "Agent"),
    (KindFilter::Only(SpanKind::Turn), "Turn"),
];

impl KindFilter {
    pub fn matches(self, kind: SpanKind) -> bool {
        match self {
            Self::All => true,
            Self::Only(only) => only == kind,
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Self::All => 0x1A1A2E,
            Self::Only(kind) => kind_color(kind),
        }
    }
}

/// One visible row of the waterfall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The index in the span list.
    pub index: usize,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
    /// The kind filter or the search does not match: the row shows at 35 %.
    pub dim: bool,
}

/// The rows under the collapsed spans drop out. Spans come parents first, siblings by start.
pub fn rows(
    spans: &[Span],
    collapsed: &HashSet<String>,
    filter: KindFilter,
    query: &str,
) -> Vec<Row> {
    let index: HashMap<&str, usize> = spans
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let parent_of = |i: usize| {
        spans[i]
            .parent
            .as_deref()
            .and_then(|p| index.get(p).copied())
    };
    let mut has_children = vec![false; spans.len()];
    for i in 0..spans.len() {
        if let Some(p) = parent_of(i) {
            has_children[p] = true;
        }
    }
    let query = query.trim().to_lowercase();
    let mut out = Vec::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        let (mut depth, mut hidden, mut at) = (0, false, parent_of(i));
        while let Some(p) = at {
            depth += 1;
            hidden |= collapsed.contains(&spans[p].id);
            if depth > spans.len() {
                break;
            }
            at = parent_of(p);
        }
        if hidden {
            continue;
        }
        out.push(Row {
            index: i,
            depth,
            has_children: has_children[i],
            collapsed: collapsed.contains(&span.id),
            dim: !filter.matches(span.kind) || (!query.is_empty() && !matches_query(span, &query)),
        });
    }
    out
}

/// `query` is lower case and trimmed. Name, agent, tool and input are searched.
pub fn matches_query(span: &Span, query: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(query);
    has(&span.name) || has(&span.agent) || has(&span.input) || span.tool.as_deref().is_some_and(has)
}

/// One mark of the minute axis: `frac` is the place on the axis, 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub frac: f32,
    pub label: String,
}

/// The minute axis of the waterfall.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub origin: UnixMs,
    /// The length of the axis in ms: whole minutes.
    pub total: i64,
    pub ticks: Vec<Tick>,
    /// The blue "now" line. `None` when no span runs.
    pub now: Option<f32>,
}

/// Tick steps in minutes. The step is the smallest one that gives 5 ticks or fewer.
const STEPS: [i64; 11] = [1, 2, 5, 10, 15, 30, 60, 120, 240, 720, 1440];

impl Axis {
    pub fn new(spans: &[Span], now: UnixMs) -> Self {
        let origin = spans.iter().map(|s| s.start).min().unwrap_or(now);
        let end = spans
            .iter()
            .map(|s| s.end.unwrap_or(now))
            .max()
            .unwrap_or(now)
            .max(origin);
        let minutes = ((end - origin + MINUTE_MS - 1) / MINUTE_MS).max(1);
        let step = STEPS
            .iter()
            .copied()
            .find(|s| minutes / s <= 4)
            .unwrap_or(1440);
        let total = minutes * MINUTE_MS;
        let ticks = (0..=minutes / step)
            .map(|k| Tick {
                frac: (k * step) as f32 / minutes as f32,
                label: tick_label(k * step),
            })
            .collect();
        let running = spans.iter().any(|s| s.end.is_none());
        Self {
            origin,
            total,
            ticks,
            now: running.then(|| ((now - origin) as f64 / total as f64).clamp(0., 1.) as f32),
        }
    }

    /// Where a span starts and how wide its bar is, as fractions of the axis. A running span
    /// ends at `now`. The bar is never thinner than 0.35 %.
    pub fn bar(&self, span: &Span, now: UnixMs) -> (f32, f32) {
        let end = span.end.unwrap_or(now);
        let x = ((span.start - self.origin) as f64 / self.total as f64).clamp(0., 1.);
        let w = ((end - span.start).max(0) as f64 / self.total as f64).max(0.0035);
        (x as f32, w.min(1. - x) as f32)
    }
}

fn tick_label(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// `36 s`, `6.2 min`, `4 min`, `2.5 h`.
pub fn fmt_dur(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 59_500 {
        return format!("{} s", (ms as f64 / 1000.).round() as i64);
    }
    let minutes = ms as f64 / MINUTE_MS as f64;
    if minutes >= 120. {
        format!("{} h", one_decimal(minutes / 60.))
    } else {
        format!("{} min", one_decimal(minutes))
    }
}

fn one_decimal(x: f64) -> String {
    let r = (x * 10.).round() / 10.;
    if r.fract() == 0. {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

/// `412,380`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `HH:MM:SS`: UTC in demo mode, local time otherwise.
pub fn clock_hms(at: UnixMs, utc: bool) -> String {
    let Some(t) = chrono::DateTime::from_timestamp_millis(at) else {
        return String::new();
    };
    if utc {
        t.format("%H:%M:%S").to_string()
    } else {
        t.with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string()
    }
}

/// The status word of a span, a DAG card and the evidence panel.
pub fn status_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Done => "done",
        SessionState::Working => "running",
        SessionState::NeedsYou => "waiting",
        SessionState::Error => "error",
        SessionState::Idle => "idle",
        SessionState::Parked => "planned",
    }
}

/// Text and background color of a status pill.
pub fn status_colors(state: SessionState) -> (u32, u32) {
    match state {
        SessionState::Done => (0x15803D, 0xEAF7EE),
        SessionState::Working => (0x1A1A2E, 0xEEF1F8),
        SessionState::NeedsYou => (0xC2410C, 0xFFF1E6),
        SessionState::Error => (0xB91C1C, 0xFDECEC),
        SessionState::Idle | SessionState::Parked => (0x5A6578, 0xF1F3F6),
    }
}

pub fn provider_label(provider: &str) -> String {
    match provider {
        "claude" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "gemini" => "Gemini".into(),
        "opencode" => "OpenCode".into(),
        "goose" => "Goose".into(),
        other => other.into(),
    }
}

/// The 12 fields of the evidence panel, in reading order (two columns).
pub fn evidence_fields(
    span: &Span,
    provider: &str,
    now: UnixMs,
    utc: bool,
) -> Vec<(&'static str, String)> {
    vec![
        ("Kind", kind_name(span.kind).into()),
        ("Duration", fmt_dur(span.end.unwrap_or(now) - span.start)),
        ("Agent", span.agent.clone()),
        ("Model", span.model.clone()),
        ("Provider", provider_label(provider)),
        ("Tool", span.tool.clone().unwrap_or_else(|| "—".into())),
        ("Input tokens", thousands(span.tokens_in)),
        ("Output tokens", thousands(span.tokens_out)),
        ("Cost", format!("${:.2}", span.cost)),
        ("Span", span.id.clone()),
        ("Start", clock_hms(span.start, utc)),
        (
            "End",
            match span.end {
                Some(end) => clock_hms(end, utc),
                None => "running".into(),
            },
        ),
    ]
}

/// The five cells of the stats strip. `None` shows a dash: the daemon has no trace.
pub fn stats_cells(
    subagents: u32,
    turns: Option<u32>,
    tool_calls: Option<u32>,
    cost: f64,
    context_pct: u8,
) -> [(&'static str, String); 5] {
    let count = |n: Option<u32>| n.map_or_else(|| "—".to_string(), |n| n.to_string());
    [
        ("Agents", format!("1 + {subagents} sub")),
        ("Turns", count(turns)),
        ("Tool calls", count(tool_calls)),
        ("Cost", format!("${cost:.2}")),
        ("Context", format!("{context_pct}%")),
    ]
}

/// The line above the composer: what the session does now.
pub fn activity_text(pane: &Pane, now: UnixMs) -> String {
    let minutes = pane.minutes(now);
    match pane.state {
        SessionState::NeedsYou => format!("Waiting for your answer · {minutes} min"),
        SessionState::Error => format!("Stopped {minutes} min ago"),
        SessionState::Idle => "Idle · waits for a task".to_string(),
        SessionState::Parked => "Parked".to_string(),
        SessionState::Done => "Done".to_string(),
        SessionState::Working => match pane.tail.iter().rev().find(|l| l.kind == LineKind::Tool) {
            Some(l) => format!(
                "Running {} · {}",
                l.label.as_deref().unwrap_or("Think"),
                l.text
            ),
            None => "Running Think".to_string(),
        },
    }
}

/// The transcript line that belongs to a span, for "Open in transcript".
pub fn find_line(lines: &[Line], span: &Span) -> Option<usize> {
    let position = |f: &dyn Fn(&Line) -> bool| lines.iter().position(f);
    match span.kind {
        SpanKind::Run => (!lines.is_empty()).then_some(0),
        SpanKind::Turn => {
            let number: String = span
                .name
                .trim_start_matches(|c: char| !c.is_ascii_digit())
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let want = format!("TURN {number}");
            position(&|l| {
                l.kind == LineKind::Turn
                    && l.label
                        .as_deref()
                        .is_some_and(|label| label.eq_ignore_ascii_case(&want))
            })
        }
        SpanKind::Tool => position(&|l| {
            l.kind == LineKind::Tool && !span.input.is_empty() && l.text.contains(&span.input)
        }),
        SpanKind::Agent => position(&|l| {
            l.kind == LineKind::Subagent && l.label.as_deref() == Some(span.agent.as_str())
        }),
        SpanKind::Llm => {
            let head: String = span.output.chars().take(24).collect();
            if head.is_empty() {
                return None;
            }
            position(&|l| l.kind == LineKind::Agent && l.text.contains(&head))
        }
    }
}

/// The newest turn that wrote a line of the file (the `turn 2` of the diff header).
pub fn file_turn(file: &DiffFile) -> Option<u32> {
    file.lines
        .iter()
        .filter_map(|l| l.blame.as_ref().map(|b| b.turn))
        .max()
}

/// The right text of a DAG card: the run time, or `waits` and `planned`.
pub fn node_duration(node: &DagNode) -> String {
    match (node.duration_ms, node.state) {
        (Some(ms), _) => fmt_dur(ms as i64),
        (None, SessionState::NeedsYou) => "waits".into(),
        (None, SessionState::Parked) => "planned".into(),
        (None, _) => "—".into(),
    }
}

/// The edge into a node: its color and whether it is dashed (waiting and planned nodes).
pub fn edge_style(to: SessionState) -> (u32, bool) {
    let color = if to == SessionState::Parked {
        0xC9CED8
    } else {
        0x8A93A6
    };
    (
        color,
        matches!(to, SessionState::Parked | SessionState::NeedsYou),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;
    use aivory_protocol::{SessionTrace, layout};

    fn trace() -> (SessionTrace, UnixMs) {
        let demo = Demo::new();
        let trace = demo.traces.get("a01c").expect("demo trace a01c").clone();
        (trace, demo.war_room.now)
    }

    fn span(id: &str, parent: Option<&str>, kind: SpanKind, start: i64, end: Option<i64>) -> Span {
        Span {
            id: id.into(),
            parent: parent.map(Into::into),
            kind,
            name: format!("name {id}"),
            agent: "main".into(),
            model: "opus-5.5".into(),
            start,
            end,
            state: SessionState::Done,
            tokens_in: 0,
            tokens_out: 0,
            tokens_cache_read: None,
            tokens_cache_write: None,
            cost: 0.,
            tool: None,
            input: String::new(),
            output: String::new(),
        }
    }

    fn ids(spans: &[Span], rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| spans[r.index].id.clone()).collect()
    }

    #[test]
    fn rows_show_depth_and_children() {
        let (trace, _) = trace();
        let rows = rows(&trace.spans, &HashSet::new(), KindFilter::All, "");
        assert_eq!(rows.len(), 21);
        assert_eq!(
            (rows[0].depth, rows[0].has_children, rows[0].collapsed),
            (0, true, false)
        );
        // s6 (chat of the explorer) sits under s5 under s2 under s1.
        let s6 = rows
            .iter()
            .find(|r| trace.spans[r.index].id == "s6")
            .expect("row s6");
        assert_eq!((s6.depth, s6.has_children), (3, false));
        assert!(rows.iter().all(|r| !r.dim));
    }

    #[test]
    fn collapse_hides_the_whole_subtree() {
        let (trace, _) = trace();
        let collapsed: HashSet<String> = ["s5".to_string()].into();
        let rows = rows(&trace.spans, &collapsed, KindFilter::All, "");
        let shown = ids(&trace.spans, &rows);
        assert_eq!(rows.len(), 18);
        assert!(shown.contains(&"s5".to_string()));
        assert!(
            ["s6", "s7", "s8"]
                .iter()
                .all(|id| !shown.contains(&id.to_string()))
        );
        let s5 = rows
            .iter()
            .find(|r| trace.spans[r.index].id == "s5")
            .unwrap();
        assert!(s5.collapsed && s5.has_children);
        // A collapsed run keeps only itself.
        let collapsed: HashSet<String> = ["s1".to_string()].into();
        assert_eq!(rows_len(&trace.spans, &collapsed), 1);
    }

    fn rows_len(spans: &[Span], collapsed: &HashSet<String>) -> usize {
        rows(spans, collapsed, KindFilter::All, "").len()
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        let spans = vec![
            span("a", Some("b"), SpanKind::Turn, 0, Some(1)),
            span("b", Some("a"), SpanKind::Turn, 0, Some(1)),
        ];
        assert_eq!(rows(&spans, &HashSet::new(), KindFilter::All, "").len(), 2);
    }

    #[test]
    fn kind_chips_dim_the_other_rows() {
        let (trace, _) = trace();
        let tools = rows(
            &trace.spans,
            &HashSet::new(),
            KindFilter::Only(SpanKind::Tool),
            "",
        );
        assert_eq!(tools.len(), 21, "a chip dims, it does not hide");
        for r in &tools {
            assert_eq!(r.dim, trace.spans[r.index].kind != SpanKind::Tool);
        }
        assert!(KindFilter::All.matches(SpanKind::Run));
        assert!(!KindFilter::Only(SpanKind::Llm).matches(SpanKind::Tool));
        assert_eq!(KindFilter::Only(SpanKind::Agent).color(), 0x0EA5A4);
        assert_eq!(CHIPS.len(), 5);
    }

    #[test]
    fn the_search_dims_the_rows_that_do_not_match() {
        let (trace, _) = trace();
        let rows = rows(
            &trace.spans,
            &HashSet::new(),
            KindFilter::All,
            " Cargo TEST ",
        );
        let lit: Vec<String> = rows
            .iter()
            .filter(|r| !r.dim)
            .map(|r| trace.spans[r.index].id.clone())
            .collect();
        assert_eq!(lit, ["s15", "s16"]);
    }

    #[test]
    fn the_axis_of_the_artboard_has_five_ticks_in_five_minute_steps() {
        let (trace, now) = trace();
        let axis = Axis::new(&trace.spans, now);
        assert_eq!(axis.total, 24 * MINUTE_MS);
        let labels: Vec<&str> = axis.ticks.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["0 min", "5 min", "10 min", "15 min", "20 min"]);
        assert!((axis.ticks[1].frac - 5. / 24.).abs() < 1e-6);
        assert!((axis.now.expect("a running span") - 23.4 / 24.).abs() < 1e-3);
    }

    #[test]
    fn a_finished_session_has_no_now_line_and_hour_ticks_for_long_runs() {
        let spans = vec![span("a", None, SpanKind::Run, 0, Some(3 * 60 * MINUTE_MS))];
        let axis = Axis::new(&spans, 10 * 60 * MINUTE_MS);
        assert_eq!(axis.now, None);
        let labels: Vec<&str> = axis.ticks.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["0 min", "1 h", "2 h", "3 h"]);
        let short = Axis::new(&[span("a", None, SpanKind::Run, 0, Some(30_000))], 0);
        assert_eq!(short.total, MINUTE_MS);
        assert_eq!(short.ticks.len(), 2);
    }

    #[test]
    fn bars_follow_the_artboard() {
        let (trace, now) = trace();
        let axis = Axis::new(&trace.spans, now);
        let s2 = trace.spans.iter().find(|s| s.id == "s2").unwrap();
        let (x, w) = axis.bar(s2, now);
        assert!(x.abs() < 1e-6 && (w - 6.2 / 24.).abs() < 1e-4);
        // A running span reaches the now line.
        let s12 = trace.spans.iter().find(|s| s.id == "s12").unwrap();
        let (x, w) = axis.bar(s12, now);
        assert!((x + w - 23.4 / 24.).abs() < 1e-3);
        // A 1 s tool call stays visible.
        let s4 = trace.spans.iter().find(|s| s.id == "s4").unwrap();
        assert!(axis.bar(s4, now).1 >= 0.0035);
    }

    #[test]
    fn durations_read_like_the_artboard() {
        assert_eq!(fmt_dur(36_000), "36 s");
        assert_eq!(fmt_dur(1_200), "1 s");
        assert_eq!(fmt_dur(372_000), "6.2 min");
        assert_eq!(fmt_dur(4 * MINUTE_MS), "4 min");
        assert_eq!(fmt_dur(59_800), "1 min");
        assert_eq!(fmt_dur(150 * MINUTE_MS), "2.5 h");
        assert_eq!(thousands(412_380), "412,380");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1_000), "1,000");
    }

    #[test]
    fn stats_read_like_the_artboard() {
        let (trace, _) = trace();
        let s = &trace.stats;
        let cells = stats_cells(
            s.subagents,
            Some(s.turns),
            Some(s.tool_calls),
            s.cost,
            s.context_pct,
        );
        let values: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
        // The artboard shows 41 %; the demo pane of a01c carries its own context value.
        let context = format!("{}%", s.context_pct);
        assert_eq!(values, ["1 + 3 sub", "3", "47", "$3.12", context.as_str()]);
        let keys: Vec<&str> = cells.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, ["Agents", "Turns", "Tool calls", "Cost", "Context"]);
        let none = stats_cells(0, None, None, 0.5, 7);
        assert_eq!(none[1].1, "—");
        assert_eq!(none[0].1, "1 + 0 sub");
    }

    #[test]
    fn the_evidence_panel_has_twelve_fields() {
        let (trace, now) = trace();
        let s12 = trace.spans.iter().find(|s| s.id == "s12").unwrap();
        let fields = evidence_fields(s12, "claude", now, true);
        assert_eq!(fields.len(), 12);
        let get = |k: &str| fields.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        assert_eq!(get("Kind"), "agent");
        assert_eq!(get("Agent"), "test-writer");
        assert_eq!(get("Provider"), "Claude Code");
        assert_eq!(get("Input tokens"), "61,300");
        assert_eq!(get("Cost"), "$0.44");
        assert_eq!(get("End"), "running");
        assert_eq!(get("Tool"), "—");
        let s15 = trace.spans.iter().find(|s| s.id == "s15").unwrap();
        let tool = evidence_fields(s15, "claude", now, true);
        assert_eq!(tool.iter().find(|(k, _)| *k == "Tool").unwrap().1, "bash");
        let s4 = trace.spans.iter().find(|s| s.id == "s4").unwrap();
        let done = evidence_fields(s4, "claude", now, true);
        assert_eq!(done[11].1.len(), 8, "HH:MM:SS");
        assert_eq!(kind_tag(SpanKind::Agent), "AGENT");
        assert_eq!(kind_color(SpanKind::Tool), 0xE8A23B);
    }

    #[test]
    fn the_transcript_lookup_finds_turns_tools_and_agents() {
        let lines = vec![
            Line::meta("claude opus-5.5"),
            Line::user("cap subagent fan-out"),
            Line::tool("Read", "crates/orch/src/spawn.rs"),
            Line::subagent("explorer", "map every spawn call site"),
            Line::turn("TURN 2", "6.2 min"),
            Line::agent("Budget struct in place. 22 tests pass."),
        ];
        let mut s = span("s10", None, SpanKind::Turn, 0, None);
        s.name = "turn 2 · write the budget struct".into();
        assert_eq!(find_line(&lines, &s), Some(4));
        s.name = "turn 9".into();
        assert_eq!(find_line(&lines, &s), None);
        let mut tool = span("s4", None, SpanKind::Tool, 0, None);
        tool.input = "crates/orch/src/spawn.rs".into();
        assert_eq!(find_line(&lines, &tool), Some(2));
        let mut agent = span("s5", None, SpanKind::Agent, 0, None);
        agent.agent = "explorer".into();
        assert_eq!(find_line(&lines, &agent), Some(3));
        let mut llm = span("s9", None, SpanKind::Llm, 0, None);
        llm.output = "Budget struct in place. 22 tests pass.".into();
        assert_eq!(find_line(&lines, &llm), Some(5));
        assert_eq!(
            find_line(&lines, &span("s1", None, SpanKind::Run, 0, None)),
            Some(0)
        );
    }

    #[test]
    fn dag_edges_are_dashed_into_waiting_and_planned_nodes() {
        assert_eq!(edge_style(SessionState::Parked), (0xC9CED8, true));
        assert_eq!(edge_style(SessionState::NeedsYou), (0x8A93A6, true));
        assert_eq!(edge_style(SessionState::Working), (0x8A93A6, false));
        assert_eq!(edge_style(SessionState::Done), (0x8A93A6, false));
        let (trace, _) = trace();
        let texts: Vec<String> = trace.nodes.iter().map(node_duration).collect();
        assert_eq!(
            texts,
            [
                "4.2 min", "8.6 min", "16.4 min", "8.6 min", "waits", "planned"
            ]
        );
        let pos = layout::dag(&trace.nodes, &trace.edges);
        assert_eq!(pos.len(), 6);
    }

    #[test]
    fn the_diff_header_names_the_newest_turn() {
        let demo = Demo::new();
        let plain = demo.diffs.get("a01c").expect("demo diff a01c");
        assert!(plain.files.iter().all(|f| file_turn(f).is_none()));
        let blamed = demo.diffs.get("a7e2").expect("demo diff a7e2");
        assert!(blamed.files.iter().any(|f| file_turn(f).is_some()));
    }

    #[test]
    fn status_words_match_the_dag_labels() {
        assert_eq!(status_label(SessionState::Done), "done");
        assert_eq!(status_label(SessionState::Working), "running");
        assert_eq!(status_label(SessionState::NeedsYou), "waiting");
        assert_eq!(status_label(SessionState::Parked), "planned");
        assert_eq!(status_colors(SessionState::NeedsYou), (0xC2410C, 0xFFF1E6));
    }

    #[test]
    fn the_activity_line_names_the_running_tool() {
        let demo = Demo::new();
        let pane = demo.war_room.session("a01c").expect("demo session a01c");
        let text = activity_text(pane, demo.war_room.now);
        assert!(text.starts_with("Running "), "{text}");
    }
}
