//! The snapshot that a theme layout reads (ADR 0024 sections 3 and 5). The model builds it when
//! its data changes, never per frame. A path is `$root.key.0.key`; `$item` is the current
//! element of a `repeat`; `$$` writes a literal `$`.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::LazyLock;

use aivory_protocol::rpc::DaemonInfo;
use aivory_protocol::{Line, SessionState, WarRoom};
use serde_json::{Map, Value, json};

/// The data paths of a layout, as patterns. `N` is a list index. The roots are the roots that
/// the validator of `aivory-theme` accepts (`layout::ROOTS`).
#[cfg(test)]
pub const PATHS: &[&str] = &[
    "$codebases.N.{name,...}",
    "$sessions.N.{id,title,codebase,part,provider,model,state,since,cost,tokens,contextPct,prompt,ask,subagents,composer}",
    "$asks.N.{id,title,codebase,ask}",
    "$gates.{needsYou,working,blocked,idle}.{count,ids}",
    "$usage.N.{...}",
    "$events.N.{...}",
    "$inbox.open",
    "$tasks.queued",
    "$repos.{count,items}",
    "$daemon.{version,name,demo,host}",
    "$analysis.N.{source,label,value,unit}",
    "$intent.{mode,asks,errors}",
    "$selected.{same as a session}",
    "$display.{class,width,tier}",
];

/// The density tier of ADR 0022 section 6 for a number of live sessions.
pub fn tier(live: usize) -> &'static str {
    match live {
        0 | 1 => "1",
        2..=4 => "2-4",
        5..=12 => "5-12",
        13..=30 => "13-30",
        _ => "31+",
    }
}

fn to<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// One step of a data path.
#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
enum Root {
    Item,
    Selected,
    Key(String),
}

/// A data path, parsed once when the theme loads (`$sessions.0.title`). `$strings.*` and `$$`
/// are no data paths; the host reads them at load time.
#[derive(Debug, Clone, PartialEq)]
pub struct DataPath {
    root: Root,
    segs: Vec<Seg>,
}

impl DataPath {
    /// `None` for a text that is no data path.
    pub fn parse(text: &str) -> Option<Self> {
        let body = text.strip_prefix('$')?;
        if body.is_empty() || body.starts_with('$') {
            return None;
        }
        let mut parts = body.split('.');
        let first = parts.next()?;
        if first == "strings" || first.is_empty() {
            return None;
        }
        let root = match first {
            "item" => Root::Item,
            "selected" => Root::Selected,
            other => Root::Key(other.to_string()),
        };
        let segs = parts
            .map(|p| {
                p.parse::<usize>()
                    .map_or_else(|_| Seg::Key(p.to_string()), Seg::Index)
            })
            .collect();
        Some(Self { root, segs })
    }
}

static ONE: LazyLock<Value> = LazyLock::new(|| Value::String("one".into()));

/// The data of all roots, as JSON, and the transcript tails that only the transcript and
/// terminal components read.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeSnapshot {
    root: Value,
    tails: HashMap<String, Vec<Line>>,
    /// The index of the selected session in `$sessions`. The view sets it for each frame.
    selected: Cell<Option<usize>>,
}

impl Default for ThemeSnapshot {
    fn default() -> Self {
        Self::build(None, None, 1440)
    }
}

/// The order of the model: live sessions first, an open ask first, then the longest wait.
fn model_order(a: &aivory_protocol::Pane, b: &aivory_protocol::Pane) -> std::cmp::Ordering {
    (!a.state.is_live(), a.ask.is_none(), a.since, &a.id).cmp(&(
        !b.state.is_live(),
        b.ask.is_none(),
        b.since,
        &b.id,
    ))
}

impl ThemeSnapshot {
    /// Build the snapshot from the war room.
    pub fn build(war: Option<&WarRoom>, info: Option<&DaemonInfo>, width: u32) -> Self {
        let mut root = Map::new();
        let mut sessions = Vec::new();
        let mut asks = Vec::new();
        let mut tails = HashMap::new();
        let mut live = 0;
        let mut errors = 0;
        if let Some(w) = war {
            let mut order: Vec<&aivory_protocol::Pane> = w.sessions.iter().collect();
            order.sort_by(|a, b| model_order(a, b));
            for s in order {
                if s.state.is_live() {
                    live += 1;
                }
                if s.state == SessionState::Error {
                    errors += 1;
                }
                let mut v = to(s);
                // The transcript tail is not a layout input, and it is large.
                if let Some(o) = v.as_object_mut() {
                    o.remove("tail");
                }
                sessions.push(v);
                tails.insert(s.id.clone(), s.tail.clone());
            }
            for id in &w.board.asks {
                if let Some(s) = w.sessions.iter().find(|s| &s.id == id) {
                    asks.push(json!({
                        "id": s.id, "title": s.title, "codebase": s.codebase, "ask": s.ask,
                    }));
                }
            }
            let mut codebases = to(&w.codebases);
            let mut repos = Vec::new();
            for (v, c) in codebases
                .as_array_mut()
                .into_iter()
                .flatten()
                .zip(&w.codebases)
            {
                if let Some(o) = v.as_object_mut() {
                    o.insert("name".into(), json!(c.id));
                }
                let n = w
                    .sessions
                    .iter()
                    .filter(|s| s.codebase == c.id && s.state.is_live())
                    .count();
                repos.push(json!({
                    "id": c.id, "name": c.id, "path": c.path, "branch": c.branch,
                    "worktrees": c.worktrees, "sessions": n,
                    "languages": c.languages.join(", "),
                }));
            }
            let events: Vec<Value> = w
                .activity
                .iter()
                .map(|a| {
                    json!({
                        "at": a.at, "active": a.active, "waiting": a.waiting, "idle": a.idle,
                        "text": format!("{} active, {} waiting, {} idle", a.active, a.waiting, a.idle),
                    })
                })
                .collect();
            root.insert("codebases".into(), codebases);
            root.insert("usage".into(), to(&w.usage));
            root.insert("events".into(), Value::Array(events));
            root.insert("gates".into(), to(&w.board));
            root.insert("inbox".into(), json!({ "open": w.inbox_open }));
            root.insert("tasks".into(), json!({ "queued": w.queued }));
            root.insert(
                "repos".into(),
                json!({ "count": repos.len(), "items": repos }),
            );
            root.insert("analysis".into(), Value::Array(analysis(w)));
        } else {
            for k in ["codebases", "usage", "events", "analysis"] {
                root.insert(k.into(), json!([]));
            }
            let none = json!({ "count": 0, "ids": [] });
            root.insert(
                "gates".into(),
                json!({ "needsYou": none, "working": none, "blocked": none, "idle": none }),
            );
            root.insert("inbox".into(), json!({ "open": 0 }));
            root.insert("tasks".into(), json!({ "queued": 0 }));
            root.insert("repos".into(), json!({ "count": 0, "items": [] }));
        }
        let mode = if !asks.is_empty() || errors > 0 {
            "alert"
        } else if live > 0 {
            "monitor"
        } else {
            "idle"
        };
        root.insert(
            "intent".into(),
            json!({ "mode": mode, "asks": asks.len(), "errors": errors }),
        );
        root.insert("sessions".into(), Value::Array(sessions));
        root.insert("asks".into(), Value::Array(asks));
        root.insert(
            "daemon".into(),
            info.map_or_else(
                || json!({ "version": "", "name": "", "demo": false, "host": "" }),
                |i| json!({ "version": i.version, "name": i.name, "demo": i.demo, "host": i.host }),
            ),
        );
        root.insert(
            "display".into(),
            json!({ "class": "desktop", "width": width, "tier": tier(live) }),
        );
        Self {
            root: Value::Object(root),
            tails,
            selected: Cell::new(None),
        }
    }

    /// Select the session with this id for `$selected` (or none).
    pub fn select(&self, id: Option<&str>) {
        let idx = id.and_then(|id| self.list("sessions").iter().position(|s| s["id"] == id));
        self.selected.set(idx);
    }

    /// The transcript tail of a session.
    pub fn tail(&self, id: &str) -> &[Line] {
        self.tails.get(id).map_or(&[], Vec::as_slice)
    }

    /// The number of live sessions (the tier counts them).
    pub fn live(&self) -> usize {
        self.list("sessions")
            .iter()
            .filter(|s| !matches!(s["state"].as_str(), Some("parked" | "done")))
            .count()
    }

    /// A list root as a slice.
    pub fn list(&self, root: &str) -> &[Value] {
        self.root
            .get(root)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    /// The value of a parsed path.
    pub fn get<'a>(&'a self, path: &DataPath, item: Option<&'a Value>) -> Option<&'a Value> {
        let mut node = match &path.root {
            Root::Item => item?,
            Root::Selected => self.root.get("sessions")?.get(self.selected.get()?)?,
            Root::Key(k) => self.root.get(k)?,
        };
        if matches!(&path.root, Root::Key(k) if k == "intent")
            && matches!(path.segs.as_slice(), [Seg::Key(m)] if m == "mode")
            && self.selected.get().is_some()
            && node.get("mode").and_then(Value::as_str) != Some("alert")
        {
            return Some(&ONE);
        }
        for seg in &path.segs {
            node = match (seg, node) {
                (Seg::Index(i), Value::Array(a)) => a.get(*i)?,
                (Seg::Key(k), Value::Object(o)) => o.get(k)?,
                (Seg::Index(i), Value::Object(o)) => o.get(&i.to_string())?,
                _ => return None,
            };
        }
        Some(node)
    }

    /// The value of a data path (`$sessions.0.title`), with `item` for `$item`. `None` when the
    /// path leads nowhere. `$$x` gives the text `$x`.
    pub fn resolve(&self, path: &str, item: Option<&Value>) -> Option<Value> {
        if let Some(rest) = path.strip_prefix("$$") {
            return Some(Value::String(format!("${rest}")));
        }
        let p = DataPath::parse(path)?;
        self.get(&p, item).cloned()
    }

    /// The text of a path for a `text` node. A number prints as is; `null` and a miss give "".
    pub fn text(&self, path: &str, item: Option<&Value>) -> String {
        as_text(self.resolve(path, item).as_ref())
    }

    /// The names of the data roots.
    #[cfg(test)]
    pub fn roots(&self) -> Vec<&str> {
        self.root
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .chain(std::iter::once("selected"))
            .collect()
    }
}

/// The text of a value: a string as is, a number or a bool printed, all else "".
pub fn as_text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// The items that the app computes from the war room. Each item names its source.
fn analysis(w: &WarRoom) -> Vec<Value> {
    let mut out = Vec::new();
    for u in &w.usage {
        out.push(json!({
            "source": "usage", "label": format!("{} {}", u.provider, u.label),
            "value": u.pct, "unit": "%",
        }));
    }
    let live: Vec<_> = w.sessions.iter().filter(|s| s.state.is_live()).collect();
    if !live.is_empty() {
        let mean = live.iter().map(|s| u32::from(s.context_pct)).sum::<u32>() / live.len() as u32;
        out.push(json!({
            "source": "sessions", "label": "Context used (mean)", "value": mean, "unit": "%",
        }));
        let cost: f64 = w.sessions.iter().map(|s| s.cost).sum();
        out.push(json!({
            "source": "sessions", "label": "Cost of all sessions",
            "value": (cost * 100.).round() / 100., "unit": "$",
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    #[test]
    fn every_root_of_the_validator_exists() {
        let s = ThemeSnapshot::default();
        let mut roots = s.roots();
        roots.sort_unstable();
        let mut want = aivory_theme::layout::ROOTS.to_vec();
        want.sort_unstable();
        assert_eq!(roots, want);
        for pattern in PATHS {
            let root = pattern[1..].split('.').next().unwrap();
            assert!(want.contains(&root), "{pattern}");
        }
    }

    #[test]
    fn paths_read_the_war_room() {
        let war = Demo::new().war_room;
        let first = war.sessions[0].clone();
        let s = ThemeSnapshot::build(Some(&war), None, 1200);
        s.select(Some(&first.id));
        assert_eq!(s.text("$sessions.0.title", None), first.title);
        assert_eq!(s.text("$selected.id", None), first.id);
        assert_eq!(s.text("$display.class", None), "desktop");
        assert!(s.resolve("$sessions.0.tail", None).is_none());
        assert!(s.resolve("$sessions.999.id", None).is_none());
        assert!(s.resolve("$nope", None).is_none());
        let item = json!({ "id": "x" });
        assert_eq!(s.text("$item.id", Some(&item)), "x");
        assert_eq!(s.text("$$5", None), "$5");
    }

    #[test]
    fn tiers_follow_the_sessions() {
        assert_eq!(tier(1), "1");
        assert_eq!(tier(4), "2-4");
        assert_eq!(tier(12), "5-12");
        assert_eq!(tier(30), "13-30");
        assert_eq!(tier(31), "31+");
    }
}
