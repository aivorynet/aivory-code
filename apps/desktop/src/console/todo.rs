//! The todo list of a `TodoWrite` call, and the status line of a working pane.

use aivory_protocol::Line;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoState {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub text: String,
    pub state: TodoState,
}

/// The todos of a `TodoWrite` call. The call shows them as JSON in its head text, or as
/// checkbox rows (`[x] text`, `[ ] text`, `[~] text`) in its result rows. A call without
/// either has no list.
pub fn parse_todos(head_text: &str, results: &[Line]) -> Vec<Todo> {
    if let Some(list) = from_json(head_text) {
        return list;
    }
    let from_rows: Vec<Todo> = results.iter().filter_map(|l| row(&l.text)).collect();
    if !from_rows.is_empty() {
        return from_rows;
    }
    std::iter::once(head_text).filter_map(row).collect()
}

fn from_json(text: &str) -> Option<Vec<Todo>> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let items = match &value {
        Value::Array(a) => a,
        Value::Object(o) => o.get("todos")?.as_array()?,
        _ => return None,
    };
    let list: Vec<Todo> = items
        .iter()
        .filter_map(|item| {
            let text = ["content", "text", "title", "activeForm"]
                .iter()
                .find_map(|k| item.get(k)?.as_str())?;
            let state = match item.get("status").and_then(Value::as_str) {
                Some("completed" | "done") => TodoState::Done,
                Some("in_progress" | "active") => TodoState::Active,
                _ => TodoState::Pending,
            };
            Some(Todo {
                text: text.to_string(),
                state,
            })
        })
        .collect();
    (!list.is_empty()).then_some(list)
}

fn row(text: &str) -> Option<Todo> {
    let t = text.trim_start();
    let t = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .unwrap_or(t);
    let (state, rest) = if let Some(rest) = t.strip_prefix('☐') {
        (TodoState::Pending, rest)
    } else if let Some(rest) = t.strip_prefix('☒').or_else(|| t.strip_prefix('☑')) {
        (TodoState::Done, rest)
    } else {
        let inner = t.strip_prefix('[')?;
        let mark = inner.chars().next()?;
        let rest = inner[mark.len_utf8()..].strip_prefix(']')?;
        let state = match mark {
            'x' | 'X' | '✓' => TodoState::Done,
            '~' | '>' | '-' | '*' => TodoState::Active,
            ' ' => TodoState::Pending,
            _ => return None,
        };
        (state, rest)
    };
    let text = rest.trim();
    (!text.is_empty()).then(|| Todo {
        text: text.to_string(),
        state,
    })
}

/// A short count of the list: `2/5`.
pub fn progress(todos: &[Todo]) -> String {
    let done = todos.iter().filter(|t| t.state == TodoState::Done).count();
    format!("{done}/{}", todos.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todos_come_from_json() {
        let json = r#"{"todos":[{"content":"Read the code","status":"completed"},
            {"content":"Write the test","status":"in_progress"},{"content":"Ship","status":"pending"}]}"#;
        let todos = parse_todos(json, &[]);
        let states: Vec<_> = todos.iter().map(|t| t.state).collect();
        assert_eq!(
            states,
            [TodoState::Done, TodoState::Active, TodoState::Pending]
        );
        assert_eq!(todos[1].text, "Write the test");
        assert_eq!(progress(&todos), "1/3");
    }

    #[test]
    fn todos_come_from_checkbox_rows() {
        let rows = vec![
            Line::output("[x] Parse the file"),
            Line::output("- [~] Fold the rows"),
            Line::output("[ ] Draw the box"),
            Line::output("☐ Test it"),
            Line::output("not a todo"),
        ];
        let todos = parse_todos("", &rows);
        assert_eq!(todos.len(), 4);
        assert_eq!(todos[0].state, TodoState::Done);
        assert_eq!(todos[1].state, TodoState::Active);
        assert_eq!(todos[2].text, "Draw the box");
        assert_eq!(todos[3].state, TodoState::Pending);
    }

    #[test]
    fn a_call_without_a_list_has_no_todos() {
        assert!(parse_todos("", &[]).is_empty());
        assert!(parse_todos("[1,2]", &[]).is_empty());
        assert!(parse_todos("[a] b", &[]).is_empty());
    }
}
