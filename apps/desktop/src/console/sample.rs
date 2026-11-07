//! Synthetic transcripts for `--demo-rich`: the demo data of the daemon has plain text only, so
//! the markdown, the diffs, the colors and the todo list have nothing to show in a visual
//! check. The text is made up; no real session is read.

use aivory_protocol::{Line, SessionState, WarRoom};

/// A transcript with agent text in markdown.
pub fn markdown_lines() -> Vec<Line> {
    let mut lines = vec![
        Line::meta("claude opus-5.5 · billing · session 7f3a"),
        Line::user("explain the retry design of the webhook handler"),
    ];
    for row in [
        "## Retry design",
        "The handler retries with **exponential backoff** and a *jitter* of 20%.",
        "- First attempt fails: wait `BASE_MS` (250 ms)",
        "- Next attempts double the delay, up to `30_000`",
        "  - Stop after 6 attempts",
        "1. Edit `src/payment/retry.rs`",
        "2. Run the tests with `cargo test -p payment`",
        "> The retry table needs a new column before staging.",
        "```rust",
        "fn delay(attempt: u32) -> u64 {",
        "    // capped backoff",
        "    (BASE_MS * 2u64.pow(attempt)).min(30_000)",
        "}",
        "```",
        "| Attempt | Delay |",
        "|---|---|",
        "| 1 | 250 ms |",
        "| 2 | 500 ms |",
        "See [the retry notes](https://example.com/retry) for the details.",
    ] {
        lines.push(Line::agent(row));
    }
    lines
}

/// A transcript with tool calls: folded results, a diff, ANSI colors, a todo list.
pub fn tool_lines() -> Vec<Line> {
    let mut lines = vec![
        Line::meta("claude sonnet-5.5 · billing · session 8d07"),
        Line::user("add the backoff to the retry module and run the tests"),
        Line::agent("I read the module first, then I change the delay."),
        Line::tool("Read", "src/payment/retry.rs"),
        Line::output("212 lines"),
        Line::tool("Grep", "\"retry\" src/payment"),
    ];
    for i in 0..9 {
        lines.push(Line::output(format!(
            "src/payment/webhook.rs:{}: retry(attempt, &policy)",
            20 + i * 7
        )));
    }
    lines.push(Line::tool("Edit", "src/payment/retry.rs"));
    for row in [
        Line::output("@@ -41,6 +41,8 @@ fn delay"),
        Line::output("    let base = BASE_MS;"),
        Line::removed("    thread::sleep(Duration::from_secs(1));"),
        Line::added("    let delay = BASE_MS * 2u64.pow(attempt);"),
        Line::added("    thread::sleep(Duration::from_millis(delay.min(30_000)));"),
        Line::output("    attempt += 1;"),
    ] {
        lines.push(row);
    }
    lines.push(Line::tool("Bash", "cargo test -p payment"));
    for row in [
        "running 22 tests",
        "test retry::backoff_doubles ... \u{1b}[32mok\u{1b}[0m",
        "test retry::backoff_caps ... \u{1b}[32mok\u{1b}[0m",
        "test webhook::replay ... \u{1b}[31mFAILED\u{1b}[0m",
        "\u{1b}[1;31merror\u{1b}[0m: 1 test failed",
    ] {
        lines.push(Line::output(row));
    }
    lines.push(Line::tool("TodoWrite", ""));
    for row in [
        "[x] Read the retry module",
        "[x] Add the backoff",
        "[~] Fix webhook::replay",
        "[ ] Update the changelog",
    ] {
        lines.push(Line::output(row));
    }
    lines.push(Line::subagent("test-writer", "2 cases for retry::backoff"));
    lines.push(Line::tool("Bash", "cargo test -p payment webhook::replay"));
    lines
}

/// The transcript of the working demo session `a01c` for the Focus view: three prompts (turns),
/// a plan, edits in the files of its demo diff, and an edit that is still running at the end.
pub fn focus_lines() -> Vec<Line> {
    let mut lines = vec![
        Line::meta("claude opus-5.5 · aivory-code · session a01c"),
        Line::user("read .aivory/map.toml and stay inside part \"orchestrator\""),
        Line::agent("Part \"orchestrator\" maps to `crates/orch/src/`. I keep my edits inside it."),
        Line::tool("Read", "README.md"),
        Line::output("66 lines"),
        Line::tool("Glob", "\"crates/orch/src/**\""),
        Line::output("34 files"),
        Line::user("cap subagent fan-out per session and per codebase"),
        Line::agent("I start with the budget in `crates/orch/src/budget.rs`, then the spawn path."),
        Line::tool("TodoWrite", ""),
    ];
    for row in [
        "[x] Map the spawn call sites",
        "[x] Add max_children to Budget",
        "[x] Add max_per_codebase to Budget",
        "[x] Check the budget in spawn",
        "[~] Write the budget tests",
        "[ ] Export the module",
        "[ ] Run the orch tests",
        "[ ] Update the changelog",
    ] {
        lines.push(Line::output(row));
    }
    lines.extend([
        Line::tool("Read", "crates/orch/src/spawn.rs"),
        Line::output("222 lines"),
        Line::tool("Grep", "\"spawn_child\" crates/orch/src"),
        Line::output("14 matches in 6 files"),
        Line::tool("Read", "crates/orch/src/budget.rs"),
        Line::output("178 lines"),
        Line::tool("Edit", "crates/orch/src/budget.rs"),
        Line::output("@@ -1,12 +1,46 @@"),
        Line::added("pub struct Budget {"),
        Line::added("    pub max_children: u8,      // default 4"),
        Line::added("    pub max_per_codebase: u8, // default 12"),
        Line::subagent("explorer", "mapped 14 spawn call sites"),
        Line::tool("Edit", "crates/orch/src/spawn.rs"),
        Line::output("@@ -40,6 +40,21 @@"),
        Line::removed("    spawn_child(parent, spec)"),
        Line::added("    budget.check(parent)?;"),
        Line::added("    spawn_child(parent, spec)"),
        Line::agent("The budget is in the spawn path. Now the tests for `Budget::exceeds`."),
        Line::tool("Write", "tests/budget_exceeds.rs"),
        Line::added("use orch::budget::Budget;"),
        Line::subagent("test-writer", "writing budget::exceeds"),
        Line::tool("Bash", "cargo test -p orch budget"),
        Line::output("test result: ok. 22 passed; 0 failed"),
        Line::user("export the module too, and keep the default at 4"),
        Line::agent("I export `budget` from `lib.rs`. The default stays 4."),
        Line::tool("Edit", "crates/orch/src/lib.rs"),
        Line::output("@@ -1,3 +1,4 @@"),
        Line::added("pub mod budget;"),
        Line::agent("Now I wire the limit into the spawn path."),
        Line::tool("Edit", "crates/orch/src/spawn.rs"),
    ]);
    lines
}

/// Replace the tails of the first sessions with the sample transcripts: the first two that
/// wait for an answer, then the first working one.
pub fn enrich(war: &mut WarRoom) {
    let mut samples = [
        (SessionState::NeedsYou, markdown_lines()),
        (SessionState::NeedsYou, tool_lines()),
        (SessionState::Working, focus_lines()),
    ]
    .into_iter();
    let mut next = samples.next();
    for pane in &mut war.sessions {
        let Some((state, lines)) = &next else {
            break;
        };
        if pane.state != *state {
            continue;
        }
        pane.lines_total = lines.len() as u32;
        pane.tail = lines.clone();
        next = samples.next();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::blocks::{BlockKind, Pending, group};
    use aivory_protocol::demo::Demo;

    #[test]
    fn the_samples_group_into_blocks() {
        let blocks = group(&tool_lines(), 0, Pending::Running, &Default::default());
        assert!(blocks.iter().any(|b| b.kind == BlockKind::Call));
        assert_eq!(
            blocks.last().map(|b| b.pending),
            Some(Pending::Running),
            "the last call has no result: it runs"
        );
    }

    #[test]
    fn the_enrich_replaces_the_tails_of_three_panes() {
        let mut war = Demo::new().war_room;
        enrich(&mut war);
        let rich = war
            .sessions
            .iter()
            .filter(|s| s.tail.len() > 15)
            .all(|s| s.lines_total as usize == s.tail.len());
        assert!(war.sessions.iter().filter(|s| s.tail.len() > 15).count() >= 3);
        assert!(rich);
    }
}
