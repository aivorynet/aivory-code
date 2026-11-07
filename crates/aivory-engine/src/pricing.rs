//! Model prices and the cost estimate of a session.
//!
//! The table holds USD per 1M tokens for the models of each provider: plain input, output,
//! cache write and cache read. [`PriceTable::lookup`] matches the model id by the longest
//! prefix, so `claude-opus-4-5-20251101` finds `claude-opus-4-5`. A model that no prefix
//! matches has no price (`None`); the estimate then skips its calls and counts them as unpriced.
//!
//! An optional TOML file overrides or extends the built-in prices ([`PriceTable::with_file`]):
//!
//! ```toml
//! [[price]]
//! provider = "claude"             # claude, codex or gemini
//! model = "claude-opus-5-5"       # a prefix of the model id
//! input = 4.0                     # USD per 1M tokens
//! output = 20.0
//! cache_write = 5.0               # optional, default: input
//! cache_read = 0.2                # optional, default: input
//! ```
//!
//! # The cache split
//!
//! A [`Span`] of kind `Llm` has `tokens_in`, and `tokens_in` includes the cache reads and the
//! cache writes of the call. When the transcript tells the split (`tokens_cache_read` or
//! `tokens_cache_write` is set), [`llm_calls`] uses it: plain input costs the input price, a
//! cache write the cache-write price, a cache read the cache-read price.
//!
//! # The fallback is an estimate
//!
//! Without the split, [`llm_calls`] estimates it: a call reads from the cache what the previous call of the same agent and model already
//! had as input, `min(previous tokens_in, tokens_in)`. The rest is new input. New input is
//! billed at the cache-write price, because the agents cache the whole prefix. For OpenAI and
//! Google the cache-write price equals the input price, so the rule holds for them too. The
//! first call of an agent has no cache read. The five-minute expiry of a cache entry and the
//! compaction of a context are not modelled. Both make the real cost higher or lower than the
//! estimate, so the result is an estimate, never a bill.

use std::path::Path;

use aivory_protocol::{Span, SpanKind, UnixMs};
use serde::Deserialize;

/// USD per 1M tokens of one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

impl Price {
    const fn new(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_write,
            cache_read,
        }
    }

    /// A provider that does not charge for a cache write: write = input.
    const fn cached(input: f64, cache_read: f64, output: f64) -> Self {
        Self::new(input, output, input, cache_read)
    }

    /// Anthropic: a 5 minute cache write costs 1.25 times the input price.
    const fn claude(input: f64, output: f64, cache_read: f64) -> Self {
        Self::new(input, output, input * 1.25, cache_read)
    }
}

type Row = (&'static str, Price);

// Claude (provider "claude").
// Source: https://platform.claude.com/docs/en/about-claude/pricing (current models and prices
// from the claude-api skill model table, cached 2026-10-06), read 2026-10-22.
// Not re-checked on 2026-10-22: the rows of the older models (Opus 4.1 and older, Sonnet 4.x and
// 3.x, Haiku 3.x). They are the public list prices.
// Cache write: 1.25 x input (5 minute TTL). Cache read: 0.1 x input, except Fable 5.1 and
// Mythos 5.1 (0.025 x), Opus 5.5 (0.05 x) and Haiku 5.5 (0.1 x). Haiku 5.5 is the price for a
// prompt of 100K tokens or fewer.
const CLAUDE: &[Row] = &[
    ("claude-fable-5-1", Price::claude(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", Price::claude(10.0, 50.0, 0.25)),
    ("claude-fable-5", Price::claude(10.0, 50.0, 1.0)),
    ("claude-mythos-5", Price::claude(10.0, 50.0, 1.0)),
    ("claude-opus-5-5", Price::claude(4.0, 20.0, 0.20)),
    ("claude-opus-5", Price::claude(5.0, 25.0, 0.50)),
    ("claude-opus-4-8", Price::claude(5.0, 25.0, 0.50)),
    ("claude-opus-4-7", Price::claude(5.0, 25.0, 0.50)),
    ("claude-opus-4-6", Price::claude(5.0, 25.0, 0.50)),
    ("claude-opus-4-5", Price::claude(5.0, 25.0, 0.50)),
    ("claude-opus-4-1", Price::claude(15.0, 75.0, 1.50)),
    ("claude-opus-4-2025", Price::claude(15.0, 75.0, 1.50)),
    ("claude-sonnet-5-5", Price::claude(2.0, 10.0, 0.20)),
    ("claude-sonnet-5", Price::claude(2.0, 10.0, 0.20)),
    ("claude-sonnet-4", Price::claude(3.0, 15.0, 0.30)),
    ("claude-3-7-sonnet", Price::claude(3.0, 15.0, 0.30)),
    ("claude-3-5-sonnet", Price::claude(3.0, 15.0, 0.30)),
    ("claude-haiku-5-5", Price::claude(0.10, 0.50, 0.01)),
    ("claude-haiku-4-5", Price::claude(1.0, 5.0, 0.10)),
    ("claude-3-5-haiku", Price::claude(0.80, 4.0, 0.08)),
    ("claude-3-haiku", Price::claude(0.25, 1.25, 0.03)),
];

// OpenAI (provider "codex", the models of the Codex CLI).
// Source: https://developers.openai.com/api/docs/pricing (standard tier, input up to 272K
// tokens), read 2026-10-22.
// Cache write: free, so it equals the input price. A model without a listed cached-input price
// (the pro models) uses the input price for a cache read.
// Not on the page, so taken from the earlier public list: gpt-5.1-codex-mini, codex-mini-latest.
const CODEX: &[Row] = &[
    ("gpt-6-astra", Price::cached(10.0, 1.0, 50.0)),
    ("gpt-6.1-sol", Price::cached(2.0, 0.10, 10.0)),
    ("gpt-6-sol", Price::cached(2.0, 0.20, 10.0)),
    ("gpt-6-luna", Price::cached(0.10, 0.01, 0.50)),
    ("gpt-5.6-sol", Price::cached(4.0, 0.40, 20.0)),
    ("gpt-5.6-terra", Price::cached(2.0, 0.20, 12.0)),
    ("gpt-5.6-luna", Price::cached(0.20, 0.02, 1.20)),
    ("gpt-5.5-pro", Price::cached(30.0, 30.0, 180.0)),
    ("gpt-5.5", Price::cached(5.0, 0.50, 30.0)),
    ("gpt-5.4-pro", Price::cached(30.0, 30.0, 180.0)),
    ("gpt-5.4-mini", Price::cached(0.75, 0.075, 4.50)),
    ("gpt-5.4-nano", Price::cached(0.20, 0.02, 1.25)),
    ("gpt-5.4", Price::cached(2.50, 0.25, 15.0)),
    ("gpt-5.3-codex", Price::cached(1.75, 0.175, 14.0)),
    ("gpt-5.2-codex", Price::cached(1.75, 0.175, 14.0)),
    ("gpt-5.2-pro", Price::cached(21.0, 21.0, 168.0)),
    ("gpt-5.2", Price::cached(1.75, 0.175, 14.0)),
    ("gpt-5.1-codex-mini", Price::cached(0.25, 0.025, 2.0)),
    ("gpt-5.1-codex", Price::cached(1.25, 0.125, 10.0)),
    ("gpt-5.1", Price::cached(1.25, 0.125, 10.0)),
    ("gpt-5-codex", Price::cached(1.25, 0.125, 10.0)),
    ("gpt-5-pro", Price::cached(15.0, 15.0, 120.0)),
    ("gpt-5-mini", Price::cached(0.25, 0.025, 2.0)),
    ("gpt-5-nano", Price::cached(0.05, 0.005, 0.40)),
    ("gpt-5", Price::cached(1.25, 0.125, 10.0)),
    ("codex-mini-latest", Price::cached(1.50, 0.375, 6.0)),
    ("o4-mini", Price::cached(1.10, 0.275, 4.40)),
    ("o3-pro", Price::cached(20.0, 20.0, 80.0)),
    ("o3-mini", Price::cached(1.10, 0.55, 4.40)),
    ("o3", Price::cached(2.0, 0.50, 8.0)),
];

// Google (provider "gemini").
// Source: https://ai.google.dev/gemini-api/docs/pricing (paid tier, text input), read 2026-10-22.
// The tiers above 200K prompt tokens (higher prices) are not modelled: the prices below are the
// tier up to 200K. Cache write: no per-token fee (the storage fee is not modelled), so it equals
// the input price. The page announces higher prices for gemini-4-argon (2026-12-01) and the 3.6
// and 3.8 Flash models (2027-01-01); use an override file then.
const GEMINI: &[Row] = &[
    ("gemini-4-argon", Price::cached(2.0, 0.10, 10.0)),
    ("gemini-3.8-flash", Price::cached(0.75, 0.075, 3.75)),
    ("gemini-3.6-flash", Price::cached(0.75, 0.075, 3.75)),
    ("gemini-3.5-flash-lite", Price::cached(0.30, 0.03, 2.50)),
    ("gemini-3.1-pro", Price::cached(2.0, 0.20, 12.0)),
    ("gemini-3.1-flash-lite", Price::cached(0.25, 0.025, 1.50)),
    ("gemini-3-flash", Price::cached(0.50, 0.05, 3.0)),
    ("gemini-2.5-pro", Price::cached(1.25, 0.125, 10.0)),
    ("gemini-2.5-flash-lite", Price::cached(0.10, 0.01, 0.40)),
    ("gemini-2.5-flash", Price::cached(0.30, 0.03, 2.50)),
];

const BUILTIN: &[(&str, &[Row])] = &[("claude", CLAUDE), ("codex", CODEX), ("gemini", GEMINI)];

/// Why an override file fails to load.
#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    #[error("cannot read the price file {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("bad price file: {0}")]
    Parse(String),
}

#[derive(Debug, Clone)]
struct Entry {
    provider: String,
    prefix: String,
    price: Price,
}

/// The price table: the built-in prices and the overrides.
#[derive(Debug, Clone)]
pub struct PriceTable {
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct OverrideFile {
    #[serde(default)]
    price: Vec<OverrideRow>,
}

#[derive(Deserialize)]
struct OverrideRow {
    provider: String,
    model: String,
    input: f64,
    output: f64,
    cache_write: Option<f64>,
    cache_read: Option<f64>,
}

impl Default for PriceTable {
    fn default() -> Self {
        Self::builtin()
    }
}

impl PriceTable {
    /// The built-in prices only.
    pub fn builtin() -> Self {
        let entries = BUILTIN
            .iter()
            .flat_map(|(provider, rows)| {
                rows.iter().map(|(prefix, price)| Entry {
                    provider: (*provider).to_string(),
                    prefix: (*prefix).to_string(),
                    price: *price,
                })
            })
            .collect();
        Self { entries }
    }

    /// The built-in prices, then the overrides of the TOML file at `path`.
    pub fn with_file(path: &Path) -> Result<Self, PricingError> {
        let text = std::fs::read_to_string(path).map_err(|source| PricingError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::builtin().with_overrides(&text)
    }

    /// The table with the overrides of a TOML text, see the module docs. A row with the same
    /// provider and model prefix as a built-in row replaces it. Any other row adds a model.
    pub fn with_overrides(mut self, toml_text: &str) -> Result<Self, PricingError> {
        let file: OverrideFile =
            toml::from_str(toml_text).map_err(|e| PricingError::Parse(e.to_string()))?;
        for row in file.price {
            let valid = |v: f64| v.is_finite() && v >= 0.0;
            let write = row.cache_write.unwrap_or(row.input);
            let read = row.cache_read.unwrap_or(row.input);
            if !(valid(row.input) && valid(row.output) && valid(write) && valid(read)) {
                return Err(PricingError::Parse(format!(
                    "{} {}: a price is negative or not finite",
                    row.provider, row.model
                )));
            }
            let entry = Entry {
                provider: row.provider.to_ascii_lowercase(),
                prefix: row.model.to_ascii_lowercase(),
                price: Price::new(row.input, row.output, write, read),
            };
            match self
                .entries
                .iter_mut()
                .find(|e| e.provider == entry.provider && e.prefix == entry.prefix)
            {
                Some(old) => *old = entry,
                None => self.entries.push(entry),
            }
        }
        Ok(self)
    }

    /// The price of `model` at `provider`: the entry with the longest prefix of the model id.
    /// The match ignores case. An unknown model gives `None`.
    pub fn lookup(&self, provider: &str, model: &str) -> Option<Price> {
        let provider = provider.to_ascii_lowercase();
        let model = model.to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|e| e.provider == provider && model.starts_with(&e.prefix))
            .max_by_key(|e| e.prefix.len())
            .map(|e| e.price)
    }
}

/// The tokens of one model call, with the estimated cache split (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTokens<'a> {
    /// Start of the call.
    pub at: UnixMs,
    pub model: &'a str,
    /// Input that the cache did not serve: `plain_in + cache_write`.
    pub fresh_in: u64,
    /// Input that the cache served.
    pub cache_read: u64,
    /// Input that costs the plain input price. 0 for an estimated split.
    pub plain_in: u64,
    /// Input that costs the cache-write price.
    pub cache_write: u64,
    pub out: u64,
}

/// The `Llm` spans of a session as calls with an estimated cache split.
///
/// Calls come in the order of the spans. See the module docs for the split.
pub fn llm_calls(spans: &[Span]) -> Vec<CallTokens<'_>> {
    // Previous (tokens_in, model) per agent, in start order.
    let mut order: Vec<&Span> = spans.iter().filter(|s| s.kind == SpanKind::Llm).collect();
    order.sort_by(|a, b| (&a.agent, a.start).cmp(&(&b.agent, b.start)));
    let mut calls = Vec::with_capacity(order.len());
    let mut previous: Option<(&str, u64, &str)> = None;
    for span in order {
        let prev_in = match previous {
            Some((agent, tokens_in, model)) if agent == span.agent && model == span.model => {
                tokens_in
            }
            _ => 0,
        };
        previous = Some((&span.agent, span.tokens_in, &span.model));
        let (cache_read, plain_in, cache_write) =
            if span.tokens_cache_read.is_some() || span.tokens_cache_write.is_some() {
                let read = span.tokens_cache_read.unwrap_or(0).min(span.tokens_in);
                let write = span
                    .tokens_cache_write
                    .unwrap_or(0)
                    .min(span.tokens_in - read);
                (read, span.tokens_in - read - write, write)
            } else {
                let read = prev_in.min(span.tokens_in);
                (read, 0, span.tokens_in - read)
            };
        calls.push(CallTokens {
            at: span.start,
            model: &span.model,
            fresh_in: plain_in + cache_write,
            cache_read,
            plain_in,
            cache_write,
            out: span.tokens_out,
        });
    }
    calls.sort_by_key(|c| c.at);
    calls
}

/// The estimated cost of a session.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CostEstimate {
    /// USD, the sum over the priced calls.
    pub usd: f64,
    /// Calls with a known model price.
    pub priced_calls: usize,
    /// Calls of a model that the table does not know. They add nothing to `usd`.
    pub unpriced_calls: usize,
}

/// The cost of the `Llm` spans of one session of `provider` (`claude`, `codex`, `gemini`).
///
/// `spans` is `SessionTrace::spans` of `aivory_sessions::transcript::trace`. Cache reads cost the
/// cache-read price, see the module docs.
pub fn session_cost(table: &PriceTable, provider: &str, spans: &[Span]) -> CostEstimate {
    let mut cost = CostEstimate::default();
    for call in llm_calls(spans) {
        let Some(price) = table.lookup(provider, call.model) else {
            cost.unpriced_calls += 1;
            continue;
        };
        cost.priced_calls += 1;
        cost.usd += (call.plain_in as f64 * price.input
            + call.cache_write as f64 * price.cache_write
            + call.cache_read as f64 * price.cache_read
            + call.out as f64 * price.output)
            / 1_000_000.0;
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::SessionState;

    fn llm(agent: &str, model: &str, start: UnixMs, tokens_in: u64, tokens_out: u64) -> Span {
        Span {
            id: format!("{agent}-{start}"),
            parent: None,
            kind: SpanKind::Llm,
            name: "llm".into(),
            agent: agent.into(),
            model: model.into(),
            start,
            end: Some(start + 1),
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
    fn longest_prefix_wins() {
        let t = PriceTable::builtin();
        let p = |m: &str| t.lookup("claude", m).map(|p| p.input);
        assert_eq!(p("claude-opus-4-5-20251101"), Some(5.0));
        assert_eq!(p("claude-opus-4-20250514"), Some(15.0));
        assert_eq!(p("claude-opus-5-5"), Some(4.0));
        assert_eq!(p("claude-opus-5"), Some(5.0));
        assert_eq!(p("claude-sonnet-5-5"), Some(2.0));
        assert_eq!(p("claude-sonnet-4-20250514"), Some(3.0));
        assert_eq!(p("CLAUDE-HAIKU-5-5"), Some(0.10));
        let o = |m: &str| t.lookup("codex", m).map(|p| p.output);
        assert_eq!(o("gpt-5.3-codex"), Some(14.0));
        assert_eq!(o("gpt-5.1-codex-mini"), Some(2.0));
        assert_eq!(o("gpt-5.1-codex-max"), Some(10.0));
        assert_eq!(o("gpt-5.5-pro"), Some(180.0));
        let g = |m: &str| t.lookup("gemini", m).map(|p| p.output);
        assert_eq!(g("gemini-2.5-flash-lite"), Some(0.40));
        assert_eq!(g("gemini-2.5-flash"), Some(2.50));
        assert_eq!(g("gemini-3.1-pro-preview"), Some(12.0));
    }

    #[test]
    fn unknown_model_or_provider_gives_none() {
        let t = PriceTable::builtin();
        assert!(t.lookup("claude", "mystery-1").is_none());
        assert!(t.lookup("claude", "").is_none());
        assert!(t.lookup("goose", "claude-opus-5-5").is_none());
        assert!(t.lookup("codex", "claude-opus-5-5").is_none());
    }

    #[test]
    fn claude_cache_write_is_one_and_a_quarter_times_input() {
        let p = PriceTable::builtin()
            .lookup("claude", "claude-opus-5-5")
            .unwrap();
        assert_eq!(p.cache_write, 5.0);
        assert_eq!(p.cache_read, 0.20);
    }

    #[test]
    fn builtin_tables_have_no_duplicate_prefix() {
        for (provider, rows) in BUILTIN {
            for (i, (prefix, price)) in rows.iter().enumerate() {
                assert!(rows[i + 1..].iter().all(|(o, _)| o != prefix), "{prefix}");
                assert!(price.cache_read <= price.input, "{provider} {prefix}");
                assert!(price.output >= price.input, "{provider} {prefix}");
            }
        }
    }

    #[test]
    fn overrides_replace_and_extend() {
        let toml = r#"
            [[price]]
            provider = "claude"
            model = "claude-opus-5-5"
            input = 1.0
            output = 2.0
            cache_read = 0.5

            [[price]]
            provider = "Codex"
            model = "my-model"
            input = 3.0
            output = 6.0
        "#;
        let t = PriceTable::builtin().with_overrides(toml).unwrap();
        let p = t.lookup("claude", "claude-opus-5-5-x").unwrap();
        assert_eq!(
            (p.input, p.output, p.cache_write, p.cache_read),
            (1.0, 2.0, 1.0, 0.5)
        );
        let q = t.lookup("codex", "my-model-1").unwrap();
        assert_eq!((q.cache_write, q.cache_read), (3.0, 3.0));
        // The neighbour is untouched.
        assert_eq!(t.lookup("claude", "claude-opus-5").unwrap().input, 5.0);
    }

    #[test]
    fn bad_override_is_an_error() {
        let t = PriceTable::builtin();
        assert!(t.clone().with_overrides("[[price]]\nprovider = 1").is_err());
        let neg = "[[price]]\nprovider='claude'\nmodel='x'\ninput=-1.0\noutput=1.0";
        assert!(matches!(t.with_overrides(neg), Err(PricingError::Parse(_))));
    }

    #[test]
    fn override_file_loads_and_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prices.toml");
        std::fs::write(
            &path,
            "[[price]]\nprovider='gemini'\nmodel='gemini-x'\ninput=1.0\noutput=2.0\n",
        )
        .unwrap();
        let t = PriceTable::with_file(&path).unwrap();
        assert_eq!(t.lookup("gemini", "gemini-x-1").unwrap().output, 2.0);
        assert!(matches!(
            PriceTable::with_file(&dir.path().join("none.toml")),
            Err(PricingError::Read { .. })
        ));
    }

    #[test]
    fn cache_split_follows_the_previous_call_of_the_same_agent() {
        let spans = [
            llm("main", "claude-opus-5-5", 10, 1_000, 10),
            llm("main", "claude-opus-5-5", 20, 1_500, 10),
            llm("main", "claude-opus-5-5", 30, 1_200, 10), // context shrank
            llm("sub", "claude-opus-5-5", 25, 400, 10),    // own agent: no cache yet
            llm("main", "claude-haiku-5-5", 40, 900, 10),  // model change: no cache
        ];
        let calls = llm_calls(&spans);
        let split: Vec<(UnixMs, u64, u64)> = calls
            .iter()
            .map(|c| (c.at, c.fresh_in, c.cache_read))
            .collect();
        assert_eq!(
            split,
            [
                (10, 1_000, 0),
                (20, 500, 1_000),
                (25, 400, 0),
                (30, 0, 1_200),
                (40, 900, 0)
            ]
        );
    }

    #[test]
    fn session_cost_prices_cache_reads_at_the_cache_read_price() {
        // Opus 5.5: write 5.0, read 0.2, output 20.0 per 1M tokens.
        let spans = [
            llm("main", "claude-opus-5-5", 10, 1_000_000, 0),
            llm("main", "claude-opus-5-5", 20, 1_000_000, 100_000),
        ];
        let c = session_cost(&PriceTable::builtin(), "claude", &spans);
        // Call 1: 1M write = 5.0. Call 2: 1M read = 0.2, 100k out = 2.0.
        assert!((c.usd - 7.2).abs() < 1e-9, "{}", c.usd);
        assert_eq!((c.priced_calls, c.unpriced_calls), (2, 0));
    }

    #[test]
    fn exact_cache_split_beats_the_estimate() {
        // Opus 5.5: input 5.0, write 5.0 -> use a table where all three differ.
        let table = PriceTable::builtin()
            .with_overrides(
                "[[price]]
provider='claude'
model='x'
input=2.0
output=10.0
                 cache_write=3.0
cache_read=0.5
",
            )
            .unwrap();
        let mut a = llm("main", "x-1", 10, 3_000_000, 100_000);
        a.tokens_cache_read = Some(1_000_000);
        a.tokens_cache_write = Some(500_000);
        // 1.5M plain at 2.0 = 3.0; 0.5M write at 3.0 = 1.5; 1M read at 0.5 = 0.5; 0.1M out = 1.0.
        let c = session_cost(&table, "claude", &[a.clone()]);
        assert!((c.usd - 6.0).abs() < 1e-9, "{}", c.usd);
        // Only the read count is known: the write count is 0.
        a.tokens_cache_write = None;
        let calls = llm_calls(std::slice::from_ref(&a));
        assert_eq!(
            (calls[0].plain_in, calls[0].cache_write, calls[0].cache_read),
            (2_000_000, 0, 1_000_000)
        );
        // Counts above the input are capped.
        a.tokens_cache_read = Some(9_000_000);
        let calls = llm_calls(std::slice::from_ref(&a));
        assert_eq!((calls[0].plain_in, calls[0].cache_read), (0, 3_000_000));
        // A span without a split still follows the estimate.
        let spans = [
            llm("main", "x-1", 10, 1_000_000, 0),
            llm("main", "x-1", 20, 1_000_000, 0),
        ];
        let c = session_cost(&table, "claude", &spans);
        assert!((c.usd - 3.5).abs() < 1e-9, "{}", c.usd);
    }

    #[test]
    fn unknown_model_calls_are_counted_not_priced() {
        let spans = [
            llm("main", "claude-opus-5-5", 10, 1_000_000, 0),
            llm("main", "mystery", 20, 5_000_000, 0),
        ];
        let c = session_cost(&PriceTable::builtin(), "claude", &spans);
        assert!((c.usd - 5.0).abs() < 1e-9);
        assert_eq!((c.priced_calls, c.unpriced_calls), (1, 1));
    }

    #[test]
    fn only_llm_spans_count() {
        let mut tool = llm("main", "claude-opus-5-5", 10, 1_000_000, 1_000_000);
        tool.kind = SpanKind::Tool;
        let c = session_cost(&PriceTable::builtin(), "claude", &[tool]);
        assert_eq!(c, CostEstimate::default());
    }
}
