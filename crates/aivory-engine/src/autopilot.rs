//! The autopilot planner: park idle sessions, start queued tasks, within limits.
//!
//! [`plan`] is a pure function. It reads the rules, the sessions and the queue, and it returns
//! the actions in the order to carry them out. It changes nothing and it does not read the clock.
//!
//! # Rules
//!
//! 1. When `on` is `false`, the plan is empty.
//! 2. Park a session that is `Idle` and has been idle for `park_after_min` minutes or longer
//!    (whole minutes, `Pane::minutes`). Never park a session in `NeedsYou`, `Error` or `Working`,
//!    and never park a session that has a working, waiting or failed subagent. Sessions that are
//!    already parked or done need nothing.
//! 3. When `start_queued` is `true`, start queued tasks in queue order while the live sessions
//!    are fewer than `max_running` and the live sessions of the task's codebase are fewer than
//!    `max_per_codebase`. A task that does not fit its codebase is skipped, and the next task
//!    may still start. When the global limit is full, the planner stops.
//! 4. A session that the plan parks frees its slot in the same plan. Each started task takes a
//!    slot for the rest of the plan.
//!
//! The parks come first in the result, then the starts in queue order.
//!
//! # Queue order
//!
//! The order is the `order` key of the autopilot rules. Every order is stable: a tie keeps the
//! order of `queued_tasks`, which the caller passes in creation order (`Store::queued_tasks`).
//!
//! - **Oldest first:** the smaller `Task::created_at` first. A task from old JSON has
//!   `created_at` 0 and counts as the oldest.
//! - **Priority:** the higher `Task::priority` first (`Urgent`, `High`, `Normal`, `Low`). Tasks
//!   with the same priority start oldest first.
//! - **Smallest first:** by [`task_size`]: the fewer playbook steps are left (`5 - step`), the
//!   smaller the task. Tasks with the same number of steps left compare by the length of the
//!   prompt in characters. A shorter prompt is a smaller task.

use std::collections::HashMap;

use aivory_protocol::{Autopilot, Pane, QueueOrder, SessionState, Task, UnixMs};

/// One thing for the daemon to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Park the session with this id.
    Park { session: String },
    /// Start the task with this id.
    Start { task: String },
}

/// The steps of a playbook. `Task::step` counts from 0 to this value.
const PLAYBOOK_STEPS: u8 = 5;

/// Size of a task for [`QueueOrder::SmallestFirst`]: `(steps left, prompt length in characters)`.
/// A smaller value is a smaller task.
pub fn task_size(task: &Task) -> (u8, usize) {
    (
        PLAYBOOK_STEPS.saturating_sub(task.step),
        task.prompt.chars().count(),
    )
}

/// Plan the next actions at `now`, see the module docs.
///
/// `queued_tasks` is the queue in creation order (oldest first).
pub fn plan(
    autopilot: &Autopilot,
    panes: &[Pane],
    queued_tasks: &[Task],
    now: UnixMs,
) -> Vec<Action> {
    if !autopilot.on {
        return Vec::new();
    }
    let mut actions = Vec::new();
    let mut live_total = 0u32;
    let mut live_by_codebase: HashMap<&str, u32> = HashMap::new();
    for pane in panes {
        if should_park(pane, autopilot, now) {
            actions.push(Action::Park {
                session: pane.id.clone(),
            });
        } else if pane.state.is_live() {
            live_total += 1;
            *live_by_codebase.entry(pane.codebase.as_str()).or_default() += 1;
        }
    }
    if autopilot.start_queued {
        for task in ordered(queued_tasks, autopilot.order) {
            if live_total >= autopilot.max_running {
                break;
            }
            let in_codebase = live_by_codebase.entry(task.codebase.as_str()).or_default();
            if *in_codebase >= autopilot.max_per_codebase {
                continue;
            }
            *in_codebase += 1;
            live_total += 1;
            actions.push(Action::Start {
                task: task.id.clone(),
            });
        }
    }
    actions
}

fn should_park(pane: &Pane, autopilot: &Autopilot, now: UnixMs) -> bool {
    pane.state == SessionState::Idle
        && pane.minutes(now) >= i64::from(autopilot.park_after_min)
        && pane.subagents.iter().all(|sub| {
            !matches!(
                sub.state,
                SessionState::Working | SessionState::NeedsYou | SessionState::Error
            )
        })
}

/// The queue in start order. The sort is stable, so ties keep the age order.
fn ordered(queued: &[Task], order: QueueOrder) -> Vec<&Task> {
    let mut tasks: Vec<&Task> = queued.iter().collect();
    match order {
        QueueOrder::OldestFirst => tasks.sort_by_key(|t| t.created_at),
        QueueOrder::Priority => {
            tasks.sort_by_key(|t| (std::cmp::Reverse(t.priority), t.created_at));
        }
        QueueOrder::SmallestFirst => tasks.sort_by_key(|t| task_size(t)),
    }
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{pane, sub, task};
    use aivory_protocol::{MINUTE_MS, Playbook, TaskPriority};

    const NOW: UnixMs = 1_800_000_000_000;

    fn rules() -> Autopilot {
        Autopilot {
            on: true,
            park_after_min: 15,
            max_running: 10,
            max_per_codebase: 4,
            start_queued: true,
            order: QueueOrder::Priority,
        }
    }

    /// A pane in `state` since `minutes` minutes before `NOW`.
    fn p(id: &str, codebase: &str, state: SessionState, minutes: i64) -> Pane {
        pane(id, codebase, state, NOW - minutes * MINUTE_MS)
    }

    fn park(id: &str) -> Action {
        Action::Park { session: id.into() }
    }

    fn start(id: &str) -> Action {
        Action::Start { task: id.into() }
    }

    fn started(actions: &[Action]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Start { task } => Some(task.as_str()),
                Action::Park { .. } => None,
            })
            .collect()
    }

    // ---- off ---------------------------------------------------------------------------

    #[test]
    fn off_gives_no_action() {
        let ap = Autopilot {
            on: false,
            ..rules()
        };
        let panes = [p("a", "web", SessionState::Idle, 600)];
        let queue = [task("t1", "web")];
        assert!(plan(&ap, &panes, &queue, NOW).is_empty());
    }

    // ---- park --------------------------------------------------------------------------

    #[test]
    fn parks_idle_session_at_the_threshold_not_before() {
        let panes = [
            p("at", "web", SessionState::Idle, 15),
            p("before", "web", SessionState::Idle, 14),
            p("after", "web", SessionState::Idle, 90),
        ];
        let actions = plan(&rules(), &panes, &[], NOW);
        assert_eq!(actions, vec![park("at"), park("after")]);
    }

    #[test]
    fn counts_whole_minutes() {
        // 14 min 59 s idle is 14 whole minutes.
        let pane = pane("a", "web", SessionState::Idle, NOW - 15 * MINUTE_MS + 1_000);
        assert!(plan(&rules(), &[pane], &[], NOW).is_empty());
    }

    #[test]
    fn never_parks_needs_you_error_or_working() {
        let panes = [
            p("q", "web", SessionState::NeedsYou, 600),
            p("e", "web", SessionState::Error, 600),
            p("w", "web", SessionState::Working, 600),
        ];
        assert!(plan(&rules(), &panes, &[], NOW).is_empty());
    }

    #[test]
    fn parked_and_done_sessions_need_nothing() {
        let panes = [
            p("p", "web", SessionState::Parked, 600),
            p("d", "web", SessionState::Done, 600),
        ];
        assert!(plan(&rules(), &panes, &[], NOW).is_empty());
    }

    #[test]
    fn idle_session_with_a_busy_subagent_stays() {
        for busy in [
            SessionState::Working,
            SessionState::NeedsYou,
            SessionState::Error,
        ] {
            let mut pane = p("a", "web", SessionState::Idle, 600);
            pane.subagents = vec![sub("s", busy)];
            assert!(plan(&rules(), &[pane], &[], NOW).is_empty(), "{busy:?}");
        }
        let mut pane = p("a", "web", SessionState::Idle, 600);
        pane.subagents = vec![sub("s", SessionState::Idle), sub("d", SessionState::Done)];
        assert_eq!(plan(&rules(), &[pane], &[], NOW), vec![park("a")]);
    }

    #[test]
    fn park_threshold_follows_the_rule() {
        let ap = Autopilot {
            park_after_min: 120,
            ..rules()
        };
        let panes = [
            p("a", "web", SessionState::Idle, 119),
            p("b", "web", SessionState::Idle, 120),
        ];
        assert_eq!(plan(&ap, &panes, &[], NOW), vec![park("b")]);
    }

    #[test]
    fn parks_even_when_start_queued_is_off() {
        let ap = Autopilot {
            start_queued: false,
            ..rules()
        };
        let panes = [p("a", "web", SessionState::Idle, 60)];
        let queue = [task("t1", "web")];
        assert_eq!(plan(&ap, &panes, &queue, NOW), vec![park("a")]);
    }

    // ---- start and limits --------------------------------------------------------------

    #[test]
    fn starts_nothing_when_start_queued_is_off() {
        let ap = Autopilot {
            start_queued: false,
            ..rules()
        };
        assert!(plan(&ap, &[], &[task("t1", "web")], NOW).is_empty());
    }

    #[test]
    fn starts_all_queued_tasks_when_there_is_room() {
        let queue = [task("t1", "web"), task("t2", "api")];
        let ap = Autopilot {
            order: QueueOrder::OldestFirst,
            ..rules()
        };
        assert_eq!(plan(&ap, &[], &queue, NOW), vec![start("t1"), start("t2")]);
    }

    #[test]
    fn max_running_counts_live_sessions_only() {
        let ap = Autopilot {
            max_running: 4,
            max_per_codebase: 16,
            order: QueueOrder::OldestFirst,
            ..rules()
        };
        let panes = [
            p("a", "web", SessionState::Working, 1),
            p("b", "web", SessionState::NeedsYou, 1),
            p("c", "api", SessionState::Error, 1),
            p("d", "api", SessionState::Idle, 1),
            p("e", "api", SessionState::Parked, 1),
            p("f", "api", SessionState::Done, 1),
        ];
        // Four live sessions fill the limit.
        let queue = [task("t1", "web")];
        assert!(plan(&ap, &panes, &queue, NOW).is_empty());
        // One slot more: exactly one of two tasks starts.
        let ap = Autopilot {
            max_running: 5,
            ..ap
        };
        let queue = [task("t1", "web"), task("t2", "web")];
        assert_eq!(plan(&ap, &panes, &queue, NOW), vec![start("t1")]);
    }

    #[test]
    fn max_per_codebase_skips_a_full_codebase_but_not_the_rest() {
        let ap = Autopilot {
            max_per_codebase: 2,
            order: QueueOrder::OldestFirst,
            ..rules()
        };
        let panes = [
            p("a", "web", SessionState::Working, 1),
            p("b", "web", SessionState::Working, 1),
            p("c", "api", SessionState::Working, 1),
        ];
        let queue = [
            task("t1", "web"), // web is full
            task("t2", "api"), // api has room for one
            task("t3", "api"), // api is full after t2
            task("t4", "cli"), // empty codebase
        ];
        assert_eq!(
            plan(&ap, &panes, &queue, NOW),
            vec![start("t2"), start("t4")]
        );
    }

    #[test]
    fn started_tasks_count_against_the_limits_in_the_same_plan() {
        let ap = Autopilot {
            max_running: 3,
            max_per_codebase: 2,
            order: QueueOrder::OldestFirst,
            ..rules()
        };
        let queue = [
            task("t1", "web"),
            task("t2", "web"),
            task("t3", "web"), // per-codebase limit
            task("t4", "api"),
            task("t5", "api"), // global limit
        ];
        assert_eq!(
            started(&plan(&ap, &[], &queue, NOW)),
            vec!["t1", "t2", "t4"]
        );
    }

    #[test]
    fn parked_session_frees_a_slot_in_the_same_plan() {
        let ap = Autopilot {
            max_running: 2,
            max_per_codebase: 2,
            ..rules()
        };
        let panes = [
            p("busy", "web", SessionState::Working, 1),
            p("idle", "web", SessionState::Idle, 30),
        ];
        let queue = [task("t1", "web"), task("t2", "web")];
        // Without the park the limit is full. With it, one task starts. Parks come first.
        assert_eq!(
            plan(&ap, &panes, &queue, NOW),
            vec![park("idle"), start("t1")]
        );
    }

    #[test]
    fn idle_session_below_the_threshold_keeps_its_slot() {
        let ap = Autopilot {
            max_running: 2,
            ..rules()
        };
        let panes = [
            p("busy", "web", SessionState::Working, 1),
            p("idle", "web", SessionState::Idle, 14),
        ];
        assert!(plan(&ap, &panes, &[task("t1", "web")], NOW).is_empty());
    }

    #[test]
    fn parked_session_frees_a_codebase_slot() {
        let ap = Autopilot {
            max_per_codebase: 1,
            ..rules()
        };
        let panes = [p("idle", "web", SessionState::Idle, 30)];
        let queue = [task("t1", "web")];
        assert_eq!(
            plan(&ap, &panes, &queue, NOW),
            vec![park("idle"), start("t1")]
        );
    }

    // ---- queue order -------------------------------------------------------------------

    fn queue_of(spec: &[(&str, Playbook, u8, &str)]) -> Vec<Task> {
        spec.iter()
            .map(|(id, playbook, step, prompt)| {
                let mut t = task(id, "web");
                t.playbook = *playbook;
                t.step = *step;
                t.prompt = (*prompt).into();
                t
            })
            .collect()
    }

    /// Queued tasks `(id, priority, created_at)`, in the order of the slice.
    fn queue_with(spec: &[(&str, TaskPriority, UnixMs)]) -> Vec<Task> {
        spec.iter()
            .map(|(id, priority, created_at)| {
                let mut t = task(id, "web");
                t.priority = *priority;
                t.created_at = *created_at;
                t
            })
            .collect()
    }

    fn order_of(order: QueueOrder, queue: &[Task]) -> Vec<String> {
        let ap = Autopilot {
            order,
            max_running: 64,
            max_per_codebase: 16,
            ..rules()
        };
        started(&plan(&ap, &[], queue, NOW))
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn order_priority_urgent_first_ties_by_age() {
        use TaskPriority::{High, Low, Normal, Urgent};
        let queue = queue_with(&[
            ("low", Low, 1),
            ("norm-new", Normal, 50),
            ("urgent-new", Urgent, 90),
            ("high", High, 10),
            ("norm-old", Normal, 20),
            ("urgent-old", Urgent, 30),
        ]);
        assert_eq!(
            order_of(QueueOrder::Priority, &queue),
            [
                "urgent-old",
                "urgent-new",
                "high",
                "norm-old",
                "norm-new",
                "low"
            ]
        );
    }

    #[test]
    fn order_priority_ignores_the_playbook() {
        let mut queue = queue_of(&[
            ("port", Playbook::Port, 0, "x"),
            ("fix", Playbook::Fix, 0, "x"),
        ]);
        for (i, t) in queue.iter_mut().enumerate() {
            t.created_at = 100 + i as UnixMs;
        }
        // Same priority: the older task (the port) starts first.
        assert_eq!(order_of(QueueOrder::Priority, &queue), ["port", "fix"]);
    }

    #[test]
    fn order_priority_equal_keys_keep_the_queue_order() {
        let queue = queue_with(&[
            ("b", TaskPriority::Normal, 5),
            ("a", TaskPriority::Normal, 5),
            ("c", TaskPriority::Normal, 5),
        ]);
        assert_eq!(order_of(QueueOrder::Priority, &queue), ["b", "a", "c"]);
        assert_eq!(order_of(QueueOrder::OldestFirst, &queue), ["b", "a", "c"]);
    }

    #[test]
    fn order_oldest_first_follows_created_at_and_old_json_is_oldest() {
        let queue = queue_with(&[
            ("c", TaskPriority::Urgent, 300),
            ("a", TaskPriority::Low, 100),
            ("legacy", TaskPriority::Normal, 0),
            ("b", TaskPriority::High, 200),
        ]);
        assert_eq!(
            order_of(QueueOrder::OldestFirst, &queue),
            ["legacy", "a", "b", "c"]
        );
    }

    #[test]
    fn order_smallest_first_by_steps_left_then_prompt_length() {
        let queue = queue_of(&[
            (
                "long",
                Playbook::Feature,
                0,
                "a long prompt with many words",
            ),
            ("short", Playbook::Feature, 0, "short"),
            (
                "far",
                Playbook::Feature,
                3,
                "a long prompt with many words, far along",
            ),
            ("tie1", Playbook::Fix, 0, "same"),
            ("tie2", Playbook::Port, 0, "same"),
        ]);
        assert_eq!(
            order_of(QueueOrder::SmallestFirst, &queue),
            ["far", "tie1", "tie2", "short", "long"]
        );
    }

    #[test]
    fn task_size_counts_characters_not_bytes() {
        let mut t = task("t", "web");
        t.step = 5;
        t.prompt = "äöü".into();
        assert_eq!(task_size(&t), (0, 3));
        t.step = 9; // out of range: no underflow
        assert_eq!(task_size(&t).0, 0);
    }

    #[test]
    fn order_decides_who_gets_the_last_slot() {
        let queue = queue_with(&[
            ("old-low", TaskPriority::Low, 10),
            ("new-urgent", TaskPriority::Urgent, 20),
        ]);
        let ap = Autopilot {
            max_running: 1,
            order: QueueOrder::Priority,
            ..rules()
        };
        assert_eq!(plan(&ap, &[], &queue, NOW), vec![start("new-urgent")]);
        let ap = Autopilot {
            order: QueueOrder::OldestFirst,
            ..ap
        };
        assert_eq!(plan(&ap, &[], &queue, NOW), vec![start("old-low")]);
    }
}
