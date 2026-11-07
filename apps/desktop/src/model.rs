//! The app data: the daemon link and the war-room view. Screens observe this entity.

use aivory_protocol::rpc::method;
use aivory_protocol::rpc::{DaemonInfo, Method};
use aivory_protocol::{ConsoleBackground, Display, WarRoom};
use aivoryd::Endpoint;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{App, Context, Task};

use crate::daemon::{self, Event};
use crate::notice::{self, Draft, Notice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Running,
    Offline(String),
}

pub struct Model {
    pub link: Link,
    /// What the daemon told when the link came up (its host and working folder).
    pub info: Option<DaemonInfo>,
    /// The daemon serves the demo data: clocks show daemon time (UTC), as the artboards.
    pub demo: bool,
    pub war: Option<WarRoom>,
    /// The data of a custom theme layout. Built on a model change, and only while a custom
    /// theme is on (never per frame).
    pub snapshot: std::rc::Rc<crate::themed::data::ThemeSnapshot>,
    /// The Display background: the deep dive and the Inbox draw their console with it too.
    pub background: ConsoleBackground,
    /// `--stress N`: copy live sessions until N are live (performance checks).
    pub stress: usize,
    /// `AIVORY_DEMO_SESSIONS=<n>`: the demo shows exactly n live sessions (screenshot runs).
    demo_sessions: Option<usize>,
    /// The demo sessions that `demo_sessions` took away; a delta does not bring them back.
    demo_dropped: std::collections::HashSet<String>,
    /// `--fps`: redraw every frame and log the frame rate.
    pub fps: bool,
    /// `--demo-rich`: sample transcripts with markdown, diffs, colors and todos.
    rich: bool,
    /// `--scroll-row`: the war room scrolls to this row at start.
    pub scroll_row: Option<usize>,
    /// The notices on screen (failures of calls, results of actions). The shell draws them.
    pub notices: Vec<Notice>,
    next_notice: u64,
    endpoint: Endpoint,
    /// A background call reports its failure here; the model task turns it into a notice.
    drafts: UnboundedSender<Draft>,
    _feed: Task<()>,
    _drafts: Task<()>,
    _coffee: Task<()>,
}

impl Model {
    pub fn new(
        endpoint: Endpoint,
        stress: usize,
        fps: bool,
        rich: bool,
        scroll_row: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut events = daemon::spawn_feed(endpoint.clone());
        let feed = cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                let applied = this.update(cx, |this, cx| {
                    this.on_event(event, cx);
                    this.rebuild_snapshot();
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        let (drafts, mut inbox) = unbounded::<Draft>();
        let notices = cx.spawn(async move |this, cx| {
            while let Some(draft) = inbox.next().await {
                let applied = this.update(cx, |this, cx| this.notify_user(draft, cx));
                if applied.is_err() {
                    break;
                }
            }
        });
        // `AIVORY_COFFEE_AFTER_SECS=<n>` shows the hint after n seconds, always (screenshot runs).
        let forced: Option<u64> = std::env::var("AIVORY_COFFEE_AFTER_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        let wait = forced.map_or(notice::COFFEE_AFTER, std::time::Duration::from_secs);
        let coffee = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            loop {
                let Ok(settings) = this.update(cx, |this, cx| {
                    this.call::<method::SettingsGet>(Default::default(), cx)
                }) else {
                    return;
                };
                let enabled = forced.is_some()
                    || match settings.await {
                        Ok(view) => view.settings.coffee_reminder,
                        // No daemon, no hint: try again later.
                        Err(_) => true,
                    };
                if !enabled {
                    return;
                }
                let shown = this.update(cx, |this, cx| {
                    let open = this.war.as_ref().map_or(1, |war| war.inbox_open);
                    let due = forced.is_some()
                        || notice::coffee_due(enabled, this.demo, open)
                            && this.link == Link::Running;
                    if due {
                        this.notify_user(notice::coffee(), cx);
                    }
                    due
                });
                if !matches!(shown, Ok(false)) {
                    return;
                }
                cx.background_executor().timer(notice::COFFEE_RETRY).await;
            }
        });
        Self {
            link: Link::Connecting,
            info: None,
            demo: false,
            war: None,
            snapshot: Default::default(),
            background: Display::default().background,
            stress,
            demo_sessions: std::env::var("AIVORY_DEMO_SESSIONS")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            demo_dropped: Default::default(),
            fps,
            rich,
            scroll_row,
            notices: Vec::new(),
            next_notice: 0,
            endpoint,
            drafts,
            _feed: feed,
            _drafts: notices,
            _coffee: coffee,
        }
    }

    /// A model with no feed and no timers, for tests that draw a view (the test scheduler
    /// stops a test that sees activity of another thread). The tests set `war` themselves.
    #[cfg(test)]
    pub fn offline(endpoint: Endpoint) -> Self {
        let (drafts, _) = unbounded::<Draft>();
        Self {
            link: Link::Running,
            info: None,
            demo: true,
            war: None,
            snapshot: Default::default(),
            background: Display::default().background,
            stress: 0,
            demo_sessions: None,
            demo_dropped: Default::default(),
            fps: false,
            rich: false,
            scroll_row: None,
            notices: Vec::new(),
            next_notice: 0,
            endpoint,
            drafts,
            _feed: Task::ready(()),
            _drafts: Task::ready(()),
            _coffee: Task::ready(()),
        }
    }

    /// The endpoint of the daemon.
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    /// Build the theme snapshot again (a custom theme is on).
    pub fn rebuild_snapshot(&mut self) {
        if crate::themed::active() {
            self.snapshot = std::rc::Rc::new(crate::themed::data::ThemeSnapshot::build(
                self.war.as_ref(),
                self.info.as_ref(),
                1440,
            ));
        }
    }

    /// The console theme of the Display setting.
    pub fn console_theme(&self) -> crate::theme::PaneTheme {
        // A dark look never shows a white pane; Bridge has its own console (ADR 0022).
        crate::theme::PaneTheme::for_look(self.background)
    }

    /// Read the Display background from the daemon again (the war room or Settings may have
    /// changed it).
    pub fn refresh_display(&self, cx: &mut Context<Self>) {
        let settings = self.call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            if let Ok(view) = settings.await {
                let _ = this.update(cx, |this, cx| {
                    crate::ui::set_ui_scale(view.settings.display.ui_scale);
                    crate::look::apply(&view.settings.display, false);
                    this.rebuild_snapshot();
                    crate::themed::sync(
                        &cx.entity(),
                        this.endpoint(),
                        view.settings.display.custom_theme.as_deref(),
                        cx,
                    );
                    cx.refresh_windows();
                    if this.background != view.settings.display.background {
                        this.background = view.settings.display.background;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Show a notice. It hides after [`notice::HIDE_AFTER`]; at most [`notice::MAX_OPEN`] show.
    pub fn notify_user(&mut self, draft: Draft, cx: &mut Context<Self>) {
        // The same text twice in a row is one notice (a failing call in a loop).
        if self.notices.iter().any(|n| n.text.as_ref() == draft.text) {
            return;
        }
        self.next_notice += 1;
        let id = self.next_notice;
        self.notices.push(Notice {
            id,
            kind: draft.kind,
            text: draft.text.into(),
            help: draft.help,
        });
        while self.notices.len() > notice::MAX_OPEN {
            self.notices.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(notice::HIDE_AFTER).await;
            let _ = this.update(cx, |this, cx| this.dismiss_notice(id, cx));
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_notice(&mut self, id: u64, cx: &mut Context<Self>) {
        self.notices.retain(|n| n.id != id);
        cx.notify();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Connected(info) => {
                // The link came back after a drop: say so (a restart of the daemon shows here).
                let back = matches!(self.link, Link::Offline(_));
                self.link = Link::Running;
                self.demo = info.demo;
                self.info = Some(info);
                self.refresh_display(cx);
                if back {
                    self.notify_user(Draft::info("aivoryd is back. The link is up again."), cx);
                }
            }
            Event::Subscribed(sub) => {
                if let Some(mut snapshot) = sub.snapshot {
                    if self.rich {
                        crate::console::sample::enrich(&mut snapshot);
                    }
                    inflate(&mut snapshot, self.stress);
                    if self.demo
                        && let Some(n) = self.demo_sessions
                    {
                        self.demo_dropped = fit_demo(&mut snapshot, n);
                    }
                    self.war = Some(snapshot);
                }
                if let Some(war) = &mut self.war {
                    for note in &sub.deltas {
                        war.apply(note.seq, &note.delta);
                    }
                    war.sessions.retain(|s| !self.demo_dropped.contains(&s.id));
                    answerable(war);
                }
            }
            Event::Delta(note) => {
                if let Some(war) = &mut self.war {
                    war.apply(note.seq, &note.delta);
                    if !self.demo_dropped.is_empty() {
                        war.sessions.retain(|s| !self.demo_dropped.contains(&s.id));
                    }
                    answerable(war);
                }
            }
            Event::Offline(reason) => {
                let dropped = self.link == Link::Running;
                self.link = Link::Offline(reason);
                if dropped {
                    self.notify_user(
                        Draft::failure("aivoryd does not answer. The app tries to connect again."),
                        cx,
                    );
                }
            }
        }
    }

    /// Call a method in the background. The war room changes come back as deltas.
    pub fn call<M>(&self, params: M::Params, cx: &App) -> Task<anyhow::Result<M::Result>>
    where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        let endpoint = self.endpoint.clone();
        cx.background_executor()
            .spawn(async move { daemon::call::<M>(&endpoint, &params) })
    }

    /// Fire and forget. A failure shows to the user as a notice, and goes to the log.
    pub fn send<M>(&self, params: M::Params, cx: &App)
    where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        let endpoint = self.endpoint.clone();
        let drafts = self.drafts.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = daemon::call::<M>(&endpoint, &params) {
                    eprintln!("aivory: {} failed: {e:#}", M::NAME);
                    let _ = drafts.unbounded_send(notice::failure(M::NAME, &e));
                }
            })
            .detach();
    }

    /// Wait for a call and show a failure as a notice. The result goes to `done`.
    pub fn call_notify<M>(&self, params: M::Params, cx: &App) -> Task<Option<M::Result>>
    where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        let endpoint = self.endpoint.clone();
        let drafts = self.drafts.clone();
        cx.background_executor().spawn(async move {
            match daemon::call::<M>(&endpoint, &params) {
                Ok(result) => Some(result),
                Err(e) => {
                    eprintln!("aivory: {} failed: {e:#}", M::NAME);
                    let _ = drafts.unbounded_send(notice::failure(M::NAME, &e));
                    None
                }
            }
        })
    }

    /// `HH:MM` of a daemon time: UTC in demo mode, local time otherwise.
    pub fn clock(&self, at: aivory_protocol::UnixMs) -> String {
        let Some(t) = chrono::DateTime::from_timestamp_millis(at) else {
            return String::new();
        };
        if self.demo {
            t.format("%H:%M").to_string()
        } else {
            t.with_timezone(&chrono::Local).format("%H:%M").to_string()
        }
    }
}

/// Copy live sessions (with new ids) until `live` sessions are live. Deltas do not reach the
/// copies; this is for performance checks only.
fn inflate(war: &mut WarRoom, live: usize) {
    let originals: Vec<_> = war
        .sessions
        .iter()
        .filter(|s| s.state.is_live())
        .cloned()
        .collect();
    if originals.is_empty() {
        return;
    }
    let mut count = originals.len();
    let mut k = 0;
    while count < live {
        let mut copy = originals[k % originals.len()].clone();
        copy.id = format!("{}~{k}", copy.id);
        war.sessions.push(copy);
        count += 1;
        k += 1;
    }
}

/// Trim or copy the live sessions to exactly `n` (`AIVORY_DEMO_SESSIONS`, demo data only).
/// The kept sessions mix the states: Needs you first, then Working, Blocked and Idle, in turn.
/// Returns the ids that left.
fn fit_demo(war: &mut WarRoom, n: usize) -> std::collections::HashSet<String> {
    use aivory_protocol::SessionState;
    let live = war.sessions.iter().filter(|s| s.state.is_live()).count();
    if live <= n {
        inflate(war, n);
        return Default::default();
    }
    let order = [
        SessionState::NeedsYou,
        SessionState::Working,
        SessionState::Error,
        SessionState::Idle,
    ];
    let buckets: Vec<Vec<String>> = order
        .iter()
        .map(|state| {
            war.sessions
                .iter()
                .filter(|s| s.state.is_live() && s.state == *state)
                .map(|s| s.id.clone())
                .collect()
        })
        .collect();
    let mut keep = std::collections::HashSet::new();
    let mut k = 0;
    while keep.len() < n {
        for bucket in &buckets {
            if keep.len() < n
                && let Some(id) = bucket.get(k)
            {
                keep.insert(id.clone());
            }
        }
        k += 1;
    }
    let dropped: std::collections::HashSet<String> = war
        .sessions
        .iter()
        .filter(|s| s.state.is_live() && !keep.contains(&s.id))
        .map(|s| s.id.clone())
        .collect();
    war.sessions.retain(|s| !dropped.contains(&s.id));
    dropped
}

/// aivoryd answers an open ask of every session: a session in the user's own terminal is
/// resumed through its CLI (`--resume`). So the looks show the answer controls for every
/// session with an ask; `controlled` stays false for the others (no pause or stop there).
fn answerable(war: &mut aivory_protocol::WarRoom) {
    for s in &mut war.sessions {
        if s.ask.is_some() {
            s.controlled = true;
        }
    }
}

#[cfg(test)]
mod demo_tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    fn live(war: &WarRoom) -> usize {
        war.sessions.iter().filter(|s| s.state.is_live()).count()
    }

    #[test]
    fn the_demo_count_trims_and_copies_to_exactly_n() {
        for n in [1, 6, 30, 40] {
            let mut war = Demo::new().war_room;
            fit_demo(&mut war, n);
            assert_eq!(live(&war), n, "n = {n}");
        }
        let mut war = Demo::new().war_room;
        fit_demo(&mut war, 1);
        let one = war.sessions.iter().find(|s| s.state.is_live()).unwrap();
        assert_eq!(one.state, aivory_protocol::SessionState::NeedsYou);
    }
}
