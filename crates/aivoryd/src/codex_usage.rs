//! The real Codex plan usage: Codex writes the limits of the account into its session files
//! (`token_count` events with `rate_limits`: `used_percent`, `window_minutes`, `resets_at`).
//! aivoryd reads the newest record. The files are read-only; nothing calls the CLI.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use aivory_protocol::{UnixMs, UsageWindow};
use serde_json::Value;

/// The newest session files that the reader looks at.
const FILES: usize = 12;

/// The windows of the newest `rate_limits` record under `<codex_home>/sessions`.
pub fn latest(codex_home: &Path, now: UnixMs) -> Vec<UsageWindow> {
    for file in newest_files(&codex_home.join("sessions")) {
        if let Some(limits) = last_record(&file) {
            let windows = windows(&limits, now);
            if !windows.is_empty() {
                return windows;
            }
        }
    }
    Vec::new()
}

/// The windows of one `rate_limits` object.
pub fn windows(limits: &Value, now: UnixMs) -> Vec<UsageWindow> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|k| limits.get(*k).filter(|v| v.is_object()))
        .filter_map(|w| {
            let pct = w.get("used_percent")?.as_f64()?;
            let minutes = w.get("window_minutes")?.as_i64()?;
            let resets_at = w.get("resets_at").and_then(Value::as_i64).map(|s| s * 1000);
            // The window reset since the record: nothing is used in the new one yet.
            let over = resets_at.is_some_and(|r| r <= now);
            Some(UsageWindow {
                provider: "codex".into(),
                label: label(minutes),
                pct: if over {
                    0
                } else {
                    pct.round().clamp(0., 100.) as u8
                },
                used: 0,
                resets_at: if over { None } else { resets_at },
                estimate: false,
            })
        })
        .collect()
}

/// `5 h window`, `Week`, `Month`, or `N h window`.
fn label(minutes: i64) -> String {
    match minutes {
        300 => "5 h window".into(),
        10_080 => "Week".into(),
        40_320..=44_640 => "Month".into(),
        m if m % 1440 == 0 => format!("{} d window", m / 1440),
        m => format!("{} h window", (m as f64 / 60.).round()),
    }
}

/// The `rate_limits` object of the last record in a session file that has one.
fn last_record(file: &Path) -> Option<Value> {
    let reader = BufReader::new(File::open(file).ok()?);
    let mut last = None;
    for line in reader.lines().map_while(Result::ok) {
        if line.contains("\"rate_limits\":{") {
            last = Some(line);
        }
    }
    let v: Value = serde_json::from_str(&last?).ok()?;
    find(&v, "rate_limits").cloned()
}

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map
            .get(key)
            .filter(|x| x.is_object())
            .or_else(|| map.values().find_map(|x| find(x, key))),
        Value::Array(items) => items.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

/// The newest `*.jsonl` files below `sessions/<year>/<month>/<day>`, newest first.
fn newest_files(sessions: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let dirs = |p: &Path| -> Vec<PathBuf> {
        let mut d: Vec<PathBuf> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        d.sort();
        d.reverse();
        d
    };
    'walk: for year in dirs(sessions) {
        for month in dirs(&year) {
            for day in dirs(&month) {
                for e in std::fs::read_dir(&day).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "jsonl")
                        && let Ok(t) = e.metadata().and_then(|m| m.modified())
                    {
                        files.push((t, p));
                    }
                }
                if files.len() >= FILES {
                    break 'walk;
                }
            }
        }
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().take(FILES).map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: UnixMs = 1_793_000_000_000;

    #[test]
    fn a_rate_limit_record_gives_its_windows() {
        let limits = json!({
            "limit_id": "codex",
            "primary": {"used_percent": 5.0, "window_minutes": 43200, "resets_at": 1_793_869_228},
            "secondary": {"used_percent": 61.4, "window_minutes": 300, "resets_at": 1_793_001_000},
            "plan_type": "free"
        });
        let w = windows(&limits, NOW);
        let rows: Vec<_> = w.iter().map(|w| (w.label.as_str(), w.pct)).collect();
        assert_eq!(rows, [("Month", 5), ("5 h window", 61)]);
        assert_eq!(w[0].resets_at, Some(1_793_869_228_000));
        assert!(w.iter().all(|w| !w.estimate && w.provider == "codex"));
    }

    #[test]
    fn a_window_that_reset_since_the_record_reads_zero() {
        let limits = json!({"primary": {"used_percent": 88.0, "window_minutes": 300,
            "resets_at": (NOW / 1000) - 60}, "secondary": null});
        let w = windows(&limits, NOW);
        assert_eq!((w[0].pct, w[0].resets_at), (0, None));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn the_newest_record_of_a_session_file_wins() {
        let dir = std::env::temp_dir().join(format!("aivory-codex-usage-{}", std::process::id()));
        let day = dir.join("sessions/2026/10/27");
        std::fs::create_dir_all(&day).unwrap();
        let rec = |pct: f64| {
            json!({"type": "event_msg", "payload": {"type": "token_count",
                "rate_limits": {"primary": {"used_percent": pct, "window_minutes": 10080,
                    "resets_at": NOW / 1000 + 3600}}}})
            .to_string()
        };
        std::fs::write(
            day.join("rollout-a.jsonl"),
            format!("{}\n{}\n", rec(10.), rec(42.)),
        )
        .unwrap();
        let w = latest(&dir, NOW);
        assert_eq!((w[0].label.as_str(), w[0].pct), ("Week", 42));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
