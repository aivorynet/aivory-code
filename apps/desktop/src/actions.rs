//! The one action API of all looks (ADR 0022). A look or a screen describes what the user
//! wants as an [`Action`]; [`act`] sends it to aivoryd. A failure shows as a notice, with the
//! same text in every look.

use aivory_protocol::rpc::{
    AnswerParams, AutopilotPatch, DispatchParams, NewSessionParams, SendParams, SessionRef, method,
};
use aivory_protocol::{ComposerPrefs, Look};
use gpui::{App, Entity};

use crate::model::Model;

/// What the user wants. The comment names the daemon method.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `session/answer`: an option of an ask, an own text, or both.
    Answer {
        id: String,
        ask: String,
        option: Option<String>,
        text: Option<String>,
    },
    /// `session/send`: a message; `composer` changes mode, model or effort from this turn.
    Send {
        id: String,
        text: String,
        composer: Option<ComposerPrefs>,
    },
    /// `session/pause`.
    Pause(String),
    /// `session/park`.
    Park(String),
    /// `session/stop`.
    Stop(String),
    /// `session/resume`.
    Resume(String),
    /// `autopilot/set`.
    Autopilot(AutopilotPatch),
    /// `session/mode`: the permission mode from the next turn on.
    Mode {
        id: String,
        mode: aivory_protocol::Mode,
    },
    /// `settings/set display.look`.
    SetLook(Look),
    /// `settings/set display.uiScale`: the text size in percent.
    SetUiScale(u16),
    /// A screen change: the Focus view of one session. No daemon call.
    Focus(String),
    /// `session/dispatch`: one line of work, started now or queued. The text comes from a
    /// composer that the user typed into.
    Dispatch {
        codebase: Option<String>,
        text: String,
    },
    /// `session/new`: a session with the text of a composer.
    NewSession {
        codebase: String,
        text: String,
        composer: ComposerPrefs,
    },
    /// A screen change: open a screen of the app. No daemon call.
    Open(Open),
    /// `inbox/answer`: an option or a text for an inbox item.
    InboxAnswer {
        id: String,
        option: Option<String>,
        text: Option<String>,
    },
}

/// The screens that an action opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    WarRoom,
    Inbox,
    Tasks,
    Repos,
    Settings,
}

impl Action {
    /// The screen that the action opens. `None` for an action with a daemon call.
    pub fn screen(&self) -> Option<crate::shell::Screen> {
        use crate::shell::Screen;
        match self {
            Self::Focus(id) => Some(Screen::Session(id.clone())),
            Self::Open(Open::WarRoom) => Some(Screen::WarRoom),
            Self::Open(Open::Inbox) => Some(Screen::Inbox),
            Self::Open(Open::Tasks) => Some(Screen::Tasks),
            Self::Open(Open::Repos) => Some(Screen::Repos),
            Self::Open(Open::Settings) => Some(Screen::Settings),
            _ => None,
        }
    }
}

#[cfg(test)]
impl Action {
    /// The daemon method of the action.
    pub fn method(&self) -> &'static str {
        use aivory_protocol::rpc::Method;
        match self {
            Self::Answer { .. } => method::SessionAnswer::NAME,
            Self::Send { .. } => method::SessionSend::NAME,
            Self::Mode { .. } => method::SessionMode::NAME,
            Self::Pause(_) => method::SessionPause::NAME,
            Self::Park(_) => method::SessionPark::NAME,
            Self::Stop(_) => method::SessionStop::NAME,
            Self::Resume(_) => method::SessionResume::NAME,
            Self::Autopilot(_) => method::AutopilotSet::NAME,
            Self::SetLook(_) | Self::SetUiScale(_) => method::SettingsSet::NAME,
            Self::Dispatch { .. } => method::SessionDispatch::NAME,
            Self::NewSession { .. } => method::SessionNew::NAME,
            Self::Focus(_) | Self::Open(_) => "screen",
            Self::InboxAnswer { .. } => method::InboxAnswer::NAME,
        }
    }
}

/// Send an action to aivoryd. Navigation (open a screen, focus a session) stays with the
/// screen events, because the shell owns the screens.
pub fn act(model: &Entity<Model>, action: Action, cx: &mut App) {
    if let Action::SetLook(look) = &action {
        crate::theme::set_tokens(crate::look::tokens(
            *look,
            aivory_protocol::Theme::Light,
            false,
        ));
        cx.refresh_windows();
    }
    if let Action::SetUiScale(percent) = &action {
        crate::ui::set_ui_scale(*percent);
        cx.refresh_windows();
    }
    let model = model.read(cx);
    let id = |id: String| SessionRef { id };
    match action {
        Action::Answer {
            id,
            ask,
            option,
            text,
        } => model.send::<method::SessionAnswer>(
            AnswerParams {
                id,
                ask,
                option,
                text,
            },
            cx,
        ),
        Action::Send { id, text, composer } => model.send::<method::SessionSend>(
            SendParams {
                id,
                text,
                subagent: None,
                attachments: Vec::new(),
                composer,
            },
            cx,
        ),
        // A screen change: the shell owns the screens (see `Action::screen`).
        Action::Focus(_) | Action::Open(_) => {}
        Action::Dispatch { codebase, text } => model.send::<method::SessionDispatch>(
            DispatchParams {
                codebase,
                prompt: text,
                ..Default::default()
            },
            cx,
        ),
        Action::NewSession {
            codebase,
            text,
            composer,
        } => model.send::<method::SessionNew>(
            NewSessionParams {
                codebase,
                part: None,
                title: None,
                prompt: text,
                composer,
                task: None,
                attachments: Vec::new(),
            },
            cx,
        ),
        Action::Mode { id, mode } => {
            model.send::<method::SessionMode>(aivory_protocol::rpc::ModeParams { id, mode }, cx)
        }
        Action::Pause(s) => model.send::<method::SessionPause>(id(s), cx),
        Action::Park(s) => model.send::<method::SessionPark>(id(s), cx),
        Action::Stop(s) => model.send::<method::SessionStop>(id(s), cx),
        Action::Resume(s) => model.send::<method::SessionResume>(id(s), cx),
        Action::Autopilot(patch) => model.send::<method::AutopilotSet>(patch, cx),
        Action::InboxAnswer { id, option, text } => model.send::<method::InboxAnswer>(
            aivory_protocol::rpc::InboxAnswerParams { id, option, text },
            cx,
        ),
        Action::SetUiScale(percent) => {
            model.send::<method::SettingsSet>(
                aivory_protocol::rpc::SettingsSetParams {
                    key: "display.uiScale".into(),
                    value: serde_json::json!(percent),
                },
                cx,
            );
        }
        Action::SetLook(look) => {
            model.send::<method::SettingsSet>(
                aivory_protocol::rpc::SettingsSetParams {
                    key: "display.look".into(),
                    value: serde_json::json!(look),
                },
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_names_a_daemon_method() {
        let all = [
            Action::Pause("a".into()),
            Action::Park("a".into()),
            Action::Stop("a".into()),
            Action::Resume("a".into()),
            Action::Autopilot(AutopilotPatch::default()),
            Action::SetLook(Look::Operator),
            Action::Dispatch {
                codebase: None,
                text: "x".into(),
            },
            Action::InboxAnswer {
                id: "a".into(),
                option: None,
                text: None,
            },
        ];
        for a in &all {
            assert!(
                aivory_protocol::rpc::METHODS.contains(&a.method()),
                "{}",
                a.method()
            );
        }
        assert_eq!(Action::Park("a".into()).method(), "session/park");
    }
}
