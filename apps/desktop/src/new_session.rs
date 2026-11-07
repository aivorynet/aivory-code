//! The New session dialog of all looks (board `docs/design/looks/newsession/`): a task, a
//! folder (recent first, or a new folder), the agent with model and effort. Ctrl+Enter sends
//! `session/dispatch`: a free slot starts a session, else the task goes to the queue. The
//! dialog stays open for the next task; the X button closes it.

use aivory_protocol::actions::RepoAddParams;
use aivory_protocol::rpc::{DispatchParams, method};
use aivory_protocol::{Effort, Look, SessionState, WarRoom};
use gpui::prelude::*;
use gpui::{
    Context, Entity, EventEmitter, FontWeight, KeyDownEvent, Rgba, SharedString, Subscription,
    Window, div, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, TextareaState};

use crate::compose;
use crate::model::Model;
use crate::theme::tk;
use crate::ui;
use crate::warroom::pane;

/// The agents of the dialog, in board order.
const AGENTS: [&str; 3] = ["claude", "codex", "gemini"];
const EFFORTS: [Effort; 4] = [Effort::Low, Effort::Medium, Effort::High, Effort::Max];
/// The recent folders on show; more are on the Repos screen.
const RECENT: usize = 4;

pub enum DialogEvent {
    Close,
}

pub struct NewSessionDialog {
    model: Entity<Model>,
    input: Entity<TextareaState>,
    tools: compose::Tools,
    folder: Option<String>,
    agent: &'static str,
    model_name: Option<String>,
    effort: Effort,
    busy: bool,
    adding: bool,
    /// The result of the last start: "Queued …" or "Started …", or an error.
    note: Option<(String, bool)>,
    _events: Subscription,
}

impl EventEmitter<DialogEvent> for NewSessionDialog {}

impl NewSessionDialog {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What should the agent do?")
                .auto_grow(6, 6)
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = event
            {
                this.start(window, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        Self {
            model,
            input,
            tools: compose::Tools::default(),
            folder: None,
            agent: AGENTS[0],
            model_name: None,
            effort: Effort::High,
            busy: false,
            adding: false,
            note: None,
            _events: events,
        }
    }

    /// The codebases, the one with the newest session first.
    fn folders(war: &WarRoom) -> Vec<(String, String)> {
        let mut list: Vec<(i64, &str, &str)> = war
            .codebases
            .iter()
            .map(|c| {
                let newest = war
                    .sessions
                    .iter()
                    .filter(|s| s.codebase == c.id)
                    .map(|s| s.since)
                    .max()
                    .unwrap_or(0);
                (newest, c.id.as_str(), c.path.as_str())
            })
            .collect();
        list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        list.into_iter()
            .take(RECENT)
            .map(|(_, id, path)| (id.to_string(), tidy_path(path)))
            .collect()
    }

    /// Free slots for a new session in `codebase` (0 = it waits in the queue), and the
    /// running count with the limit that is full.
    fn slots(war: &WarRoom, codebase: &str) -> (u32, u32, u32) {
        let running = |s: &&aivory_protocol::Pane| {
            matches!(s.state, SessionState::Working | SessionState::NeedsYou)
        };
        let all = war.sessions.iter().filter(running).count() as u32;
        let own = war
            .sessions
            .iter()
            .filter(running)
            .filter(|s| s.codebase == codebase)
            .count() as u32;
        let ap = &war.autopilot;
        let free_all = ap.max_running.saturating_sub(all);
        let free_own = ap.max_per_codebase.saturating_sub(own);
        if free_all < free_own {
            (free_all, all, ap.max_running)
        } else {
            (free_own, own, ap.max_per_codebase)
        }
    }

    fn models(&self) -> Vec<String> {
        pane::models(self.agent)
            .iter()
            .map(|m| m.to_string())
            .collect()
    }

    fn current_model(&self) -> String {
        let models = self.models();
        match &self.model_name {
            Some(m) if models.contains(m) => m.clone(),
            _ => models.first().cloned().unwrap_or_default(),
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.input.read(cx).value().trim().to_string();
        if prompt.is_empty() || self.busy {
            return;
        }
        let Some(folder) = self.folder.clone() else {
            return;
        };
        self.busy = true;
        let params = DispatchParams {
            codebase: Some(folder.clone()),
            part: None,
            agent: Some(self.agent.to_string()),
            model: Some(self.current_model()),
            effort: Some(self.effort),
            prompt: prompt.clone(),
        };
        let place = self.model.read(cx).war.as_ref().map_or(1, |w| w.queued + 1);
        let call = self
            .model
            .read(cx)
            .call::<method::SessionDispatch>(params, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let title: String = prompt.chars().take(60).collect();
                this.note = Some(match result {
                    Ok(done) if done.session.is_some() => {
                        (format!("Started in {folder}: {title}"), false)
                    }
                    Ok(_) => (format!("Queued in {folder}, place {place}: {title}"), false),
                    Err(e) => (format!("{e:#}"), true),
                });
                if this.note.as_ref().is_some_and(|n| !n.1) {
                    this.tools.sent();
                    this.input.update(cx, |s, cx| s.clean(window, cx));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn new_folder(&mut self, cx: &mut Context<Self>) {
        crate::forms::pick_folder(cx, |this: &mut Self, path, cx| {
            this.adding = true;
            cx.notify();
            let call = this.model.read(cx).call::<method::ReposAdd>(
                RepoAddParams {
                    path: path.to_string_lossy().into_owned(),
                },
                cx,
            );
            cx.spawn(async move |this, cx| {
                let added = call.await;
                let _ = this.update(cx, |this, cx| {
                    this.adding = false;
                    match added {
                        Ok(cb) => this.folder = Some(cb.id),
                        Err(e) => this.note = Some((format!("{e:#}"), true)),
                    }
                    cx.notify();
                });
            })
            .detach();
        });
    }

    fn dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stop) = self.tools.start_listening() else {
            self.tools.stop_listening();
            cx.notify();
            return;
        };
        cx.notify();
        compose::dictate(
            |this: &mut Self| Some(&mut this.tools),
            stop,
            self.input.clone(),
            window,
            cx,
        );
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let m = &ev.keystroke.modifiers;
        if key == "escape" {
            cx.emit(DialogEvent::Close);
            cx.stop_propagation();
        } else if key == "enter" && (m.control || m.platform) {
            self.start(window, cx);
            cx.stop_propagation();
        }
    }
}

/// `C:\Users\x\work\billing` → `~/work/billing`.
fn tidy_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
        .replace('\\', "/");
    if !home.is_empty()
        && let Some(rest) = p.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    p
}

/// The colors and sizes of one look (the boards in `docs/design/looks/newsession/`).
struct Style {
    scrim: u32,
    panel: u32,
    edge: Option<u32>,
    radius: f32,
    /// Bridge: the orange frame on the left and on top.
    frame: Option<u32>,
    /// Navigator: the blue title bar.
    title_bar: Option<u32>,
    label: u32,
    text: u32,
    bright: u32,
    muted: u32,
    field: u32,
    field_edge: Option<u32>,
    field_radius: f32,
    sel_bg: u32,
    sel_edge: u32,
    sel_text: u32,
    chip_on_bg: u32,
    chip_on_text: u32,
    chip_off_bg: u32,
    chip_off_edge: Option<u32>,
    chip_radius: f32,
    free: u32,
    primary: u32,
    primary_text: u32,
    s_title: f32,
    s_entry: f32,
    s_label: f32,
    caps: bool,
}

fn style() -> Style {
    let t = tk();
    match t.look {
        Look::Bridge => Style {
            scrim: 0x000000C7,
            panel: 0x000000,
            edge: None,
            radius: 0.,
            frame: Some(0xFF9C00),
            title_bar: None,
            label: 0xFF9C00,
            text: 0xE9DCCB,
            bright: 0xF3E3CF,
            muted: 0xA88AA8,
            field: 0x120D14,
            field_edge: None,
            field_radius: 18.,
            sel_bg: 0x160F12,
            sel_edge: 0xFF9C00,
            sel_text: 0xF3E3CF,
            chip_on_bg: 0xFFCC99,
            chip_on_text: 0x000000,
            chip_off_bg: 0x2A2130,
            chip_off_edge: None,
            chip_radius: 20.,
            free: 0xA88AA8,
            primary: 0xFF9C00,
            primary_text: 0x000000,
            s_title: 28.,
            s_entry: 18.,
            s_label: 14.,
            caps: true,
        },
        Look::Operator => Style {
            scrim: 0x010302CC,
            panel: 0x07120D,
            edge: Some(0x2F6B4A),
            radius: 0.,
            frame: None,
            title_bar: Some(0x0B1A12),
            label: 0x6DB596,
            text: 0x8FD3AE,
            bright: 0xCFFFE0,
            muted: 0x6DB596,
            field: 0x010A04,
            field_edge: Some(0x1D3A2A),
            field_radius: 0.,
            sel_bg: 0x0F2A1C,
            sel_edge: 0x9BFFC0,
            sel_text: 0xCFFFE0,
            chip_on_bg: 0x9BFFC0,
            chip_on_text: 0x04140B,
            chip_off_bg: 0x07120D,
            chip_off_edge: Some(0x1D3A2A),
            chip_radius: 0.,
            free: 0x6DB596,
            primary: 0x9BFFC0,
            primary_text: 0x04140B,
            s_title: 14.,
            s_entry: 14.,
            s_label: 12.,
            caps: false,
        },
        Look::Neon => Style {
            scrim: 0x050609CC,
            panel: 0x0E1016,
            edge: Some(0x2A2F3D),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0xA9AEBB,
            text: 0xD9DCE4,
            bright: 0xE6E8EE,
            muted: 0x8A90A0,
            field: 0x0B0C10,
            field_edge: Some(0x2A2F3D),
            field_radius: 0.,
            sel_bg: 0x15160F,
            sel_edge: 0xE8DA3A,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0x0E1016,
            chip_on_text: 0xE6E8EE,
            chip_off_bg: 0x0E1016,
            chip_off_edge: Some(0x2A2F3D),
            chip_radius: 0.,
            free: 0x4FB8C4,
            primary: 0xE8DA3A,
            primary_text: 0x0B0C10,
            s_title: 44.,
            s_entry: 18.,
            s_label: 14.,
            caps: true,
        },
        Look::Navigator => Style {
            scrim: 0x141E2D73,
            panel: 0xC6C6C6,
            edge: Some(0x6B6B6B),
            radius: 0.,
            frame: None,
            title_bar: Some(0x5A7299),
            label: 0x111111,
            text: 0x111111,
            bright: 0x111111,
            muted: 0x444444,
            field: 0xFFFFFF,
            field_edge: Some(0x6B6B6B),
            field_radius: 0.,
            sel_bg: 0x3D5A8A,
            sel_edge: 0x3D5A8A,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0xB4B4B4,
            chip_on_text: 0x111111,
            chip_off_bg: 0xC6C6C6,
            chip_off_edge: Some(0x6B6B6B),
            chip_radius: 0.,
            free: 0x444444,
            primary: 0xC6C6C6,
            primary_text: 0x111111,
            s_title: 15.,
            s_entry: 15.,
            s_label: 13.,
            caps: false,
        },
        Look::Holo => Style {
            scrim: 0x02060EB8,
            panel: 0x0A182E,
            edge: Some(0x3A5A7A),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0x93AECB,
            text: 0xDCEAF8,
            bright: 0xFFFFFF,
            muted: 0x8AA6C4,
            field: 0x061024,
            field_edge: Some(0x34506E),
            field_radius: 0.,
            sel_bg: 0x10243C,
            sel_edge: 0x7FD8FF,
            sel_text: 0xFFFFFF,
            chip_on_bg: 0x10283F,
            chip_on_text: 0xE6F2FF,
            chip_off_bg: 0x0A182E,
            chip_off_edge: Some(0x34506E),
            chip_radius: 0.,
            free: 0x7FD8FF,
            primary: 0x7FD8FF,
            primary_text: 0x04101E,
            s_title: 40.,
            s_entry: 18.,
            s_label: 14.,
            caps: true,
        },
        Look::Tactical => Style {
            scrim: 0x000000A8,
            panel: 0x0B0B0B,
            edge: Some(0x3A3A3A),
            radius: 0.,
            frame: None,
            title_bar: None,
            label: 0xA8A8A8,
            text: 0xEDEDED,
            bright: 0xEDEDED,
            muted: 0x9A9A9A,
            field: 0x000000,
            field_edge: Some(0x333333),
            field_radius: 0.,
            sel_bg: 0x161616,
            sel_edge: 0x8FD14F,
            sel_text: 0xEDEDED,
            chip_on_bg: 0x8FD14F,
            chip_on_text: 0x0B0B0B,
            chip_off_bg: 0x0B0B0B,
            chip_off_edge: Some(0x3A3A3A),
            chip_radius: 0.,
            free: 0x8FD14F,
            primary: 0x8FD14F,
            primary_text: 0x0B0B0B,
            s_title: 40.,
            s_entry: 16.,
            s_label: 12.,
            caps: true,
        },
        Look::Command => Style {
            scrim: 0x000000A0,
            panel: 0x1E2124,
            edge: Some(0x6A7076),
            radius: 0.,
            frame: None,
            title_bar: Some(0x0A0C0A),
            label: 0xA8AE96,
            text: 0xE6E8DA,
            bright: 0xF2F2E6,
            muted: 0x7E8A70,
            field: 0x050605,
            field_edge: Some(0x3A4038),
            field_radius: 0.,
            sel_bg: 0x1E2A16,
            sel_edge: 0xFFC21A,
            sel_text: 0xF2F2E6,
            chip_on_bg: 0x5EA634,
            chip_on_text: 0x0B0D07,
            chip_off_bg: 0x3C4247,
            chip_off_edge: Some(0x6E767C),
            chip_radius: 0.,
            free: 0x8FE05A,
            primary: 0x5EA634,
            primary_text: 0x0B0D07,
            s_title: 30.,
            s_entry: 21.,
            s_label: 17.,
            caps: true,
        },
        _ => Style {
            scrim: if t.night { 0x000000A0 } else { 0x1A1C2266 },
            panel: t.card,
            edge: Some(t.edge),
            radius: f32::from(t.r_card),
            frame: None,
            title_bar: None,
            label: t.ink_2,
            text: t.ink,
            bright: t.ink,
            muted: t.ink_3,
            field: t.page,
            field_edge: Some(t.field_line),
            field_radius: f32::from(t.r_button),
            sel_bg: t.track,
            sel_edge: t.ink,
            sel_text: t.ink,
            chip_on_bg: t.ink,
            chip_on_text: t.card,
            chip_off_bg: t.card,
            chip_off_edge: Some(t.edge),
            chip_radius: 999.,
            free: t.ok_text,
            primary: t.primary,
            primary_text: 0xFFFFFF,
            s_title: 22.,
            s_entry: 15.,
            s_label: 12.,
            caps: false,
        },
    }
}

fn c(v: u32) -> Rgba {
    if v > 0xFFFFFF { rgba(v) } else { rgb(v) }
}

impl Render for NewSessionDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = style();
        let caps = |s: &str| -> SharedString {
            if st.caps {
                s.to_uppercase().into()
            } else {
                s.to_string().into()
            }
        };
        let (folders, slots, queued) = match self.model.read(cx).war.as_ref() {
            Some(war) => {
                let folders = Self::folders(war);
                if self.folder.is_none() {
                    self.folder = folders.first().map(|f| f.0.clone());
                }
                let slots: Vec<(u32, u32, u32)> =
                    folders.iter().map(|f| Self::slots(war, &f.0)).collect();
                let sel = self.folder.as_ref().map(|id| Self::slots(war, id));
                (folders, slots, (sel, war.queued + 1))
            }
            None => (Vec::new(), Vec::new(), (None, 1)),
        };
        let folder = self.folder.clone().unwrap_or_default();
        let free_now = queued.0.is_none_or(|s| s.0 > 0);
        let label = |text: &str| {
            div()
                .text_size(px(st.s_label))
                .text_color(c(st.label))
                .font_weight(FontWeight::MEDIUM)
                .child(caps(text))
        };
        let navigator = tk().look == Look::Navigator;

        // ---- the task ----
        let voice = compose::VOICE_AVAILABLE;
        let listening = self.tools.listening.is_some();
        let task_box = div()
            .h(px(220.))
            .flex()
            .flex_col()
            .bg(c(st.field))
            .rounded(px(st.field_radius))
            .when_some(st.field_edge, |d, e| d.border_1().border_color(c(e)))
            .px(px(18.))
            .pt(px(14.))
            .pb(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .text_size(px(st.s_entry))
                    .line_height(relative(1.5))
                    .text_color(c(st.bright))
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(st.s_label))
                            .text_color(c(st.muted))
                            .child(caps("Enter new line")),
                    )
                    .when(voice, |d| {
                        d.child(
                            div()
                                .id("ns-mic")
                                .size(px(36.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(if st.chip_radius > 0. { 18. } else { 0. }))
                                .bg(c(if listening { st.sel_bg } else { st.chip_off_bg }))
                                .when_some(st.chip_off_edge, |d, e| d.border_1().border_color(c(e)))
                                .cursor_pointer()
                                .tooltip(ui::tip(if listening {
                                    "Stop the recording"
                                } else {
                                    "Record audio"
                                }))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.dictate(window, cx)),
                                )
                                .child(ui::icon("icons/mic.svg", 18., c(st.text))),
                        )
                    }),
            );

        // ---- the agent, model and effort ----
        let mut agents = div().flex().gap(px(8.));
        for (i, a) in AGENTS.iter().enumerate() {
            let on = self.agent == *a;
            agents = agents.child(
                div()
                    .id(("ns-agent", i))
                    .h(px(40.))
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .rounded(px(st.chip_radius))
                    .text_size(px(st.s_entry))
                    .bg(c(if on { st.chip_on_bg } else { st.chip_off_bg }))
                    .text_color(c(if on { st.chip_on_text } else { st.text }))
                    .when_some(st.chip_off_edge, |d, e| {
                        d.border_1()
                            .border_color(c(if on { st.sel_edge } else { e }))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.agent = AGENTS[i];
                        this.model_name = None;
                        cx.notify();
                    }))
                    .child(caps(a)),
            );
        }
        let model_name = self.current_model();
        let small = |id: &'static str, name: &str, value: SharedString| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_size(px(st.s_label))
                .child(div().text_color(c(st.muted)).child(caps(name)))
                .child(div().text_color(c(st.bright)).child(value))
                .child(ui::icon("icons/chevron-down.svg", 10., c(st.text)))
        };
        let pickers = div()
            .flex()
            .gap(px(28.))
            .child(
                small("ns-model", "Model", model_name.into()).on_click(cx.listener(
                    |this, _, _, cx| {
                        let models = this.models();
                        let now = this.current_model();
                        let i = models.iter().position(|m| *m == now).unwrap_or(0);
                        this.model_name = models.get((i + 1) % models.len().max(1)).cloned();
                        cx.notify();
                    },
                )),
            )
            .child(
                small("ns-effort", "Effort", caps(pane_effort(self.effort))).on_click(cx.listener(
                    |this, _, _, cx| {
                        let i = EFFORTS.iter().position(|e| *e == this.effort).unwrap_or(2);
                        this.effort = EFFORTS[(i + 1) % EFFORTS.len()];
                        cx.notify();
                    },
                )),
            );

        // ---- the folders ----
        let mut list = div().flex().flex_col();
        for (i, (id, path)) in folders.iter().enumerate() {
            let on = Some(id) == self.folder.as_ref();
            let (free, run, max) = slots[i];
            let slot_text = if free > 0 {
                format!("{free} free")
            } else {
                format!("{run} of {max} run")
            };
            let value = id.clone();
            list = list.child(
                div()
                    .id(("ns-folder", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(10.))
                    .pl(px(if on { 14. } else { 16. }))
                    .pr(px(16.))
                    .when(i > 0 && !on, |d| d.border_t_1().border_color(c(st.sel_bg)))
                    .when(on, |d| {
                        d.bg(c(st.sel_bg)).border_l_2().border_color(c(st.sel_edge))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.folder = Some(value.clone());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .text_size(px(st.s_entry))
                                    .text_color(c(if on { st.sel_text } else { st.text }))
                                    .child(id.clone()),
                            )
                            .child(
                                div()
                                    .font_family(tk().mono)
                                    .text_size(px(st.s_label))
                                    .text_color(c(if on && navigator {
                                        0xE4EAF4
                                    } else {
                                        st.muted
                                    }))
                                    .truncate()
                                    .child(path.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(st.s_label))
                            .text_color(c(if free > 0 {
                                if on && navigator { 0xE4EAF4 } else { st.free }
                            } else if on {
                                st.sel_text
                            } else {
                                st.bright
                            }))
                            .child(caps(&slot_text)),
                    ),
            );
        }
        list = list.child(
            div()
                .id("ns-new-folder")
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(12.))
                .px(px(16.))
                .cursor_pointer()
                .text_size(px(st.s_entry))
                .text_color(c(st.text))
                .on_click(cx.listener(|this, _, _, cx| this.new_folder(cx)))
                .child("+")
                .child(caps(if self.adding {
                    "Adding the folder…"
                } else {
                    "Start in a new folder"
                })),
        );

        // ---- the footer ----
        let (line1, line2) = if free_now {
            (
                format!("A slot is free in {folder}."),
                "The session starts now.".to_string(),
            )
        } else {
            (
                format!("No free slot in {folder}."),
                format!("The task waits in the queue, place {}.", queued.1),
            )
        };
        let (line1, line2) = match &self.note {
            Some((text, true)) => (text.clone(), line2),
            Some((text, false)) => (line1, text.clone()),
            None => (line1, line2),
        };
        let error = self.note.as_ref().is_some_and(|n| n.1);
        let footer = div()
            .flex()
            .items_center()
            .gap(px(20.))
            .pt(px(18.))
            .border_t_1()
            .border_color(c(st.sel_bg))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_size(px(st.s_entry))
                            .text_color(c(if error { tk().q_text } else { st.bright }))
                            .truncate()
                            .child(caps(&line1)),
                    )
                    .child(
                        div()
                            .text_size(px(st.s_label))
                            .text_color(c(st.muted))
                            .truncate()
                            .child(caps(&line2)),
                    ),
            )
            .child(
                div()
                    .id("ns-start")
                    .h(px(52.))
                    .px(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(if st.chip_radius > 0. { 26. } else { 0. }))
                    .bg(c(st.primary))
                    .text_color(c(st.primary_text))
                    .when(navigator, |d| ui::bevel_edges(true, d.border_2()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(st.s_entry))
                    .cursor_pointer()
                    .when(self.busy, |d| d.opacity(0.6))
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                    .child(caps(if free_now { "Start" } else { "Queue task" }))
                    .child(
                        div()
                            .text_size(px(st.s_label))
                            .font_weight(FontWeight::NORMAL)
                            .child(caps("Ctrl Enter")),
                    ),
            );

        // ---- the panel ----
        let head = div()
            .flex()
            .items_center()
            .child(
                div()
                    .font_family(tk().display_heading)
                    .text_size(px(st.s_title))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(if st.frame.is_some() {
                        st.label
                    } else {
                        st.bright
                    }))
                    .child(caps(if tk().look == Look::Command {
                        "New unit"
                    } else {
                        "New session"
                    })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("ns-close")
                    .cursor_pointer()
                    .text_size(px(st.s_label))
                    .text_color(c(st.muted))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)))
                    .px(px(10.))
                    .py(px(6.))
                    .child(caps("X")),
            );
        let body = div()
            .flex()
            .gap(px(40.))
            .child(
                div()
                    .w(px(540.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Task"))
                    .child(task_box)
                    .child(div().h(px(16.)))
                    .child(label("Agent"))
                    .child(agents)
                    .child(div().h(px(6.)))
                    .child(pickers),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label("Folder · recent first"))
                    .child(list),
            );
        let panel = div()
            .id("ns-panel")
            .w(px(1040.))
            .relative()
            .bg(c(st.panel))
            .rounded(px(st.radius))
            .when_some(st.edge, |d, e| d.border_1().border_color(c(e)))
            .when(navigator, |d| ui::bevel_edges(true, d.border_2()))
            .font_family(tk().sans)
            .text_color(c(st.text))
            .on_click(|_, _, cx| cx.stop_propagation())
            .when_some(st.title_bar, |d, bar| {
                d.child(
                    div()
                        .h(px(32.))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .bg(c(bar))
                        .text_size(px(st.s_label))
                        .text_color(c(if navigator { 0xFFFFFF } else { st.bright }))
                        .font_weight(FontWeight::BOLD)
                        .child(caps(if tk().look == Look::Command {
                            "New unit"
                        } else {
                            "New session"
                        })),
                )
            })
            .when_some(st.frame, |d, f| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(28.))
                        .bg(c(f))
                        .rounded_l(px(28.)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(660.))
                        .h(px(24.))
                        .bg(c(f))
                        .rounded_tl(px(28.)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(12.))
                        .bg(rgb(0x9999FF))
                        .rounded_bl(px(28.)),
                )
            })
            .child(
                div()
                    .pl(px(if st.frame.is_some() { 64. } else { 40. }))
                    .pr(px(40.))
                    .pt(px(if st.frame.is_some() { 44. } else { 32. }))
                    .pb(px(if st.frame.is_some() { 40. } else { 32. }))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .when(st.title_bar.is_none(), |d| d.child(head))
                    .when(st.title_bar.is_some(), |d| {
                        d.child(
                            div().flex().justify_end().child(
                                div()
                                    .id("ns-close")
                                    .cursor_pointer()
                                    .text_size(px(st.s_label))
                                    .text_color(c(st.muted))
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
                                    )
                                    .px(px(10.))
                                    .py(px(6.))
                                    .child(caps("X")),
                            ),
                        )
                    })
                    .child(body)
                    .child(footer),
            );
        let _ = window;
        div()
            .id("new-session-dialog")
            .absolute()
            .inset_0()
            .bg(rgba(st.scrim))
            .flex()
            .items_center()
            .justify_center()
            .on_key_down(cx.listener(Self::on_key))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)))
            .child(panel)
    }
}

fn pane_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_folder_reads_as_tilde() {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        if home.is_empty() {
            return;
        }
        let path = format!("{home}/work/billing");
        assert_eq!(tidy_path(&path), "~/work/billing");
        assert_eq!(tidy_path("/srv/x"), "/srv/x");
    }

    #[test]
    fn every_look_has_four_text_sizes_or_fewer() {
        let st = style();
        let mut sizes = vec![st.s_title, st.s_entry, st.s_label];
        sizes.dedup();
        assert!(sizes.len() <= 4);
        assert!(st.s_label >= 12.);
    }
}
