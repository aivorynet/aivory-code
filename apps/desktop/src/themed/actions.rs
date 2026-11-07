//! Controls of a theme (ADR 0024 section 5). A control maps to one action of the action API,
//! and to nothing else. Text for an agent comes from a composer that the user typed into, never
//! from the theme. `stop` and `autopilot` need a confirm that the app owns: the app writes the
//! question from its own data, so a theme label cannot trigger them unseen.

use aivory_protocol::ComposerPrefs;
use aivory_protocol::rpc::AutopilotPatch;
use serde_json::Value;

use super::data::ThemeSnapshot;
use crate::actions::{Action, Open};

/// What the user typed in a composer. Only a composer fills it.
#[derive(Debug, Clone, Default)]
pub struct ComposerInput {
    pub text: String,
    pub prefs: Option<ComposerPrefs>,
}

/// A question that the app asks before it runs the action. The text is the app's.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub question: String,
    action: Action,
}

impl Confirm {
    /// The user said yes. Only this call gives the action out.
    pub fn accept(self) -> Action {
        self.action
    }
}

/// The result of a press.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Run the action with `actions::act` (or open the screen of `Action::screen`).
    Run(Action),
    /// Show the confirm first.
    Confirm(Confirm),
    /// Open the decision dialog of this session.
    Decision(String),
}

/// The daemon method of each action of the layout format, or `screen` for a change of screen.
/// A control maps to one of these and to nothing else.
#[cfg(test)]
pub const METHODS: [(&str, &str); 13] = [
    ("answer", "session/answer"),
    ("focus", "screen"),
    ("dispatch", "session/dispatch"),
    ("newSession", "session/new"),
    ("send", "session/send"),
    ("pause", "session/pause"),
    ("park", "session/park"),
    ("stop", "session/stop"),
    ("resume", "session/resume"),
    ("autopilot", "autopilot/set"),
    ("permissionMode", "session/mode"),
    ("open", "screen"),
    ("setLook", "settings/set"),
];

/// Read one action object (`{ "action": "focus", "id": "$item.id" }`).
pub fn resolve(
    node: &Value,
    snap: &ThemeSnapshot,
    item: Option<&Value>,
    input: Option<&ComposerInput>,
) -> Result<Resolved, String> {
    let name = node
        .get("action")
        .and_then(Value::as_str)
        .ok_or("an action needs a name")?;
    let arg = |key: &str| -> Option<String> {
        match node.get(key)? {
            Value::String(s) if s.starts_with('$') => {
                let t = snap.text(s, item);
                (!t.is_empty()).then_some(t)
            }
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    };
    let session = |key: &str| arg(key).ok_or_else(|| format!("{name}: \"{key}\" has no value"));
    let typed = || {
        input
            .map(|i| i.text.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("{name}: the text comes from a composer that the user fills"))
    };
    let run = |a| Ok(Resolved::Run(a));
    match name {
        "focus" => run(Action::Focus(session("id")?)),
        "open" => match session("screen")?.as_str() {
            "warroom" => run(Action::Open(Open::WarRoom)),
            "inbox" => run(Action::Open(Open::Inbox)),
            "tasks" => run(Action::Open(Open::Tasks)),
            "repos" => run(Action::Open(Open::Repos)),
            "settings" => run(Action::Open(Open::Settings)),
            "decision" => Ok(Resolved::Decision(session("id")?)),
            other => Err(format!("open: unknown screen \"{other}\"")),
        },
        "dispatch" => run(Action::Dispatch {
            codebase: arg("codebase"),
            text: typed()?,
        }),
        "newSession" => {
            let composer = input
                .and_then(|i| i.prefs.clone())
                .ok_or("newSession: the composer gives the mode, model and effort")?;
            run(Action::NewSession {
                codebase: session("codebase")?,
                text: typed()?,
                composer,
            })
        }
        "pause" => run(Action::Pause(session("id")?)),
        "park" => run(Action::Park(session("id")?)),
        "resume" => run(Action::Resume(session("id")?)),
        "stop" => {
            let id = session("id")?;
            let title = snap
                .resolve(&format!("$sessions.{}.title", index_of(snap, &id)?), None)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            Ok(Resolved::Confirm(Confirm {
                question: format!("Stop the session \"{}\"? The agent ends.", clip(&title)),
                action: Action::Stop(id),
            }))
        }
        "autopilot" => {
            let on = session("on")? == "true";
            Ok(Resolved::Confirm(Confirm {
                question: format!("Turn the autopilot {}?", if on { "on" } else { "off" }),
                action: Action::Autopilot(AutopilotPatch {
                    on: Some(on),
                    ..Default::default()
                }),
            }))
        }
        "permissionMode" => {
            let mode = match session("mode")?.as_str() {
                "plan" => aivory_protocol::Mode::Plan,
                "edit" => aivory_protocol::Mode::Edit,
                "auto" => aivory_protocol::Mode::Auto,
                other => return Err(format!("permissionMode: unknown mode \"{other}\"")),
            };
            run(Action::Mode {
                id: session("id")?,
                mode,
            })
        }
        "setLook" => Err("setLook belongs to the look_switch component".to_string()),
        "answer" | "send" => Err(format!(
            "{name} belongs to the decision and composer components"
        )),
        other => Err(format!("unknown action \"{other}\"")),
    }
}

/// The index of a session id in `$sessions`.
fn index_of(snap: &ThemeSnapshot, id: &str) -> Result<usize, String> {
    (0..)
        .map_while(|i| {
            snap.resolve(&format!("$sessions.{i}.id"), None)
                .map(|v| (i, v))
        })
        .find(|(_, v)| v.as_str() == Some(id))
        .map(|(i, _)| i)
        .ok_or_else(|| format!("no session \"{id}\""))
}

/// A title that fits a question.
fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;
    use serde_json::json;

    fn snap() -> (ThemeSnapshot, String, String) {
        let war = Demo::new().war_room;
        let first = war.sessions[0].clone();
        (
            ThemeSnapshot::build(Some(&war), None, 1440),
            first.id,
            first.title,
        )
    }

    #[test]
    fn focus_and_open_are_screen_changes() {
        let (s, id, _) = snap();
        let item = json!({ "id": id });
        let r = resolve(
            &json!({"action":"focus","id":"$item.id"}),
            &s,
            Some(&item),
            None,
        )
        .unwrap();
        let Resolved::Run(a) = r else { panic!() };
        assert_eq!(a, Action::Focus(id));
        assert!(a.screen().is_some());
        let r = resolve(&json!({"action":"open","screen":"inbox"}), &s, None, None).unwrap();
        assert_eq!(r, Resolved::Run(Action::Open(Open::Inbox)));
        assert!(resolve(&json!({"action":"open","screen":"x"}), &s, None, None).is_err());
    }

    #[test]
    fn agent_text_comes_only_from_the_composer() {
        let (s, _, _) = snap();
        let node = json!({"action":"dispatch","codebase":"web","text":"rm -rf"});
        assert!(resolve(&node, &s, None, None).is_err());
        let typed = ComposerInput {
            text: "  fix the build ".into(),
            prefs: None,
        };
        let Resolved::Run(Action::Dispatch { codebase, text }) =
            resolve(&node, &s, None, Some(&typed)).unwrap()
        else {
            panic!()
        };
        assert_eq!(codebase.as_deref(), Some("web"));
        assert_eq!(text, "fix the build");
        let node = json!({"action":"newSession","codebase":"web"});
        assert!(resolve(&node, &s, None, Some(&typed)).is_err());
    }

    #[test]
    fn stop_and_autopilot_wait_for_the_app_confirm() {
        let (s, id, title) = snap();
        let node = json!({"action":"stop","id": id, "label":"Harmless"});
        let Resolved::Confirm(c) = resolve(&node, &s, None, None).unwrap() else {
            panic!("stop must confirm")
        };
        assert!(
            c.question
                .contains(&title.chars().take(20).collect::<String>())
        );
        assert!(!c.question.contains("Harmless"));
        assert_eq!(c.accept(), Action::Stop(id));
        let node = json!({"action":"autopilot","on":true});
        let Resolved::Confirm(c) = resolve(&node, &s, None, None).unwrap() else {
            panic!("autopilot must confirm")
        };
        assert_eq!(c.question, "Turn the autopilot on?");
        assert!(resolve(&json!({"action":"stop","id":"nope"}), &s, None, None).is_err());
    }

    #[test]
    fn each_action_maps_to_one_rpc() {
        use aivory_protocol::rpc::METHODS as RPC;
        let names: Vec<&str> = METHODS.iter().map(|(n, _)| *n).collect();
        let mut want = aivory_theme::layout::ACTIONS.to_vec();
        want.sort_unstable();
        let mut got = names.clone();
        got.sort_unstable();
        assert_eq!(got, want);
        for (name, rpc) in METHODS {
            assert!(rpc == "screen" || RPC.contains(&rpc), "{name} -> {rpc}");
        }
        // The actions that a button can fire give an action with that method.
        let (s, id, _) = snap();
        let cases = [
            (json!({"action":"focus","id":id}), "screen"),
            (json!({"action":"open","screen":"tasks"}), "screen"),
            (json!({"action":"pause","id":id}), "session/pause"),
            (json!({"action":"park","id":id}), "session/park"),
            (json!({"action":"resume","id":id}), "session/resume"),
            (
                json!({"action":"permissionMode","id":id,"mode":"plan"}),
                "session/mode",
            ),
        ];
        for (node, rpc) in cases {
            let name = node["action"].as_str().unwrap().to_string();
            let Resolved::Run(a) = resolve(&node, &s, None, None).unwrap() else {
                panic!("{name}")
            };
            assert_eq!(a.method(), rpc, "{name}");
            let table = METHODS.iter().find(|(n, _)| *n == name).unwrap().1;
            assert_eq!(table, rpc, "{name}");
        }
        let typed = ComposerInput {
            text: "x".into(),
            prefs: Some(
                aivory_protocol::demo::Demo::new().war_room.sessions[0]
                    .composer
                    .clone(),
            ),
        };
        let node = json!({"action":"dispatch"});
        let Resolved::Run(a) = resolve(&node, &s, None, Some(&typed)).unwrap() else {
            panic!()
        };
        assert_eq!(a.method(), "session/dispatch");
        let node = json!({"action":"newSession","codebase":"web"});
        let Resolved::Run(a) = resolve(&node, &s, None, Some(&typed)).unwrap() else {
            panic!()
        };
        assert_eq!(a.method(), "session/new");
        let node = json!({"action":"stop","id":id});
        let Resolved::Confirm(c) = resolve(&node, &s, None, None).unwrap() else {
            panic!()
        };
        assert_eq!(c.accept().method(), "session/stop");
        let node = json!({"action":"autopilot","on":false});
        let Resolved::Confirm(c) = resolve(&node, &s, None, None).unwrap() else {
            panic!()
        };
        assert_eq!(c.accept().method(), "autopilot/set");
        let node = json!({"action":"open","screen":"decision","id":id});
        assert_eq!(
            resolve(&node, &s, None, None).unwrap(),
            Resolved::Decision(id)
        );
    }

    #[test]
    fn other_actions_run_at_once() {
        let (s, id, _) = snap();
        let r = resolve(&json!({"action":"pause","id": id}), &s, None, None).unwrap();
        assert_eq!(r, Resolved::Run(Action::Pause(id)));
        assert!(resolve(&json!({"action":"delete"}), &s, None, None).is_err());
    }
}
