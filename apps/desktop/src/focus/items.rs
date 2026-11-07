//! The transcript of the Focus chat as items. Pure logic: the lines of a session become user
//! bubbles, agent text, tool rows with a turn number, one summary row for a run of reads, and
//! the plan of the last todo list.

use aivory_protocol::{Line, LineKind};

use crate::console::blocks::{self, Block, BlockKind, Pending, ToolState};
use crate::console::todo::{self, Todo};

/// A tool row that edits a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRow {
    /// `Edited`, `Wrote`.
    pub verb: &'static str,
    pub path: String,
    /// The changed lines: `L44` or `+3 −1`.
    pub range: String,
    pub turn: u32,
    /// The agent writes this file now.
    pub live: bool,
}

impl EditRow {
    /// The file name of the path.
    pub fn name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

/// One element of the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    User(String),
    Agent(String),
    Edit(EditRow),
    /// A run of reads and searches in one row.
    Skim {
        reads: u32,
        searches: u32,
        turn: u32,
    },
    /// Any other tool call: a command, a fetch.
    Run {
        label: String,
        text: String,
        result: String,
        state: ToolState,
        turn: u32,
    },
    Sub {
        name: String,
        text: String,
    },
    Error(String),
    Meta(String),
}

/// The items of a transcript. `start` is the index of the first line in the full transcript.
pub fn items(lines: &[Line], start: u32, pending: Pending) -> Vec<Item> {
    let blocks = blocks::group(lines, start, pending, &Default::default());
    let last = blocks.len().saturating_sub(1);
    let mut out: Vec<Item> = Vec::new();
    let mut turn = 0u32;
    for (ix, b) in blocks.iter().enumerate() {
        match b.kind {
            BlockKind::User => {
                turn += 1;
                out.push(Item::User(join(b)));
            }
            BlockKind::Agent => out.push(Item::Agent(join(b))),
            BlockKind::Meta => out.push(Item::Meta(join(b))),
            BlockKind::Error => out.push(Item::Error(join(b))),
            BlockKind::Turn => {}
            BlockKind::Subagent => {
                let l = &b.lines[0];
                out.push(Item::Sub {
                    name: l.label.clone().unwrap_or_default(),
                    text: l.text.clone(),
                });
            }
            BlockKind::Call => {
                let t = turn.max(1);
                let label = b.head().and_then(|h| h.label.clone()).unwrap_or_default();
                let text = b.head().map(|h| h.text.clone()).unwrap_or_default();
                match label.as_str() {
                    "TodoWrite" => {}
                    "Read" | "Grep" | "Glob" | "LS" | "WebSearch" | "WebFetch" => {
                        let (r, s) = if label == "Read" { (1, 0) } else { (0, 1) };
                        if let Some(Item::Skim {
                            reads,
                            searches,
                            turn: prev,
                        }) = out.last_mut()
                            && *prev == t
                        {
                            *reads += r;
                            *searches += s;
                        } else {
                            out.push(Item::Skim {
                                reads: r,
                                searches: s,
                                turn: t,
                            });
                        }
                    }
                    "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Update" => {
                        out.push(Item::Edit(edit_row(b, &label, &text, t, ix == last)));
                    }
                    _ => out.push(Item::Run {
                        result: b
                            .results()
                            .iter()
                            .find(|l| l.kind != LineKind::Added && l.kind != LineKind::Removed)
                            .map(|l| l.text.clone())
                            .unwrap_or_default(),
                        state: b.tool_state(),
                        label: if label.is_empty() {
                            "Tool".into()
                        } else {
                            label
                        },
                        text,
                        turn: t,
                    }),
                }
            }
        }
    }
    out
}

fn join(b: &Block) -> String {
    b.lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The path in the head text of a call: its first word, without quotes.
pub fn path_of(text: &str) -> String {
    text.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['"', '\'', '`'])
        .to_string()
}

fn edit_row(b: &Block, label: &str, text: &str, turn: u32, last: bool) -> EditRow {
    let (mut add, mut del) = (0, 0);
    let mut first_new = None;
    for l in b.results() {
        match l.kind {
            LineKind::Added => add += 1,
            LineKind::Removed => del += 1,
            _ => {
                if first_new.is_none() {
                    first_new = blocks::hunk_start(&l.text).map(|(_, new)| new);
                }
            }
        }
    }
    let range = match first_new {
        Some(n) => format!("L{n} · +{add} −{del}"),
        None if add + del > 0 => format!("+{add} −{del}"),
        None => String::new(),
    };
    EditRow {
        verb: if label == "Write" { "Wrote" } else { "Edited" },
        path: path_of(text),
        range,
        turn,
        // The call of a working session that has no result row yet, or that ends the tail.
        live: last,
    }
}

/// The turns of the edit rows that touch `path`, in transcript order.
pub fn edit_turns(items: &[Item], path: &str) -> Vec<u32> {
    let want = path.replace('\\', "/");
    items
        .iter()
        .filter_map(|i| match i {
            Item::Edit(e) if same_file(&e.path, &want) => Some(e.turn),
            _ => None,
        })
        .collect()
}

/// Two paths name the same file when one ends with the other (a relative and an absolute one).
pub fn same_file(a: &str, b: &str) -> bool {
    let a = a.replace('\\', "/");
    let b = b.replace('\\', "/");
    let (a, b) = (a.trim_start_matches("./"), b.trim_start_matches("./"));
    !a.is_empty()
        && !b.is_empty()
        && (a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}")))
}

/// The plan: the todo list of the last `TodoWrite` call.
pub fn plan(lines: &[Line]) -> Vec<Todo> {
    let blocks = blocks::group(lines, 0, Pending::No, &Default::default());
    blocks
        .iter()
        .rev()
        .filter(|b| b.head().and_then(|h| h.label.as_deref()) == Some("TodoWrite"))
        .map(|b| {
            todo::parse_todos(
                &b.head().map(|h| h.text.clone()).unwrap_or_default(),
                b.results(),
            )
        })
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::todo::TodoState;

    fn lines() -> Vec<Line> {
        vec![
            Line::user("build it"),
            Line::agent("I read first."),
            Line::tool("Read", "a.rs"),
            Line::output("10 lines"),
            Line::tool("Grep", "\"x\" src"),
            Line::output("3 matches"),
            Line::tool("Edit", "crates/orch/src/budget.rs"),
            Line::output("@@ -40,2 +44,3 @@"),
            Line::removed("old"),
            Line::added("new"),
            Line::added("new 2"),
            Line::user("now the tests"),
            Line::tool("Bash", "cargo test"),
            Line::output("ok"),
            Line::tool("TodoWrite", ""),
            Line::output("[x] Read"),
            Line::output("[~] Edit"),
            Line::output("[ ] Test"),
            Line::tool("Edit", "crates/orch/src/budget.rs"),
        ]
    }

    #[test]
    fn reads_merge_and_edits_carry_the_turn() {
        let it = items(&lines(), 0, Pending::Running);
        assert!(matches!(it[0], Item::User(_)));
        assert_eq!(
            it[2],
            Item::Skim {
                reads: 1,
                searches: 1,
                turn: 1
            }
        );
        let Item::Edit(e) = &it[3] else {
            panic!("an edit row")
        };
        assert_eq!((e.turn, e.verb, e.name()), (1, "Edited", "budget.rs"));
        assert_eq!(e.range, "L44 · +2 −1");
        assert!(!e.live);
        let Some(Item::Edit(last)) = it.last() else {
            panic!("the last item is the edit")
        };
        assert_eq!(last.turn, 2);
        assert!(last.live);
        assert_eq!(edit_turns(&it, "crates/orch/src/budget.rs"), vec![1, 2]);
        assert_eq!(edit_turns(&it, "src/budget.rs"), vec![1, 2]);
        assert_eq!(edit_turns(&it, "src/other.rs"), Vec::<u32>::new());
    }

    #[test]
    fn a_command_row_keeps_its_first_result() {
        let it = items(&lines(), 0, Pending::No);
        assert!(it.iter().any(|i| matches!(
            i,
            Item::Run { label, result, turn: 2, .. } if label == "Bash" && result == "ok"
        )));
    }

    #[test]
    fn the_plan_is_the_last_todo_list() {
        let plan = plan(&lines());
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].state, TodoState::Done);
        assert_eq!(plan[1].state, TodoState::Active);
        assert!(super::plan(&[Line::user("x")]).is_empty());
    }

    #[test]
    fn paths_match_by_their_tail() {
        assert!(same_file("C:\\w\\src\\a.rs", "src/a.rs"));
        assert!(same_file("./src/a.rs", "src/a.rs"));
        assert!(!same_file("src/a.rs", "src/b.rs"));
        assert!(!same_file("", ""));
    }
}
