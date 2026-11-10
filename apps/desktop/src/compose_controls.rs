//! The composer controls of every chat window, in every look: mode, provider, model, and
//! effort. One state type and one row; each look gives its colors in a [`ControlStyle`].
//!
//! The row starts from the composer of the pane, which the daemon reads from the session (the
//! transcript of a native session, or the start values and the sends of a run). A pick stays
//! until the pane reports the same values, so the row always shows the state of the session
//! or the pick that the next send carries.

use aivory_protocol::rpc::{HandoffParams, SendParams, method};
use aivory_protocol::{ComposerPrefs, Effort, Mode, Pane};
use gpui::prelude::*;
use gpui::{
    App, Context, Div, Entity, FontWeight, Rgba, SharedString, Stateful, deferred, div, px, rgb,
};

use crate::model::Model;
use crate::theme::*;
use crate::ui;
use crate::warroom::{effort_label, mode_label};

const MODES: [Mode; 3] = [Mode::Plan, Mode::Edit, Mode::Auto];
const EFFORTS: [Effort; 4] = [Effort::Low, Effort::Medium, Effort::High, Effort::Max];

/// The menu that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Provider,
    Model,
    Effort,
}

/// The colors and the shape of the row in one look.
#[derive(Debug, Clone)]
pub struct ControlStyle {
    pub pill: Rgba,
    pub pill_hover: Rgba,
    pub pill_text: Rgba,
    pub on: Rgba,
    pub on_text: Rgba,
    pub menu: Rgba,
    pub menu_text: Rgba,
    pub menu_hover: Rgba,
    /// `None`: the font of the page.
    pub font: Option<SharedString>,
    /// `None`: a pill.
    pub radius: Option<f32>,
    /// `None`: the caption size and 24 px pills. `Some(size)`: this text size in low 20 px
    /// cells (a terminal status line).
    pub text: Option<f32>,
}

impl ControlStyle {
    /// The light page style of the Minimal look.
    pub fn page() -> Self {
        let t = tk();
        Self {
            pill: rgb(t.track),
            pill_hover: rgb(t.track_2),
            pill_text: rgb(t.ink_2),
            on: rgb(t.dark),
            on_text: rgb(0xFFFFFF),
            menu: rgb(t.card),
            menu_text: rgb(t.ink),
            menu_hover: rgb(t.menu_hover),
            font: None,
            radius: None,
            text: None,
        }
    }

    /// The Operator look: a terminal status line. Plain dim text on the console, the picked
    /// value in inverse video, all in the small mono size of the look.
    pub fn operator(font: Option<SharedString>) -> Self {
        Self {
            pill: gpui::rgba(0x00000000),
            pill_hover: rgb(0x0F2A1C),
            pill_text: rgb(0x6DB596),
            on: rgb(0x9BFFC0),
            on_text: rgb(0x020A04),
            menu: rgb(0x03200E),
            menu_text: rgb(0xB8FFCC),
            menu_hover: rgb(0x125A2A),
            font,
            radius: Some(0.),
            // MX_SMALL of the Operator kit.
            text: Some(9.5),
        }
    }

    /// A dark console style from three colors: the panel, the text, and the accent.
    pub fn console(panel: u32, text: u32, accent: u32, font: Option<SharedString>) -> Self {
        Self {
            pill: rgb(panel),
            pill_hover: rgb(accent),
            pill_text: rgb(text),
            on: rgb(accent),
            on_text: rgb(panel),
            menu: rgb(panel),
            menu_text: rgb(text),
            menu_hover: rgb(accent),
            font,
            radius: Some(2.),
            text: None,
        }
    }
}

/// The state of the controls of one chat window.
#[derive(Debug, Clone, Default)]
pub struct Controls {
    /// The pane and the values that the user picked for it.
    picked: Option<(String, ComposerPrefs)>,
    open: Option<Menu>,
}

impl Controls {
    /// The values that the next send carries: the pick, or the state of the session.
    pub fn current(&self, pane: &Pane) -> ComposerPrefs {
        match &self.picked {
            Some((id, p)) if *id == pane.id => p.clone(),
            _ => state_of(pane),
        }
    }

    /// Forget a pick that the session now reports, so later changes of the session show.
    pub fn sync(&mut self, pane: &Pane) {
        if let Some((id, p)) = &self.picked
            && (*id != pane.id || *p == state_of(pane))
        {
            self.picked = None;
        }
    }

    /// Close an open menu. `true`: one was open (Escape).
    pub fn close(&mut self) -> bool {
        self.open.take().is_some()
    }

    fn pick(&mut self, pane: &str, p: ComposerPrefs) {
        self.picked = Some((pane.to_string(), p));
        self.open = None;
    }

    /// The row: mode, provider, model, effort. `get` finds these controls in the view.
    pub fn row<V: 'static>(
        &self,
        id: &'static str,
        pane: &Pane,
        providers: &[(String, Vec<String>)],
        style: &ControlStyle,
        get: fn(&mut V) -> &mut Controls,
        cx: &mut Context<V>,
    ) -> Div {
        let cur = self.current(pane);
        let pane_id = pane.id.clone();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(if style.text.is_some() { 2. } else { 6. }))
            .text_size(ui::ts(style.text.unwrap_or(T_CAPTION)));
        if let Some(f) = &style.font {
            row = row.font_family(f.clone());
        }
        for mode in MODES {
            let base = cur.clone();
            let pid = pane_id.clone();
            row = row.child(
                pill(
                    style,
                    (id, mode as usize),
                    mode_label(mode),
                    cur.mode == mode,
                )
                .tooltip(ui::tip(mode_tip(mode)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut p = base.clone();
                    p.mode = mode;
                    get(this).pick(&pid, p);
                    cx.notify();
                })),
            );
        }
        let names: Vec<(String, bool)> = providers
            .iter()
            .map(|(p, _)| (p.clone(), *p == cur.provider))
            .collect();
        row = row.child(self.picker(
            id,
            Menu::Provider,
            format!("{} ▾", cur.provider),
            names,
            &pane_id,
            &cur,
            providers,
            style,
            get,
            cx,
        ));
        let models: Vec<(String, bool)> = models_of(providers, &cur.provider, &cur.model)
            .into_iter()
            .map(|m| {
                let on = m == cur.model;
                (m, on)
            })
            .collect();
        let model_label = if cur.model.is_empty() {
            "default".to_string()
        } else {
            cur.model.clone()
        };
        row = row.child(self.picker(
            id,
            Menu::Model,
            format!("{model_label} ▾"),
            models,
            &pane_id,
            &cur,
            providers,
            style,
            get,
            cx,
        ));
        if has_effort(&cur.provider) {
            let efforts = EFFORTS
                .iter()
                .map(|e| (effort_label(*e).to_string(), *e == cur.effort))
                .collect();
            row = row.child(self.picker(
                id,
                Menu::Effort,
                format!("{} ▾", effort_label(cur.effort)),
                efforts,
                &pane_id,
                &cur,
                providers,
                style,
                get,
                cx,
            ));
        }
        row
    }

    #[allow(clippy::too_many_arguments, reason = "one row part with its menu")]
    fn picker<V: 'static>(
        &self,
        id: &'static str,
        menu: Menu,
        label: String,
        values: Vec<(String, bool)>,
        pane: &str,
        cur: &ComposerPrefs,
        providers: &[(String, Vec<String>)],
        style: &ControlStyle,
        get: fn(&mut V) -> &mut Controls,
        cx: &mut Context<V>,
    ) -> Div {
        let open = self.open == Some(menu);
        let mut wrap = div().relative().flex_none().child(
            pill(style, (id, 10 + menu as usize), label, open)
                .tooltip(ui::tip(match menu {
                    Menu::Provider => "Provider: another provider continues the session there",
                    Menu::Model => "Model of the next turns",
                    Menu::Effort => "Effort of the next turns",
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let c = get(this);
                    c.open = if c.open == Some(menu) {
                        None
                    } else {
                        Some(menu)
                    };
                    cx.notify();
                })),
        );
        if !open {
            return wrap;
        }
        let mut m = div()
            .min_w(px(140.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(style.menu)
            .text_color(style.menu_text)
            .rounded(px(style.radius.unwrap_or(8.)))
            .shadow(ui::card_shadow());
        for (i, (value, on)) in values.into_iter().enumerate() {
            let base = cur.clone();
            let pid = pane.to_string();
            let first_models: Vec<String> = match menu {
                Menu::Provider => models_of(providers, &value, ""),
                _ => Vec::new(),
            };
            let v = value.clone();
            m = m.child(
                div()
                    .id(SharedString::from(format!("{id}-mi-{menu:?}-{i}")))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(style.radius.unwrap_or(5.)))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(style.menu_hover))
                    .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(value)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut p = base.clone();
                        match menu {
                            Menu::Provider => {
                                if p.provider != v {
                                    p.provider = v.clone();
                                    p.model = first_models.first().cloned().unwrap_or_default();
                                }
                            }
                            Menu::Model => p.model = v.clone(),
                            Menu::Effort => {
                                p.effort = EFFORTS
                                    .into_iter()
                                    .find(|e| effort_label(*e) == v)
                                    .unwrap_or(Effort::High);
                            }
                        }
                        get(this).pick(&pid, p);
                        cx.notify();
                    })),
            );
        }
        wrap = wrap.child(
            deferred(
                div()
                    .absolute()
                    .bottom_full()
                    .mb(px(6.))
                    .left_0()
                    .occlude()
                    .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                        get(this).open = None;
                        cx.notify();
                    }))
                    .child(m),
            )
            .with_priority(3),
        );
        wrap
    }
}

/// The composer state of a session: the values that the daemon read from it, with the model
/// of the pane when the composer names none.
pub fn state_of(pane: &Pane) -> ComposerPrefs {
    let mut c = pane.composer.clone();
    if c.provider.is_empty() {
        c.provider.clone_from(&pane.provider);
    }
    if c.model.is_empty() {
        c.model.clone_from(&pane.model);
    }
    c
}

/// Gemini has no effort level.
pub fn has_effort(provider: &str) -> bool {
    provider != "gemini"
}

/// The models of a provider from the daemon, with the model of the session first when the
/// list does not name it (a native session can use any model).
pub fn models_of(
    providers: &[(String, Vec<String>)],
    provider: &str,
    current: &str,
) -> Vec<String> {
    let mut out: Vec<String> = providers
        .iter()
        .find(|(p, _)| p == provider)
        .map(|(_, m)| m.clone())
        .unwrap_or_default();
    if !current.is_empty() && !out.iter().any(|m| m == current) {
        out.insert(0, current.to_string());
    }
    out
}

fn mode_tip(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "Plan: the agent plans and changes no file",
        Mode::Edit => "Edit: the agent asks before it changes a file",
        Mode::Auto => "Auto: the agent changes files without a question",
    }
}

fn pill(
    style: &ControlStyle,
    id: (&'static str, usize),
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    let (bg, fg) = if on {
        (style.on, style.on_text)
    } else {
        (style.pill, style.pill_text)
    };
    let hover = style.pill_hover;
    div()
        .id(id)
        .h(px(if style.text.is_some() { 20. } else { 24. }))
        .px(px(if style.text.is_some() { 6. } else { 9. }))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(style.radius.unwrap_or(999.)))
        .whitespace_nowrap()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
        .child(label.into())
}

/// Send a message with the picked values. Another provider first hands the session off and
/// then sends the message to the new session. Image attachments go as images; the caller puts
/// other files in `text`.
pub fn submit(
    model: &Entity<Model>,
    pane: &Pane,
    text: String,
    attachments: Vec<String>,
    prefs: ComposerPrefs,
    cx: &mut App,
) {
    submit_then(model, pane, text, attachments, prefs, cx, |_, _| {});
}

/// [`submit`], and `on_new` gets the id of the new session after a handoff (a screen opens it).
pub fn submit_then(
    model: &Entity<Model>,
    pane: &Pane,
    text: String,
    attachments: Vec<String>,
    prefs: ComposerPrefs,
    cx: &mut App,
    on_new: impl FnOnce(String, &mut App) + 'static,
) {
    if prefs.provider == pane.provider {
        model.read(cx).send::<method::SessionSend>(
            SendParams {
                id: pane.id.clone(),
                text,
                subagent: None,
                attachments,
                composer: Some(prefs),
            },
            cx,
        );
        return;
    }
    let handoff = model.read(cx).call_notify::<method::SessionHandoff>(
        HandoffParams {
            id: pane.id.clone(),
            provider: prefs.provider.clone(),
            model: (!prefs.model.is_empty()).then(|| prefs.model.clone()),
        },
        cx,
    );
    let model = model.clone();
    cx.spawn(async move |cx| {
        if let Some(created) = handoff.await {
            cx.update(|cx| {
                let id = created.id.clone();
                model.read(cx).send::<method::SessionSend>(
                    SendParams {
                        id: created.id,
                        text,
                        subagent: None,
                        attachments,
                        composer: Some(prefs),
                    },
                    cx,
                );
                on_new(id, cx);
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Pane {
        let mut p = aivory_protocol::demo::Demo::new().war_room.sessions[0].clone();
        p.provider = "claude".into();
        p.model = "claude-opus-5-5".into();
        p.composer = ComposerPrefs {
            mode: Mode::Plan,
            provider: "claude".into(),
            model: String::new(),
            effort: Effort::Medium,
        };
        p
    }

    #[test]
    fn the_row_starts_from_the_session_and_keeps_a_pick_until_the_session_has_it() {
        let p = pane();
        let mut c = Controls::default();
        let start = c.current(&p);
        assert_eq!(start.mode, Mode::Plan);
        assert_eq!(
            start.model, "claude-opus-5-5",
            "the pane model fills an empty one"
        );
        let mut picked = start.clone();
        picked.mode = Mode::Auto;
        c.pick(&p.id, picked.clone());
        assert_eq!(c.current(&p).mode, Mode::Auto);
        c.sync(&p);
        assert_eq!(
            c.current(&p).mode,
            Mode::Auto,
            "the session has not changed yet"
        );
        let mut after = p.clone();
        after.composer = picked;
        c.sync(&after);
        assert!(c.picked.is_none());
        let mut other = p.clone();
        other.id = "other".into();
        c.pick(&p.id, start);
        assert_eq!(
            c.current(&other).mode,
            Mode::Plan,
            "a pick belongs to one pane"
        );
    }

    #[test]
    fn the_model_list_names_the_model_of_the_session() {
        let providers = vec![("claude".to_string(), vec!["opus".into(), "sonnet".into()])];
        assert_eq!(
            models_of(&providers, "claude", "claude-opus-5-5"),
            ["claude-opus-5-5", "opus", "sonnet"]
        );
        assert_eq!(models_of(&providers, "claude", "opus"), ["opus", "sonnet"]);
        assert!(models_of(&providers, "codex", "").is_empty());
        assert!(!has_effort("gemini"));
    }
}
