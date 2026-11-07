//! Messages for the user: a failure of a daemon call or the result of an action. The model keeps
//! the open notices; the shell draws them as calm toasts that hide after [`HIDE_AFTER`].

use std::time::Duration;

use aivory_protocol::rpc::{RpcError, code};
use gpui::SharedString;

/// How long a notice stays on screen.
pub const HIDE_AFTER: Duration = Duration::from_secs(6);

/// At most this many notices show at once; the oldest leaves first.
pub const MAX_OPEN: usize = 3;

/// The page that explains the coffee link. The Stripe link lives on that page.
pub const COFFEE_PAGE: &str = "https://aivory.net/coffee";

/// The public source of the app.
pub const SOURCE_PAGE: &str = "https://github.com/aivorynet/aivory-code";

/// The app must run this long before the coffee hint can show.
pub const COFFEE_AFTER: Duration = Duration::from_secs(10 * 60);

/// While a decision waits, ask again after this time.
pub const COFFEE_RETRY: Duration = Duration::from_secs(60);

/// A button next to the text of a notice: it opens a URL (a settings page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Help {
    pub label: &'static str,
    pub url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Failure,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: u64,
    pub kind: Kind,
    pub text: SharedString,
    pub help: Option<Help>,
}

/// A notice before the model gives it an id.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub kind: Kind,
    pub text: String,
    pub help: Option<Help>,
}

impl Draft {
    pub fn failure(text: impl Into<String>) -> Self {
        Self {
            kind: Kind::Failure,
            text: text.into(),
            help: None,
        }
    }

    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: Kind::Info,
            text: text.into(),
            help: None,
        }
    }
}

/// The coffee hint: calm, and only one time per app start.
pub fn coffee() -> Draft {
    Draft {
        kind: Kind::Info,
        text: "Hope AIVory Code saves you time. It's free, but I'd love a coffee :)".to_string(),
        help: Some(Help {
            label: "Buy Paul a coffee",
            url: COFFEE_PAGE,
        }),
    }
}

/// May the coffee hint show now? Never in a demo, and never while a decision waits.
pub fn coffee_due(enabled: bool, demo: bool, open_decisions: u32) -> bool {
    enabled && !demo && open_decisions == 0
}

/// The words for a method in "Pause failed". Methods that are not here show their wire name.
pub fn action_name(method: &str) -> &str {
    match method {
        "session/send" => "Send",
        "session/answer" => "Answer",
        "session/pause" => "Pause",
        "session/park" => "Park",
        "session/stop" => "Stop",
        "session/resume" => "Resume",
        "session/fork" => "Fork",
        "session/handoff" => "Hand off",
        "session/new" => "New session",
        "autopilot/set" => "Autopilot",
        "settings/set" => "Save the setting",
        "inbox/answer" => "Answer",
        "inbox/snooze" => "Snooze",
        "tasks/update" => "Move the task",
        "tasks/create" => "Add the task",
        "block/override" => "Override",
        "ledger/revert" => "Revert the turn",
        "radar/ping" => "Ping",
        "radar/pause" => "Pause",
        "repos/add" => "Add the repo",
        "repos/remove" => "Remove the repo",
        "project/new" => "New project",
        "map/addPart" => "Add the part",
        "providers/addAcp" => "Add the agent",
        "providers/removeAcp" => "Remove the agent",
        "settings/importClaude" => "Import",
        "daemon/restart" => "Restart",
        "daemon/scale" => "Scale",
        other => other,
    }
}

/// What went wrong, in words for a person. A daemon error keeps its own message; the codes that
/// say the same thing every time get a fixed sentence.
pub fn reason(err: &anyhow::Error) -> String {
    if let Some(rpc) = err.downcast_ref::<RpcError>() {
        return match rpc.code {
            code::SESSION_NOT_FOUND => "The session no longer exists.".to_string(),
            code::STALE_ASK => "The question is no longer open.".to_string(),
            code::NOT_AVAILABLE => {
                format!("This daemon cannot do that. {}", sentence(&rpc.message))
            }
            _ => sentence(&rpc.message),
        };
    }
    if err.downcast_ref::<std::io::Error>().is_some() {
        return "The daemon does not answer.".to_string();
    }
    sentence(&format!("{err:#}"))
}

/// `Action failed: reason`, for a failed call.
pub fn failure(method: &str, err: &anyhow::Error) -> Draft {
    Draft::failure(format!("{} failed. {}", action_name(method), reason(err)))
}

/// The text with a capital at the start and a full stop at the end, without a double stop.
pub fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut first = text.chars();
    let text = match first.next() {
        Some(c) => c.to_uppercase().collect::<String>() + first.as_str(),
        None => String::new(),
    };
    if text.is_empty() || text.ends_with(['.', '!', '?']) {
        text
    } else {
        format!("{text}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coffee_hint_stays_quiet() {
        assert!(coffee_due(true, false, 0));
        assert!(!coffee_due(false, false, 0));
        assert!(!coffee_due(true, true, 0));
        assert!(!coffee_due(true, false, 1));
        assert_eq!(coffee().help.map(|h| h.url), Some(COFFEE_PAGE));
    }

    fn rpc(code: i64, message: &str) -> anyhow::Error {
        anyhow::Error::new(RpcError::new(code, message))
    }

    #[test]
    fn a_failed_pause_names_the_action_and_the_reason() {
        let d = failure(
            "session/pause",
            &rpc(code::INVALID_STATE, "session is parked"),
        );
        assert_eq!(d.kind, Kind::Failure);
        assert_eq!(d.text, "Pause failed. Session is parked.");
    }

    #[test]
    fn fixed_codes_have_fixed_sentences() {
        assert_eq!(
            reason(&rpc(code::STALE_ASK, "x")),
            "The question is no longer open."
        );
        assert_eq!(
            reason(&rpc(code::SESSION_NOT_FOUND, "x")),
            "The session no longer exists."
        );
        assert_eq!(
            reason(&rpc(code::NOT_AVAILABLE, "needs the live engine")),
            "This daemon cannot do that. Needs the live engine."
        );
    }

    #[test]
    fn a_refused_move_keeps_the_text_of_the_daemon() {
        let text = "Planned cannot go to Review. Allowed: Working, Done.";
        let d = failure("tasks/update", &rpc(code::INVALID_STATE, text));
        assert_eq!(d.text, format!("Move the task failed. {text}"));
    }

    #[test]
    fn a_broken_pipe_says_the_daemon_is_silent() {
        let io = anyhow::Error::new(std::io::Error::other("pipe closed"));
        assert_eq!(reason(&io), "The daemon does not answer.");
    }

    #[test]
    fn an_unknown_method_shows_its_wire_name() {
        assert_eq!(action_name("foo/bar"), "foo/bar");
    }
}
