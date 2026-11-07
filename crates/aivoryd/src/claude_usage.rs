//! The real Claude plan usage: `claude -p /usage` prints the limits of the logged-in account
//! (session window, week, week per model) with their reset times. The command runs no model
//! call, and `--no-session-persistence` keeps it out of `~/.claude/projects`. aivoryd never
//! reads the login: the installed CLI does the request.
//!
//! The probe runs in its own thread, at start and then every [`PROBE_EVERY`]. Its windows
//! replace the estimate of `aivory_engine::usage` for Claude while they are fresh.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use aivory_protocol::{UnixMs, UsageWindow};
use chrono::{Datelike, Local, NaiveDate, TimeZone};

/// How often the probe asks the CLI.
pub const PROBE_EVERY: Duration = Duration::from_secs(5 * 60);
/// A probe result older than this does not count any more (the estimate shows).
pub const FRESH_MS: i64 = 15 * 60 * 1000;
/// A CLI that does not answer in this time is stopped.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The last probe result and when it came.
pub type Shared = Arc<Mutex<Option<(Vec<UsageWindow>, UnixMs)>>>;

/// Start the probe thread. `program` is the `claude` binary, `cwd` a scratch folder.
pub fn spawn(program: PathBuf, cwd: PathBuf) -> Shared {
    let shared: Shared = Arc::default();
    let out = Arc::clone(&shared);
    let _ = thread::Builder::new()
        .name("claude-usage".into())
        .spawn(move || {
            loop {
                if let Some(text) = run(&program, &cwd) {
                    let now = Local::now().timestamp_millis();
                    let windows = parse(&text, now);
                    if !windows.is_empty()
                        && let Ok(mut slot) = out.lock()
                    {
                        *slot = Some((windows, now));
                    }
                }
                thread::sleep(PROBE_EVERY);
            }
        });
    shared
}

/// The windows of a fresh probe, else `None`.
pub fn fresh(shared: &Shared, now: UnixMs) -> Option<Vec<UsageWindow>> {
    let slot = shared.lock().ok()?;
    let (windows, at) = slot.as_ref()?;
    (now - at < FRESH_MS).then(|| windows.clone())
}

/// Put the probe windows in place of the estimated Claude windows.
pub fn merge(estimate: Vec<UsageWindow>, real: Vec<UsageWindow>) -> Vec<UsageWindow> {
    merge_for("claude", estimate, real)
}

/// Put the real windows of `provider` in place of its estimated ones. The token counts of the
/// estimate stay on the window with the same label.
pub fn merge_for(
    provider: &str,
    estimate: Vec<UsageWindow>,
    real: Vec<UsageWindow>,
) -> Vec<UsageWindow> {
    if real.is_empty() {
        return estimate;
    }
    let mut out: Vec<UsageWindow> = Vec::new();
    let mut real = Some(real);
    for w in &estimate {
        if w.provider != provider {
            out.push(w.clone());
        } else if let Some(mut r) = real.take() {
            for x in &mut r {
                if let Some(e) = estimate
                    .iter()
                    .find(|e| e.provider == provider && e.label == x.label)
                {
                    x.used = e.used;
                }
            }
            out.extend(r);
        }
    }
    // No estimate for the provider yet: its real windows go where its table order puts them.
    if let Some(r) = real {
        out.extend(r);
    }
    out
}

fn run(program: &Path, cwd: &Path) -> Option<String> {
    std::fs::create_dir_all(cwd).ok()?;
    let mut child = Command::new(program)
        .args(["-p", "/usage", "--no-session-persistence"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// The windows of a `/usage` text. Lines like
/// `Current session: 18% used · resets Oct 27, 5:39pm (Europe/Berlin)` and
/// `Current week (all models): 80% used · resets Oct 29, 1:59pm (Europe/Berlin)`.
/// The reset time is read in the local time zone (the CLI prints the local zone).
pub fn parse(text: &str, now: UnixMs) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("Current ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let label = match name.trim() {
            "session" => "5 h window".to_string(),
            "week (all models)" => "Week".to_string(),
            other => match other
                .strip_prefix("week (")
                .and_then(|m| m.strip_suffix(')'))
            {
                Some(model) => format!("Week ({model})"),
                None => continue,
            },
        };
        let Some((pct, after)) = value.trim().split_once('%') else {
            continue;
        };
        let Ok(pct) = pct.trim().parse::<u8>() else {
            continue;
        };
        let resets_at = after
            .split_once("resets ")
            .and_then(|(_, when)| reset_time(when, now));
        out.push(UsageWindow {
            provider: "claude".into(),
            label,
            pct: pct.min(100),
            used: 0,
            resets_at,
            estimate: false,
        });
    }
    out
}

/// `Oct 29, 1:59pm (Europe/Berlin)` or `Oct 29, 2pm` as local time. The year is the one that
/// puts the time closest after `now`.
fn reset_time(when: &str, now: UnixMs) -> Option<UnixMs> {
    let when = when.split('(').next()?.trim();
    let (date, time) = when.split_once(',')?;
    let mut date = date.split_whitespace();
    let month = match date.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day: u32 = date.next()?.parse().ok()?;
    let time = time.trim().to_ascii_lowercase();
    let (clock, pm) = match (time.strip_suffix("pm"), time.strip_suffix("am")) {
        (Some(c), _) => (c, true),
        (_, Some(c)) => (c, false),
        _ => return None,
    };
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    let hour = match (h % 12, pm) {
        (h, true) => h + 12,
        (h, false) => h,
    };
    let year = Local.timestamp_millis_opt(now).single()?.year();
    // A reset is never far in the past: a date before now by more than a day is next year.
    [year, year + 1]
        .into_iter()
        .filter_map(|y| {
            let naive = NaiveDate::from_ymd_opt(y, month, day)?.and_hms_opt(hour, m, 0)?;
            Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.timestamp_millis())
        })
        .find(|&t| t > now - 24 * 3_600_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "You are currently using your subscription to power your Claude Code usage

Current session: 18% used · resets Oct 27, 5:39pm (Europe/Berlin)
Current week (all models): 80% used · resets Oct 29, 1:59pm (Europe/Berlin)
Current week (Fable): 0% used · resets Oct 29, 2pm (Europe/Berlin)

What's contributing to your limits usage?
Last 24h · 5743 requests · 32 sessions
  90%  Give \"general-purpose\" subagents a cheaper model
";

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> UnixMs {
        let naive = NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap();
        Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn the_usage_text_gives_three_real_windows() {
        let now = local(2026, 10, 27, 15, 52);
        let w = parse(SAMPLE, now);
        let labels: Vec<_> = w.iter().map(|w| (w.label.as_str(), w.pct)).collect();
        assert_eq!(
            labels,
            [("5 h window", 18), ("Week", 80), ("Week (Fable)", 0)]
        );
        assert_eq!(w[0].resets_at, Some(local(2026, 10, 27, 17, 39)));
        assert_eq!(w[1].resets_at, Some(local(2026, 10, 29, 13, 59)));
        assert_eq!(w[2].resets_at, Some(local(2026, 10, 29, 14, 0)));
        assert!(w.iter().all(|w| !w.estimate && w.provider == "claude"));
        // The tips under the limits are no windows.
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn a_reset_after_new_year_takes_the_next_year() {
        let now = local(2026, 12, 31, 20, 0);
        assert_eq!(
            reset_time("Jan 2, 9am (Europe/Berlin)", now),
            Some(local(2027, 1, 2, 9, 0))
        );
        assert_eq!(
            reset_time("Dec 31, 12am", now),
            Some(local(2026, 12, 31, 0, 0))
        );
    }

    #[test]
    fn the_real_windows_replace_the_claude_estimate() {
        let est = |p: &str, l: &str, used| UsageWindow {
            provider: p.into(),
            label: l.into(),
            pct: 100,
            used,
            resets_at: None,
            estimate: true,
        };
        let estimate = vec![
            est("claude", "5 h window", 6_000_000),
            est("claude", "Week", 48_000_000),
            est("codex", "5 h window", 10),
        ];
        let real = parse(SAMPLE, local(2026, 10, 27, 15, 52));
        let out = merge(estimate, real);
        let rows: Vec<_> = out
            .iter()
            .map(|w| (w.provider.as_str(), w.label.as_str(), w.pct, w.used))
            .collect();
        assert_eq!(
            rows,
            [
                ("claude", "5 h window", 18, 6_000_000),
                ("claude", "Week", 80, 48_000_000),
                ("claude", "Week (Fable)", 0, 0),
                ("codex", "5 h window", 100, 10),
            ]
        );
    }
}
