//! Fixtures for the unit tests.

use aivory_protocol::demo::task_steps;
use aivory_protocol::{
    ComposerPrefs, Effort, Mode, Pane, Playbook, SessionState, Subagent, Task, TaskColumn,
    TaskPriority, UnixMs,
};

pub fn pane(id: &str, codebase: &str, state: SessionState, since: UnixMs) -> Pane {
    Pane {
        id: id.into(),
        title: format!("session {id}"),
        codebase: codebase.into(),
        part: "core".into(),
        provider: "claude".into(),
        model: "opus-5.5".into(),
        state,
        since,
        cost: 0.0,
        tokens: 0,
        controlled: false,
        context_pct: 10,
        prompt: "do the work".into(),
        ask: None,
        subagents: Vec::new(),
        tail: Vec::new(),
        lines_total: 0,
        composer: ComposerPrefs {
            mode: Mode::Edit,
            provider: "claude".into(),
            model: "opus-5.5".into(),
            effort: Effort::High,
        },
        task: None,
    }
}

pub fn sub(id: &str, state: SessionState) -> Subagent {
    Subagent {
        id: id.into(),
        name: format!("agent {id}"),
        state,
        task: "help".into(),
    }
}

/// A queued feature task.
pub fn task(id: &str, codebase: &str) -> Task {
    Task {
        id: id.into(),
        column: TaskColumn::Planned,
        title: format!("task {id}"),
        codebase: codebase.into(),
        part: "core".into(),
        playbook: Playbook::Feature,
        step: 0,
        waiting_for: None,
        agents: Vec::new(),
        prompt: "do the work".into(),
        priority: TaskPriority::Normal,
        created_at: 0,
        started: None,
        cost: 0.0,
        sessions: Vec::new(),
        gates: Vec::new(),
        steps: task_steps(Playbook::Feature, TaskColumn::Planned, 0, false),
    }
}
