//! The Repos screen (`Repos.dc.html`): the codebases of the daemons, the parts of each codebase
//! (`.aivory/map.toml`), the sessions that work in each part, the files they claim, and the
//! recent activity. Left the repo list, right the detail with the tabs Parts, Files, Activity.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use aivory_protocol::actions::{AddPartParams, ProjectNewParams, ProjectTemplate, RepoAddParams};
use aivory_protocol::layout::{self, FlowBox, FlowColumn, FlowItem, Sankey};
use aivory_protocol::rpc::{CodebaseRef, Empty, NewSessionParams, method};
use aivory_protocol::{
    ActivityRow, Codebase, ComposerPrefs, Effort, FileNode, MINUTE_MS, MapFile, Mode, PartStats,
    RepoDetail, RepoList, SessionState, UnixMs,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, Div, ElementId, Entity, EventEmitter, FontWeight,
    PathBuilder, Pixels, SharedString, Stateful, Subscription, Window, canvas, deferred, div, fill,
    point, px, relative, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};

use crate::forms;
use crate::model::Model;
use crate::radar::{self, RadarEvent, RadarView};
use crate::theme::*;
use crate::ui;

#[derive(Debug, Clone)]
pub enum ReposEvent {
    /// Show the codebase in the war room. The shell opens the war room; it does not scroll to the
    /// codebase yet, because the war room has no public reveal.
    #[allow(
        dead_code,
        reason = "the shell reads the id when the war room can reveal a group"
    )]
    ShowInWarRoom(String),
    /// A click on a file: show it in the Lens.
    OpenFile { codebase: String, path: String },
    /// "Open both" of the radar: the deep dives of these sessions.
    OpenSessions(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Parts,
    Files,
    Activity,
    Radar,
}

/// The tabs, in design order.
const TABS: [(Tab, &str); 4] = [
    (Tab::Parts, "Parts"),
    (Tab::Files, "Files"),
    (Tab::Activity, "Activity"),
    (Tab::Radar, "Radar"),
];

const LIST_W: f32 = 340.;
/// Under this window width the map panel goes below the parts table (`@media (max-width:1200px)`).
const SPLIT_AT: f32 = 1200.;
const MAP_W: f32 = 380.;

// Surfaces that no other screen shares.
fn tab_track() -> u32 {
    crate::theme::pick(0xECEEF2, tk().track)
}
fn button_border() -> u32 {
    tk().edge
}
fn button_border_hover() -> u32 {
    tk().muted
}
fn row_hover() -> u32 {
    crate::theme::pick(0xF8F9FB, tk().menu_hover)
}
const ROW_LINE: u32 = 0x0D0D0E0F;
const SELECT_RING: u32 = 0x3574F01F;
fn line_no() -> u32 {
    crate::theme::pick(0xA0A7B5, tk().quiet)
}
fn claim_bg() -> u32 {
    crate::theme::pick(0xEEF2FF, tk().work_bg)
}
fn added() -> u32 {
    tk().ok_text
}
const SYNTAX_COMMENT: u32 = 0x8A93A6;
const SYNTAX_KEY: u32 = 0x4B5BDB;
const SYNTAX_STRING: u32 = 0x15803D;
const SYNTAX_HEADER: u32 = 0x7C3AED;

/// A calm note after a call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Note {
    /// Beside the buttons of the parts table.
    Parts(SharedString),
}

/// The form that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Popup {
    None,
    NewProject,
    AddPart,
}

/// The templates of a new project, in menu order.
const TEMPLATES: [(ProjectTemplate, &str); 4] = [
    (ProjectTemplate::Empty, "Empty"),
    (ProjectTemplate::Rust, "Rust"),
    (ProjectTemplate::Node, "Node"),
    (ProjectTemplate::Python, "Python"),
];

/// The fields of the New project form.
struct ProjectForm {
    name: Option<Entity<InputState>>,
    parent: Option<PathBuf>,
    template: ProjectTemplate,
    error: Option<String>,
    busy: bool,
    _events: Vec<Subscription>,
}

/// The fields of the Add a part form.
#[derive(Default)]
struct PartForm {
    name: Option<Entity<InputState>>,
    paths: Option<Entity<InputState>>,
    error: Option<String>,
    busy: bool,
    _events: Vec<Subscription>,
}

/// The call of New project, or the first thing that is wrong with the input.
fn project_request(
    name: &str,
    parent: Option<&Path>,
    template: ProjectTemplate,
) -> Result<ProjectNewParams, String> {
    let name = forms::project_name(name)?;
    let parent = parent.ok_or_else(|| "Choose the parent folder.".to_string())?;
    Ok(ProjectNewParams {
        name,
        parent: parent.to_string_lossy().into_owned(),
        template: Some(template),
    })
}

/// The call of Add a part, or the first thing that is wrong with the input.
fn part_request(codebase: &str, name: &str, paths: &str) -> Result<AddPartParams, String> {
    Ok(AddPartParams {
        codebase: codebase.to_string(),
        name: forms::part_name(name)?,
        paths: forms::globs(paths)?,
    })
}

/// A live session as the screen needs it.
struct Live {
    id: String,
    codebase: String,
    state: SessionState,
}

pub struct ReposScreen {
    model: Entity<Model>,
    repos: Option<Rc<RepoList>>,
    /// The pick of the user. [`resolve_selection`] decides what shows.
    selected: Option<String>,
    tab: Tab,
    show_map: bool,
    details: HashMap<String, Rc<RepoDetail>>,
    maps: HashMap<String, Rc<MapFile>>,
    note: Option<Note>,
    /// The form that is open, and a form closed by a click outside (so the click on its own
    /// button does not open it again).
    popup: Popup,
    closed: Option<(Popup, Instant)>,
    project: ProjectForm,
    part: PartForm,
    /// The codebases that the user added with Add a repo: only these have a Remove button.
    pinned: HashSet<String>,
    /// A `repos/add` call is in flight.
    adding: bool,
    /// A `session/new` call for the mapping session is in flight.
    remapping: bool,
    /// A refresh is in flight.
    refreshing: bool,
    /// A delta came while a refresh was in flight.
    dirty: bool,
    /// The `(epoch, cursor)` of the war room that the last refresh answered.
    seen: Option<(u64, u64)>,
    /// The Radar tab: the sessions graph and the hits. It also holds the live claims.
    radar: Entity<RadarView>,
    _observe: Subscription,
    _radar: Vec<Subscription>,
}

impl EventEmitter<ReposEvent> for ReposScreen {}

impl ReposScreen {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |this, model, cx| {
            let key = model.read(cx).war.as_ref().map(|w| (w.epoch, w.cursor));
            if key.is_some() && key != this.seen {
                this.seen = key;
                this.refresh(cx);
            }
        });
        let radar = cx.new(|_| RadarView::new(model.clone()));
        let radar_subs = vec![
            cx.subscribe(&radar, |_, _, event: &RadarEvent, cx| match event {
                RadarEvent::OpenSessions(ids) => cx.emit(ReposEvent::OpenSessions(ids.clone())),
            }),
            cx.observe(&radar, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            model,
            repos: None,
            selected: None,
            tab: Tab::Parts,
            show_map: false,
            details: HashMap::new(),
            maps: HashMap::new(),
            note: None,
            popup: Popup::None,
            closed: None,
            project: ProjectForm {
                name: None,
                parent: None,
                template: ProjectTemplate::Empty,
                error: None,
                busy: false,
                _events: Vec::new(),
            },
            part: PartForm::default(),
            pinned: HashSet::new(),
            adding: false,
            remapping: false,
            refreshing: false,
            dirty: false,
            seen: None,
            radar,
            _observe: observe,
            _radar: radar_subs,
        };
        this.refresh(cx);
        this
    }

    /// The id of the codebase on screen.
    fn selected_id(&self) -> Option<String> {
        let repos = self.repos.as_ref()?;
        resolve_selection(&repos.codebases, self.selected.as_deref()).map(str::to_string)
    }

    /// Load the list, the detail of the selected codebase and, when it is open, its map. One
    /// refresh at a time: a delta that comes meanwhile asks for one more.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.dirty = true;
            return;
        }
        self.refreshing = true;
        let list = self.model.read(cx).call::<method::ReposList>(Empty {}, cx);
        let settings = self
            .model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            let list = list.await;
            let pinned = settings.await.ok().map(|view| {
                view.settings
                    .pinned
                    .iter()
                    .map(|p| p.id.clone())
                    .collect::<HashSet<_>>()
            });
            let next = this
                .update(cx, |this, cx| {
                    match list {
                        Ok(list) => this.repos = Some(Rc::new(list)),
                        Err(e) => eprintln!("aivory: repos/list failed: {e:#}"),
                    }
                    if let Some(pinned) = pinned {
                        this.pinned = pinned;
                    }
                    let id = this.selected_id()?;
                    let model = this.model.read(cx);
                    let detail = model.call::<method::ReposGet>(CodebaseRef { id: id.clone() }, cx);
                    let map = this.show_map.then(|| {
                        model.call::<method::ReposMap>(CodebaseRef { id: id.clone() }, cx)
                    });
                    Some((id, detail, map))
                })
                .ok()
                .flatten();
            let loaded = match next {
                Some((id, detail, map)) => {
                    let detail = detail.await;
                    let map = match map {
                        Some(map) => Some(map.await),
                        None => None,
                    };
                    Some((id, detail, map))
                }
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                if let Some((id, ..)) = &loaded {
                    let id = id.clone();
                    this.radar.update(cx, |r, cx| {
                        r.set_codebase(&id, cx);
                        r.refresh(cx);
                    });
                }
                if let Some((id, detail, map)) = loaded {
                    match detail {
                        Ok(detail) => {
                            this.details.insert(id.clone(), Rc::new(detail));
                        }
                        Err(e) => eprintln!("aivory: repos/get failed: {e:#}"),
                    }
                    match map {
                        Some(Ok(map)) => {
                            this.maps.insert(id, Rc::new(map));
                        }
                        Some(Err(e)) => eprintln!("aivory: repos/map failed: {e:#}"),
                        None => {}
                    }
                }
                if std::mem::take(&mut this.dirty) {
                    this.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.note = None;
        self.refresh(cx);
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn toggle_map(&mut self, cx: &mut Context<Self>) {
        self.show_map = !self.show_map;
        if self.show_map {
            self.refresh(cx);
        }
        cx.notify();
    }

    /// Open or close a form. The fields are made on first use.
    fn toggle_popup(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((closed, at)) = self.closed.take()
            && closed == popup
            && at.elapsed() < Duration::from_millis(400)
        {
            cx.notify();
            return;
        }
        if self.popup == popup {
            self.popup = Popup::None;
            cx.notify();
            return;
        }
        self.popup = popup;
        match popup {
            Popup::NewProject => {
                self.ensure_project_form(window, cx);
                if self.project.parent.is_none() {
                    let selected = self.selected_id();
                    self.project.parent = self
                        .repos
                        .as_ref()
                        .and_then(|r| {
                            r.codebases
                                .iter()
                                .find(|c| Some(&c.id) == selected.as_ref())
                                .and_then(|c| Path::new(&c.path).parent().map(Path::to_path_buf))
                        })
                        .or_else(std::env::home_dir);
                }
                if let Some(name) = self.project.name.clone() {
                    name.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            Popup::AddPart => {
                self.ensure_part_form(window, cx);
                if let Some(name) = self.part.name.clone() {
                    name.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            Popup::None => {}
        }
        cx.notify();
    }

    fn ensure_project_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.name.is_some() {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("shop-api"));
        let events = vec![
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.create_project(cx);
                }
            }),
        ];
        self.project.name = Some(name);
        self.project._events = events;
    }

    fn ensure_part_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.part.name.is_some() {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("payment"));
        let paths =
            cx.new(|cx| InputState::new(window, cx).placeholder("crates/pay/**, docs/pay/*.md"));
        let events = vec![
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && let Some(paths) = this.part.paths.clone()
                {
                    paths.update(cx, |s, cx| s.focus(window, cx));
                }
            }),
            cx.subscribe_in(&paths, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.create_part(cx);
                }
            }),
        ];
        self.part.name = Some(name);
        self.part.paths = Some(paths);
        self.part._events = events;
    }

    /// Add a repo: pick a folder, the daemon registers the repository that holds it.
    fn add_repo(&mut self, cx: &mut Context<Self>) {
        if self.adding {
            return;
        }
        forms::pick_folder(cx, |this: &mut Self, path, cx| {
            this.adding = true;
            cx.notify();
            let call = this.model.read(cx).call_notify::<method::ReposAdd>(
                RepoAddParams {
                    path: path.to_string_lossy().into_owned(),
                },
                cx,
            );
            let model = this.model.clone();
            cx.spawn(async move |this, cx| {
                let added = call.await;
                let _ = this.update(cx, |this, cx| {
                    this.adding = false;
                    if let Some(cb) = added {
                        this.selected = Some(cb.id.clone());
                        let text = format!("Added the repo {}.", cb.id);
                        model.update(cx, |m, cx| {
                            m.notify_user(crate::notice::Draft::info(text), cx)
                        });
                        this.refresh(cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        });
    }

    /// Choose the parent folder of the new project.
    fn pick_parent(&mut self, cx: &mut Context<Self>) {
        forms::pick_folder(cx, |this: &mut Self, path, cx| {
            this.project.parent = Some(path);
            this.project.error = None;
            cx.notify();
        });
    }

    /// New project: the daemon makes the folder, runs `git init`, and registers it.
    fn create_project(&mut self, cx: &mut Context<Self>) {
        if self.project.busy {
            return;
        }
        let Some(name) = self.project.name.clone() else {
            return;
        };
        let params = match project_request(
            &name.read(cx).value(),
            self.project.parent.as_deref(),
            self.project.template,
        ) {
            Ok(params) => params,
            Err(e) => {
                self.project.error = Some(e);
                cx.notify();
                return;
            }
        };
        self.project.busy = true;
        self.project.error = None;
        let call = self.model.read(cx).call::<method::ProjectNew>(params, cx);
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.project.busy = false;
                match result {
                    Ok(cb) => {
                        this.popup = Popup::None;
                        this.selected = Some(cb.id.clone());
                        this.project.parent = None;
                        let text = format!("Created the project {} in {}.", cb.id, cb.path);
                        model.update(cx, |m, cx| {
                            m.notify_user(crate::notice::Draft::info(text), cx)
                        });
                        this.refresh(cx);
                    }
                    Err(e) => this.project.error = Some(crate::notice::reason(&e)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Add a part to the map of the selected codebase.
    fn create_part(&mut self, cx: &mut Context<Self>) {
        if self.part.busy {
            return;
        }
        let (Some(codebase), Some(name), Some(paths)) = (
            self.selected_id(),
            self.part.name.clone(),
            self.part.paths.clone(),
        ) else {
            return;
        };
        let params = match part_request(&codebase, &name.read(cx).value(), &paths.read(cx).value())
        {
            Ok(params) => params,
            Err(e) => {
                self.part.error = Some(e);
                cx.notify();
                return;
            }
        };
        let part = params.name.clone();
        self.part.busy = true;
        self.part.error = None;
        let call = self.model.read(cx).call::<method::MapAddPart>(params, cx);
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.part.busy = false;
                match result {
                    Ok(map) => {
                        this.popup = Popup::None;
                        this.maps.insert(codebase.clone(), Rc::new(map));
                        let text = format!("Added the part {part} to {codebase}.");
                        model.update(cx, |m, cx| {
                            m.notify_user(crate::notice::Draft::info(text), cx)
                        });
                        this.refresh(cx);
                    }
                    Err(e) => this.part.error = Some(crate::notice::reason(&e)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Remove a repo that the user added: the daemon forgets it, the folder stays.
    fn remove_repo(&mut self, id: String, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call_notify::<method::ReposRemove>(CodebaseRef { id: id.clone() }, cx);
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            if call.await.is_some() {
                let text = format!("Removed the repo {id}. The folder stays on the disk.");
                model.update(cx, |m, cx| {
                    m.notify_user(crate::notice::Draft::info(text), cx)
                });
                let _ = this.update(cx, |this, cx| {
                    this.pinned.remove(&id);
                    this.details.remove(&id);
                    if this.selected.as_deref() == Some(id.as_str()) {
                        this.selected = None;
                    }
                    this.refresh(cx);
                });
            }
        })
        .detach();
    }

    /// Paint a form above everything; a click outside closes it.
    fn overlay(&self, el: Div, cx: &mut Context<Self>) -> AnyElement {
        deferred(
            el.occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    let open = std::mem::replace(&mut this.popup, Popup::None);
                    this.closed = Some((open, Instant::now()));
                    cx.notify();
                })),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// The form of New project.
    fn project_form(&self, cx: &mut Context<Self>) -> Div {
        let f = &self.project;
        let mut templates = div().flex().flex_wrap().gap(px(6.));
        for (ix, (template, label)) in TEMPLATES.into_iter().enumerate() {
            templates = templates.child(
                forms::chip(("np-tpl", ix), label, f.template == template).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.project.template = template;
                        cx.notify();
                    },
                )),
            );
        }
        let parent = f
            .parent
            .as_deref()
            .map(|p| p.to_string_lossy().into_owned());
        ui::popover(420.)
            .child(div().font_weight(FontWeight::BOLD).child("New project"))
            .child(forms::field(
                "Name",
                forms::boxed().children(f.name.clone()),
            ))
            .child(forms::field(
                "Parent folder",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        forms::boxed()
                            .flex_1()
                            .min_w(px(0.))
                            .child(forms::path_box(parent.as_deref(), "No folder chosen")),
                    )
                    .child(
                        forms::button("np-pick", "Choose…", false, false)
                            .h(px(36.))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_parent(cx))),
                    ),
            ))
            .child(forms::field("Template", templates))
            .when_some(f.error.clone(), |d, e| d.child(forms::error_line(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(forms::button("np-cancel", "Cancel", false, false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.popup = Popup::None;
                            cx.notify();
                        }),
                    ))
                    .child(
                        forms::button("np-create", "Create project", true, f.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.create_project(cx))),
                    ),
            )
    }

    /// The form of Add a part.
    fn part_form(&self, cx: &mut Context<Self>) -> Div {
        let f = &self.part;
        ui::popover(420.)
            .child(div().font_weight(FontWeight::BOLD).child("Add a part"))
            .child(forms::field(
                "Name",
                forms::boxed().children(f.name.clone()),
            ))
            .child(forms::field(
                "Paths (patterns relative to the repo, separated by commas)",
                forms::boxed().children(f.paths.clone()),
            ))
            .when_some(f.error.clone(), |d, e| d.child(forms::error_line(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(forms::button("ap-cancel", "Cancel", false, false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.popup = Popup::None;
                            cx.notify();
                        }),
                    ))
                    .child(
                        forms::button("ap-create", "Add the part", true, f.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.create_part(cx))),
                    ),
            )
    }

    /// Start a session that writes a new `.aivory/map.toml`.
    fn remap(&mut self, codebase: &Codebase, cx: &mut Context<Self>) {
        if self.remapping {
            return;
        }
        self.remapping = true;
        self.note = None;
        // The mapping session uses the provider of a live session of the codebase.
        let (provider, model) = self
            .model
            .read(cx)
            .war
            .as_ref()
            .and_then(|w| {
                w.sessions
                    .iter()
                    .find(|s| s.codebase == codebase.id && s.state.is_live())
                    .map(|s| (s.provider.clone(), s.model.clone()))
            })
            .unwrap_or_else(|| ("claude".to_string(), String::new()));
        let params = NewSessionParams {
            codebase: codebase.id.clone(),
            part: None,
            title: Some(format!("Re-map parts of {}", codebase.id)),
            prompt: remap_prompt(codebase),
            composer: ComposerPrefs {
                mode: Mode::Edit,
                provider,
                model,
                effort: Effort::High,
            },
            task: None,
        };
        let call = self.model.read(cx).call::<method::SessionNew>(params, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.remapping = false;
                let text = match result {
                    Ok(created) => format!(
                        "Mapping session {} started. Watch it in the war room.",
                        created.id
                    ),
                    Err(e) => format!("Cannot start the mapping session: {e:#}"),
                };
                this.note = Some(Note::Parts(text.into()));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn now(&self, cx: &gpui::App) -> UnixMs {
        self.model
            .read(cx)
            .war
            .as_ref()
            .map_or_else(|| chrono::Utc::now().timestamp_millis(), |w| w.now)
    }

    // ---- Header and list ----

    fn header(&self, codebases: &[Codebase], cx: &mut Context<Self>) -> Div {
        let open = self.popup == Popup::NewProject;
        let mut new_project = div().relative().flex_none().child(
            ui::dark_button("new-project", "New project")
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_popup(Popup::NewProject, window, cx)
                })),
        );
        if open {
            new_project = new_project.child(
                self.overlay(
                    div()
                        .absolute()
                        .top(px(42.))
                        .right_0()
                        .child(self.project_form(cx)),
                    cx,
                ),
            );
        }
        div()
            .flex_none()
            .min_h(HEADER_H)
            .py(px(12.))
            .px(px(32.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .font_family(crate::theme::tk().display)
                    .text_size(crate::ui::ts(crate::theme::T_DISPLAY))
                    .font_weight(FontWeight::NORMAL)
                    .child("Repos"),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(header_text(codebases)),
            )
            .child(div().flex_1())
            .child(
                outline_button("add-repo", "Add a repo")
                    .when(self.adding, |d| d.opacity(0.6))
                    .tooltip(ui::tip("Pick a folder inside a git repository"))
                    .on_click(cx.listener(|this, _, _, cx| this.add_repo(cx))),
            )
            .child(new_project)
    }

    fn repo_list(
        &self,
        codebases: &[Codebase],
        live: &[Live],
        selected: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div()
            .id("repo-list")
            .flex_none()
            .w(px(LIST_W))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pl(px(32.))
            .pr(px(16.))
            .pt(px(16.))
            .pb(px(24.))
            .border_r_1()
            .border_color(rgba(tk().line))
            .overflow_y_scroll();
        for cb in codebases {
            let states: Vec<SessionState> = live
                .iter()
                .filter(|l| l.codebase == cb.id)
                .map(|l| l.state)
                .collect();
            col = col.child(self.repo_row(cb, &states, selected == Some(cb.id.as_str()), cx));
        }
        col.into_any_element()
    }

    /// One row of the list: name and state dots, then sessions · parts and the daemon.
    fn repo_row(
        &self,
        cb: &Codebase,
        states: &[SessionState],
        on: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = cb.id.clone();
        let dots = state_dots(states, 3.).flex_none();
        div()
            .id(SharedString::from(format!("repo-{}", cb.id)))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(5.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgba(if on { tk().primary << 8 | 0xFF } else { 0 }))
            .cursor_pointer()
            .when(on, |d| {
                d.bg(rgb(tk().card)).shadow(vec![BoxShadow {
                    color: ui::hsla(SELECT_RING),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .when(!on, |d| {
                d.hover(|s| s.bg(rgb(tk().card)).border_color(rgba(tk().line)))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(cb.id.clone()),
                    )
                    .child(dots),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .child(repo_meta(states.len(), cb.parts.len()))
                    .child(div().whitespace_nowrap().child(cb.daemon.clone())),
            )
            .into_any_element()
    }

    // ---- Detail ----

    #[allow(clippy::too_many_arguments)]
    fn detail(
        &self,
        cb: &Codebase,
        live: &[Live],
        flow: &[FlowItem],
        now: UnixMs,
        width: Pixels,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = self.details.get(&cb.id).cloned();
        let body = match (self.tab, detail.as_deref()) {
            (_, None) => muted_note("Loads the repo from aivoryd…"),
            (Tab::Parts, Some(d)) => self.parts_tab(cb, d, live, flow, now, width, cx),
            (Tab::Files, Some(d)) => self.files_tab(cb, d, live, cx),
            (Tab::Activity, Some(d)) => self.activity_tab(d, narrow, cx),
            (Tab::Radar, Some(_)) => self.radar.clone().into_any_element(),
        };
        let show = cb.id.clone();
        div()
            .id("repo-detail")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .pt(px(20.))
            .pb(px(32.))
            .pl(px(if narrow { 16. } else { 28. }))
            .pr(px(if narrow { 16. } else { 32. }))
            .overflow_y_scroll()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(crate::ui::ts(crate::theme::T_HEADING))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(cb.id.clone()),
                            )
                            .child(
                                // The path in mono, the facts in the interface font.
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                                    .text_color(rgb(tk().ink_3))
                                    .child(div().font_family(tk().mono).child(cb.path.clone()))
                                    .child(format!("· {}", fact_line(cb))),
                            ),
                    )
                    .child(div().flex_1())
                    .when(self.pinned.contains(&cb.id), |d| {
                        let id = cb.id.clone();
                        d.child(
                            outline_button("remove-repo", "Remove")
                                .tooltip(ui::tip(
                                    "Remove the repo from the list. The folder stays.",
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_repo(id.clone(), cx)
                                })),
                        )
                    })
                    .child(
                        outline_button("show-war-room", "Show in war room").on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.emit(ReposEvent::ShowInWarRoom(show.clone()));
                            },
                        )),
                    ),
            )
            .child(self.tabs(cx))
            .child(body)
            .into_any_element()
    }

    fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let mut track = div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .p(px(2.))
            .bg(rgb(tab_track()))
            .rounded(tk().r_button);
        let at_risk = self.radar.read(cx).at_risk();
        for (tab, label) in TABS {
            let on = self.tab == tab;
            let badge = (tab == Tab::Radar && at_risk > 0).then(|| at_risk.to_string());
            track = track.child(
                div()
                    .id(SharedString::from(format!("tab-{label}")))
                    .h(px(30.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![BoxShadow {
                                color: ui::hsla(tk().line),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(2.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                    .gap(px(6.))
                    .child(label)
                    .when_some(badge, |d, n| {
                        d.child(
                            div()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(tk().q_text))
                                .child(n),
                        )
                    }),
            );
        }
        div().flex_none().flex().child(track)
    }

    // ---- Parts ----

    #[allow(clippy::too_many_arguments)]
    fn parts_tab(
        &self,
        cb: &Codebase,
        detail: &RepoDetail,
        live: &[Live],
        flow: &[FlowItem],
        now: UnixMs,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let two = self.show_map && width > px(SPLIT_AT);
        let mut split = div().flex_none().w_full().flex().gap(px(16.));
        split = if two {
            split.items_start()
        } else {
            split.flex_col()
        };
        let table = div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .shadow(ui::card_shadow())
            .child(self.parts_table(&detail.parts, live, now))
            .child(self.parts_actions(cb, cx));
        split = split.child(if two { table.flex_1() } else { table.w_full() });
        if self.show_map {
            split = split.child(self.map_panel(&cb.id, now, two));
        }
        let mut col = div().flex_none().flex().flex_col().gap(px(18.));
        if !flow.is_empty() {
            col = col.child(flow_card(cb, flow));
        }
        col.child(split).into_any_element()
    }

    fn parts_table(&self, parts: &[PartStats], live: &[Live], now: UnixMs) -> Div {
        let head = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .px(px(14.))
                .py(px(12.))
                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(tk().ink_3))
                .child(label)
        };
        let mut table = div().w_full().flex().flex_col().child(
            div()
                .w_full()
                .flex()
                .border_b_1()
                .border_color(rgba(tk().line))
                .child(head("PART").w(crate::ui::sw(COL_PART)).flex_none())
                .child(head("PATHS").flex_1().min_w(px(0.)))
                .child(head("LIVE SESSIONS").w(crate::ui::sw(COL_LIVE)).flex_none())
                .child(head("CLAIMS").w(crate::ui::sw(COL_CLAIMS)).flex_none())
                .child(head("LAST CHANGE").w(crate::ui::sw(COL_LAST)).flex_none())
                .child(head("LINES").w(crate::ui::sw(COL_LINES)).flex_none()),
        );
        let count = parts.len();
        for (i, p) in parts.iter().enumerate() {
            let states: Vec<SessionState> = p
                .sessions
                .iter()
                .filter_map(|id| live.iter().find(|l| l.id == *id).map(|l| l.state))
                .collect();
            let dots = state_dots(&states, 4.);
            let sessions = short_ids(&p.sessions);
            let cell = |w: Option<f32>| {
                let d = div().flex().items_center().px(px(14.)).py(px(11.));
                match w {
                    Some(w) => d.w(crate::ui::sw(w)).flex_none().overflow_hidden(),
                    None => d.flex_1().min_w(px(0.)).overflow_hidden(),
                }
            };
            let mono = |text: String| {
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .whitespace_nowrap()
                    .child(text)
            };
            table = table.child(
                div()
                    .w_full()
                    .flex()
                    .when(i + 1 < count, |d| {
                        d.border_b_1().border_color(rgba(ROW_LINE))
                    })
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .child(
                        cell(Some(COL_PART))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(div().whitespace_nowrap().child(p.name.clone())),
                    )
                    .child(
                        cell(None).child(
                            div()
                                .min_w(px(0.))
                                .font_family(tk().mono)
                                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                                .text_color(rgb(tk().ink_2))
                                .child(p.paths.join("  ")),
                        ),
                    )
                    .child(
                        cell(Some(COL_LIVE))
                            .gap(px(4.))
                            .child(dots)
                            .child(mono(sessions)),
                    )
                    .child(cell(Some(COL_CLAIMS)).child(mono(p.claims.to_string())))
                    .child(
                        cell(Some(COL_LAST)).child(mono(
                            p.last_change
                                .map_or_else(|| "—".to_string(), |at| ago_text(now, at)),
                        )),
                    )
                    .child(cell(Some(COL_LINES)).child(mono(thousands(p.lines)))),
            );
        }
        table
    }

    fn parts_actions(&self, cb: &Codebase, cx: &mut Context<Self>) -> Div {
        let note = match &self.note {
            Some(Note::Parts(text)) => Some(text.clone()),
            _ => None,
        };
        let busy = self.remapping;
        let target = cb.clone();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgba(tk().line))
            .child(
                ghost_button("remap", "Re-map parts with an agent", true)
                    .when(busy, |d| d.opacity(0.6))
                    .on_click(cx.listener(move |this, _, _, cx| this.remap(&target, cx))),
            )
            .child({
                let open = self.popup == Popup::AddPart;
                let mut wrap = div().relative().flex_none().child(
                    ghost_button("add-part", "Add a part", false).on_click(cx.listener(
                        |this, _, window, cx| this.toggle_popup(Popup::AddPart, window, cx),
                    )),
                );
                if open {
                    wrap = wrap.child(
                        self.overlay(
                            div()
                                .absolute()
                                .bottom(px(38.))
                                .left_0()
                                .child(self.part_form(cx)),
                            cx,
                        ),
                    );
                }
                wrap
            })
            .child(
                ghost_button(
                    "view-map",
                    if self.show_map {
                        "Hide map.toml"
                    } else {
                        "View map.toml"
                    },
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_map(cx))),
            )
            .when_some(note, |d, text| d.child(note_text(text)))
    }

    /// `.aivory/map.toml`, syntax-colored, with the author and the age of the file.
    fn map_panel(&self, id: &str, now: UnixMs, two: bool) -> Div {
        let map = self.maps.get(id).cloned();
        let byline = map.as_deref().map(|m| map_byline(m, now));
        let mut lines = div().flex().flex_col().py(px(8.));
        match map.as_deref() {
            Some(m) => {
                for (i, line) in m.source.lines().enumerate() {
                    let tone = map_tone(line);
                    lines = lines.child(
                        div()
                            .w_full()
                            .flex()
                            .font_family(tk().mono)
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .line_height(relative(1.7))
                            .child(
                                div()
                                    .w(px(36.))
                                    .flex_none()
                                    .flex()
                                    .justify_end()
                                    .pr(px(10.))
                                    .text_color(rgb(line_no()))
                                    .child((i + 1).to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .pr(px(12.))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(rgb(tone.color()))
                                    .when(tone == Tone::Header, |d| {
                                        d.font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .child(if line.is_empty() {
                                        " ".to_string()
                                    } else {
                                        line.to_string()
                                    }),
                            ),
                    );
                }
            }
            None => {
                lines = lines.child(
                    div()
                        .px(px(14.))
                        .py(px(8.))
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().muted))
                        .child("Loads the map…"),
                );
            }
        }
        div()
            .min_w(px(0.))
            .flex_none()
            .when(two, |d| d.w(px(MAP_W)))
            .when(!two, |d| d.w_full())
            .bg(rgb(tk().card))
            .border_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .h(px(40.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(rgba(tk().line))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child(".aivory/map.toml")
                    .child(div().flex_1())
                    .when_some(byline, |d, text| {
                        d.child(
                            div()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .text_color(rgb(tk().ink_3))
                                .whitespace_nowrap()
                                .child(text),
                        )
                    }),
            )
            .child(lines)
    }

    // ---- Files ----

    fn files_tab(
        &self,
        cb: &Codebase,
        detail: &RepoDetail,
        live: &[Live],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut tree = div().flex().flex_col().py(px(8.));
        if detail.files.is_empty() {
            tree = tree.child(muted_note("No files mapped yet."));
        }
        // The live claims of the daemon; the radar view holds them.
        let radar = self.radar.read(cx);
        let coord = (radar.codebase() == cb.id).then(|| radar.coord()).flatten();
        for (i, node) in detail.files.iter().enumerate() {
            let claim = (!node.dir)
                .then(|| claim_of(node, coord.as_deref(), live))
                .flatten();
            let mut row = file_row(i, node, claim);
            if !node.dir {
                let (codebase, path) = (cb.id.clone(), node.path.clone());
                row = row
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ReposEvent::OpenFile {
                            codebase: codebase.clone(),
                            path: path.clone(),
                        })
                    }));
            }
            tree = tree.child(row);
        }
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .border_1()
            .border_color(rgba(tk().line))
            .child(
                div()
                    .h(px(40.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(rgba(tk().line))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .child(cb.path.clone())
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .child("part tags from map.toml · live claims"),
                    ),
            )
            .child(tree)
            .into_any_element()
    }

    // ---- Activity ----

    fn activity_tab(
        &self,
        detail: &RepoDetail,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .shadow(ui::card_shadow());
        if detail.activity.is_empty() {
            card = card.child(muted_note("No activity yet."));
        }
        let count = detail.activity.len();
        for (i, row) in detail.activity.iter().enumerate() {
            let time = self.model.read(cx).clock(row.at);
            card = card.child(activity_row(row, time, i + 1 < count, narrow));
        }
        card.into_any_element()
    }
}

/// Widths of the parts table columns. The paths column takes the rest.
const COL_PART: f32 = 104.;
const COL_LIVE: f32 = 116.;
const COL_CLAIMS: f32 = 64.;
const COL_LAST: f32 = 100.;
const COL_LINES: f32 = 64.;

impl Render for ReposScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = window.viewport_size().width;
        let narrow = width < NARROW;
        let repos = self.repos.clone();
        let codebases: &[Codebase] = repos.as_ref().map_or(&[], |r| r.codebases.as_slice());
        let selected = resolve_selection(codebases, self.selected.as_deref());
        let now = self.now(cx);
        // The live sessions and the flows of the selected codebase, as owned data.
        let (live, flow) = {
            let model = self.model.read(cx);
            match &model.war {
                Some(war) => (
                    war.sessions
                        .iter()
                        .filter(|s| s.state.is_live())
                        .map(|s| Live {
                            id: s.id.clone(),
                            codebase: s.codebase.clone(),
                            state: s.state,
                        })
                        .collect::<Vec<_>>(),
                    selected
                        .map(|id| layout::flow_items(&war.sessions, id))
                        .unwrap_or_default(),
                ),
                None => (Vec::new(), Vec::new()),
            }
        };
        let body = match (
            &repos,
            selected.and_then(|id| codebases.iter().find(|c| c.id == id)),
        ) {
            (None, _) => centered_note("Waits for aivoryd"),
            (Some(_), None) => centered_note("No codebases yet. Add a repo to start."),
            (Some(_), Some(cb)) => div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .when(!narrow, |d| {
                    d.child(self.repo_list(codebases, &live, selected, cx))
                })
                .child(self.detail(cb, &live, &flow, now, width, narrow, cx))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .line_height(relative(1.3))
            .child(self.header(codebases, cx))
            .child(body)
    }
}

// ---- Widgets ----

/// The header button with a thin border (`.hb`).
fn outline_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgba(tk().line_2))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(tk().ink)))
        .child(label)
}

/// The button of the parts table (`.gb`); `primary` is the blue one (`.gb.pri`).
fn ghost_button(id: impl Into<ElementId>, label: &'static str, primary: bool) -> Stateful<Div> {
    let button = div()
        .id(id)
        .h(px(30.))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .whitespace_nowrap()
        .cursor_pointer();
    if primary {
        button
            .border_color(rgb(tk().primary))
            .bg(rgb(tk().primary))
            .text_color(rgb(tk().card))
            .font_weight(FontWeight::MEDIUM)
            .hover(|s| {
                s.bg(rgb(tk().primary_hover))
                    .border_color(rgb(tk().primary_hover))
            })
            .child(label)
    } else {
        button
            .border_color(rgb(button_border()))
            .bg(rgb(tk().card))
            .hover(|s| s.border_color(rgb(button_border_hover())))
            .child(label)
    }
}

fn note_text(text: SharedString) -> Div {
    div()
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().ink_3))
        .child(text)
}

fn muted_note(text: &'static str) -> AnyElement {
    div()
        .flex_none()
        .px(px(16.))
        .py(px(14.))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text)
        .into_any_element()
}

fn centered_note(text: &'static str) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().muted))
        .child(text)
        .into_any_element()
}

fn pill(bg: u32, text: u32, label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .whitespace_nowrap()
        .child(label.into())
}

/// The claim pill of a file: the session of the strongest live claim (editing before intent),
/// else the session that the repo detail names. The flag is true when that session needs you.
fn claim_of(
    node: &FileNode,
    coord: Option<&radar::Coord>,
    live: &[Live],
) -> Option<(String, bool)> {
    let session = coord
        .and_then(|c| radar::claim_pill(&c.claims, &node.path))
        .map(|(s, _)| s)
        .or_else(|| node.claimed_by.clone())?;
    let needs = match live.iter().find(|l| l.id == session) {
        Some(l) => l.state == SessionState::NeedsYou,
        None => {
            node.claimed_by.as_deref() == Some(session.as_str())
                && node.claim_state == Some(SessionState::NeedsYou)
        }
    };
    Some((session, needs))
}

/// The most state dots in a row; more sessions show as "+N".
const MAX_DOTS: usize = 6;

/// The state dots of some sessions: the ones that need you, fail or work first, at most
/// [`MAX_DOTS`], then "+N" for the rest.
fn state_dots(states: &[SessionState], gap: f32) -> Div {
    let rank = |s: &SessionState| match s {
        SessionState::NeedsYou => 0,
        SessionState::Error => 1,
        SessionState::Working => 2,
        _ => 3,
    };
    let mut sorted = states.to_vec();
    sorted.sort_by_key(rank);
    let mut dots = div().flex().items_center().gap(px(gap));
    for state in sorted.iter().take(MAX_DOTS) {
        dots = dots.child(ui::state_dot(*state, 8.));
    }
    if sorted.len() > MAX_DOTS {
        dots = dots.child(
            div()
                .ml(px(2.))
                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                .text_color(rgb(tk().ink_3))
                .whitespace_nowrap()
                .child(format!("+{}", sorted.len() - MAX_DOTS)),
        );
    }
    dots
}

/// One row of the file tree: indent, twist or dot, name, the claim pill and the part tag.
fn file_row(i: usize, node: &FileNode, claim: Option<(String, bool)>) -> Stateful<Div> {
    let icon = if node.dir {
        ui::icon("icons/twist-down.svg", 10., rgb(tk().muted)).into_any_element()
    } else {
        div().child("·").into_any_element()
    };
    let claim = claim.map(|(id, needs)| {
        let (bg, text) = if needs {
            (tk().q_bg, tk().q_text)
        } else {
            (claim_bg(), tk().link)
        };
        pill(bg, text, format!("claimed by {id}"))
    });
    let part = node
        .part
        .as_ref()
        .map(|p| pill(tk().track, tk().ink_2, p.clone()));
    div()
        .id(("file", i))
        .w_full()
        .h(px(30.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(indent(node.depth)))
        .pr(px(14.))
        .font_family(tk().mono)
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .hover(|s| s.bg(rgb(row_hover())))
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .text_color(rgb(tk().muted))
                .child(icon),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(node.name.clone()),
        )
        .when_some(claim, |d, p| d.child(p))
        .when_some(part, |d, p| d.child(p))
}

/// One row of the activity tab: time, provider · session, what, +added −removed, cost.
fn activity_row(row: &ActivityRow, time: String, line: bool, narrow: bool) -> Div {
    let mono = |w: f32, text: String| {
        div()
            .w(px(w))
            .flex_none()
            .font_family(tk().mono)
            .text_size(crate::ui::ts(crate::theme::T_SMALL))
            .text_color(rgb(tk().ink_3))
            .whitespace_nowrap()
            .child(text)
    };
    div()
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(11.))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .when(line, |d| d.border_b_1().border_color(rgba(ROW_LINE)))
        .child(mono(if narrow { 56. } else { 64. }, time))
        .when(!narrow, |d| {
            d.child(
                div()
                    .w(px(170.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(7.))
                            .rounded_full()
                            .bg(rgb(provider_color(&row.provider))),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(format!("{} · {}", row.provider, row.session)),
                    ),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .child(row.what.clone()),
        )
        .when(!narrow, |d| {
            d.child(
                div()
                    .w(px(90.))
                    .flex_none()
                    .flex()
                    .gap(px(5.))
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .text_color(rgb(added()))
                            .child(format!("+{}", row.added)),
                    )
                    .child(
                        div()
                            .text_color(rgb(tk().e_text))
                            .child(format!("−{}", row.removed)),
                    ),
            )
        })
        .child(mono(if narrow { 70. } else { 64. }, money(row.cost)))
}

// ---- The Sankey ----

/// The card of the parts tab: the title and the 1000 × 120 flow box.
fn flow_card(cb: &Codebase, items: &[FlowItem]) -> Div {
    let sankey = layout::sankey(items, cb.part_names(), &FlowBox::WIDE);
    let agents: u32 = items.iter().map(|i| i.weight).sum();
    div()
        .flex_none()
        .w_full()
        .px(px(18.))
        .py(px(16.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .shadow(ui::card_shadow())
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("agents → parts → state"),
                )
                .child(
                    div()
                        .text_color(rgb(tk().ink_3))
                        .child(flow_meta(items.len(), agents)),
                ),
        )
        .child(flow_box(sankey))
}

/// The flow box: links and nodes on a canvas, labels on top. The view box is stretched on x
/// only (`preserveAspectRatio="none"`); stroke widths do not scale.
fn flow_box(sankey: Sankey) -> Div {
    let w = sankey.width as f32;
    let labels = sankey.labels.clone();
    let mut box_ = div().relative().h(px(sankey.height as f32)).w_full().child(
        canvas(
            move |_, _, _| sankey,
            move |bounds: Bounds<Pixels>, sankey, window, _| {
                let sx = bounds.size.width / px(w);
                let at = |x: f64, y: f64| {
                    point(
                        bounds.origin.x + px(x as f32) * sx,
                        bounds.origin.y + px(y as f32),
                    )
                };
                for link in &sankey.links {
                    let (color, alpha) = link_color(link.state);
                    let xm = (link.x0 + link.x1) / 2.;
                    let mut path = PathBuilder::stroke(px(link.width as f32));
                    path.move_to(at(link.x0, link.y0));
                    path.cubic_bezier_to(at(link.x1, link.y1), at(xm, link.y0), at(xm, link.y1));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, ui::hsla(color << 8 | alpha));
                    }
                }
                for node in &sankey.nodes {
                    let color = match node.state {
                        Some(state) => ui::state_color(state),
                        None => tk().ink,
                    };
                    let origin = at(node.x, node.y);
                    let size = gpui::size(px(node.w as f32) * sx, px(node.h.max(2.) as f32));
                    window
                        .paint_quad(fill(Bounds { origin, size }, rgb(color)).corner_radii(px(2.)));
                }
            },
        )
        .absolute()
        .inset_0(),
    );
    for label in labels {
        let frac = (label.anchor_x / w as f64) as f32;
        let mut l = div()
            .absolute()
            .top(px(label.y as f32 - 5.5))
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .line_height(px(11.))
            .whitespace_nowrap()
            .text_color(rgb(tk().ink));
        l = match label.column {
            FlowColumn::Provider => l.right(relative(1. - frac + 0.008)),
            FlowColumn::Part => l
                .left(relative(frac + 0.008))
                .max_w(relative(0.3))
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD),
            FlowColumn::State => {
                let l = l.left(relative(frac + 0.01));
                match label.state {
                    Some(SessionState::NeedsYou) => l
                        .text_color(rgb(tk().q_text))
                        .font_weight(FontWeight::SEMIBOLD),
                    Some(SessionState::Error) => l
                        .text_color(rgb(tk().e_text))
                        .font_weight(FontWeight::SEMIBOLD),
                    Some(SessionState::Idle) => l.text_color(rgb(tk().ink_3)),
                    _ => l,
                }
            }
        };
        box_ = box_.child(l.child(label.text));
    }
    box_
}

/// Link color and alpha byte: working ink 9 %, idle 18 %, needs you and error 50 %.
fn link_color(state: SessionState) -> (u32, u32) {
    match state {
        SessionState::NeedsYou => (tk().q, 0x80),
        SessionState::Error => (tk().e, 0x80),
        SessionState::Idle => (tk().idle_ring, 0x2E),
        _ => (tk().ink, 0x17),
    }
}

// ---- Pure parts ----

/// `1 codebase`, `10 codebases`.
fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// `10 codebases · 3 daemons · 31 parts`.
fn header_text(codebases: &[Codebase]) -> String {
    let daemons: HashSet<&str> = codebases.iter().map(|c| c.daemon.as_str()).collect();
    let parts: usize = codebases.iter().map(|c| c.parts.len()).sum();
    format!(
        "{} · {} · {}",
        plural(codebases.len(), "codebase"),
        plural(daemons.len(), "daemon"),
        plural(parts, "part")
    )
}

/// `4 sessions · 4 parts`, or `quiet · 2 parts` when no session is live.
fn repo_meta(sessions: usize, parts: usize) -> String {
    let parts = plural(parts, "part");
    if sessions == 0 {
        format!("quiet · {parts}")
    } else {
        format!("{} · {parts}", plural(sessions, "session"))
    }
}

/// `main · 4 worktrees · desktop · Rust`: the facts after the path.
fn fact_line(cb: &Codebase) -> String {
    let mut facts = vec![cb.branch.clone()];
    if cb.worktrees > 0 {
        facts.push(plural(cb.worktrees as usize, "worktree"));
    }
    facts.push(cb.daemon.clone());
    if !cb.languages.is_empty() {
        facts.push(cb.languages.join(", "));
    }
    facts.join(" · ")
}

/// `4 sessions · 7 agents`.
fn flow_meta(sessions: usize, agents: u32) -> String {
    format!(
        "{} · {}",
        plural(sessions, "session"),
        plural(agents as usize, "agent")
    )
}

/// The codebase on screen: the pick of the user, or the first codebase.
fn resolve_selection<'a>(codebases: &'a [Codebase], selected: Option<&str>) -> Option<&'a str> {
    selected
        .and_then(|id| codebases.iter().find(|c| c.id == id))
        .or_else(|| codebases.first())
        .map(|c| c.id.as_str())
}

/// The left padding of a tree row in px: a part root sits at 14, each level adds 22.
fn indent(depth: u8) -> f32 {
    14. + 22. * f32::from(depth)
}

/// How a line of `map.toml` is colored (`c-c`, `c-k`, `c-s`, `c-h` of the design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Plain,
    Comment,
    /// A `name` line.
    Key,
    /// A `paths` line.
    String,
    /// A `[[part]]` line.
    Header,
}

impl Tone {
    fn color(self) -> u32 {
        match self {
            Tone::Plain => tk().ink,
            Tone::Comment => SYNTAX_COMMENT,
            Tone::Key => SYNTAX_KEY,
            Tone::String => SYNTAX_STRING,
            Tone::Header => SYNTAX_HEADER,
        }
    }
}

fn map_tone(line: &str) -> Tone {
    let line = line.trim_start();
    if line.starts_with('#') {
        return Tone::Comment;
    }
    if line.starts_with("[[") {
        return Tone::Header;
    }
    let key = line.split(['=', ' ']).next().unwrap_or_default();
    match key {
        "name" => Tone::Key,
        "paths" => Tone::String,
        _ => Tone::Plain,
    }
}

/// `by claude opus-5.5 · 26 min ago`.
fn map_byline(map: &MapFile, now: UnixMs) -> String {
    let age = ago_text(now, map.updated);
    match &map.author {
        Some(author) => format!("by {author} · {age}"),
        None => age,
    }
}

/// `just now`, `6 min ago`, `3 h ago`, `2 days ago`.
fn ago_text(now: UnixMs, at: UnixMs) -> String {
    let minutes = (now - at).max(0) / MINUTE_MS;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => {
            let days = minutes / 1440;
            format!("{days} {} ago", if days == 1 { "day" } else { "days" })
        }
    }
}

/// `5928` as `5,928`.
fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn money(cost: f64) -> String {
    format!("${cost:.2}")
}

/// The prompt of the mapping session: write `.aivory/map.toml` and nothing else.
fn remap_prompt(cb: &Codebase) -> String {
    let current: Vec<&str> = cb.part_names().collect();
    let keep = if current.is_empty() {
        String::new()
    } else {
        format!(
            " Keep the names of the current parts when they still fit: {}.",
            current.join(", ")
        )
    };
    format!(
        "Map the codebase {} at {}. Read the directory layout and the build files. Split the \
         code into parts that one agent can own. Write `.aivory/map.toml` with `version`, \
         `updated`, and one `[[part]]` block per part with `name` and `paths` (globs relative \
         to the repository root). Every source file belongs to one part.{keep} Do not change \
         any other file.",
        cb.id, cb.path
    )
}

/// Session ids for a table cell: live ids are UUIDs, so show their first 4 characters,
/// at most three, then `+N`.
fn short_ids(ids: &[String]) -> String {
    if ids.is_empty() {
        return "—".to_string();
    }
    let mut text = ids
        .iter()
        .take(3)
        .map(|id| id.chars().take(4).collect::<String>())
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > 3 {
        text.push_str(&format!(" +{}", ids.len() - 3));
    }
    text
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_shows_the_session_of_its_live_claim() {
        use aivory_protocol::coord::{Claim, ClaimState};
        let node = |claimed_by: Option<&str>, state| FileNode {
            path: "src/a.rs".into(),
            name: "a.rs".into(),
            dir: false,
            depth: 1,
            part: None,
            claimed_by: claimed_by.map(str::to_string),
            claim_state: state,
        };
        let live = |id: &str, state| Live {
            id: id.into(),
            codebase: "billing".into(),
            state,
        };
        let coord = radar::Coord {
            claims: vec![Claim {
                session: "7f3a".into(),
                codebase: "billing".into(),
                key: "path:src/a.rs".into(),
                state: ClaimState::Editing,
                path: Some("src/a.rs".into()),
                range: None,
                lease_s: 60,
            }],
            ..Default::default()
        };
        let lives = [
            live("7f3a", SessionState::NeedsYou),
            live("k9d4", SessionState::Working),
        ];
        // A live claim wins over the repo detail, and a session that needs you is orange.
        assert_eq!(
            claim_of(&node(Some("k9d4"), None), Some(&coord), &lives),
            Some(("7f3a".to_string(), true))
        );
        // Without claims the detail names the session.
        assert_eq!(
            claim_of(
                &node(Some("k9d4"), Some(SessionState::NeedsYou)),
                None,
                &lives
            ),
            Some(("k9d4".to_string(), false))
        );
        assert_eq!(
            claim_of(
                &node(Some("gone"), Some(SessionState::NeedsYou)),
                None,
                &lives
            ),
            Some(("gone".to_string(), true))
        );
        assert_eq!(claim_of(&node(None, None), None, &lives), None);
    }

    #[test]
    fn a_new_project_needs_a_clean_name_and_a_parent_folder() {
        let parent = Path::new("C:/work");
        let ok = project_request(" shop ", Some(parent), ProjectTemplate::Rust).expect("valid");
        assert_eq!(ok.name, "shop");
        assert_eq!(ok.parent, "C:/work");
        assert_eq!(ok.template, Some(ProjectTemplate::Rust));
        assert!(project_request("", Some(parent), ProjectTemplate::Empty).is_err());
        assert!(project_request("a/b", Some(parent), ProjectTemplate::Empty).is_err());
        assert_eq!(
            project_request("shop", None, ProjectTemplate::Empty),
            Err("Choose the parent folder.".to_string())
        );
    }

    #[test]
    fn a_new_part_needs_a_name_and_patterns() {
        let ok = part_request("billing", "payment", "crates/pay/**, docs/*.md").expect("valid");
        assert_eq!(ok.codebase, "billing");
        assert_eq!(ok.paths, ["crates/pay/**", "docs/*.md"]);
        assert!(part_request("billing", "pay ment", "a/**").is_err());
        assert!(part_request("billing", "payment", "").is_err());
        assert!(part_request("billing", "payment", "/abs/**").is_err());
    }

    #[test]
    fn short_ids_cut_uuids() {
        let ids: Vec<String> = ["be16c383-be28", "38a57809-39a0", "a01c", "d3f0", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(short_ids(&ids), "be16, 38a5, a01c +2");
        assert_eq!(short_ids(&[]), "—");
    }

    use super::*;
    use aivory_protocol::demo::{Demo, map_file};

    fn codebase(id: &str, daemon: &str, parts: &[&str]) -> Codebase {
        Codebase {
            id: id.into(),
            path: format!("~/work/{id}"),
            branch: "main".into(),
            worktrees: 0,
            daemon: daemon.into(),
            languages: vec!["Rust".into()],
            parts: parts
                .iter()
                .map(|p| aivory_protocol::Part {
                    name: (*p).into(),
                    paths: vec![format!("{p}/**")],
                })
                .collect(),
        }
    }

    #[test]
    fn header_counts_codebases_daemons_and_parts() {
        let cbs = [
            codebase("a", "desktop", &["x", "y"]),
            codebase("b", "desktop", &["z"]),
            codebase("c", "runner", &[]),
        ];
        assert_eq!(header_text(&cbs), "3 codebases · 2 daemons · 3 parts");
        assert_eq!(header_text(&cbs[..1]), "1 codebase · 1 daemon · 2 parts");
        assert_eq!(header_text(&[]), "0 codebases · 0 daemons · 0 parts");
    }

    #[test]
    fn header_of_the_demo_matches_its_data() {
        let demo = Demo::new();
        let cbs = &demo.war_room.codebases;
        let text = header_text(cbs);
        assert!(text.starts_with("10 codebases · 3 daemons · "), "{text}");
        let parts: usize = cbs.iter().map(|c| c.parts.len()).sum();
        assert!(text.ends_with(&format!("{parts} parts")), "{text}");
    }

    #[test]
    fn list_meta_names_quiet_repos() {
        assert_eq!(repo_meta(4, 4), "4 sessions · 4 parts");
        assert_eq!(repo_meta(1, 3), "1 session · 3 parts");
        assert_eq!(repo_meta(0, 2), "quiet · 2 parts");
    }

    #[test]
    fn fact_line_follows_the_design() {
        let mut cb = codebase("aivory-code", "desktop", &["ui"]);
        cb.worktrees = 4;
        cb.languages = vec!["Python".into(), "TS".into()];
        assert_eq!(fact_line(&cb), "main · 4 worktrees · desktop · Python, TS");
        cb.worktrees = 1;
        assert_eq!(fact_line(&cb), "main · 1 worktree · desktop · Python, TS");
        cb.worktrees = 0;
        assert_eq!(fact_line(&cb), "main · desktop · Python, TS");
    }

    #[test]
    fn selection_falls_back_to_the_first_repo() {
        let cbs = [codebase("a", "d", &[]), codebase("b", "d", &[])];
        assert_eq!(resolve_selection(&cbs, Some("b")), Some("b"));
        assert_eq!(resolve_selection(&cbs, Some("gone")), Some("a"));
        assert_eq!(resolve_selection(&cbs, None), Some("a"));
        assert_eq!(resolve_selection(&[], Some("a")), None);
    }

    #[test]
    fn tree_rows_indent_by_depth() {
        assert_eq!(indent(0), 14.);
        assert_eq!(indent(1), 36.);
        assert_eq!(indent(2), 58.);
        // The demo tree: a part root at depth 0 with the part tag, its files at depth 1.
        let demo = Demo::new();
        let files = &demo.repos[0].files;
        assert!(
            files
                .iter()
                .any(|f| f.dir && f.depth == 0 && f.part.is_some())
        );
        assert!(files.iter().all(|f| f.dir || f.depth == 1));
        assert!(files.iter().any(|f| f.claimed_by.is_some()));
    }

    #[test]
    fn map_lines_get_the_tones_of_the_design() {
        let cb = codebase("billing", "build-01", &["payment", "invoices"]);
        let source = map_file(&cb).source;
        let tones: Vec<Tone> = source.lines().map(map_tone).collect();
        assert_eq!(
            tones,
            [
                Tone::Comment,
                Tone::Plain,
                Tone::Plain,
                Tone::Plain,
                Tone::Header,
                Tone::Key,
                Tone::String,
                Tone::Plain,
                Tone::Header,
                Tone::Key,
                Tone::String,
            ]
        );
        assert_eq!(map_tone("  # note"), Tone::Comment);
        assert_eq!(map_tone("name=\"x\""), Tone::Key);
        assert_eq!(map_tone("version = 3"), Tone::Plain);
        assert_eq!(Tone::Header.color(), SYNTAX_HEADER);
    }

    #[test]
    fn map_byline_shows_author_and_age() {
        let map = map_file(&codebase("billing", "d", &["payment"]));
        let now = aivory_protocol::demo::DEMO_NOW;
        assert_eq!(map_byline(&map, now), "by claude opus-5.5 · 28 min ago");
        let mut anonymous = map;
        anonymous.author = None;
        assert_eq!(map_byline(&anonymous, now), "28 min ago");
    }

    #[test]
    fn ages_and_numbers_read_like_the_design() {
        let now = 10 * 86_400_000;
        assert_eq!(ago_text(now, now), "just now");
        assert_eq!(ago_text(now, now - 6 * MINUTE_MS), "6 min ago");
        assert_eq!(ago_text(now, now - 180 * MINUTE_MS), "3 h ago");
        assert_eq!(ago_text(now, now - 86_400_000), "1 day ago");
        assert_eq!(ago_text(now, now - 3 * 86_400_000), "3 days ago");
        assert_eq!(thousands(5), "5");
        assert_eq!(thousands(1258), "1,258");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(money(0.5), "$0.50");
    }

    #[test]
    fn remap_prompt_names_the_repo_and_keeps_the_parts() {
        let cb = codebase("billing", "build-01", &["payment", "invoices"]);
        let prompt = remap_prompt(&cb);
        assert!(prompt.contains("billing at ~/work/billing"));
        assert!(prompt.contains(".aivory/map.toml"));
        assert!(prompt.contains("payment, invoices"));
        assert!(!remap_prompt(&codebase("x", "d", &[])).contains("Keep the names"));
    }
}
