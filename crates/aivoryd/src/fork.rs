//! Fork at a turn: the worktree at the snapshot of the turn, and the handoff packet.
//!
//! `session/fork` with `at` = n starts a new session in a worktree whose files equal the
//! repository at the end of turn n. The new agent does not know the old conversation, so the
//! first prompt is a packet: the prompts of the turns 1 to n and the last agent text of each
//! turn. The packet has at most [`PACKET_CHARS`] characters. A cut keeps the newest turns.

use std::path::{Path, PathBuf};

use aivory_vcs::{Vcs, Workspace, WorkspaceRequest};

use crate::ledger::TurnText;

/// The size limit of a handoff packet, in characters.
pub const PACKET_CHARS: usize = 8_000;

/// The packet for the turns in `turns` (oldest first). `from` names the provider of the old
/// session, `upto` the turn where the fork starts.
pub fn handoff_packet(turns: &[TurnText], from: &str, upto: u32) -> String {
    handoff_packet_within(turns, from, upto, PACKET_CHARS)
}

fn handoff_packet_within(turns: &[TurnText], from: &str, upto: u32, limit: usize) -> String {
    let head = format!(
        "You continue the work of an earlier {from} session. The files in this folder are the \
         files as they were at the end of turn {upto}. Below are the prompts of the earlier \
         turns and the last answer of the agent in each turn.\n"
    );
    // The room for the note about the turns that the cut leaves out.
    let note_room = 60;
    let mut room = limit.saturating_sub(count(&head) + note_room);
    let mut kept: Vec<String> = Vec::new();
    for turn in turns.iter().rev() {
        let block = turn_block(turn, usize::MAX);
        let size = count(&block);
        if size <= room {
            room -= size;
            kept.push(block);
        } else {
            if kept.is_empty() && room > 0 {
                // The newest turn alone is too big: keep its start.
                kept.push(turn_block(turn, room));
            }
            break;
        }
    }
    let left_out = turns.len() - kept.len();
    kept.reverse();
    let mut out = head;
    if left_out > 0 {
        out.push_str(&format!("\n({left_out} earlier turns are left out.)\n"));
    }
    for block in kept {
        out.push_str(&block);
    }
    crate::ledger::cut(&out, limit)
}

/// One turn as text. `max` limits the size in characters (the end is cut).
fn turn_block(turn: &TurnText, max: usize) -> String {
    let mut block = format!("\nTurn {}\nPrompt: {}\n", turn.n, turn.prompt.trim());
    if !turn.agent.trim().is_empty() {
        block.push_str(&format!("Agent: {}\n", turn.agent.trim()));
    }
    if count(&block) > max {
        let mut cut = crate::ledger::cut(&block, max.saturating_sub(2));
        cut.push_str("…\n");
        return crate::ledger::cut(&cut, max);
    }
    block
}

fn count(text: &str) -> usize {
    text.chars().count()
}

/// A folder name from a codebase name.
fn folder_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_matches('.').to_string();
    if clean.is_empty() {
        "repo".into()
    } else {
        clean
    }
}

/// Create the worktree `<data>/workspaces/<repo>/<session>` at the commit `rev`.
pub fn create_worktree(
    vcs: &dyn Vcs,
    repo: &Path,
    data_dir: &Path,
    repo_name: &str,
    session: &str,
    rev: &str,
) -> Result<Workspace, String> {
    let root: PathBuf = data_dir.join("workspaces").join(folder_name(repo_name));
    vcs.create_workspace(
        repo,
        &WorkspaceRequest {
            root,
            name: session.to_string(),
            branch: None,
            create_branch: false,
            start_point: Some(rev.to_string()),
            sparse: Vec::new(),
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: u32, prompt: &str, agent: &str) -> TurnText {
        TurnText {
            n,
            prompt: prompt.into(),
            agent: agent.into(),
        }
    }

    #[test]
    fn the_packet_lists_every_turn_in_order() {
        let turns = [turn(1, "Add a parser", "Done."), turn(2, "Test it", "")];
        let p = handoff_packet(&turns, "claude", 2);
        let one = p.find("Turn 1").unwrap();
        let two = p.find("Turn 2").unwrap();
        assert!(one < two);
        assert!(p.contains("Prompt: Add a parser\nAgent: Done."));
        assert!(p.contains("Prompt: Test it\n"));
        assert!(!p.contains("left out"));
        assert!(p.contains("end of turn 2"));
    }

    #[test]
    fn a_cut_keeps_the_newest_turns() {
        let turns: Vec<TurnText> = (1..=30)
            .map(|n| {
                turn(
                    n,
                    &format!("prompt {n} {}", "x".repeat(400)),
                    &"y".repeat(100),
                )
            })
            .collect();
        let p = handoff_packet(&turns, "codex", 30);
        assert!(p.chars().count() <= PACKET_CHARS);
        assert!(p.contains("Turn 30\n"));
        assert!(p.contains("earlier turns are left out"));
        assert!(!p.contains("Turn 1\n"));
        // The kept turns are the newest ones, without a gap.
        let first_kept = (1..=30)
            .find(|n| p.contains(&format!("Turn {n}\n")))
            .unwrap();
        assert!((first_kept..=30).all(|n| p.contains(&format!("Turn {n}\n"))));
    }

    #[test]
    fn one_huge_turn_is_cut_to_the_limit() {
        let turns = [turn(1, &"p".repeat(20_000), "a")];
        let p = handoff_packet(&turns, "claude", 1);
        assert!(p.chars().count() <= PACKET_CHARS);
        assert!(p.contains("Turn 1\nPrompt: ppp"));
    }

    #[test]
    fn small_limits_hold() {
        let turns = [turn(1, "hello", "world")];
        let p = handoff_packet_within(&turns, "claude", 1, 50);
        assert!(p.chars().count() <= 50);
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("my repo/x"), "my_repo_x");
        assert_eq!(folder_name(".."), "repo");
    }
}
