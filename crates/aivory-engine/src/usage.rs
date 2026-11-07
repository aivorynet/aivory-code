//! Provider usage windows, estimated from local token events.
//!
//! The daemon reads token counts from the session transcripts. It does not ask the provider, so
//! every window is an estimate (`UsageWindow::estimate` is `true`). Two windows exist for each
//! provider in [`LIMITS`]: `5 h window` and `Week`.
//!
//! The `5 h window` is a block, as a Claude subscription counts it: a block starts at the first
//! call after the last block ended and lasts 5 h; it resets at its start + 5 h. The `Week` is a
//! sliding window that ends at `now`: an event counts when `now - window < at <= now`, and it
//! frees up when its oldest call leaves it. An event in the future (a clock that steps back)
//! does not count.
//!
//! A provider gets windows only when it is in [`LIMITS`] and `events` holds at least one event of
//! it. The windows come in table order, `5 h window` before `Week`.

use aivory_protocol::{MINUTE_MS, Span, UnixMs, UsageWindow};

use crate::pricing::llm_calls;

/// Length of the short window.
pub const FIVE_HOURS_MS: i64 = 5 * 60 * MINUTE_MS;
/// Length of the long window.
pub const WEEK_MS: i64 = 7 * 24 * 60 * MINUTE_MS;

/// The token limits of one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub provider: &'static str,
    /// Tokens (input plus output) that fill the `5 h window`.
    pub five_hour: u64,
    /// Tokens (input plus output) that fill the `Week` window.
    pub week: u64,
}

/// The limits per provider, the only place that holds them.
///
/// # Assumptions
///
/// The providers do not publish token limits for a subscription. The numbers below are
/// assumptions, not facts. They put a percentage on the bar of the status line. Tune them from
/// real use, or replace them with a value that the provider reports. A provider that is not in
/// the table (`gemini`, `opencode`, `goose`) has no estimate: its limits are daily or monthly,
/// or they are money.
pub const LIMITS: &[Limits] = &[
    Limits {
        provider: "claude",
        five_hour: 3_000_000,
        week: 30_000_000,
    },
    Limits {
        provider: "codex",
        five_hour: 2_000_000,
        week: 20_000_000,
    },
];

/// Tokens that one model call used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEvent {
    pub provider: String,
    pub at: UnixMs,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// The usage windows at `now`, see the module docs.
pub fn estimate(events: &[TokenEvent], now: UnixMs) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    for limits in LIMITS {
        let own = || events.iter().filter(|e| e.provider == limits.provider);
        if own().next().is_none() {
            continue;
        }
        let sum = |from: UnixMs| {
            own()
                .filter(|e| e.at <= now && e.at >= from)
                .fold(0u64, |sum, e| {
                    sum.saturating_add(e.tokens_in).saturating_add(e.tokens_out)
                })
        };
        // The 5 h window is the current block; the week slides.
        let block = block_start(own().map(|e| e.at), now, FIVE_HOURS_MS);
        let week_from = now.saturating_sub(WEEK_MS) + 1;
        let week_oldest = own()
            .map(|e| e.at)
            .filter(|&at| at <= now && at >= week_from)
            .min();
        for (label, limit, used, resets_at) in [
            (
                "5 h window",
                limits.five_hour,
                block.map_or(0, sum),
                block.map(|s| s + FIVE_HOURS_MS),
            ),
            (
                "Week",
                limits.week,
                sum(week_from),
                // A sliding window frees up when its oldest call leaves it.
                week_oldest.map(|at| at + WEEK_MS),
            ),
        ] {
            windows.push(UsageWindow {
                provider: limits.provider.to_string(),
                label: label.to_string(),
                pct: percent(used, limit),
                used,
                resets_at,
                estimate: true,
            });
        }
    }
    windows
}

/// The start of the block that holds `now`, or `None` when no block is open. A block starts at
/// the first call after the last block ended and lasts `len` (the 5 h blocks of a Claude
/// subscription).
pub fn block_start(times: impl Iterator<Item = UnixMs>, now: UnixMs, len: i64) -> Option<UnixMs> {
    let mut times: Vec<UnixMs> = times.filter(|&t| t <= now).collect();
    times.sort_unstable();
    let mut start: Option<UnixMs> = None;
    for t in times {
        if start.is_none_or(|s| t >= s + len) {
            start = Some(t);
        }
    }
    start.filter(|&s| now < s + len)
}

/// The usage windows from the `Llm` spans of many sessions.
///
/// Each item is a provider (`claude`, `codex`) and the spans of one session
/// (`SessionTrace::spans`). Every `Llm` span is one [`TokenEvent`] at its start time. The
/// tokens in are the input that the cache did not serve: a cache read is cheap for the provider
/// and does not fill a window like new input. The split is the estimate of
/// [`crate::pricing::llm_calls`]. Every window is an estimate, see [`estimate`].
pub fn usage_windows<'a>(
    sessions: impl IntoIterator<Item = (&'a str, &'a [Span])>,
    now: UnixMs,
) -> Vec<UsageWindow> {
    let mut events = Vec::new();
    for (provider, spans) in sessions {
        events.extend(llm_calls(spans).into_iter().map(|c| TokenEvent {
            provider: provider.to_string(),
            at: c.at,
            tokens_in: c.fresh_in,
            tokens_out: c.out,
        }));
    }
    estimate(&events, now)
}

/// `used / limit` in whole percent, rounded half up, capped at 100.
fn percent(used: u64, limit: u64) -> u8 {
    if limit == 0 {
        return if used == 0 { 0 } else { 100 };
    }
    let pct = (u128::from(used) * 100 + u128::from(limit) / 2) / u128::from(limit);
    pct.min(100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: UnixMs = 1_800_000_000_000;

    fn ev(provider: &str, at: UnixMs, tokens_in: u64, tokens_out: u64) -> TokenEvent {
        TokenEvent {
            provider: provider.into(),
            at,
            tokens_in,
            tokens_out,
        }
    }

    fn pcts(windows: &[UsageWindow], provider: &str) -> (u8, u8) {
        let get = |label: &str| {
            windows
                .iter()
                .find(|w| w.provider == provider && w.label == label)
                .unwrap_or_else(|| panic!("no {provider} {label}"))
                .pct
        };
        (get("5 h window"), get("Week"))
    }

    #[test]
    fn two_estimated_windows_per_provider() {
        let w = estimate(&[ev("claude", NOW - 1, 600_000, 300_000)], NOW);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|w| w.estimate && w.provider == "claude"));
        assert_eq!(w[0].label, "5 h window");
        assert_eq!(w[1].label, "Week");
        // 900k of 3M = 30 %; 900k of 30M = 3 %.
        assert_eq!(pcts(&w, "claude"), (30, 3));
    }

    #[test]
    fn a_block_starts_at_the_first_call_after_the_last_block() {
        let h = 60 * MINUTE_MS;
        // Calls at -7 h, -6 h (block -7 h .. -2 h), then -1 h starts a new block.
        let times = [NOW - 7 * h, NOW - 6 * h, NOW - h, NOW - h / 2];
        assert_eq!(
            block_start(times.into_iter(), NOW, FIVE_HOURS_MS),
            Some(NOW - h)
        );
        // No call in the last 5 h: no open block.
        assert_eq!(
            block_start([NOW - 6 * h].into_iter(), NOW, FIVE_HOURS_MS),
            None
        );
        let w = estimate(&[ev("claude", NOW - h, 100, 50)], NOW);
        assert_eq!(w[0].resets_at, Some(NOW + 4 * h));
        assert_eq!(w[0].used, 150);
    }

    #[test]
    fn five_hour_edge_is_excluded_one_ms_later_is_included() {
        let on_edge = estimate(&[ev("claude", NOW - FIVE_HOURS_MS, 3_000_000, 0)], NOW);
        // Out of the 5 h window, still in the week window.
        assert_eq!(pcts(&on_edge, "claude"), (0, 10));
        let inside = estimate(&[ev("claude", NOW - FIVE_HOURS_MS + 1, 3_000_000, 0)], NOW);
        assert_eq!(pcts(&inside, "claude"), (100, 10));
    }

    #[test]
    fn week_edge_is_excluded_one_ms_later_is_included() {
        let on_edge = estimate(&[ev("claude", NOW - WEEK_MS, 30_000_000, 0)], NOW);
        assert_eq!(pcts(&on_edge, "claude"), (0, 0));
        let inside = estimate(&[ev("claude", NOW - WEEK_MS + 1, 30_000_000, 0)], NOW);
        assert_eq!(pcts(&inside, "claude"), (0, 100));
    }

    #[test]
    fn event_at_now_counts_and_future_event_does_not() {
        let w = estimate(
            &[
                ev("codex", NOW, 1_000_000, 0),
                ev("codex", NOW + 1, 1_000_000, 0),
            ],
            NOW,
        );
        assert_eq!(pcts(&w, "codex"), (50, 5));
    }

    #[test]
    fn percent_rounds_half_up_and_caps_at_100() {
        // 5 h limit 3M: 15_000 tokens = 0.5 % -> 1; 14_999 -> 0.
        let w = estimate(&[ev("claude", NOW, 15_000, 0)], NOW);
        assert_eq!(pcts(&w, "claude").0, 1);
        let w = estimate(&[ev("claude", NOW, 14_999, 0)], NOW);
        assert_eq!(pcts(&w, "claude").0, 0);
        let w = estimate(&[ev("claude", NOW, u64::MAX, u64::MAX)], NOW);
        assert_eq!(pcts(&w, "claude"), (100, 100));
    }

    #[test]
    fn providers_are_independent_and_follow_table_order() {
        let w = estimate(
            &[
                ev("codex", NOW, 1_000_000, 0),
                ev("claude", NOW, 300_000, 0),
                ev("claude", NOW - 1000, 0, 300_000),
            ],
            NOW,
        );
        let order: Vec<_> = w.iter().map(|w| w.provider.as_str()).collect();
        assert_eq!(order, ["claude", "claude", "codex", "codex"]);
        assert_eq!(pcts(&w, "claude"), (20, 2));
        assert_eq!(pcts(&w, "codex"), (50, 5));
    }

    #[test]
    fn unknown_provider_and_no_events_give_no_window() {
        assert!(estimate(&[], NOW).is_empty());
        assert!(estimate(&[ev("goose", NOW, 1_000, 1_000)], NOW).is_empty());
    }

    #[test]
    fn provider_with_only_old_events_shows_zero() {
        let w = estimate(&[ev("claude", NOW - 2 * WEEK_MS, 1_000, 0)], NOW);
        assert_eq!(pcts(&w, "claude"), (0, 0));
    }

    #[test]
    fn limit_table_has_no_duplicate_provider_and_week_exceeds_five_hours() {
        for (i, l) in LIMITS.iter().enumerate() {
            assert!(l.week > l.five_hour, "{}", l.provider);
            assert!(LIMITS[i + 1..].iter().all(|o| o.provider != l.provider));
        }
    }

    fn llm_span(agent: &str, start: UnixMs, tokens_in: u64, tokens_out: u64) -> Span {
        use aivory_protocol::{SessionState, SpanKind};
        Span {
            id: format!("{agent}-{start}"),
            parent: None,
            kind: SpanKind::Llm,
            name: "llm".into(),
            agent: agent.into(),
            model: "m".into(),
            start,
            end: None,
            state: SessionState::Done,
            tokens_in,
            tokens_out,
            tokens_cache_read: None,
            tokens_cache_write: None,
            cost: 0.0,
            tool: None,
            input: String::new(),
            output: String::new(),
        }
    }

    #[test]
    fn windows_from_spans_count_new_input_and_output_not_cache_reads() {
        // Call 2 reads 1.0M from the cache and adds 0.5M: 0.5M + 0.2M output count.
        let a = [
            llm_span("main", NOW - 2000, 1_000_000, 100_000),
            llm_span("main", NOW - 1000, 1_500_000, 200_000),
        ];
        // The second session: a codex call of 1M, 5 days old (week only).
        let b = [llm_span(
            "main",
            NOW - 5 * 24 * 60 * MINUTE_MS,
            1_000_000,
            0,
        )];
        let w = usage_windows([("claude", &a[..]), ("codex", &b[..])], NOW);
        // Claude: 1.1M + 0.7M = 1.8M of 3M = 60 %; of 30M = 6 %.
        assert_eq!(pcts(&w, "claude"), (60, 6));
        // Codex: 1M of 20M = 5 % per week, nothing in the 5 h window.
        assert_eq!(pcts(&w, "codex"), (0, 5));
        assert!(w.iter().all(|w| w.estimate));
    }

    #[test]
    fn windows_from_no_spans_are_empty() {
        assert!(usage_windows(std::iter::empty(), NOW).is_empty());
        let none: [Span; 0] = [];
        assert!(usage_windows([("claude", &none[..])], NOW).is_empty());
    }
}
