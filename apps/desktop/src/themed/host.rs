//! The host of a theme layout (ADR 0024 sections 3 to 6). It compiles the layouts of a bundle
//! once, when the theme loads: the tree becomes [`Node`] values, data paths are parsed, parts and
//! `$strings.*` are inserted, the tokens become lookup tables. A frame only walks the tree.
//!
//! [`ThemedView`] is the one view that draws a layout. It keeps the state that a re-layout
//! must not lose, under a stable key of the node: the text of a composer, the scroll position
//! of a transcript, the open tab. The key is the node `id` when the layout has one, else the
//! position in the tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use aivory_protocol::theme::ThemeBundle;
use gpui::prelude::*;
use gpui::{
    AnyView, Context, Entity, EventEmitter, KeyDownEvent, ScrollHandle, Subscription, Window,
    canvas, div, rgba,
};
use gpui_base::input::TextareaState;
use serde_json::{Map, Value};

use super::actions::{Confirm, Resolved};
use super::data::DataPath;
use crate::actions::Action;
use crate::model::Model;
use crate::shell::Screen;

/// The width classes, smallest first (ADR 0022 section 6).
pub const CLASSES: [&str; 3] = ["compact", "regular", "wide"];
/// The names of the text sizes.
pub const TYPES: [&str; 5] = ["xs", "sm", "md", "lg", "xl"];
/// Layout nesting that a part may add (the validator forbids cycles; this is the last guard).
const MAX_DEPTH: usize = 64;

// ---- the compiled tree ---------------------------------------------------------------------

/// A property of a node.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    /// A literal. `$strings.*` and `$$` are already resolved.
    Lit(Value),
    Path(DataPath),
    /// `{ "path": "$...", "map": { ... }, "else": ... }`
    Map {
        path: DataPath,
        table: Map<String, Value>,
        other: Option<Value>,
    },
}

/// The one test of `when`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cond {
    pub path: DataPath,
    pub test: Test,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Test {
    NonEmpty,
    Empty,
    Eq(Value),
    Ne(Value),
    In(Vec<Value>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierLayout {
    List,
    Grid,
    Scene,
}

/// A template of a `sessions` node for one density tier.
#[derive(Debug, Clone)]
pub struct Tier {
    pub name: &'static str,
    pub layout: TierLayout,
    pub template: Node,
}

/// A node of a layout.
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: &'static str,
    /// The stable key: the `id`, or the position below the root.
    pub key: String,
    pub props: Vec<(String, Prop)>,
    pub children: Vec<Node>,
    pub fallback: Option<Box<Node>>,
    pub when: Option<Cond>,
    /// `on.press`, as the action object.
    pub press: Option<Rc<Value>>,
    /// `on.key`, as the action object with `combo`.
    pub key_action: Option<Rc<Value>>,
    pub tiers: Vec<Tier>,
    /// `responsive`: the class rank (0 to 2) and the node.
    pub classes: Vec<(usize, Node)>,
}

impl Node {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, p)| p)
    }

    /// The first node below this one (or this one) for which `f` is true.
    pub fn any(&self, f: &dyn Fn(&Node) -> bool) -> bool {
        f(self)
            || self.children.iter().any(|c| c.any(f))
            || self.fallback.as_ref().is_some_and(|c| c.any(f))
            || self.tiers.iter().any(|t| t.template.any(f))
            || self.classes.iter().any(|(_, c)| c.any(f))
    }
}

/// The tokens that the renderer reads by name, and the layouts of the desktop class.
#[derive(Debug)]
pub struct Compiled {
    pub id: String,
    /// `0xRRGGBBAA`, by name (`bg`, `accent`, `state.needsYou`).
    pub colors: HashMap<String, u32>,
    pub types: [f32; 5],
    pub space: HashMap<String, f32>,
    pub radii: HashMap<String, f32>,
    pub glass_alpha: f32,
    pub screens: HashMap<String, Node>,
}

impl Compiled {
    /// The layout of a screen. Settings keeps its Display section: a layout of the Settings
    /// screen that has no `settings_section` is not used.
    pub fn layout(&self, screen: &str) -> Option<&Node> {
        let node = self.screens.get(screen)?;
        if screen == "settings" && !node.any(&|n| n.kind == "settings_section") {
            return None;
        }
        Some(node)
    }

    pub fn color(&self, name: &str) -> Option<u32> {
        self.colors.get(name).copied()
    }
}

fn parse_color(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#')?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some((n << 8) | 0xFF),
        8 => Some(n),
        _ => None,
    }
}

fn number_table(v: Option<&Value>) -> HashMap<String, f32> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect()
        })
        .unwrap_or_default()
}

struct Compiler<'a> {
    parts: &'a std::collections::BTreeMap<String, Value>,
    strings: Option<&'a std::collections::BTreeMap<String, String>>,
}

impl Compiler<'_> {
    fn text(&self, s: &str) -> Value {
        if let Some(rest) = s.strip_prefix("$$") {
            return Value::String(format!("${rest}"));
        }
        if let Some(key) = s.strip_prefix("$strings.") {
            let found = self.strings.and_then(|t| t.get(key));
            return Value::String(found.cloned().unwrap_or_else(|| key.to_string()));
        }
        Value::String(s.to_string())
    }

    /// Strings of a literal value: `$strings.*` and `$$` become text.
    fn literal(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => self.text(s),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.literal(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.literal(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn prop(&self, v: &Value) -> Prop {
        match v {
            Value::String(s) if s.starts_with('$') => {
                DataPath::parse(s).map_or_else(|| Prop::Lit(self.text(s)), Prop::Path)
            }
            Value::Object(o) if o.contains_key("map") => {
                let path = o
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(DataPath::parse);
                match (path, o.get("map").and_then(Value::as_object)) {
                    (Some(path), Some(table)) => Prop::Map {
                        path,
                        table: table
                            .iter()
                            .map(|(k, x)| (k.clone(), self.literal(x)))
                            .collect(),
                        other: o.get("else").map(|x| self.literal(x)),
                    },
                    _ => Prop::Lit(Value::Null),
                }
            }
            other => Prop::Lit(self.literal(other)),
        }
    }

    fn cond(&self, v: &Value) -> Result<Cond, String> {
        let path = v
            .get("path")
            .and_then(Value::as_str)
            .and_then(DataPath::parse)
            .ok_or("when needs a data path")?;
        let value = v.get("value").cloned();
        let list = |v: Option<Value>| match v {
            Some(Value::Array(a)) => Ok(a),
            _ => Err("\"in\" needs a list"),
        };
        let test = match v.get("is").and_then(Value::as_str) {
            Some("nonEmpty") => Test::NonEmpty,
            Some("empty") => Test::Empty,
            Some("eq") => Test::Eq(value.unwrap_or(Value::Null)),
            Some("ne") => Test::Ne(value.unwrap_or(Value::Null)),
            Some("in") => Test::In(list(value)?),
            _ => return Err("unknown test".into()),
        };
        Ok(Cond { path, test })
    }

    fn node(&self, v: &Value, key: String, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("the layout is too deep".into());
        }
        let obj = v.as_object().ok_or("a node must be an object")?;
        if let Some(part) = obj.get("use").and_then(Value::as_str) {
            let root = self
                .parts
                .get(part)
                .ok_or_else(|| format!("unknown part \"{part}\""))?;
            let mut n = self.node(root, key, depth + 1)?;
            if let Some(w) = obj.get("when") {
                n.when = Some(self.cond(w)?);
            }
            return Ok(n);
        }
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .and_then(|t| {
                aivory_theme::layout::CATALOG
                    .iter()
                    .copied()
                    .find(|c| *c == t)
            })
            .ok_or("unknown component")?;
        let key = obj
            .get("id")
            .and_then(Value::as_str)
            .map_or(key, str::to_string);
        let mut node = Node {
            kind,
            key: key.clone(),
            props: Vec::new(),
            children: Vec::new(),
            fallback: None,
            when: None,
            press: None,
            key_action: None,
            tiers: Vec::new(),
            classes: Vec::new(),
        };
        for (k, val) in obj {
            match k.as_str() {
                "type" | "id" => {}
                "children" => {
                    for (i, c) in val.as_array().into_iter().flatten().enumerate() {
                        node.children
                            .push(self.node(c, format!("{key}/{i}"), depth + 1)?);
                    }
                }
                "fallback" => {
                    node.fallback =
                        Some(Box::new(self.node(val, format!("{key}/fb"), depth + 1)?));
                }
                "when" => node.when = Some(self.cond(val)?),
                "on" => {
                    node.press = val.get("press").map(|a| Rc::new(self.literal(a)));
                    node.key_action = val.get("key").map(|a| Rc::new(self.literal(a)));
                }
                "tiers" if kind == "sessions" => {
                    for name in aivory_theme::layout::TIERS {
                        let Some(t) = val.get(name) else { continue };
                        let layout = match t.get("layout").and_then(Value::as_str) {
                            Some("grid") => TierLayout::Grid,
                            Some("scene") => TierLayout::Scene,
                            _ => TierLayout::List,
                        };
                        let template = t.get("template").ok_or("a tier needs a template")?;
                        node.tiers.push(Tier {
                            name,
                            layout,
                            template: self.node(template, format!("{key}/t{name}"), depth + 1)?,
                        });
                    }
                }
                "classes" if kind == "responsive" => {
                    for (rank, name) in CLASSES.iter().enumerate() {
                        if let Some(c) = val.get(*name) {
                            node.classes
                                .push((rank, self.node(c, format!("{key}/{name}"), depth + 1)?));
                        }
                    }
                }
                _ => node.props.push((k.clone(), self.prop(val))),
            }
        }
        Ok(node)
    }
}

/// Compile the layouts and the tokens of a bundle. Fails on the first broken node (the daemon
/// checks a theme, so this is a guard, not a validator).
pub fn compile(bundle: &ThemeBundle) -> Result<Compiled, String> {
    let t = &bundle.tokens;
    let mut colors = HashMap::new();
    for (name, v) in t
        .get("colors")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        match v {
            Value::String(s) => {
                if let Some(c) = parse_color(s) {
                    colors.insert(name.clone(), c);
                }
            }
            Value::Object(states) if name == "state" => {
                for (sn, sv) in states {
                    if let Some(c) = sv.as_str().and_then(parse_color) {
                        colors.insert(format!("state.{sn}"), c);
                    }
                }
            }
            _ => {}
        }
    }
    let mut types = [11., 12., 14., 18., 24.];
    for (i, name) in TYPES.iter().enumerate() {
        if let Some(n) = t.pointer(&format!("/type/{name}")).and_then(Value::as_f64) {
            types[i] = n as f32;
        }
    }
    let glass_alpha = t
        .pointer("/effects/glass_alpha")
        .and_then(Value::as_f64)
        .map_or(0.6, |a| a as f32);
    let strings = bundle
        .strings
        .get("en")
        .or_else(|| bundle.strings.values().next());
    let compiler = Compiler {
        parts: &bundle.parts,
        strings,
    };
    let mut screens = HashMap::new();
    for (name, root) in &bundle.layouts {
        let node = compiler
            .node(root, name.clone(), 0)
            .map_err(|e| format!("layouts/{name}: {e}"))?;
        screens.insert(name.clone(), node);
    }
    Ok(Compiled {
        id: bundle.id.clone(),
        colors,
        types,
        space: number_table(t.get("space")),
        radii: number_table(t.get("radii")),
        glass_alpha,
        screens,
    })
}

// ---- the compiled themes of this run ---------------------------------------------------------

thread_local! {
    static THEMES: RefCell<HashMap<String, Rc<Compiled>>> = RefCell::default();
    static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn store(c: Compiled) {
    THEMES.with(|t| t.borrow_mut().insert(c.id.clone(), Rc::new(c)));
}

pub fn drop_theme(id: &str) {
    THEMES.with(|t| t.borrow_mut().remove(id));
}

/// The theme that the app draws now (`look::apply` sets it).
pub fn set_current(id: Option<&str>) {
    CURRENT.with(|c| *c.borrow_mut() = id.map(str::to_string));
}

pub fn current() -> Option<Rc<Compiled>> {
    let id = CURRENT.with(|c| c.borrow().clone())?;
    THEMES.with(|t| t.borrow().get(&id).cloned())
}

/// True when the current theme has a layout for the screen.
pub fn has_layout(screen: &str) -> bool {
    current().is_some_and(|c| c.layout(screen).is_some())
}

// ---- the view ----------------------------------------------------------------------------

pub enum ThemedEvent {
    /// A control opened a screen.
    Screen(Screen),
    /// A control opened the decision dialog of a session.
    Decision(String),
    /// The app closes the decision dialog.
    CloseDecision,
}

/// What a composer does on send.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerCfg {
    /// `send` (to the selected session), `dispatch` or `newSession`.
    pub mode: String,
    pub session: Option<String>,
    /// The id of the ask that an `answer` composer replies to.
    pub ask: Option<String>,
    pub codebase: Option<String>,
}

/// The key combo of an `on.key` action.
#[derive(Debug, Clone)]
pub struct KeyBind {
    pub combo: String,
    pub action: Rc<Value>,
    pub item: Option<Value>,
}

pub struct ThemedView {
    pub(super) model: Entity<Model>,
    /// The compiled Settings screen, for `settings_section`.
    pub(super) settings: AnyView,
    pub(super) screen: String,
    pub(super) selected: Option<String>,
    /// The decision dialog is open for this session.
    pub(super) dialog: Option<String>,
    pub(super) inputs: HashMap<String, Entity<TextareaState>>,
    pub(super) subs: Vec<Subscription>,
    pub(super) scrolls: HashMap<String, ScrollHandle>,
    pub(super) tabs: HashMap<String, usize>,
    pub(super) composers: HashMap<String, ComposerCfg>,
    pub(super) confirm: Option<Confirm>,
    pub(super) binds: Vec<KeyBind>,
}

impl EventEmitter<ThemedEvent> for ThemedView {}

impl ThemedView {
    pub fn new(model: Entity<Model>, settings: AnyView) -> Self {
        Self {
            model,
            settings,
            screen: String::new(),
            selected: None,
            dialog: None,
            inputs: HashMap::new(),
            subs: Vec::new(),
            scrolls: HashMap::new(),
            tabs: HashMap::new(),
            composers: HashMap::new(),
            confirm: None,
            binds: Vec::new(),
        }
    }

    /// What the shell shows now. No redraw here: the shell draws the view next.
    pub fn show(&mut self, screen: &str, selected: Option<&str>, dialog: Option<&str>) {
        if self.screen != screen {
            self.screen = screen.to_string();
        }
        self.selected = selected.map(str::to_string);
        self.dialog = dialog.map(str::to_string);
    }

    /// Run the result of a press.
    pub(super) fn press(
        &mut self,
        spec: &Value,
        item: Option<&Value>,
        composer: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snap = self.model.read(cx).snapshot.clone();
        snap.select(self.selected.as_deref());
        let input = composer.and_then(|key| self.composer_input(key, &snap, cx));
        match super::actions::resolve(spec, &snap, item, input.as_ref()) {
            Ok(Resolved::Run(a)) => {
                self.run(a, cx);
                if let Some(key) = composer {
                    self.clear_input(key, window, cx);
                }
            }
            Ok(Resolved::Confirm(c)) => {
                self.confirm = Some(c);
                cx.notify();
            }
            Ok(Resolved::Decision(id)) => cx.emit(ThemedEvent::Decision(id)),
            Err(e) => eprintln!("aivory: theme action: {e}"),
        }
    }

    pub(super) fn run(&mut self, action: Action, cx: &mut Context<Self>) {
        if let Some(screen) = action.screen() {
            cx.emit(ThemedEvent::Screen(screen));
            return;
        }
        crate::actions::act(&self.model, action, cx);
        cx.notify();
    }

    /// A key press: fire the `on.key` actions that match.
    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.is_held {
            return;
        }
        // Esc belongs to the app: it closes the confirm, then the decision dialog.
        if e.keystroke.key == "escape" {
            if self.confirm.take().is_some() {
                cx.notify();
            } else if self.dialog.is_some() {
                cx.emit(ThemedEvent::CloseDecision);
            }
            return;
        }
        let hits: Vec<KeyBind> = self
            .binds
            .iter()
            .filter(|b| combo_matches(&b.combo, e))
            .cloned()
            .collect();
        for b in hits {
            self.press(&b.action, b.item.as_ref(), None, window, cx);
        }
    }
}

/// `Ctrl+K`, `Alt+Shift+1`, `F5`: modifiers and one key.
fn combo_matches(combo: &str, e: &KeyDownEvent) -> bool {
    let m = &e.keystroke.modifiers;
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut key = String::new();
    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            other => key = other.to_string(),
        }
    }
    !key.is_empty()
        && e.keystroke.key == key
        && (m.control || m.platform) == ctrl
        && m.alt == alt
        && m.shift == shift
}

impl Render for ThemedView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let keys = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let weak = weak.clone();
                window.on_key_event(move |e: &KeyDownEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        let _ = weak.update(cx, |v, cx| v.key_down(e, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        let Some(theme) = current() else {
            return div().size_full().child(keys).into_any_element();
        };
        let snap = self.model.read(cx).snapshot.clone();
        let selected = self.selected.clone().or_else(|| self.dialog.clone());
        snap.select(selected.as_deref());
        self.binds.clear();
        let frame = super::render::Frame {
            theme: &theme,
            snap: &snap,
            width: f32::from(window.viewport_size().width),
        };
        let mut root = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgba(theme.color("bg").unwrap_or(0xFFFFFFFF)))
            .text_color(rgba(theme.color("text").unwrap_or(0x000000FF)))
            .font_family(crate::theme::tk().sans)
            .child(keys);
        if let Some(node) = theme.layout(&self.screen) {
            let sc = super::render::Scope {
                item: None,
                prefix: "",
            };
            root = root.child(self.draw(node, &frame, &sc, window, cx));
        }
        if let Some(id) = self.dialog.clone()
            && let Some(node) = theme.layout("decision")
        {
            let sc = super::render::Scope {
                item: None,
                prefix: "dialog/",
            };
            let body = self.draw(node, &frame, &sc, window, cx);
            root = root.child(self.dialog_frame(&frame, body, &id, cx));
        }
        if let Some(c) = self.confirm.clone() {
            root = root.child(self.confirm_dialog(&frame, &c, cx));
        }
        root.into_any_element()
    }
}
