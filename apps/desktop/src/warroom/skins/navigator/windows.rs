//! The windows of the Navigator around the 3D view, as in the mockups `NavigatorMix` and
//! `NavigatorGroup`: Permission locks, Power, the selected session, osview, the aivoryd log,
//! the notifier dialog of the oldest ask, and the camera wall of a group.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use aivory_protocol::rpc::{AutopilotPatch, method};
use aivory_protocol::{AskKind, LineKind, Mode, Pane, SessionState, WarRoom};
use gpui::ClickEvent;
use gpui::prelude::*;
use gpui::{
    Div, FontWeight, HighlightStyle, SharedString, StyledText, canvas, div, px, relative, rgb,
};

use super::panels::{
    BTN, BTN_HOT, Click, Controls, FACE, KEY_AUTO, KEY_EDIT, KEY_PLAN, LED_AMBER, LED_GREEN,
    LED_OFF, LED_RED, SCREEN, SCREEN_TEXT, TEXT, TEXT_2, TITLE, TITLE_ASK, bevel, button, frame,
    led, screen, scroll_area, strip_chart, sunken,
};
use super::panels::{OUTLINE, Press};
use super::{Pick, WarRoomScreen};
use crate::actions::Action;
use crate::theme::*;
use crate::ui;
use crate::warroom::Overlay;
use crate::warroom::decisions::terminal_note;
use crate::warroom::skins::common::{
    Painter, TextCache, clip, ink, last_tool, live, part_of, short_tool,
};

/// The height of the Toolchest bar of the shell; the dialog centers on the whole window.
const TOOLCHEST_H: f32 = 26.;
/// The size of the mockups, which the window positions are fractions of.
const REF_W: f32 = 1440.;
const REF_H: f32 = 900.;
/// Samples that a strip chart keeps: one per second, four minutes.
const KEEP: usize = 240;
/// The width of a bar of a strip chart and the gap after it.
/// The colors of the strip charts.
const CHART: [u32; 3] = [0x6E8CFF, 0x3CE05A, 0xF2B233];
const SPEND: u32 = 0xFF8A80;

/// Place a window at the mockup position, as fractions of the screen.
pub(super) fn place(d: Div, (x, y, w, h): (f32, f32, f32, f32)) -> Div {
    d.absolute()
        .left(relative(x / REF_W))
        .top(relative(y / REF_H))
        .w(relative(w / REF_W))
        .h(relative(h / REF_H))
}

/// The state of the windows that lives between frames.
#[derive(Default)]
pub(super) struct Panels {
    /// The codebase that the camera wall shows, in place of the 3D view.
    pub group: Option<String>,
    /// The decision dialog shows, and which ask it shows (the oldest when none).
    pub dialog_open: bool,
    pub dialog_pick: Option<String>,
    /// The asks that the user has seen (shown in the dialog, or known at the start), and the
    /// new ones that arrived while the dialog was open.
    seen: HashSet<String>,
    fresh: Vec<String>,
    seen_primed: bool,
    /// The minimized windows in the order of minimizing; they stack along the bottom edge.
    min_order: Vec<&'static str>,
    /// The mode keys that the user pressed. The daemon has no method for a mode of a codebase.
    pub modes: HashMap<String, Mode>,
    /// Pause all is on, and the sessions that it paused.
    pub paused_all: bool,
    pub paused: Vec<String>,
    /// Minimized and maximized windows, by key, for the session.
    wins: HashMap<&'static str, WinState>,
    /// Window types that differ from the default: slot key to the kind that the slot shows.
    kinds: HashMap<&'static str, &'static str>,
    /// The slot that the window being built sits in (a window kind can show in another slot).
    cur_slot: std::cell::Cell<Option<&'static str>>,
    /// The window whose window menu is open.
    pub win_menu: Option<&'static str>,
    /// How far the dialog moved from the window center, and the last mouse point of a drag.
    pub dialog_off: [f32; 2],
    pub dialog_drag: Option<[f32; 2]>,
    history: History,
    /// The scroll state of the windows whose body can be taller than the window.
    scroll_locks: gpui::ScrollHandle,
    scroll_log: gpui::ScrollHandle,
    scroll_osview: gpui::ScrollHandle,
    scroll_live: gpui::ScrollHandle,
    scroll_dialog: gpui::ScrollHandle,
}

/// The state of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum WinState {
    #[default]
    Normal,
    /// Only the title bar shows.
    Min,
    /// The window fills the main area.
    Max,
    /// The window is hidden; the Windows menu of the Toolchest opens it again.
    Closed,
}

/// The window types: key and title. A slot shows one type; the default is its own key.
pub(super) const KINDS: [(&str, &str); 6] = [
    ("fsn", "fsn — 3D view"),
    ("locks", "Permission locks"),
    ("power", "Power"),
    ("osview", "osview"),
    ("log", "aivoryd log"),
    ("sel", "Selected session"),
];

impl Panels {
    /// The type that a slot shows.
    pub fn kind_of(&self, slot: &str) -> &'static str {
        self.kinds
            .get(slot)
            .copied()
            .or_else(|| KINDS.iter().map(|k| k.0).find(|k| *k == slot))
            .unwrap_or("fsn")
    }

    /// The slot that shows a type.
    pub fn slot_of_kind(&self, kind: &str) -> &'static str {
        KINDS
            .iter()
            .map(|k| k.0)
            .find(|s| self.kind_of(s) == kind)
            .unwrap_or("fsn")
    }

    /// Show `kind` in `slot`; the slot that showed `kind` takes the old type of `slot`.
    fn set_kind(&mut self, slot: &'static str, kind: &'static str) {
        let old = self.kind_of(slot);
        let other = self.slot_of_kind(kind);
        self.kinds.insert(other, old);
        self.kinds.insert(slot, kind);
        self.kinds.retain(|s, k| s != k);
    }

    /// Build the next window for the slot that shows type `kind`.
    pub fn cur_slot_set(&self, kind: &str) {
        self.cur_slot.set(Some(self.slot_of_kind(kind)));
    }

    pub fn cur_slot_clear(&self) {
        self.cur_slot.set(None);
    }

    /// At least one window shows a changed type.
    pub fn types_changed(&self) -> bool {
        !self.kinds.is_empty()
    }

    pub fn state_of(&self, key: &str) -> WinState {
        self.wins.get(key).copied().unwrap_or_default()
    }

    fn set_state(&mut self, key: &'static str, to: WinState) {
        if to == WinState::Normal {
            self.wins.remove(key);
        } else {
            self.wins.insert(key, to);
        }
        self.min_order.retain(|k| *k != key);
        if to == WinState::Min {
            self.min_order.push(key);
        }
    }

    /// The rect of a window (x, y, width, height in mockup units). Minimized windows stack
    /// as title bars along the bottom edge; the open windows share the freed space.
    fn rect(&self, key: &'static str) -> (f32, f32, f32, f32) {
        if self.state_of(key) == WinState::Min
            && let Some(i) = self.min_order.iter().position(|k| *k == key)
        {
            return (12. + i as f32 * 228., 864., 220., 24.);
        }
        let any_min = self.wins.values().any(|s| *s == WinState::Min);
        let hu = if any_min { 840. } else { 876. };
        let on = |k: &str| !matches!(self.state_of(k), WinState::Min | WinState::Closed);
        let top = on("fsn");
        let bottom: Vec<&str> = ["osview", "log"].into_iter().filter(|k| on(k)).collect();
        let right: Vec<&str> = ["locks", "power", "sel"]
            .into_iter()
            .filter(|k| on(k))
            .collect();
        let left_any = top || !bottom.is_empty();
        let (lx, lw, rx, rw) = match (left_any, !right.is_empty()) {
            (true, true) => (12., 880., 904., 524.),
            (true, false) => (12., 1416., 12., 0.),
            (false, true) => (12., 0., 12., 1416.),
            (false, false) => (12., 1416., 12., 1416.),
        };
        match key {
            "fsn" => {
                let h = if bottom.is_empty() {
                    hu
                } else {
                    (hu - 12.) * 560. / 864.
                };
                (lx, 12., lw, h)
            }
            "osview" | "log" => {
                let bh = if top { (hu - 12.) * 304. / 864. } else { hu };
                let y = 12. + hu - bh;
                if bottom.len() == 2 {
                    let ow = (lw - 12.) * 440. / 868.;
                    if key == "osview" {
                        (lx, y, ow, bh)
                    } else {
                        (lx + ow + 12., y, (lw - 12.) * 428. / 868., bh)
                    }
                } else {
                    (lx, y, lw, bh)
                }
            }
            _ => {
                let pref = |k: &str| match k {
                    "locks" => 268.,
                    "power" => 190.,
                    _ => 394.,
                };
                let n = right.len() as f32;
                let avail = hu - 12. * (n - 1.).max(0.);
                let sum: f32 = right.iter().map(|k| pref(k)).sum();
                let mut y = 12.;
                for k in &right {
                    let h = avail * pref(k) / sum;
                    if *k == key {
                        return (rx, y, rw, h);
                    }
                    y += h + 12.;
                }
                (rx, 12., rw, 100.)
            }
        }
    }

    /// A new ask opens the dialog; the ones that arrive while it shows wait their turn.
    pub fn open_ask(&mut self, id: Option<String>) {
        self.dialog_pick = id;
        self.dialog_open = true;
    }

    /// After an answer: close, and show the next decision only if one arrived meanwhile.
    pub fn after_answer(&mut self) {
        self.dialog_open = false;
        self.dialog_pick = None;
        if !self.fresh.is_empty() {
            let id = self.fresh.remove(0);
            self.open_ask(Some(id));
        }
    }
}

impl Panels {
    /// Screenshot runs: `AIVORY_NAV_GROUP=<codebase>` opens its camera wall, and
    /// `AIVORY_NAV_OPEN` opens the dialog.
    pub fn screenshot_hooks(&mut self, war: &WarRoom) {
        if let Ok(group) = std::env::var("AIVORY_NAV_GROUP") {
            self.group = Some(group);
        }
        // `AIVORY_NAV_DIALOG=<x>,<y>` moves the dialog; `AIVORY_NAV_WIN=log:min,fsn:max`
        // sets window states.
        if let Ok(v) = std::env::var("AIVORY_NAV_DIALOG") {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [x, y] = n[..] {
                self.dialog_off = [x, y];
            }
        }
        if let Ok(v) = std::env::var("AIVORY_NAV_WIN") {
            for item in v.split(',') {
                let Some((key, state)) = item.split_once(':') else {
                    continue;
                };
                let key = ["fsn", "locks", "power", "osview", "log", "sel"]
                    .into_iter()
                    .find(|k| *k == key);
                let state = match state {
                    "min" => WinState::Min,
                    "max" => WinState::Max,
                    _ => WinState::Normal,
                };
                if let Some(key) = key {
                    self.set_state(key, state);
                }
            }
        }
        // `AIVORY_NAV_OPEN=<n>` opens the dialog on the n-th oldest ask; `AIVORY_NAV_MENU=<key>`
        // opens the window menu of a window (key `dlg`: the dialog).
        if let Some(n) = std::env::var("AIVORY_NAV_OPEN")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            let mut asks: Vec<&Pane> = live(war).into_iter().filter(|s| s.ask.is_some()).collect();
            asks.sort_by_key(|s| s.since);
            if let Some(a) = asks.get(n).and_then(|s| s.ask.as_ref()) {
                self.open_ask(Some(a.id.clone()));
            }
        }
        if let Ok(v) = std::env::var("AIVORY_NAV_MENU") {
            self.win_menu = ["fsn", "locks", "power", "osview", "log", "sel", "dlg"]
                .into_iter()
                .find(|k| *k == v);
        }
    }

    /// Take the numbers of this frame.
    pub fn observe(&mut self, war: &WarRoom, now: Instant) {
        self.history.observe(war, now);
        let mut pending: Vec<&Pane> = live(war).into_iter().filter(|s| s.ask.is_some()).collect();
        pending.sort_by_key(|s| s.since);
        let ids: Vec<String> = pending
            .iter()
            .filter_map(|s| s.ask.as_ref().map(|a| a.id.clone()))
            .collect();
        if !self.seen_primed {
            // The decisions that wait at the start are known: no dialog for them.
            self.seen.extend(ids.iter().cloned());
            self.seen_primed = true;
        } else {
            for id in &ids {
                if self.seen.insert(id.clone()) {
                    self.fresh.push(id.clone());
                }
            }
            if !self.dialog_open && !self.fresh.is_empty() {
                let id = self.fresh.remove(0);
                self.open_ask(Some(id));
            }
        }
        self.fresh.retain(|id| ids.contains(id));
        // The ask on show ended (answered, or the session moved on): the dialog closes.
        if self
            .dialog_pick
            .as_ref()
            .is_some_and(|id| !ids.contains(id))
        {
            self.dialog_open = false;
            self.dialog_pick = None;
        }
        if ids.is_empty() {
            self.dialog_open = false;
        }
    }
}

/// The tokens and the spend per second, by provider.
#[derive(Default)]
struct History {
    at: Option<Instant>,
    last: HashMap<String, f64>,
    series: HashMap<String, VecDeque<f32>>,
}

impl History {
    fn observe(&mut self, war: &WarRoom, now: Instant) {
        let dt = match self.at {
            Some(a) if (now - a).as_secs_f32() < 1. => return,
            Some(a) => (now - a).as_secs_f32(),
            None => 1.,
        };
        self.at = Some(now);
        let mut totals: HashMap<String, f64> = HashMap::new();
        for s in live(war) {
            *totals.entry(s.provider.clone()).or_default() += s.tokens as f64;
            *totals.entry("$".to_string()).or_default() += s.cost;
        }
        for (key, total) in &totals {
            let before = self.last.insert(key.clone(), *total);
            let grew = before.map_or(0., |b| (total - b).max(0.));
            let list = self.series.entry(key.clone()).or_default();
            list.push_back((grew / f64::from(dt)) as f32);
            while list.len() > KEEP {
                list.pop_front();
            }
        }
        for (key, list) in &mut self.series {
            if !totals.contains_key(key) {
                list.push_back(0.);
                while list.len() > KEEP {
                    list.pop_front();
                }
            }
        }
    }

    /// The mean of the last `n` samples of a series.
    fn mean(&self, key: &str, n: usize) -> f32 {
        let Some(list) = self.series.get(key) else {
            return 0.;
        };
        let take: Vec<f32> = list.iter().rev().take(n).copied().collect();
        if take.is_empty() {
            0.
        } else {
            take.iter().sum::<f32>() / take.len() as f32
        }
    }
}

/// `12.4k`, `830`.
fn short_count(n: f32) -> String {
    if n >= 1000. {
        format!("{:.1}k", n / 1000.)
    } else {
        format!("{n:.0}")
    }
}

/// `14:39:12` in local time.
fn clock(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_millis_opt(ms).single().map_or_else(
        || "--:--:--".to_string(),
        |d| d.format("%H:%M:%S").to_string(),
    )
}

/// A text with some bold parts.
fn rich(parts: &[(String, bool)]) -> StyledText {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (s, bold) in parts {
        let from = text.len();
        text.push_str(s);
        if *bold {
            spans.push((
                from..text.len(),
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(text).with_highlights(spans)
}

/// A blink with a period of 1.2 s: full, then faint.
fn blink(t: f32) -> f32 {
    if ((t / 0.6) as u32).is_multiple_of(2) {
        1.
    } else {
        0.15
    }
}

/// The first line of a text, cut to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    clip(text.lines().next().unwrap_or(""), max)
}

/// The turn number of a session as `07`, from its last TURN line.
fn turn_of(s: &Pane) -> Option<String> {
    s.tail
        .iter()
        .rev()
        .filter(|l| l.kind == LineKind::Turn)
        .find_map(|l| {
            let digits: String = l
                .label
                .as_deref()
                .unwrap_or(&l.text)
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            digits.parse::<u32>().ok().map(|n| format!("{n:02}"))
        })
}

/// The words after a state, `Waits for you · 6 min`.
fn status_text(s: &Pane, now: i64) -> String {
    let m = s.minutes(now);
    let word = match (&s.ask, s.state) {
        (_, SessionState::Error) => "Error".to_string(),
        (Some(_), _) => "Waits for you".to_string(),
        (None, SessionState::Idle) => "Idle".to_string(),
        _ => "Working".to_string(),
    };
    format!("{word} · {m} min")
}

/// The LED color of a session.
fn led_of(s: &Pane) -> u32 {
    match (&s.ask, s.state) {
        (_, SessionState::Error) => LED_AMBER,
        (Some(_), _) => LED_RED,
        (None, SessionState::Idle) => 0x9A9A9A,
        _ => LED_GREEN,
    }
}

/// The lines of the live view of a session: the end of its transcript and its open question.
fn view_lines(s: &Pane, max: usize) -> Vec<(String, u32)> {
    use crate::console::display::{DKind, last};
    let mut out: Vec<(String, u32)> = Vec::new();
    for d in last(&s.tail, max) {
        let text = first_line(&d.text, 160);
        let (line, color) = match d.kind {
            DKind::Tool => (
                format!(
                    "● {}",
                    short_tool(d.label.as_deref().unwrap_or(""), &d.text)
                ),
                SCREEN_TEXT,
            ),
            DKind::Output => (format!("  └ {text}"), SCREEN_TEXT),
            DKind::You => (format!("> {text}"), 0x9EB4FF),
            DKind::Error => (format!("! {text}"), 0xF2C063),
            DKind::Heading => (text.to_uppercase(), 0xFFFFFF),
            // Table rows come padded with capped columns (console/display.rs).
            DKind::Bullet | DKind::Agent | DKind::Code | DKind::Table => (text, SCREEN_TEXT),
            DKind::Meta => (text, 0x8A8A8A),
        };
        out.push((line, color));
    }
    if let Some(ask) = &s.ask {
        out.push((String::new(), SCREEN_TEXT));
        out.push((
            format!(
                "? {}",
                first_line(&crate::warroom::decisions::question(s, ask), 120)
            ),
            0xF2C063,
        ));
    }
    out
}

/// A screen of text lines, the end of the list at the bottom.
fn lines_box(lines: &[(String, u32)], size: f32, lh: f32) -> Div {
    lines_flow(lines, size, lh).size_full().overflow_hidden()
}

/// The same lines at their full height, for a scroll area.
fn lines_flow(lines: &[(String, u32)], size: f32, lh: f32) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .font_family(tk().mono)
        .text_size(ui::ts(size))
        .line_height(ui::ts(lh));
    for (text, color) in lines {
        col = col.child(
            div()
                .min_h(ui::ts(lh))
                .flex_none()
                .text_color(rgb(*color))
                .child(if text.is_empty() {
                    " ".to_string()
                } else {
                    text.clone()
                }),
        );
    }
    col
}

impl WarRoomScreen {
    /// A click on a waiting tower or camera shows the dialog of its decision.
    pub(super) fn nav_open_ask(&mut self, s: &Pane) {
        if let Some(a) = &s.ask {
            self.navigator.panels.open_ask(Some(a.id.clone()));
        }
    }

    /// A window of the war room at its place, with working title-bar boxes: minimize to the
    /// title bar, maximize over the main area, and the window menu.
    pub(super) fn win(
        &self,
        cx: &mut gpui::Context<Self>,
        key: &'static str,
        title: impl Into<SharedString>,
        body: Div,
    ) -> Div {
        let key = self.navigator.panels.cur_slot.get().unwrap_or(key);
        let st = self.navigator.panels.state_of(key);
        let menu: Click = Rc::new(cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
            let p = &mut this.navigator.panels;
            p.win_menu = if p.win_menu == Some(key) {
                None
            } else {
                Some(key)
            };
            cx.notify();
        }));
        let min: Click = Rc::new(cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
            let p = &mut this.navigator.panels;
            let to = if p.state_of(key) == WinState::Min {
                WinState::Normal
            } else {
                WinState::Min
            };
            p.set_state(key, to);
            cx.notify();
        }));
        let close: Click = Rc::new(cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| {
            this.navigator.panels.set_state(key, WinState::Closed);
            cx.notify();
        }));
        let ctl = Controls {
            key: key.to_string(),
            menu: Some(menu),
            min: Some(min),
            close: Some(close),
            drag: None,
        };
        let rect = self.navigator.panels.rect(key);
        if st == WinState::Closed {
            return div().absolute().size(px(0.));
        }
        let body = if st == WinState::Min { div() } else { body };
        let f = frame(title, TITLE, ctl, body);
        match st {
            WinState::Normal => place(f, rect),
            WinState::Min => place(f, rect).h(px(24.)),
            WinState::Closed => f,
            WinState::Max => f
                .absolute()
                .left(px(12.))
                .top(px(12.))
                .right(px(12.))
                .bottom(px(12.)),
        }
    }

    /// The windows of the Navigator for the Windows menu of the Toolchest: key, title, and
    /// whether the window shows.
    pub(crate) fn nav_window_list(&self) -> Vec<(&'static str, &'static str, bool)> {
        let p = &self.navigator.panels;
        let mut list: Vec<(&'static str, &'static str, bool)> = [
            ("fsn", "fsn — 3D view"),
            ("locks", "Permission locks"),
            ("power", "Power"),
            ("osview", "osview"),
            ("log", "aivoryd log"),
            ("sel", "Selected session"),
        ]
        .into_iter()
        .map(|(k, t)| {
            let t = KINDS
                .iter()
                .find(|x| x.0 == p.kind_of(k))
                .map_or(t, |x| x.1);
            (k, t, p.state_of(k) != WinState::Closed)
        })
        .collect();
        list.push(("dialog", "Decision dialog", p.dialog_open));
        list
    }

    /// Show a window again (or restore it); "dialog" brings the decision dialog back.
    pub(crate) fn nav_open_window(&mut self, key: &str, cx: &mut gpui::Context<Self>) {
        let p = &mut self.navigator.panels;
        if key == "dialog" {
            p.open_ask(None);
        } else if let Some(k) = ["fsn", "locks", "power", "osview", "log", "sel"]
            .into_iter()
            .find(|k| *k == key)
        {
            p.set_state(k, WinState::Normal);
        }
        cx.notify();
    }

    /// How many decisions wait, and whether the dialog shows.
    pub(crate) fn nav_decisions(&self) -> (usize, bool) {
        let n = self
            .war
            .as_ref()
            .map_or(0, |w| live(w).iter().filter(|s| s.ask.is_some()).count());
        (n, self.navigator.panels.dialog_open)
    }

    /// The window menu of the title-bar box: Restore, Minimize, Maximize, Close, and the type.
    pub(super) fn nav_win_menu(&self, cx: &mut gpui::Context<Self>) -> Option<Div> {
        let key = self.navigator.panels.win_menu?;
        if key == "dlg" {
            // The dialog shows its own menu inside its frame.
            return None;
        }
        let (x, y) = if self.navigator.panels.state_of(key) == WinState::Max {
            (12., 12.)
        } else {
            let r = self.navigator.panels.rect(key);
            (r.0, r.1)
        };
        let mut list = div().flex().flex_col().p(px(2.)).bg(rgb(FACE));
        for (label, to) in [
            ("Restore", WinState::Normal),
            ("Minimize", WinState::Min),
            ("Maximize", WinState::Max),
            ("Close", WinState::Closed),
        ] {
            list = list.child(
                div()
                    .id(SharedString::from(format!("nav-wm-{label}")))
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .hover(|h| h.bg(rgb(BTN_HOT)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigator.panels.set_state(key, to);
                        this.navigator.panels.win_menu = None;
                        cx.notify();
                    })),
            );
        }
        let cur = self.navigator.panels.kind_of(key);
        list = list.child(div().h(px(1.)).my(px(2.)).bg(rgb(OUTLINE)));
        for (kind, title) in KINDS {
            list = list.child(
                div()
                    .id(SharedString::from(format!("nav-wt-{kind}")))
                    .h(px(22.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(ui::ts(T_SMALL))
                    .hover(|h| h.bg(rgb(BTN_HOT)))
                    .child(format!(
                        "{} {title}",
                        if kind == cur { "\u{2022}" } else { " " }
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigator.panels.set_kind(key, kind);
                        this.navigator.panels.win_menu = None;
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .absolute()
                .left(relative((x + 6.) / REF_W))
                .top(relative((y + 22.) / REF_H))
                .w(px(170.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .font_family(tk().sans)
                .text_color(rgb(TEXT))
                .child(bevel(true, list)),
        )
    }

    /// The mode of a codebase: the key that the user pressed, else the mode of its first live
    /// session.
    fn lock_mode(&self, war: &WarRoom, codebase: &str) -> Mode {
        if let Some(m) = self.navigator.panels.modes.get(codebase) {
            return *m;
        }
        // The mode of the sessions that aivoryd runs (a session in a terminal keeps the mode
        // of its own CLI); the newest one wins.
        live(war)
            .iter()
            .filter(|s| s.codebase == codebase && s.controlled)
            .max_by_key(|s| s.since)
            .map_or(Mode::Edit, |s| s.composer.mode)
    }

    /// Permission locks: a row per codebase with the keys Plan, Edit and Auto.
    pub(super) fn nav_locks(&self, war: &WarRoom, cx: &mut gpui::Context<Self>) -> Div {
        let mut rows = div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(8.));
        for cb in &war.codebases {
            let mode = self.lock_mode(war, &cb.id);
            let tint = match mode {
                Mode::Plan => KEY_PLAN,
                Mode::Edit => KEY_EDIT,
                Mode::Auto => KEY_AUTO,
            };
            // The lock: a closed shackle for Plan and Edit, an open one for Auto.
            let open = mode == Mode::Auto;
            let lock = div()
                .relative()
                .w(px(26.))
                .h(px(22.))
                .flex_none()
                .child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .top(px(if open { 0. } else { 2. }))
                        .w(px(10.))
                        .h(px(10.))
                        .border_2()
                        .border_b_0()
                        .border_color(rgb(0x222222))
                        .rounded_t_full(),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(1.))
                        .top(px(9.))
                        .w(px(16.))
                        .h(px(11.))
                        .bg(rgb(tint))
                        .border_1()
                        .border_color(rgb(0x222222)),
                );
            let mut keys = div().flex().w(px(240.)).h(px(28.)).bg(rgb(0x9A9A9A));
            for (m, label, color) in [
                (Mode::Plan, "Plan", KEY_PLAN),
                (Mode::Edit, "Edit", KEY_EDIT),
                (Mode::Auto, "Auto", KEY_AUTO),
            ] {
                let on = m == mode;
                let id = cb.id.clone();
                let face = div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(ui::ts(T_SMALL))
                    .bg(rgb(if on { color } else { BTN }))
                    .text_color(rgb(if on { 0xFFFFFF } else { TEXT }))
                    .child(label);
                keys = keys.child(
                    div()
                        .id(SharedString::from(format!("nav-lock-{}-{label}", cb.id)))
                        .flex_1()
                        .cursor_pointer()
                        .child(bevel(!on, face.h(px(24.))).size_full())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The mode goes to every session of the codebase that aivoryd
                            // runs (`session/mode`, from the next turn on).
                            this.navigator.panels.modes.insert(id.clone(), m);
                            let ids: Vec<String> = this
                                .model
                                .read(cx)
                                .war
                                .as_ref()
                                .map(|w| {
                                    live(w)
                                        .iter()
                                        .filter(|s| s.codebase == id && s.controlled)
                                        .map(|s| s.id.clone())
                                        .collect()
                                })
                                .unwrap_or_default();
                            for sid in ids {
                                this.model.read(cx).send::<method::SessionMode>(
                                    aivory_protocol::rpc::ModeParams { id: sid, mode: m },
                                    cx,
                                );
                            }
                            cx.notify();
                        })),
                );
            }
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .child(lock)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui::ts(T_BODY))
                            .font_weight(FontWeight::BOLD)
                            .child(cb.id.clone()),
                    )
                    .child(bevel(false, keys).flex_none()),
            );
        }
        self.win(
            cx,
            "locks",
            "Permission locks",
            div().size_full().child(scroll_area(
                "nav-locks-rows",
                &self.navigator.panels.scroll_locks,
                false,
                rows,
            )),
        )
    }

    /// Power: the switches Autopilot, New tasks and Pause all.
    pub(super) fn nav_power(&self, war: &WarRoom, cx: &mut gpui::Context<Self>) -> Div {
        let a = &war.autopilot;
        let paused = self.navigator.panels.paused_all;
        let cards: [(&str, String, bool, u32); 3] = [
            (
                "Autopilot",
                if a.on {
                    format!("idle {} min · max {}", a.park_after_min, a.max_running)
                } else {
                    "off · tasks wait for you".to_string()
                },
                a.on,
                LED_GREEN,
            ),
            (
                "New tasks",
                if a.start_queued {
                    format!("queue {} · free slot", war.queued)
                } else {
                    format!("queue {} · held", war.queued)
                },
                a.start_queued,
                LED_GREEN,
            ),
            (
                "Pause all",
                if paused {
                    format!("on · {} paused", self.navigator.panels.paused.len())
                } else {
                    "off · all agents run".to_string()
                },
                paused,
                LED_AMBER,
            ),
        ];
        let mut row = div().flex().gap(px(10.)).p(px(12.));
        for (i, (name, sub, on, lit)) in cards.into_iter().enumerate() {
            let switch = sunken(
                div().w(px(36.)).h(px(52.)).relative().child(
                    bevel(true, div().h(px(20.)).w(px(30.)).bg(rgb(0xD8D8D8)))
                        .absolute()
                        .left(px(1.))
                        .top(px(if on { 2. } else { 28. })),
                ),
            )
            .bg(rgb(0x2A2A2A))
            .flex_none();
            let card = bevel(
                true,
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .py(px(10.))
                    .size_full()
                    .bg(rgb(BTN))
                    .child(switch)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(ui::ts(T_SMALL))
                            .font_weight(FontWeight::BOLD)
                            .child(led(if on { lit } else { LED_OFF }))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(TEXT_2))
                            .text_center()
                            .child(sub),
                    ),
            );
            row = row.child(
                div()
                    .id(SharedString::from(format!("nav-power-{i}")))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .child(card.size_full())
                    .on_click(cx.listener(move |this, _, _, cx| this.nav_power_toggle(i, cx))),
            );
        }
        let mut body = div().flex().flex_col().child(row);
        if self.navigator.panels.types_changed() {
            body = body.child(
                div().flex().px(px(12.)).pb(px(8.)).child(bevel(
                    true,
                    div()
                        .id("nav-reset-windows")
                        .h(px(24.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .bg(rgb(BTN))
                        .hover(|h| h.bg(rgb(BTN_HOT)))
                        .text_size(ui::ts(T_SMALL))
                        .font_weight(FontWeight::BOLD)
                        .child("Reset windows")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.navigator.panels.kinds.clear();
                            cx.notify();
                        })),
                )),
            );
        }
        self.win(cx, "power", "Power", body)
    }

    /// A press on a power switch.
    fn nav_power_toggle(&mut self, which: usize, cx: &mut gpui::Context<Self>) {
        let Some(war) = self.war.clone() else { return };
        match which {
            0 => self.act(
                Action::Autopilot(AutopilotPatch {
                    on: Some(!war.autopilot.on),
                    ..AutopilotPatch::default()
                }),
                cx,
            ),
            1 => self.act(
                Action::Autopilot(AutopilotPatch {
                    start_queued: Some(!war.autopilot.start_queued),
                    ..AutopilotPatch::default()
                }),
                cx,
            ),
            _ => {
                if self.navigator.panels.paused_all {
                    let ids = std::mem::take(&mut self.navigator.panels.paused);
                    for id in ids {
                        self.act(Action::Resume(id), cx);
                    }
                    self.navigator.panels.paused_all = false;
                } else {
                    let ids: Vec<String> = live(&war)
                        .iter()
                        .filter(|s| s.controlled && s.state == SessionState::Working)
                        .map(|s| s.id.clone())
                        .collect();
                    for id in &ids {
                        self.act(Action::Pause(id.clone()), cx);
                    }
                    self.navigator.panels.paused = ids;
                    self.navigator.panels.paused_all = true;
                }
            }
        }
        cx.notify();
    }

    /// The selected session: its state, a live view of the transcript and the controls.
    pub(super) fn nav_selected(
        &self,
        war: &WarRoom,
        s: Option<&Pane>,
        t: f32,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let Some(s) = s else {
            return self.win(
                cx,
                "sel",
                "No session selected",
                div()
                    .p(px(12.))
                    .text_size(ui::ts(T_SMALL))
                    .child("Click a tower or a camera to see its session here."),
            );
        };
        let mut meta = format!("{} {}", s.provider, s.model);
        if let Some(turn) = turn_of(s) {
            meta.push_str(&format!(" · turn {turn}"));
        }
        meta.push_str(&format!(" · context {}%", s.context_pct));
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .text_size(ui::ts(T_SMALL))
            .child(led(led_of(s)).when(s.ask.is_some(), |d| d.opacity(blink(t))))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(status_text(s, war.now)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(TEXT_2))
                    .child(meta),
            );
        let view = div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .mx(px(12.))
            .child(
                bevel(
                    false,
                    div()
                        .size_full()
                        .bg(rgb(SCREEN))
                        .pt(px(26.))
                        .px(px(10.))
                        .pb(px(8.))
                        .child(scroll_area(
                            "nav-live-scroll",
                            &self.navigator.panels.scroll_live,
                            true,
                            div()
                                .min_h_full()
                                .flex()
                                .flex_col()
                                .justify_end()
                                .child(lines_flow(&view_lines(s, 14), T_SMALL, 18.)),
                        )),
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(2.))
                    .right(px(2.))
                    .top(px(2.))
                    .flex()
                    .justify_between()
                    .px(px(8.))
                    .py(px(4.))
                    .bg(ink(0x000000, 0.6))
                    .font_family(tk().mono)
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(11.))
                    .text_color(rgb(0xFFFFFF))
                    .child("LIVE VIEW")
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(div().text_color(rgb(LED_RED)).opacity(blink(t)).child("●"))
                            .child("REC"),
                    ),
            );
        let (resume, park, stop, focus) = (s.id.clone(), s.id.clone(), s.id.clone(), s.id.clone());
        let controls = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .py(px(10.))
            .child(button("nav-resume", "▶ Resume", false).on_click(
                cx.listener(move |this, _, _, cx| this.act(Action::Resume(resume.clone()), cx)),
            ))
            .child(button("nav-park", "❚❚ Park", false).on_click(
                cx.listener(move |this, _, _, cx| this.act(Action::Park(park.clone()), cx)),
            ))
            .child(button("nav-stop", "■ Stop", false).on_click(
                cx.listener(move |this, _, _, cx| this.act(Action::Stop(stop.clone()), cx)),
            ))
            .child(div().flex_1())
            .child(
                button("nav-focus", "Open Focus view", true)
                    .on_click(cx.listener(move |this, _, _, cx| this.skin_focus(&focus, cx))),
            );
        self.win(
            cx,
            "sel",
            format!("{} / {} — {}", s.codebase, part_of(s), s.title),
            div()
                .flex()
                .flex_col()
                .child(head)
                .child(view)
                .child(controls),
        )
    }

    /// osview: strip charts of the tokens per minute of each provider and of the spend.
    pub(super) fn nav_osview(
        &self,
        war: &WarRoom,
        _now: Instant,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let h = &self.navigator.panels.history;
        let mut providers: Vec<String> = live(war).iter().map(|s| s.provider.clone()).collect();
        for u in &war.usage {
            providers.push(u.provider.clone());
        }
        providers.sort();
        providers.dedup();
        let mut col = div().flex().flex_col().gap(px(10.)).px(px(10.)).py(px(8.));
        // (name, caption, color, readout, window gauge)
        type Chart = (String, String, u32, String, Option<(u8, String)>);
        let mut charts: Vec<Chart> = Vec::new();
        for (i, p) in providers.iter().take(3).enumerate() {
            let mut name = p.clone();
            let mut gauge = None;
            if let Some(u) = war.usage.iter().find(|u| &u.provider == p) {
                name = format!("{p} · {} {}%", u.label, u.pct);
                gauge = Some((u.pct, format!("{} window used", u.label)));
            }
            // The rate from the growth of the token counts between polls; at rest, the total.
            let rate = h.mean(p, 10) * 60.;
            let held: u64 = live(war)
                .iter()
                .filter(|s| &s.provider == p)
                .map(|s| s.tokens)
                .sum();
            let value = if rate > 0. {
                format!("{}/min", short_count(rate))
            } else {
                format!("{} tok", short_count(held as f32))
            };
            let gauge = gauge.map(|(pct, text)| {
                (
                    pct,
                    format!("{text} · {} tokens held now", short_count(held as f32)),
                )
            });
            charts.push((name, value, CHART[i], p.clone(), gauge));
        }
        let total: f64 = live(war).iter().map(|s| s.cost).sum();
        let per_hour = h.mean("$", 60) * 3600.;
        charts.push((
            format!("spend · ${total:.2} · ${per_hour:.2}/h"),
            if per_hour > 0. {
                format!("${per_hour:.2}/h")
            } else {
                format!("${total:.2}")
            },
            SPEND,
            "$".to_string(),
            None,
        ));
        for (name, value, color, key, gauge) in charts {
            let bars: Vec<f32> = h
                .series
                .get(&key)
                .map(|l| l.iter().copied().collect())
                .unwrap_or_default();
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(color))
                            .child(name)
                            .child(div().font_family(tk().mono).child(value)),
                    )
                    .children(gauge.map(|(pct, text)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .py(px(3.))
                            .child(
                                div().flex_1().h(px(10.)).bg(rgb(0x1B3A22)).child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(f32::from(pct.min(100)) / 100.))
                                        .bg(rgb(color)),
                                ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(ui::ts(T_CAPTION))
                                    .text_color(rgb(SCREEN_TEXT))
                                    .child(text),
                            )
                    }))
                    .child(strip_chart(&bars, color, 44., 1., "per s")),
            );
        }
        self.win(
            cx,
            "osview",
            "osview — tokens / min · last 4 min",
            div().size_full().p(px(6.)).child(
                screen(scroll_area(
                    "nav-osview-scroll",
                    &self.navigator.panels.scroll_osview,
                    false,
                    col,
                ))
                .size_full(),
            ),
        )
    }

    /// winterm: the log of aivoryd, one line per live session, the oldest first.
    pub(super) fn nav_log(&self, war: &WarRoom, t: f32, cx: &mut gpui::Context<Self>) -> Div {
        let mut list: Vec<&Pane> = live(war);
        list.sort_by_key(|s| s.since);
        let mut lines: Vec<(String, &str, String, String, u32)> = Vec::new();
        for s in list {
            let (kind, color, detail) = match (&s.ask, s.state) {
                (_, SessionState::Error) => (
                    "error",
                    0xF2C063,
                    s.ask
                        .as_ref()
                        .map_or_else(String::new, |a| first_line(&a.text, 40)),
                ),
                (Some(a), _) => (
                    "ask",
                    0xFF6B60,
                    match a.kind {
                        AskKind::Permission => format!("permission: {}", first_line(&a.text, 30)),
                        AskKind::Plan => format!("plan ready, {} steps", a.steps.len()),
                        _ => format!("question: {}", first_line(&a.text, 30)),
                    },
                ),
                (None, SessionState::Idle) => ("session", 0x8A8A8A, "turn done, idle".to_string()),
                _ => (
                    "tool",
                    SCREEN_TEXT,
                    last_tool(s).map_or_else(|| "working".to_string(), |l| clip(&l, 40)),
                ),
            };
            lines.push((clock(s.since), kind, s.title.clone(), detail, color));
        }
        let cursor = div()
            .w(px(8.))
            .h(px(14.))
            .flex_none()
            .bg(rgb(SCREEN_TEXT))
            .opacity(blink(t));
        let body = scroll_area(
            "nav-log-scroll",
            &self.navigator.panels.scroll_log,
            true,
            div()
                .min_h_full()
                .px(px(10.))
                .py(px(6.))
                .flex()
                .flex_col()
                .justify_end()
                .child({
                    let mut col = div()
                        .flex()
                        .flex_col()
                        .font_family(tk().mono)
                        .text_size(ui::ts(11.5))
                        .line_height(ui::ts(16.));
                    for (time, kind, title, detail, color) in &lines {
                        let cell = |w: f32, text: String| {
                            div()
                                .w(px(w))
                                .flex_none()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(text)
                        };
                        col = col.child(
                            div()
                                .h(ui::ts(16.))
                                .flex_none()
                                .flex()
                                .gap(px(10.))
                                .text_color(rgb(*color))
                                .child(cell(64., time.clone()))
                                .child(cell(52., kind.to_string()))
                                // Title and detail share the rest of the row; a narrow
                                // window cuts both with an ellipsis, not the detail alone.
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .max_w(px(220.))
                                        .truncate()
                                        .child(title.clone()),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(detail.clone())),
                        );
                    }
                    col
                })
                .child(cursor),
        );
        self.win(
            cx,
            "log",
            "winterm — aivoryd.log",
            div().size_full().p(px(6.)).child(screen(body).size_full()),
        )
    }

    /// The notifier dialog for the oldest ask that the user has not sent away. None when no
    /// ask waits. The cameras have no buttons: every decision comes through here.
    pub(super) fn nav_dialog(
        &self,
        war: &WarRoom,
        queue: &[&Pane],
        cx: &mut gpui::Context<Self>,
    ) -> Option<Div> {
        if !self.navigator.panels.dialog_open {
            return None;
        }
        let want = self.navigator.panels.dialog_pick.as_deref();
        let (index, s) = queue
            .iter()
            .enumerate()
            .find(|(_, p)| p.ask.as_ref().is_some_and(|a| Some(a.id.as_str()) == want))
            .or_else(|| queue.iter().enumerate().find(|(_, p)| p.ask.is_some()))?;
        let ask = s.ask.as_ref()?;
        let word = match ask.kind {
            AskKind::Permission => "permission",
            AskKind::Plan => "plan approval",
            AskKind::Question => "question",
            AskKind::Error => "error",
        };
        let wait = s.minutes(war.now);
        let who = |tail: &str| {
            rich(&[
                (s.provider.clone(), true),
                (" on ".to_string(), false),
                (format!("{} / {}", s.codebase, part_of(s)), true),
                (format!(" {tail} It waits {wait} min."), false),
            ])
        };
        let (head, detail) = match ask.kind {
            AskKind::Permission => (who("asks to run a command."), ask.text.clone()),
            AskKind::Plan => (
                who("wants to run this plan."),
                ask.steps
                    .iter()
                    .enumerate()
                    .map(|(i, st)| format!("{}  {st}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            AskKind::Question => (who("asks a question."), ask.text.clone()),
            AskKind::Error => (who("hit an error."), ask.text.clone()),
        };
        let mut note = format!("Session: {}", s.title);
        if let Some(turn) = turn_of(s) {
            note.push_str(&format!(" · turn {turn}"));
        }
        if let Some(tool) = last_tool(s) {
            note.push_str(&format!(" · {}", clip(&tool, 40)));
        }
        let icon = canvas(
            |_, _, _| (),
            |bounds, _, window, cx| {
                let mut cache = TextCache::default();
                let mut pen = Painter::new(window, cx, &mut cache);
                let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let tri = [
                    [ox + 24., oy + 2.],
                    [ox + 46., oy + 42.],
                    [ox + 2., oy + 42.],
                ];
                pen.polygon(&tri, 0xF2D23A, 1.);
                pen.polyline(&tri, 0x222222, 2., 1., true);
                pen.rect((ox + 22., oy + 14., 4., 15.), 0x222222, 1.);
                pen.rect((ox + 22., oy + 33., 4., 4.), 0x222222, 1.);
            },
        )
        .w(px(48.))
        .h(px(44.))
        .flex_none();
        let plan = ask.kind == AskKind::Plan;
        let mono_box = sunken(scroll_area(
            "nav-dlg-scroll",
            &self.navigator.panels.scroll_dialog,
            false,
            div()
                .px(px(8.))
                .py(px(6.))
                .font_family(tk().mono)
                .text_size(ui::ts(if plan { T_CAPTION } else { T_SMALL }))
                .line_height(ui::ts(18.))
                .text_color(rgb(TEXT))
                .whitespace_nowrap()
                .children(detail.lines().map(|l| div().child(l.to_string()))),
        ))
        .flex_1()
        .min_h(px(0.));
        let body = div()
            .flex_1()
            .flex()
            .gap(px(14.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(10.))
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(ui::ts(T_BODY)).child(head))
                    .child(mono_box)
                    .child(
                        div()
                            .truncate()
                            .text_size(ui::ts(T_CAPTION))
                            .text_color(rgb(TEXT_2))
                            .child(note),
                    ),
            );
        // The buttons: Show session at the left, the answers at the right.
        let target = (*s).clone();
        let mut buttons = div()
            .flex()
            .gap(px(8.))
            .px(px(16.))
            .pb(px(14.))
            .child(
                button("nav-dlg-show", "Show session", false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.navigator.panels.dialog_open = false;
                        this.nav_select_session(&target, cx);
                    },
                )),
            )
            .child(div().flex_1());
        if s.controlled {
            let rank = |o: &str| -> usize {
                match (ask.kind, o) {
                    (AskKind::Permission, "Deny") => 0,
                    (AskKind::Permission, "Always allow") => 1,
                    (AskKind::Permission, "Allow") => 2,
                    (AskKind::Plan, o) if o.starts_with("Reject") => 0,
                    (AskKind::Plan, o) if o.starts_with("Edit") => 2,
                    (AskKind::Plan, o) if o.starts_with("Approve") => 3,
                    _ => 1,
                }
            };
            let mut options: Vec<&String> = ask.options.iter().collect();
            options.sort_by_key(|o| rank(o));
            for (k, option) in options.into_iter().enumerate() {
                if plan && rank(option) == 3 && !ask.options.iter().any(|o| o.starts_with("Edit")) {
                    // Edit plan sits before Approve.
                    let id = s.id.clone();
                    buttons = buttons.child(button("nav-dlg-edit", "Edit plan…", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            // TODO(daemon): no method edits a plan; Focus view opens it.
                            this.skin_focus(&id, cx);
                        }),
                    ));
                }
                let label = match (ask.kind, option.as_str()) {
                    (AskKind::Permission, "Always allow") => format!(
                        "Always allow {}",
                        ask.text.split_whitespace().next().unwrap_or("this")
                    ),
                    _ => option.clone(),
                };
                let primary = rank(option) == if plan { 3 } else { 2 }
                    || (!plan && ask.kind != AskKind::Permission && k == 0);
                let (session, ask_id, pick) = (s.id.clone(), ask.id.clone(), option.clone());
                buttons = buttons.child(button(format!("nav-dlg-{k}"), &label, primary).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.skin_answer(&session, &ask_id, &pick, cx);
                        this.navigator.panels.after_answer();
                    }),
                ));
            }
        } else {
            buttons = buttons.child(terminal_note().text_color(rgb(TEXT_2)));
        }
        let (w, h) = if plan { (560., 300.) } else { (520., 236.) };
        let drag: Press = Rc::new(cx.listener(
            |this: &mut Self, ev: &gpui::MouseDownEvent, _, cx| {
                this.navigator.panels.dialog_drag =
                    Some([f32::from(ev.position.x), f32::from(ev.position.y)]);
                cx.notify();
            },
        ));
        // Minimize and Close both send the dialog away; the Decisions button of the Toolchest and
        // a new decision bring it back.
        let hide = |cx: &mut gpui::Context<Self>| -> Click {
            Rc::new(cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
                this.navigator.panels.dialog_open = false;
                this.navigator.panels.win_menu = None;
                cx.notify();
            }))
        };
        let min = hide(cx);
        let close = hide(cx);
        let menu: Click = Rc::new(cx.listener(|this: &mut Self, _: &ClickEvent, _, cx| {
            let p = &mut this.navigator.panels;
            p.win_menu = if p.win_menu == Some("dlg") {
                None
            } else {
                Some("dlg")
            };
            cx.notify();
        }));
        let ctl = Controls {
            key: "dlg".to_string(),
            menu: Some(menu),
            min: Some(min),
            close: Some(close),
            drag: Some(drag),
        };
        // The window menu of the dialog: Minimize, Close, Next decision, Previous decision.
        let ids: Vec<String> = queue
            .iter()
            .filter_map(|p| p.ask.as_ref().map(|a| a.id.clone()))
            .collect();
        let at = ids.iter().position(|i| *i == ask.id).unwrap_or(0);
        let menu_popup = (self.navigator.panels.win_menu == Some("dlg")).then(|| {
            let mut list = div().flex().flex_col().p(px(2.)).bg(rgb(FACE));
            for (k, label) in ["Minimize", "Close", "Next decision", "Previous decision"]
                .into_iter()
                .enumerate()
            {
                let n = ids.len().max(1);
                let target = match k {
                    2 => ids.get((at + 1) % n).cloned(),
                    3 => ids.get((at + n - 1) % n).cloned(),
                    _ => None,
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("nav-dlg-menu-{k}")))
                        .h(px(22.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(ui::ts(T_SMALL))
                        .hover(|h| h.bg(rgb(BTN_HOT)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let p = &mut this.navigator.panels;
                            p.win_menu = None;
                            match (k, &target) {
                                (2 | 3, Some(id)) => p.dialog_pick = Some(id.clone()),
                                (0 | 1, _) => p.dialog_open = false,
                                _ => {}
                            }
                            cx.notify();
                        })),
                );
            }
            div()
                .absolute()
                .left(px(6.))
                .top(px(2.))
                .w(px(170.))
                .occlude()
                .border_1()
                .border_color(rgb(OUTLINE))
                .child(bevel(true, list))
        });
        let off = self.navigator.panels.dialog_off;
        Some(
            div()
                .absolute()
                .top(px(-TOOLCHEST_H))
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_move(cx.listener(
                    move |this: &mut Self, ev: &gpui::MouseMoveEvent, window, cx| {
                        let p = &mut this.navigator.panels;
                        let Some(last) = p.dialog_drag else { return };
                        if !ev.dragging() {
                            p.dialog_drag = None;
                            return;
                        }
                        let now = [f32::from(ev.position.x), f32::from(ev.position.y)];
                        let view = window.viewport_size();
                        // The dialog stays inside the window.
                        let room = [
                            ((f32::from(view.width) - w) / 2.).max(0.),
                            ((f32::from(view.height) - h) / 2.).max(0.),
                        ];
                        for k in 0..2 {
                            p.dialog_off[k] =
                                (p.dialog_off[k] + now[k] - last[k]).clamp(-room[k], room[k]);
                        }
                        p.dialog_drag = Some(now);
                        cx.notify();
                    },
                ))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this: &mut Self, _: &gpui::MouseUpEvent, _, cx| {
                        this.navigator.panels.dialog_drag = None;
                        cx.notify();
                    }),
                )
                .child(
                    frame(
                        format!("aivoryd — {word} {} of {}", index + 1, queue.len()),
                        TITLE_ASK,
                        ctl,
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .child(body)
                            .child(buttons)
                            .children(menu_popup),
                    )
                    .relative()
                    .left(px(off[0]))
                    .top(px(off[1]))
                    .flex_none()
                    .occlude()
                    .w(px(w))
                    .h(px(h)),
                ),
        )
    }

    /// The camera wall of a group: a live view of each session of a codebase.
    pub(super) fn nav_wall(
        &self,
        war: &WarRoom,
        codebase: &str,
        t: f32,
        cx: &mut gpui::Context<Self>,
    ) -> Div {
        let all = live(war);
        let sessions: Vec<&Pane> = all
            .iter()
            .copied()
            .filter(|s| s.codebase == codebase)
            .collect();
        let path = format!("/fleet/{codebase}");
        let idle: Vec<String> = sessions
            .iter()
            .filter(|s| s.state == SessionState::Idle && s.controlled)
            .map(|s| s.id.clone())
            .collect();
        let bar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(4.))
            .border_b_1()
            .border_color(rgb(0x8A8A8A))
            .child(
                button("nav-fleet", "◀ Fleet", false).on_click(cx.listener(|this, _, _, cx| {
                    this.navigator.panels.group = None;
                    cx.notify();
                })),
            )
            .child(
                sunken(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .font_family(tk().mono)
                        .text_size(ui::ts(T_SMALL))
                        .text_color(rgb(TEXT))
                        .child(path),
                )
                .flex_1(),
            )
            .child(
                button("nav-start", "Start session…", false)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(Overlay::NewSession, cx))),
            )
            .child(
                button("nav-park-idle", "Park idle", false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        for id in &idle {
                            this.act(Action::Park(id.clone()), cx);
                        }
                    },
                )),
            );
        let n = sessions.len();
        let cols = if n <= 4 { 2 } else { 3 };
        let rows = n.div_ceil(cols).max(1);
        let mut grid = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(8.))
            .bg(rgb(0x7E7E7E));
        if n == 0 {
            grid = grid.child(
                div()
                    .text_size(ui::ts(T_BODY))
                    .text_color(rgb(0xFFFFFF))
                    .child("No live session in this codebase."),
            );
        }
        for r in 0..rows {
            let mut row = div().flex_1().min_h_0().flex().gap(px(6.));
            for c in 0..cols {
                let Some(s) = sessions.get(r * cols + c) else {
                    row = row.child(div().flex_1());
                    continue;
                };
                row = row.child(self.nav_camera(&all, s, t, cx));
            }
            grid = grid.child(row);
        }
        div().flex().flex_col().size_full().child(bar).child(grid)
    }

    /// One camera of the wall. A click selects the session. No answer buttons.
    fn nav_camera(
        &self,
        all: &[&Pane],
        s: &Pane,
        t: f32,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<Div> {
        let no = all.iter().position(|p| p.id == s.id).map_or(0, |i| i + 1);
        let (word, dot, edge, rec) = match (&s.ask, s.state) {
            (_, SessionState::Error) => ("ERROR", LED_AMBER, LED_AMBER, false),
            (Some(_), _) => ("WAITS", LED_RED, LED_RED, true),
            (None, SessionState::Idle) => ("IDLE", 0x777777, 0x2A2A2A, false),
            _ => ("REC", LED_RED, 0x2A2A2A, true),
        };
        let target = s.clone();
        let sel = self
            .navigator
            .pick
            .as_ref()
            .and_then(|p| p.session.as_deref())
            == Some(&s.id);
        div()
            .id(SharedString::from(format!("nav-cam-{}", s.id)))
            .relative()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(SCREEN))
            .cursor_pointer()
            .border_2()
            .border_color(rgb(if sel { 0xFFFFFF } else { edge }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .flex()
                    .justify_between()
                    .px(px(8.))
                    .py(px(4.))
                    .bg(ink(0x000000, 0.65))
                    .font_family(tk().mono)
                    .font_weight(FontWeight::BOLD)
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(rgb(0xFFFFFF))
                    .child(format!("CAM {no:02} {}/{}", s.codebase, part_of(s)))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_color(rgb(dot))
                                    .when(rec, |d| d.opacity(blink(t)))
                                    .child("●"),
                            )
                            .child(word),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .top(px(28.))
                    .bottom(px(24.))
                    .child(lines_box(&view_lines(s, 10), T_SMALL, 18.)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .bottom(px(6.))
                    .flex()
                    .justify_between()
                    .font_family(tk().mono)
                    .text_size(ui::ts(11.))
                    .text_color(rgb(0x9A9A9A))
                    .child(div().truncate().child(s.title.clone()))
                    .child(format!(
                        "{}m",
                        s.minutes(self.war.as_ref().map_or(0, |w| w.now))
                    )),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.nav_open_ask(&target);
                this.nav_select_session(&target, cx)
            }))
    }
}

/// The name of a pick, for the path of the location bar.
pub(super) fn path_of(pick: Option<&Pick>, group: Option<&str>) -> String {
    let mut path = "/fleet".to_string();
    if let Some(g) = group {
        path.push('/');
        path.push_str(g);
        return path;
    }
    if let Some(p) = pick {
        path.push('/');
        path.push_str(&p.codebase);
        if let Some(part) = &p.part {
            path.push('/');
            path.push_str(part);
        }
    }
    path
}

/// The legend strip under the 3D view: one row of colour swatches with labels.
pub(super) fn legend() -> Div {
    let sw = |c: u32, w: &str| {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(div().size(px(10.)).bg(rgb(c)))
            .child(w.to_string())
    };
    div()
        .flex_none()
        .h(px(24.))
        .flex()
        .items_center()
        .gap(px(16.))
        .px(px(10.))
        .bg(rgb(0x050A1E))
        .text_size(ui::ts(T_CAPTION))
        .text_color(rgb(0xC9D6FF))
        .child("height = tokens this turn")
        .child(sw(0x6E8CFF, "working"))
        .child(sw(0xFF5A4E, "waits for you"))
        .child(sw(0xF2B233, "error"))
        .child(sw(0xA0A6B2, "idle"))
}
