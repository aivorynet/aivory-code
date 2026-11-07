//! Map a permission request of Claude Code to asks, and an answer to the result that Claude expects.
//!
//! Claude calls the permission tool with `{ tool_name, input, tool_use_id }`. The tool result is
//! one text block that holds JSON: `{"behavior":"allow","updatedInput":{..}}` or
//! `{"behavior":"deny","message":".."}`.

use aivory_protocol::{Ask, AskKind};
use serde_json::{Value, json};

const MAX_TEXT_CHARS: usize = 400;
const MAX_STEPS: usize = 40;

/// The user answer to one ask.
///
/// For a question, `message` holds the chosen option (or free text). For a refusal, `message`
/// tells the agent why. `always` is only for permissions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub allow: bool,
    pub always: bool,
    pub message: Option<String>,
}

impl Answer {
    pub fn allow() -> Self {
        Self {
            allow: true,
            ..Self::default()
        }
    }

    pub fn allow_always() -> Self {
        Self {
            allow: true,
            always: true,
            message: None,
        }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self {
            allow: false,
            always: false,
            message: Some(message.into()),
        }
    }

    /// An allow with a chosen option or free text, for a question.
    pub fn choice(text: impl Into<String>) -> Self {
        Self {
            allow: true,
            always: false,
            message: Some(text.into()),
        }
    }
}

/// One question of `AskUserQuestion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Question {
    pub text: String,
    pub options: Vec<String>,
}

/// What Claude wants to do, sorted by the kind of ask it needs.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Request {
    Plan { title: String, steps: Vec<String> },
    Questions(Vec<Question>),
    Tool { text: String, risky: bool },
}

pub(super) fn classify(tool: &str, input: &Value) -> Request {
    match tool {
        "ExitPlanMode" => {
            let plan = input.get("plan").and_then(Value::as_str).unwrap_or("");
            let (title, steps) = plan_parts(plan);
            Request::Plan { title, steps }
        }
        "AskUserQuestion" => {
            let questions = questions(input);
            if questions.is_empty() {
                Request::Tool {
                    text: describe(tool, input),
                    risky: false,
                }
            } else {
                Request::Questions(questions)
            }
        }
        _ => Request::Tool {
            text: describe(tool, input),
            risky: matches!(tool, "Bash" | "PowerShell"),
        },
    }
}

/// The text of a permission ask: the command, the path, or a short form of the input.
pub(super) fn describe(tool: &str, input: &Value) -> String {
    let field = |name: &str| input.get(name).and_then(Value::as_str).map(str::to_string);
    let text = match tool {
        "Bash" | "PowerShell" => field("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            field("file_path").or_else(|| field("notebook_path"))
        }
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Glob" | "Grep" => field("pattern"),
        _ => None,
    };
    let text = text.unwrap_or_else(|| format!("{tool} {input}"));
    truncate(&text, MAX_TEXT_CHARS)
}

/// The title and the steps of a plan in Markdown. A step is a list item.
pub(super) fn plan_parts(plan: &str) -> (String, Vec<String>) {
    let mut title = None;
    let mut steps = Vec::new();
    let mut loose = Vec::new();
    for raw in plan.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(item) = list_item(line) {
            steps.push(clean(item));
        } else if let Some(head) = line.strip_prefix('#') {
            if title.is_none() {
                title = Some(clean(head.trim_start_matches('#')));
            }
        } else {
            if title.is_none() {
                title = Some(clean(line));
            }
            loose.push(clean(line));
        }
    }
    if steps.is_empty() {
        // A plan without a list: each text line is a step, but the title line is not one.
        steps = loose.into_iter().skip(1).collect();
    }
    steps.truncate(MAX_STEPS);
    let steps = steps.into_iter().map(|s| truncate(&s, 300)).collect();
    let title = title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Approve the plan?".to_string());
    (truncate(&title, 200), steps)
}

fn list_item(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest);
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        for marker in [". ", ") "] {
            if let Some(rest) = rest.strip_prefix(marker) {
                return Some(rest);
            }
        }
    }
    None
}

fn clean(text: &str) -> String {
    text.trim().replace("**", "")
}

pub(super) fn questions(input: &Value) -> Vec<Question> {
    let Some(list) = input.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|q| {
            let text = q.get("question").and_then(Value::as_str)?.to_string();
            let options = q
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .filter_map(|o| match o {
                            Value::String(s) => Some(s.clone()),
                            _ => o.get("label").and_then(Value::as_str).map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Question { text, options })
        })
        .collect()
}

pub(super) fn permission_ask(id: String, tool: &str, text: String, risky: bool) -> Ask {
    Ask {
        id,
        kind: AskKind::Permission,
        text,
        options: vec!["Allow".into(), "Always allow".into(), "Deny".into()],
        steps: Vec::new(),
        reason: Some(format!("Claude asks to use {tool}")),
        risky,
    }
}

pub(super) fn plan_ask(id: String, title: String, steps: Vec<String>) -> Ask {
    Ask {
        id,
        kind: AskKind::Plan,
        text: title,
        options: vec!["Approve".into(), "Edit plan".into(), "Reject".into()],
        steps,
        reason: Some("Claude asks to leave plan mode".into()),
        risky: false,
    }
}

pub(super) fn question_ask(id: String, q: &Question) -> Ask {
    Ask {
        id,
        kind: AskKind::Question,
        text: q.text.clone(),
        options: q.options.clone(),
        steps: Vec::new(),
        reason: None,
        risky: false,
    }
}

/// The JSON that the permission tool returns when the user allows the call.
///
/// `input` is the original tool input. For a question, `answers` maps each question text to
/// the chosen option.
///
/// Claude 2.1.306 ignores `updatedPermissions` in this result, so "always" is not sent to
/// Claude. The server remembers it (see [`rule_key`]).
pub(super) fn allow_payload(input: &Value, answers: Option<&[(String, String)]>) -> Value {
    let mut updated = input.clone();
    if let (Some(answers), Some(obj)) = (answers, updated.as_object_mut()) {
        let map: serde_json::Map<String, Value> = answers
            .iter()
            .map(|(q, a)| (q.clone(), Value::String(a.clone())))
            .collect();
        obj.insert("answers".into(), Value::Object(map));
    }
    json!({ "behavior": "allow", "updatedInput": updated })
}

/// What "always allow" remembers. A shell command: that exact command. Any other tool: the
/// whole tool, for the rest of the run.
pub(super) fn rule_key(tool: &str, input: &Value) -> String {
    match (tool, input.get("command").and_then(Value::as_str)) {
        ("Bash" | "PowerShell", Some(cmd)) => format!("{tool}\0{cmd}"),
        _ => tool.to_string(),
    }
}

pub(super) fn deny_payload(message: &str) -> Value {
    json!({ "behavior": "deny", "message": message })
}

/// Cut a text to `max` characters. A cut text ends with an ellipsis.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_text_is_the_command() {
        let r = classify("Bash", &json!({"command": "echo hi", "description": "x"}));
        assert_eq!(
            r,
            Request::Tool {
                text: "echo hi".into(),
                risky: true
            }
        );
    }

    #[test]
    fn edit_text_is_the_path() {
        let r = classify("Edit", &json!({"file_path": "src/a.rs", "old_string": "x"}));
        assert_eq!(
            r,
            Request::Tool {
                text: "src/a.rs".into(),
                risky: false
            }
        );
    }

    #[test]
    fn plan_steps_come_from_list_items() {
        let plan = "# Add retry\n\nWhy: flaky.\n\n1. Add the table\n2. **Wire** it\n- Test it\n";
        let (title, steps) = plan_parts(plan);
        assert_eq!(title, "Add retry");
        assert_eq!(steps, ["Add the table", "Wire it", "Test it"]);
    }

    #[test]
    fn plan_without_list_uses_text_lines() {
        let (title, steps) = plan_parts("Do the thing\nFirst part\nSecond part");
        assert_eq!(title, "Do the thing");
        assert_eq!(steps, ["First part", "Second part"]);
        let (title, steps) = plan_parts("");
        assert_eq!(title, "Approve the plan?");
        assert!(steps.is_empty());
    }

    #[test]
    fn questions_read_labels() {
        let input = json!({"questions": [{
            "question": "Which color?", "header": "Color", "multiSelect": false,
            "options": [{"label": "Red", "description": "r"}, {"label": "Blue", "description": "b"}]
        }]});
        let q = questions(&input);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].text, "Which color?");
        assert_eq!(q[0].options, ["Red", "Blue"]);
    }

    #[test]
    fn allow_payload_keeps_the_input() {
        let input = json!({"command": "echo hi"});
        let p = allow_payload(&input, None);
        assert_eq!(p["behavior"], "allow");
        assert_eq!(p["updatedInput"], input);
    }

    #[test]
    fn rule_key_is_exact_for_shell_and_per_tool_otherwise() {
        let a = rule_key("Bash", &json!({"command": "echo a"}));
        let b = rule_key("Bash", &json!({"command": "echo b"}));
        assert_ne!(a, b);
        assert_eq!(
            a,
            rule_key("Bash", &json!({"command": "echo a", "description": "x"}))
        );
        assert_eq!(rule_key("Edit", &json!({"file_path": "a"})), "Edit");
        assert_eq!(rule_key("Edit", &json!({"file_path": "b"})), "Edit");
    }

    #[test]
    fn answers_go_into_the_updated_input() {
        let input = json!({"questions": []});
        let a = [("Which?".to_string(), "Red".to_string())];
        let p = allow_payload(&input, Some(&a));
        assert_eq!(p["updatedInput"]["answers"]["Which?"], "Red");
    }
}
