//! The Settings screen (`Settings.dc.html`): providers and models, autopilot, display, daemons
//! and devices, notifications, account. Every control sends `settings/set` (or a device call)
//! and shows the value that the daemon returns. Changes save at once.

use aivory_protocol::actions::{
    AddAcpParams, DaemonRef, DaemonScaleParams, ImportChange, ImportReport, ProviderRef,
};
use aivory_protocol::rpc::{DeviceRef, Empty, SettingsSetParams, method};
use aivory_protocol::{
    Account, Autopilot, Channels, ConsoleBackground, Daemon, DaemonKind, Device, Display, Effort,
    LinkState, Mode, Notify, Pairing, ProviderInfo, ProviderPrefs, ProviderStatus, QueueOrder,
    SettingsView, Theme, UnixMs, UsageWindow,
};
use gpui::prelude::*;
use gpui::{
    BoxShadow, ClipboardItem, Context, Div, Entity, FontWeight, Pixels, ScrollHandle, SharedString,
    Stateful, Subscription, Window, canvas, div, fill, point, px, relative, rgb, rgba, size,
};
use gpui_base::input::{InputEvent, InputState};
use serde_json::{Value, json};

use crate::forms;
use crate::model::Model;
use crate::notice::Draft;
use crate::theme::*;
use crate::ui;

mod themes;

/// The row divider inside a card: rgba(13,13,14,.06).
const ROW_LINE: u32 = 0x0D0D0E0F;
/// The track of a usage bar and the hover of a nav item.
fn soft() -> u32 {
    crate::theme::pick(0xECEEF2, tk().track)
}
fn button_line() -> u32 {
    tk().edge
}
fn ok_bg() -> u32 {
    tk().ok_bg
}
fn ok_text() -> u32 {
    tk().ok_text
}
/// The window width under which the section nav becomes a row.
const COMPACT: f32 = 900.;
const NAV_W: f32 = 260.;
const CONTENT_MAX: f32 = 880.;
/// A usage bar turns orange above this percent.
const HOT_PCT: u8 = 75;
/// The sample line of the console preview.
const PREVIEW: &str = "● Bash  cargo test -p orch budget    └ test result: ok. 22 passed";

// ---- Pure parts ----

/// The setting keys of the controls: dotted camelCase, as `Settings::set_key` reads them.
mod keys {
    pub fn provider(id: &str, field: &str) -> String {
        format!("providers.{id}.{field}")
    }

    pub fn autopilot(field: &str) -> String {
        format!("autopilot.{field}")
    }

    pub fn display(field: &str) -> String {
        format!("display.{field}")
    }

    pub fn notify(event: &str, channel: &str) -> String {
        format!("notify.{event}.{channel}")
    }

    pub const QUIET_HOURS: &str = "quietHours.on";
    pub const COFFEE_REMINDER: &str = "coffeeReminder";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Providers,
    Autopilot,
    Display,
    Devices,
    Notifications,
    Account,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Providers,
        Section::Autopilot,
        Section::Display,
        Section::Devices,
        Section::Notifications,
        Section::Account,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Providers => "Providers and models",
            Section::Autopilot => "Autopilot",
            Section::Display => "Display",
            Section::Devices => "Daemons and devices",
            Section::Notifications => "Notifications",
            Section::Account => "About and license",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The small text at the right of a nav item.
    fn count(self, view: &SettingsView) -> String {
        match self {
            Section::Providers => view.providers.len().to_string(),
            Section::Autopilot => if view.settings.autopilot.on {
                "on"
            } else {
                "off"
            }
            .to_string(),
            Section::Devices => (view.daemons.len() + view.devices.len()).to_string(),
            _ => String::new(),
        }
    }
}

/// An autopilot rule with a stepper.
struct Rule {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    lo: u32,
    hi: u32,
    step: u32,
}

const RULES: [Rule; 3] = [
    Rule {
        key: "parkAfterMin",
        label: "Park idle sessions after",
        help: "A parked session keeps its history and resumes in one click.",
        lo: Autopilot::PARK_AFTER_MIN.0,
        hi: Autopilot::PARK_AFTER_MIN.1,
        step: Autopilot::PARK_AFTER_MIN.2,
    },
    Rule {
        key: "maxRunning",
        label: "Max running sessions",
        help: "Across all codebases and daemons.",
        lo: Autopilot::MAX_RUNNING.0,
        hi: Autopilot::MAX_RUNNING.1,
        step: 1,
    },
    Rule {
        key: "maxPerCodebase",
        label: "Max sessions per codebase",
        help: "Keeps one repo from taking all slots.",
        lo: Autopilot::MAX_PER_CODEBASE.0,
        hi: Autopilot::MAX_PER_CODEBASE.1,
        step: 1,
    },
];

fn rule_value(a: &Autopilot, key: &str) -> u32 {
    match key {
        "parkAfterMin" => a.park_after_min,
        "maxRunning" => a.max_running,
        _ => a.max_per_codebase,
    }
}

fn rule_text(key: &str, value: u32) -> String {
    if key == "parkAfterMin" {
        format!("{value} min")
    } else {
        value.to_string()
    }
}

/// The value after one step, kept in its range.
fn step_value(value: i64, lo: i64, hi: i64, delta: i64) -> i64 {
    (value + delta).clamp(lo, hi)
}

fn autopilot_text(a: &Autopilot) -> String {
    if a.on {
        format!(
            "On. Parks idle sessions after {} min and keeps at most {} running.",
            a.park_after_min, a.max_running
        )
    } else {
        "Off. Idle sessions stay open.".to_string()
    }
}

fn queue_text(queued: u32) -> String {
    match queued {
        0 => "No task is in the queue now.".to_string(),
        1 => "1 task is in the queue now.".to_string(),
        n => format!("{n} tasks are in the queue now."),
    }
}

/// A usage bar is orange above 75 %.
fn usage_hot(pct: u8) -> bool {
    pct > HOT_PCT
}

/// The notification events: key, title, help.
const EVENTS: [(&str, &str, &str); 5] = [
    (
        "needsAnswer",
        "Needs an answer",
        "Question, permission or plan",
    ),
    ("error", "Error", "A session stops on an error"),
    (
        "review",
        "Ready for review",
        "A task reaches the review gate",
    ),
    ("conflict", "Conflict", "Two sessions edit the same code"),
    (
        "autopilot",
        "Autopilot action",
        "A session parks or a queued task starts",
    ),
];

/// The notification channels: key, label.
const CHANNELS: [(&str, &str); 3] = [
    ("desktop", "Desktop"),
    ("phone", "Phone"),
    ("email", "E-mail"),
];

fn notify_on(n: &Notify, event: &str, channel: &str) -> bool {
    let c: Channels = match event {
        "needsAnswer" => n.needs_answer,
        "error" => n.error,
        "review" => n.review,
        "conflict" => n.conflict,
        _ => n.autopilot,
    };
    match channel {
        "desktop" => c.desktop,
        "phone" => c.phone,
        _ => c.email,
    }
}

/// The badge of a provider card: an off switch hides the status.
fn provider_badge(status: ProviderStatus, enabled: bool) -> (&'static str, Tone) {
    match (enabled, status) {
        (false, _) | (_, ProviderStatus::Off) => ("off", Tone::Off),
        (true, ProviderStatus::Connected) => ("connected", Tone::Ok),
        (true, ProviderStatus::Acp) => ("ACP", Tone::Off),
    }
}

/// The buttons of a daemon row. Restart and Open shell need this machine: a remote daemon has
/// neither. Every daemon can scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DaemonButtons {
    restart: bool,
    shell: bool,
    scale: bool,
}

fn daemon_buttons(kind: DaemonKind) -> DaemonButtons {
    let local = kind == DaemonKind::Local;
    DaemonButtons {
        restart: local,
        shell: local,
        scale: true,
    }
}

/// `settings.model: opus-5.5 → sonnet-5.5`, or `key: → value` when nothing was set before.
fn change_line(change: &ImportChange) -> String {
    let show = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match &change.from {
        Some(from) => format!("{}: {} → {}", change.key, show(from), show(&change.to)),
        None => format!("{}: {}", change.key, show(&change.to)),
    }
}

/// The words over the list of an import.
fn import_summary(report: &ImportReport) -> String {
    if !report.found {
        return "No Claude Code settings file was found on this machine.".to_string();
    }
    match report.changes.len() {
        0 => "Nothing to change. AIVory Code and Claude Code agree.".to_string(),
        1 => "Imported 1 value from the Claude Code settings.".to_string(),
        n => format!("Imported {n} values from the Claude Code settings."),
    }
}

/// The call of Add an ACP agent, or the first thing that is wrong with the input.
fn acp_request(name: &str, command: &str, args: &str) -> Result<AddAcpParams, String> {
    Ok(AddAcpParams {
        name: forms::acp_name(name)?,
        command: forms::acp_command(command)?,
        args: forms::split_args(args)?,
        env: None,
    })
}

/// The fields of the Add an ACP agent form.
#[derive(Default)]
struct AcpForm {
    name: Option<Entity<InputState>>,
    command: Option<Entity<InputState>>,
    args: Option<Entity<InputState>>,
    error: Option<String>,
    busy: bool,
    _events: Vec<Subscription>,
}

/// The page of the team on the hub, when a hub is set up.
fn team_url(account: &Account) -> Option<String> {
    let hub = account.hub.trim();
    (account.hub_state != LinkState::Off && !hub.is_empty()).then(|| format!("https://{hub}"))
}

fn hub_badge(state: LinkState) -> (&'static str, Tone) {
    match state {
        LinkState::Connected => ("connected", Tone::Ok),
        LinkState::Connecting => ("connecting", Tone::Warn),
        LinkState::Offline => ("offline", Tone::Error),
        LinkState::Off => ("off", Tone::Off),
    }
}

/// `2 min ago`, `3 h ago`, `yesterday`.
fn seen_text(last_seen: UnixMs, now: UnixMs) -> String {
    let minutes = (now - last_seen).max(0) / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        1440..=2879 => "yesterday".to_string(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

/// `YYYY-MM-DD`: UTC for the demo data, local time otherwise.
fn date_text(at: UnixMs, local: bool) -> String {
    let Some(t) = chrono::DateTime::from_timestamp_millis(at) else {
        return String::new();
    };
    if local {
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

/// The modules of a QR code: `width` x `width`, row by row, `true` is dark.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Qr {
    width: usize,
    dark: Vec<bool>,
}

fn qr_matrix(payload: &str) -> Option<Qr> {
    let code = qrcode::QrCode::with_error_correction_level(payload, qrcode::EcLevel::M).ok()?;
    Some(Qr {
        width: code.width(),
        dark: code
            .into_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect(),
    })
}

impl Qr {
    /// The runs of dark modules of one row: (first module, length).
    fn runs(&self, row: usize) -> Vec<(usize, usize)> {
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..=self.width {
            let dark = x < self.width && self.dark[row * self.width + x];
            match (dark, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, x - s));
                    start = None;
                }
                _ => {}
            }
        }
        runs
    }
}

/// The text of the expiry line: `code AIV-4820 · valid until 14:50`.
fn code_text(code: &str, until: &str) -> String {
    format!("code {code} · valid until {until}")
}

// ---- Widgets ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Ok,
    Off,
    Warn,
    Error,
}

fn badge(label: &str, tone: Tone) -> Div {
    let (bg, text) = match tone {
        Tone::Ok => (ok_bg(), ok_text()),
        Tone::Off => (tk().track, tk().ink_3),
        Tone::Warn => (tk().q_bg, tk().q_text),
        Tone::Error => (tk().e_bg, tk().e_text),
    };
    div()
        .flex_none()
        .px(px(9.))
        .py(px(2.))
        .rounded_full()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_size(crate::ui::ts(crate::theme::T_CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .when(ui::irix(), |d| {
            d.rounded(px(0.)).border_1().border_color(rgb(text))
        })
        .child(label.to_string())
}

fn card() -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .bg(rgb(tk().card))
        .rounded(tk().r_card)
        .shadow(ui::card_shadow())
        .when(ui::irix(), |d| {
            d.rounded(px(0.)).shadow(Vec::new()).bg(rgb(0xBDBDBD))
        })
        .when(ui::irix(), |d| ui::bevel_edges(true, d))
}

/// The title and help text at the left of a row.
fn row_label(title: impl Into<SharedString>, help: impl Into<SharedString>) -> Div {
    div()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .font_weight(FontWeight::MEDIUM)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .line_height(relative(1.45))
                .text_color(rgb(tk().ink_3))
                .child(help.into()),
        )
}

fn section_head(title: &str, lead: Option<&str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(crate::ui::ts(crate::theme::T_HEADING))
                .font_weight(FontWeight::SEMIBOLD)
                .when(ui::irix(), |d| {
                    // An etched section title, as in the other Navigator screens.
                    d.font_weight(FontWeight::BOLD)
                        .pb(px(6.))
                        .border_b_1()
                        .border_color(rgb(0x6A6A6A))
                })
                .child(title.to_string()),
        )
        .when_some(lead, |d, text| {
            d.child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.55))
                    .text_color(rgb(tk().ink_2))
                    .child(text.to_string()),
            )
        })
}

fn knob_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: ui::hsla(0x0D0D0E33),
        offset: point(px(0.), px(1.)),
        blur_radius: px(2.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// The 36x20 switch of the design: blue when on.
fn switch(on: bool) -> Div {
    div()
        .flex_none()
        .relative()
        .w(px(36.))
        .h(px(20.))
        .rounded_full()
        .bg(rgb(if on { tk().primary } else { tk().switch_off }))
        .when(ui::irix(), |d| {
            d.rounded(px(0.))
                .bg(rgb(if on { 0x5E7396 } else { 0xA0A0A0 }))
        })
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 18. } else { 2. }))
                .size(px(16.))
                .rounded_full()
                .bg(rgb(tk().card))
                .shadow(knob_shadow())
                .when(ui::irix(), |d| d.rounded(px(0.)).shadow(Vec::new()))
                .when(ui::irix(), |d| ui::bevel_edges(true, d)),
        )
        .when(ui::irix(), |d| ui::bevel_edges(false, d))
}

/// The outline button of the design (`.gb`); `primary` is the blue one (`.gb.pri`).
fn button(id: impl Into<SharedString>, label: &str, primary: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(32.))
        .px(px(12.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(if primary { tk().primary } else { button_line() }))
        .bg(rgb(if primary { tk().primary } else { tk().card }))
        .text_color(rgb(if primary { tk().card } else { tk().ink }))
        .when(primary, |d| d.font_weight(FontWeight::MEDIUM))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .whitespace_nowrap()
        .cursor_pointer()
        .when(primary, |d| d.hover(|s| s.bg(rgb(tk().primary_hover))))
        .when(!primary, |d| d.hover(|s| s.border_color(rgb(tk().muted))))
        .when(ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(0xC8C8C8))
                .text_color(rgb(0x111111))
        })
        .when(ui::irix() && primary, |d| {
            d.border_2().border_color(rgb(0x111111))
        })
        .when(ui::irix(), |d| ui::bevel_edges(true, d))
        .child(label.to_string())
}

/// A choice of a segmented control: label, the value it saves, its own font.
struct Choice {
    label: SharedString,
    on: bool,
    value: Value,
    font: Option<&'static str>,
}

impl Choice {
    fn new(label: impl Into<SharedString>, on: bool, value: Value) -> Self {
        Self {
            label: label.into(),
            on,
            value,
            font: None,
        }
    }
}

/// A usage bar: label and value on a line, then the 6 px bar.
fn usage_bar(window: &UsageWindow) -> Div {
    let hot = usage_hot(window.pct);
    let estimate = if window.estimate { " estimate" } else { "" };
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgb(tk().ink_2))
                .child(window.label.clone())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(tk().ink))
                        .child(format!("{}%{estimate}", window.pct)),
                ),
        )
        .child(
            div()
                .h(px(6.))
                .rounded(px(3.))
                .bg(rgb(soft()))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(f32::from(window.pct.min(100)) / 100.))
                        .rounded(px(3.))
                        .bg(rgb(if hot { tk().q } else { tk().ink })),
                ),
        )
}

/// The QR code, drawn as quads: one per run of dark modules.
fn qr_view(qr: Qr) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let n = qr.width as f32;
            let module = (f32::from(bounds.size.width.min(bounds.size.height)) / n)
                .floor()
                .max(1.);
            let side = px(module * n);
            let x0 = bounds.origin.x + (bounds.size.width - side) / 2.;
            let y0 = bounds.origin.y + (bounds.size.height - side) / 2.;
            for y in 0..qr.width {
                for (start, len) in qr.runs(y) {
                    window.paint_quad(fill(
                        gpui::Bounds::new(
                            point(x0 + px(start as f32 * module), y0 + px(y as f32 * module)),
                            size(px(len as f32 * module), px(module)),
                        ),
                        rgb(tk().ink),
                    ));
                }
            }
        },
    )
    .size_full()
}

// ---- The screen ----

pub struct SettingsScreen {
    model: Entity<Model>,
    section: Section,
    view: Option<SettingsView>,
    /// `settings/get` failed.
    load_error: Option<String>,
    /// The last save that failed, shown in the header.
    save_error: Option<String>,
    /// `settings/set` calls sent and the newest answer applied: an older answer is dropped.
    sent: u64,
    applied: u64,
    pairing: Option<(Pairing, Option<Qr>)>,
    pairing_busy: bool,
    pairing_error: Option<String>,
    /// The pairing link is shown (and copied).
    link_shown: bool,
    /// The Add an ACP agent form is open.
    acp_open: bool,
    acp: AcpForm,
    /// The result of the last import from the Claude Code settings.
    import: Option<ImportReport>,
    importing: bool,
    /// The window is narrower than the rail limit: rows stack.
    narrow: bool,
    /// The scroll position of the body (the Navigator look draws a bar for it).
    scroll: ScrollHandle,
    themes: themes::ThemesPanel,
}

impl SettingsScreen {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            model,
            // `AIVORY_SETTINGS=about` opens that section (screenshot runs).
            section: match std::env::var("AIVORY_SETTINGS").as_deref() {
                Ok("about") => Section::Account,
                Ok("display") => Section::Display,
                _ => Section::Providers,
            },
            view: None,
            load_error: None,
            save_error: None,
            sent: 0,
            applied: 0,
            pairing: None,
            pairing_busy: false,
            pairing_error: None,
            link_shown: false,
            acp_open: false,
            acp: AcpForm::default(),
            import: None,
            importing: false,
            narrow: false,
            scroll: ScrollHandle::new(),
            themes: Default::default(),
        };
        this.refresh(cx);
        this
    }

    /// Read the settings again (the war room can change the autopilot meanwhile).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let at = self.sent;
        let call = self
            .model
            .read(cx)
            .call::<method::SettingsGet>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(mut view) => {
                        // A save went out meanwhile: its answer is newer than this read.
                        if at != this.sent
                            && let Some(old) = &this.view
                        {
                            view.settings = old.settings.clone();
                        }
                        this.view = Some(view);
                        this.load_error = None;
                    }
                    Err(e) => {
                        eprintln!("aivory: settings/get failed: {e:#}");
                        this.load_error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Save one value and show what the daemon returns.
    fn save(&mut self, key: String, value: Value, cx: &mut Context<Self>) {
        self.sent += 1;
        let seq = self.sent;
        let call = self.model.read(cx).call::<method::SettingsSet>(
            SettingsSetParams {
                key: key.clone(),
                value,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(settings) => {
                        if crate::look::apply(&settings.display, false) {
                            cx.refresh_windows();
                        }
                        let max_full = settings.display.maximize_full_screen;
                        this.model.update(cx, |m, cx| {
                            if max_full && !m.maximize_full_screen {
                                m.full_screen_hint = Some(std::time::Instant::now());
                            }
                            m.maximize_full_screen = max_full;
                            cx.notify();
                        });
                        let endpoint = this.model.read(cx).endpoint();
                        crate::themed::sync(
                            &this.model.clone(),
                            endpoint,
                            settings.display.custom_theme.as_deref(),
                            cx,
                        );
                        if seq > this.applied {
                            this.applied = seq;
                            if let Some(view) = &mut this.view {
                                view.settings = settings;
                            }
                        }
                        this.save_error = None;
                    }
                    Err(e) => {
                        eprintln!("aivory: settings/set {key} failed: {e:#}");
                        this.save_error = Some(format!("Cannot save {key}: {e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        if section == Section::Devices && self.pairing.is_none() && self.pairing_error.is_none() {
            self.pair(cx);
        }
        cx.notify();
    }

    /// Start a pairing: a new code replaces the old one.
    fn pair(&mut self, cx: &mut Context<Self>) {
        if self.pairing_busy {
            return;
        }
        self.pairing_busy = true;
        self.link_shown = false;
        let call = self
            .model
            .read(cx)
            .call::<method::DevicesPair>(Default::default(), cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.pairing_busy = false;
                match result {
                    Ok(pairing) => {
                        let qr = qr_matrix(&pairing.url);
                        this.pairing = Some((pairing, qr));
                        this.pairing_error = None;
                    }
                    Err(e) => {
                        eprintln!("aivory: devices/pair failed: {e:#}");
                        this.pairing_error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_device(&mut self, id: String, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::DevicesRemove>(DeviceRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        this.save_error = None;
                        this.refresh(cx);
                    }
                    Err(e) => {
                        eprintln!("aivory: devices/remove {id} failed: {e:#}");
                        this.save_error = Some(format!("Cannot remove {id}: {e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_link(&mut self, cx: &mut Context<Self>) {
        if let Some((pairing, _)) = &self.pairing {
            cx.write_to_clipboard(ClipboardItem::new_string(pairing.url.clone()));
            self.link_shown = true;
            cx.notify();
        }
    }

    /// A notice for the user.
    fn toast(&self, draft: Draft, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.notify_user(draft, cx));
    }

    /// Open or close the Add an ACP agent form.
    fn toggle_acp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.acp_open = !self.acp_open;
        if self.acp_open {
            self.ensure_acp_form(window, cx);
            if let Some(name) = self.acp.name.clone() {
                name.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        cx.notify();
    }

    fn ensure_acp_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.acp.name.is_some() {
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Goose"));
        let command = cx.new(|cx| InputState::new(window, cx).placeholder("goose"));
        let args = cx.new(|cx| InputState::new(window, cx).placeholder("acp --stdio"));
        let events = vec![
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && let Some(next) = this.acp.command.clone()
                {
                    next.update(cx, |s, cx| s.focus(window, cx));
                }
            }),
            cx.subscribe_in(
                &command,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event
                        && let Some(next) = this.acp.args.clone()
                    {
                        next.update(cx, |s, cx| s.focus(window, cx));
                    }
                },
            ),
            cx.subscribe_in(&args, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_acp(cx);
                }
            }),
        ];
        self.acp.name = Some(name);
        self.acp.command = Some(command);
        self.acp.args = Some(args);
        self.acp._events = events;
    }

    /// Add an ACP agent: the daemon checks that the command exists and lists it as a provider.
    fn add_acp(&mut self, cx: &mut Context<Self>) {
        if self.acp.busy {
            return;
        }
        let (Some(name), Some(command), Some(args)) = (
            self.acp.name.clone(),
            self.acp.command.clone(),
            self.acp.args.clone(),
        ) else {
            return;
        };
        let params = match acp_request(
            &name.read(cx).value(),
            &command.read(cx).value(),
            &args.read(cx).value(),
        ) {
            Ok(params) => params,
            Err(e) => {
                self.acp.error = Some(e);
                cx.notify();
                return;
            }
        };
        let label = params.name.clone();
        self.acp.busy = true;
        self.acp.error = None;
        let call = self
            .model
            .read(cx)
            .call::<method::ProvidersAddAcp>(params, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.acp.busy = false;
                match result {
                    Ok(added) => {
                        this.acp_open = false;
                        let text = if added.found {
                            let version = added.version.map(|v| format!(" ({v})")).unwrap_or_default();
                            Draft::info(format!("Added the agent {label}{version}."))
                        } else {
                            Draft::failure(format!(
                                "Added the agent {label}, but the command was not found on this machine."
                            ))
                        };
                        this.toast(text, cx);
                        this.refresh(cx);
                    }
                    Err(e) => this.acp.error = Some(crate::notice::reason(&e)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_acp(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call_notify::<method::ProvidersRemoveAcp>(ProviderRef { id }, cx);
        cx.spawn(async move |this, cx| {
            if call.await.is_some() {
                let _ = this.update(cx, |this, cx| {
                    this.toast(Draft::info(format!("Removed the agent {name}.")), cx);
                    this.refresh(cx);
                });
            }
        })
        .detach();
    }

    /// Import the model and the permission mode from the Claude Code settings.
    fn import_claude(&mut self, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        self.importing = true;
        let call = self
            .model
            .read(cx)
            .call_notify::<method::SettingsImportClaude>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let report = call.await;
            let _ = this.update(cx, |this, cx| {
                this.importing = false;
                if let Some(report) = report {
                    if let Some(view) = &mut this.view {
                        view.settings = report.settings.clone();
                    }
                    this.import = Some(report);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Restart the local daemon. The link drops and comes back: the model tells both.
    fn restart_daemon(&mut self, daemon: String, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call_notify::<method::DaemonRestart>(DaemonRef { daemon }, cx);
        cx.spawn(async move |this, cx| {
            if call.await.is_some() {
                let _ = this.update(cx, |this, cx| {
                    this.toast(Draft::info("aivoryd restarts. The app connects again."), cx)
                });
            }
        })
        .detach();
    }

    /// Open a terminal in the folder of the local daemon.
    fn open_shell(&mut self, cx: &mut Context<Self>) {
        let cwd = self
            .model
            .read(cx)
            .info
            .as_ref()
            .map(|i| i.cwd.clone())
            .unwrap_or_default();
        if let Err(e) = crate::terminal::open(&cwd) {
            self.toast(Draft::failure(format!("Cannot open a shell. {e}")), cx);
        }
    }

    /// Set how many sessions a daemon runs at once.
    fn scale_daemon(&mut self, daemon: String, max_running: u32, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call_notify::<method::DaemonScale>(
            DaemonScaleParams {
                daemon,
                max_running,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Some(autopilot) = call.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(view) = &mut this.view {
                        view.settings.autopilot = autopilot;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn now(&self, cx: &gpui::App) -> UnixMs {
        self.model
            .read(cx)
            .war
            .as_ref()
            .map_or_else(|| chrono::Utc::now().timestamp_millis(), |w| w.now)
    }

    /// A row of a card: label left, control right. Only the first row has no divider. On a
    /// narrow window the control goes under the label; otherwise the label keeps its own width.
    fn row(&self, first: bool, label: Div, control: impl IntoElement) -> Div {
        let narrow = self.narrow;
        div()
            .flex()
            .when(narrow, |d| d.flex_col().items_start().gap(px(10.)))
            .when(!narrow, |d| d.items_center().justify_between().gap(px(16.)))
            .px(px(20.))
            .py(px(14.))
            .when(!first, |d| d.border_t_1().border_color(rgba(ROW_LINE)))
            .child(if narrow {
                label
            } else {
                label.flex_none().whitespace_nowrap()
            })
            .child(control)
    }

    // ---- Controls ----

    fn toggle(
        &self,
        id: impl Into<SharedString>,
        on: bool,
        key: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id.into())
            .flex_none()
            .cursor_pointer()
            .child(switch(on))
            .on_click(cx.listener(move |this, _, _, cx| this.save(key.clone(), json!(!on), cx)))
    }

    /// A segmented control. A click on a choice saves its value under `key`. `dense` is for
    /// short choices such as numbers: they sit closer.
    /// One small card per look: its page, card and main action colors, its name in its face.
    fn look_cards(
        &self,
        current: aivory_protocol::Look,
        custom: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div().flex().flex_wrap().gap(px(8.));
        for (look, name) in crate::look::LOOKS {
            let t = crate::look::tokens(look, aivory_protocol::Theme::Light, false);
            let on = !custom && look == current;
            let key = keys::display("look");
            row = row.child(
                div()
                    .id(SharedString::from(format!("dp-look-{name}")))
                    .w(px(112.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(4.))
                    .rounded(px(10.))
                    .border_2()
                    .border_color(if on { rgb(tk().ink) } else { rgba(0) })
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk().track)))
                    .child(
                        div()
                            .h(px(56.))
                            .rounded(px(6.))
                            .bg(rgb(t.page))
                            .border_1()
                            .border_color(rgba(tk().line))
                            .p(px(6.))
                            .flex()
                            .gap(px(4.))
                            .child(div().w(px(14.)).h_full().rounded(px(3.)).bg(rgb(t.track)))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(div().h(px(22.)).rounded(px(3.)).bg(rgb(t.card)))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(4.))
                                            .child(
                                                div()
                                                    .w(px(22.))
                                                    .h(px(10.))
                                                    .rounded(px(5.))
                                                    .bg(rgb(t.primary)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(10.))
                                                    .h(px(10.))
                                                    .rounded(px(5.))
                                                    .bg(rgb(t.q)),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px(px(2.))
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .font_family(t.sans)
                            .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                            .when(on, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.save(key.clone(), json!(look), cx);
                        }
                    })),
            );
        }
        row
    }

    fn seg(
        &self,
        id: &str,
        key: &str,
        choices: Vec<Choice>,
        dense: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut track = div()
            .flex()
            .flex_wrap()
            .gap(px(2.))
            .p(px(2.))
            .bg(rgb(tk().track))
            .rounded(tk().r_button);
        if ui::irix() {
            track = ui::bevel_edges(false, track.rounded(px(0.)).bg(rgb(0xBDBDBD)).p(px(4.)));
        }
        for (i, choice) in choices.into_iter().enumerate() {
            let (key, value, on) = (key.to_string(), choice.value, choice.on);
            track = track.child(
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .h(px(28.))
                    .px(px(if dense { 8. } else { 11. }))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .when_some(choice.font, |d, f| d.font_family(f))
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![BoxShadow {
                                color: ui::hsla(0x0D0D0E14),
                                offset: point(px(0.), px(1.)),
                                blur_radius: px(2.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    })
                    .when(!on, |d| d.hover(|s| s.text_color(rgb(tk().ink))))
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .shadow(Vec::new())
                            .text_color(rgb(0x111111))
                            .bg(rgb(if on { 0xA6A6A6 } else { 0xC8C8C8 }))
                    })
                    .when(ui::irix(), |d| ui::bevel_edges(!on, d))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !on {
                            this.save(key.clone(), value.clone(), cx);
                        }
                    }))
                    .child(choice.label),
            );
        }
        track
    }

    /// `− value +` with the 30 px buttons of the design. Each click saves the next value.
    fn stepper(
        &self,
        id: &str,
        text: String,
        key: String,
        value: i64,
        (lo, hi, step): (i64, i64, i64),
        cx: &mut Context<Self>,
    ) -> Div {
        let mut stepper = div()
            .flex()
            .flex_none()
            .items_center()
            .rounded(tk().r_button)
            .border_1()
            .border_color(rgba(tk().line_2))
            .text_size(crate::ui::ts(crate::theme::T_SMALL));
        if ui::irix() {
            stepper = ui::bevel_edges(false, stepper.rounded(px(0.)).border_0().bg(rgb(0xFFFFFF)));
        }
        for (suffix, label, delta) in [("dec", "−", -step), ("inc", "+", step)] {
            let key = key.clone();
            let next = step_value(value, lo, hi, delta);
            let button = div()
                .id(SharedString::from(format!("{id}-{suffix}")))
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::ui::ts(crate::theme::T_BODY))
                .text_color(rgb(tk().ink_2))
                .cursor_pointer()
                .hover(|s| s.text_color(rgb(tk().ink)).bg(rgb(tk().page)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if next != value {
                        this.save(key.clone(), json!(next), cx);
                    }
                }))
                .child(label);
            if delta < 0 {
                stepper = stepper.child(button).child(
                    div()
                        .min_w(px(64.))
                        .flex()
                        .justify_center()
                        .child(text.clone()),
                );
            } else {
                stepper = stepper.child(button);
            }
        }
        stepper
    }

    // ---- Sections ----

    fn header(&self) -> Div {
        div()
            .flex_none()
            .h(HEADER_H)
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(32.))
            .bg(rgb(tk().card))
            .border_b_1()
            .border_color(rgba(tk().line))
            .when(ui::irix(), |d| {
                d.bg(rgb(0xBDBDBD)).border_color(rgb(0x6A6A6A))
            })
            .child(
                div()
                    .font_family(crate::theme::tk().display)
                    .text_size(crate::ui::ts(crate::theme::T_DISPLAY))
                    .font_weight(FontWeight::NORMAL)
                    .when(ui::irix(), |d| {
                        d.font_family(tk().sans)
                            .text_size(px(28.))
                            .font_weight(FontWeight::BOLD)
                    })
                    .child("Settings"),
            )
            .child(match &self.save_error {
                Some(error) => div()
                    .min_w(px(0.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().e_text))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(error.clone()),
                None => div()
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child("changes save at once"),
            })
    }

    fn nav(&self, view: &SettingsView, compact: bool, cx: &mut Context<Self>) -> Div {
        let mut nav = div().flex().gap(px(2.));
        nav = if compact {
            nav.flex_none()
                .flex_wrap()
                .px(px(16.))
                .py(px(12.))
                .border_b_1()
        } else {
            nav.flex_none()
                .w(px(NAV_W))
                .h_full()
                .flex_col()
                .pt(px(20.))
                .pb(px(20.))
                .pl(px(32.))
                .pr(px(16.))
                .border_r_1()
        }
        .border_color(rgba(tk().line));
        if ui::irix() {
            nav = ui::bevel_edges(false, nav.bg(rgb(0xFFFFFF)).gap(px(0.)));
            if !compact {
                nav = nav
                    .h_auto()
                    .ml(px(24.))
                    .my(px(24.))
                    .pl(px(14.))
                    .pr(px(14.))
                    .border_0();
            }
        }
        for section in Section::ALL {
            let on = section == self.section;
            let count = section.count(view);
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("nav-{}", section.index())))
                    .h(px(36.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(tk().r_button)
                    .when(!compact, |d| d.w_full())
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .whitespace_nowrap()
                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(rgb(tk().card))
                            .font_weight(FontWeight::MEDIUM)
                            .shadow(vec![
                                BoxShadow {
                                    color: ui::hsla(0x0D0D0E0F),
                                    offset: point(px(0.), px(1.)),
                                    blur_radius: px(2.),
                                    spread_radius: px(0.),
                                    inset: false,
                                },
                                BoxShadow {
                                    color: ui::hsla(0x0D0D0E0F),
                                    offset: point(px(0.), px(0.)),
                                    blur_radius: px(0.),
                                    spread_radius: px(1.),
                                    inset: false,
                                },
                            ])
                    })
                    .when(!on, |d| {
                        d.hover(|s| s.bg(rgb(soft())).text_color(rgb(tk().ink)))
                    })
                    .when(ui::irix(), |d| {
                        d.rounded(px(0.))
                            .shadow(Vec::new())
                            .h(px(40.))
                            .bg(rgb(if on { 0x2F4A7A } else { 0xFFFFFF }))
                            .text_color(rgb(if on { 0xFFFFFF } else { 0x111111 }))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select(section, cx)))
                    .child(section.label())
                    .when(!compact && !count.is_empty(), |d| {
                        d.child(div().flex_1()).child(
                            div()
                                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                                .text_color(rgb(tk().ink_3))
                                .child(count),
                        )
                    }),
            );
        }
        nav
    }

    fn providers_section(&self, view: &SettingsView, narrow: bool, cx: &mut Context<Self>) -> Div {
        let mut section = div().flex().flex_col().gap(px(20.)).child(section_head(
            "Providers and models",
            Some(
                "AIVory Code runs each provider's own agent. Set a default model, effort and \
                 permission mode per provider. A session can change them in its input box.",
            ),
        ));
        for p in &view.providers {
            let prefs = view
                .settings
                .providers
                .get(&p.id)
                .cloned()
                .unwrap_or_else(|| ProviderPrefs {
                    enabled: false,
                    model: p.models.first().cloned().unwrap_or_default(),
                    effort: Effort::High,
                    mode: Mode::Edit,
                });
            section = section.child(self.provider_card(p, &prefs, narrow, cx));
        }
        section
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        button("acp-add", "Add an ACP agent", true).on_click(
                            cx.listener(|this, _, window, cx| this.toggle_acp(window, cx)),
                        ),
                    )
                    .child(
                        button("import-claude", "Import from Claude Code settings", false)
                            .when(self.importing, |d| d.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.import_claude(cx))),
                    ),
            )
            .when(self.acp_open, |d| d.child(self.acp_form(cx)))
            .children(self.import.as_ref().map(|r| self.import_card(r, cx)))
    }

    /// The form of Add an ACP agent. It shows no environment values: the form has none.
    fn acp_form(&self, cx: &mut Context<Self>) -> Div {
        let f = &self.acp;
        card()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child("Add an ACP agent"),
            )
            .child(forms::field(
                "Name",
                forms::boxed().children(f.name.clone()),
            ))
            .child(forms::field(
                "Command (a program on the PATH, or the path of a file)",
                forms::boxed().children(f.command.clone()),
            ))
            .child(forms::field(
                "Arguments",
                forms::boxed().children(f.args.clone()),
            ))
            .when_some(f.error.clone(), |d, e| d.child(forms::error_line(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        forms::button("acp-cancel", "Cancel", false, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.acp_open = false;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        forms::button("acp-create", "Add the agent", true, f.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.add_acp(cx))),
                    ),
            )
    }

    /// The result of an import: what changed, and what the daemon did not import.
    fn import_card(&self, report: &ImportReport, cx: &mut Context<Self>) -> Div {
        let mut card = card().p(px(20.)).flex().flex_col().gap(px(10.)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(import_summary(report)),
                )
                .child(button("import-close", "Close", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.import = None;
                        cx.notify();
                    },
                ))),
        );
        let line = |text: String| {
            div()
                .font_family(tk().mono)
                .text_size(crate::ui::ts(crate::theme::T_SMALL))
                .text_color(rgb(tk().ink_2))
                .child(text)
        };
        for change in &report.changes {
            card = card.child(line(change_line(change)));
        }
        if !report.skipped.is_empty() {
            card = card.child(
                div()
                    .pt(px(6.))
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child("Not imported"),
            );
            for skipped in &report.skipped {
                card = card.child(line(skipped.clone()));
            }
        }
        card
    }

    fn provider_card(
        &self,
        p: &ProviderInfo,
        prefs: &ProviderPrefs,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = p.id.as_str();
        let (badge_text, tone) = provider_badge(p.status, prefs.enabled);
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(34.))
                    .rounded(px(10.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(provider_color(id)))
                    .text_color(rgb(tk().card))
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .font_weight(FontWeight::BOLD)
                    .child(p.monogram.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(p.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(p.auth.clone()),
                    ),
            )
            .child(badge(badge_text, tone))
            .when(p.status == ProviderStatus::Acp, |d| {
                let (acp_id, acp_name) = (p.id.clone(), p.name.clone());
                d.child(
                    button(format!("pv-{id}-remove"), "Remove", false)
                        .tooltip(ui::tip("Remove this ACP agent"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_acp(acp_id.clone(), acp_name.clone(), cx)
                        })),
                )
            })
            .child(self.toggle(
                format!("pv-{id}-on"),
                prefs.enabled,
                keys::provider(id, "enabled"),
                cx,
            ));
        let mut card = div()
            .w_full()
            .p(px(20.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .bg(rgb(tk().card))
            .rounded(tk().r_card)
            .shadow(ui::card_shadow())
            .child(head);
        if !p.usage.is_empty() {
            let per_row = if narrow { 1 } else { 2 };
            let mut usage = div().flex().flex_col().gap(px(12.));
            for chunk in p.usage.chunks(per_row) {
                let mut line = div().flex().gap(px(12.));
                for window in chunk {
                    line = line.child(usage_bar(window));
                }
                // An odd bar keeps the width of one column.
                if chunk.len() < per_row {
                    line = line.child(div().flex_1());
                }
                usage = usage.child(line);
            }
            card = card.child(usage);
        }
        let mut models = p.models.clone();
        if !models.contains(&prefs.model) {
            models.insert(0, prefs.model.clone());
        }
        let model_choices = models
            .iter()
            .map(|m| Choice::new(m.clone(), *m == prefs.model, json!(m)))
            .collect();
        let effort_choices = [
            (Effort::Low, "low"),
            (Effort::Medium, "medium"),
            (Effort::High, "high"),
            (Effort::Max, "max"),
        ]
        .into_iter()
        .map(|(e, label)| Choice::new(label, e == prefs.effort, json!(e)))
        .collect();
        let mode_choices = [
            (Mode::Plan, "Plan"),
            (Mode::Edit, "Edit"),
            (Mode::Auto, "Auto"),
        ]
        .into_iter()
        .map(|(m, label)| Choice::new(label, m == prefs.mode, json!(m)))
        .collect();
        let group = |label: &str, control: Div| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().ink_3))
                        .child(label.to_string()),
                )
                .child(control)
        };
        card.child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(28.))
                .gap_y(px(14.))
                .pt(px(14.))
                .border_t_1()
                .border_color(rgba(ROW_LINE))
                .child(group(
                    "Default model",
                    self.seg(
                        &format!("pv-{id}-model"),
                        &keys::provider(id, "model"),
                        model_choices,
                        true,
                        cx,
                    ),
                ))
                .child(group(
                    "Effort",
                    self.seg(
                        &format!("pv-{id}-effort"),
                        &keys::provider(id, "effort"),
                        effort_choices,
                        false,
                        cx,
                    ),
                ))
                .child(group(
                    "Permission mode",
                    self.seg(
                        &format!("pv-{id}-mode"),
                        &keys::provider(id, "mode"),
                        mode_choices,
                        false,
                        cx,
                    ),
                )),
        )
    }

    fn autopilot_section(&self, view: &SettingsView, cx: &mut Context<Self>) -> Div {
        let a = &view.settings.autopilot;
        let queued = self.model.read(cx).war.as_ref().map_or(0, |w| w.queued);
        let mut rules = card().child(self.row(
            true,
            row_label("Autopilot", autopilot_text(a)),
            self.toggle("ap-on", a.on, keys::autopilot("on"), cx),
        ));
        for rule in &RULES {
            let value = rule_value(a, rule.key);
            rules = rules.child(self.row(
                false,
                row_label(rule.label, rule.help),
                self.stepper(
                    &format!("ap-{}", rule.key),
                    rule_text(rule.key, value),
                    keys::autopilot(rule.key),
                    i64::from(value),
                    (i64::from(rule.lo), i64::from(rule.hi), i64::from(rule.step)),
                    cx,
                ),
            ));
        }
        let orders = [
            (QueueOrder::Priority, "Priority"),
            (QueueOrder::OldestFirst, "Oldest first"),
            (QueueOrder::SmallestFirst, "Smallest first"),
        ]
        .into_iter()
        .map(|(o, label)| Choice::new(label, o == a.order, json!(o)))
        .collect();
        let rules = rules
            .child(self.row(
                false,
                row_label("Start queued tasks in free slots", queue_text(queued)),
                self.toggle(
                    "ap-queued",
                    a.start_queued,
                    keys::autopilot("startQueued"),
                    cx,
                ),
            ))
            .child(self.row(
                false,
                row_label(
                    "Order of the queue",
                    "Which task starts first when a slot is free.",
                ),
                self.seg("ap-order", &keys::autopilot("order"), orders, false, cx),
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_head(
                "Autopilot",
                Some(
                    "Autopilot parks idle sessions and starts queued tasks in free slots. \
                     It never answers a question or a permission for you.",
                ),
            ))
            .child(rules)
    }

    fn display_section(&self, view: &SettingsView, cx: &mut Context<Self>) -> Div {
        let d = &view.settings.display;
        let themes = [
            (Theme::Light, "Light"),
            (Theme::Dark, "Dark"),
            (Theme::System, "System"),
        ]
        .into_iter()
        .map(|(t, label)| Choice::new(label, t == d.theme, json!(t)))
        .collect();
        let per_row = std::iter::once(Choice::new("Auto", d.per_row.is_none(), Value::Null))
            .chain(
                [1u8, 2, 3, 4, 8, 9, 16]
                    .into_iter()
                    .map(|n| Choice::new(n.to_string(), d.per_row == Some(n), json!(n))),
            )
            .collect();
        let fonts = Display::FONTS
            .iter()
            .map(|f| Choice {
                font: Some(f),
                ..Choice::new(*f, *f == d.font, json!(f))
            })
            .collect();
        let backgrounds = [
            (ConsoleBackground::White, "Light"),
            (ConsoleBackground::Grey, "Grey"),
            (ConsoleBackground::Dark, "Dark"),
            (ConsoleBackground::Navy, "Navy"),
        ]
        .into_iter()
        .map(|(b, label)| Choice::new(label, b == d.background, json!(b)))
        .collect();
        let console = PaneTheme::of(d.background);
        let font = Display::FONTS
            .iter()
            .find(|f| **f == d.font)
            .copied()
            .unwrap_or(Display::FONTS[0]);
        let (lo, hi) = Display::FONT_SIZE;
        let (ui_lo, ui_hi) = Display::UI_SCALE;
        let card = card()
            .child(self.row(
                true,
                row_label("Look", "Redraws every screen. Also in the status bar."),
                self.look_cards(d.look, d.custom_theme.is_some(), cx),
            ))
            .child(self.themes_block(d.custom_theme.as_deref(), cx))
            .child(self.row(
                false,
                row_label("Theme", "Light or dark. Applies to the Minimal look."),
                self.seg("dp-theme", &keys::display("theme"), themes, false, cx),
            ))
            .child(self.row(
                false,
                row_label(
                    "Panes per row",
                    "Auto fits as many consoles as the window width allows.",
                ),
                self.seg("dp-per-row", &keys::display("perRow"), per_row, true, cx),
            ))
            .child(self.row(
                false,
                row_label("Console font", "Used in all session panes."),
                self.seg("dp-font", &keys::display("font"), fonts, false, cx),
            ))
            .child(self.row(
                false,
                row_label("Console font size", "9 to 18 px. 16 px is 12 pt."),
                self.stepper(
                    "dp-size",
                    format!("{} px", d.font_size),
                    keys::display("fontSize"),
                    i64::from(d.font_size),
                    (i64::from(lo), i64::from(hi), 1),
                    cx,
                ),
            ))
            .child(self.row(
                false,
                row_label(
                    "Interface text size",
                    "All text outside the consoles, 80 to 150 %.",
                ),
                self.stepper(
                    "dp-ui-scale",
                    format!("{} %", d.ui_scale),
                    keys::display("uiScale"),
                    i64::from(d.ui_scale),
                    (i64::from(ui_lo), i64::from(ui_hi), 5),
                    cx,
                ),
            ))
            .child(self.row(
                false,
                row_label(
                    "Maximize is full screen",
                    "The maximize button hides the title bar and the frame. F11 goes back.",
                ),
                self.toggle(
                    "dp-max-full",
                    d.maximize_full_screen,
                    keys::display("maximizeFullScreen"),
                    cx,
                ),
            ))
            .child(self.row(
                false,
                row_label(
                    "Console background",
                    "Text and diff colors follow the background.",
                ),
                self.seg(
                    "dp-background",
                    &keys::display("background"),
                    backgrounds,
                    false,
                    cx,
                ),
            ))
            .child(
                div()
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(rgba(ROW_LINE))
                    .child(
                        div()
                            .w_full()
                            .px(px(14.))
                            .py(px(12.))
                            .rounded(tk().r_button)
                            .border_1()
                            .border_color(rgba(tk().line))
                            .bg(rgba(console.bg))
                            .text_color(rgba(console.text))
                            .font_family(font)
                            .text_size(px(f32::from(d.font_size)))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(PREVIEW),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_head(
                "Display",
                Some(
                    "Defaults for the war room. The Display menu in the war room changes them \
                     for the current view.",
                ),
            ))
            .child(card)
    }

    fn device_row(
        first: bool,
        icon: &'static str,
        name: String,
        detail: String,
        badge: Div,
        action: impl IntoElement,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(20.))
            .py(px(14.))
            .when(!first, |d| d.border_t_1().border_color(rgba(ROW_LINE)))
            .child(
                div()
                    .flex_none()
                    .size(px(36.))
                    .rounded(px(10.))
                    .bg(rgb(tk().track))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::icon(icon, 16., rgb(tk().ink_2))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(crate::ui::ts(crate::theme::T_SMALL))
                            .text_color(rgb(tk().ink_3))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(detail),
                    ),
            )
            .child(badge)
            .child(action)
    }

    fn daemon_card(&self, daemons: &[Daemon], max_running: u32, cx: &mut Context<Self>) -> Div {
        let mut card = card();
        for (i, d) in daemons.iter().enumerate() {
            let buttons = daemon_buttons(d.kind);
            let name = d.name.clone();
            let (text, tone) = if d.running {
                ("running", Tone::Ok)
            } else {
                ("offline", Tone::Off)
            };
            let mut actions = div().flex().items_center().gap(px(8.));
            if buttons.scale {
                let (dec_id, inc_id) = (d.id.clone(), d.id.clone());
                let stepper = ui::stepper(
                    &format!("scale-{}", d.id),
                    format!("{max_running} at once"),
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.scale_daemon(dec_id.clone(), max_running.saturating_sub(1).max(1), cx)
                    })),
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.scale_daemon(inc_id.clone(), (max_running + 1).min(64), cx)
                    })),
                );
                actions = actions.child(
                    div()
                        .id(SharedString::from(format!("scale-tip-{}", d.id)))
                        .tooltip(ui::tip("How many sessions this daemon runs at once"))
                        .child(stepper),
                );
            }
            if buttons.shell {
                actions = actions.child(
                    button(format!("shell-{}", d.id), "Open shell", false)
                        .tooltip(ui::tip("Open a terminal in the folder of the daemon"))
                        .on_click(cx.listener(|this, _, _, cx| this.open_shell(cx))),
                );
            }
            if buttons.restart {
                let id = d.id.clone();
                actions = actions.child(
                    button(format!("restart-{}", d.id), "Restart", false)
                        .tooltip(ui::tip("Restart aivoryd. The app connects again."))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.restart_daemon(id.clone(), cx)),
                        ),
                );
            }
            card = card.child(Self::device_row(
                i == 0,
                "icons/device-daemon.svg",
                name,
                format!("{} · {} sessions", d.detail, d.sessions),
                badge(text, tone),
                actions,
            ));
        }
        card
    }

    fn phone_card(&self, devices: &[Device], now: UnixMs, cx: &mut Context<Self>) -> Div {
        let local = !self.model.read(cx).demo;
        let mut card = card();
        if devices.is_empty() {
            return card.child(
                div()
                    .px(px(20.))
                    .py(px(14.))
                    .text_color(rgb(tk().ink_3))
                    .child("No phone is paired."),
            );
        }
        for (i, d) in devices.iter().enumerate() {
            let id = d.id.clone();
            let (text, tone) = if d.online {
                ("online", Tone::Ok)
            } else {
                ("offline", Tone::Off)
            };
            card = card.child(Self::device_row(
                i == 0,
                "icons/device-phone.svg",
                d.name.clone(),
                format!(
                    "{} · paired {} · last seen {}",
                    d.os,
                    date_text(d.paired, local),
                    seen_text(d.last_seen, now)
                ),
                badge(text, tone),
                button(format!("remove-{}", d.id), "Remove", false).on_click(
                    cx.listener(move |this, _, _, cx| this.remove_device(id.clone(), cx)),
                ),
            ));
        }
        card
    }

    fn pair_card(&self, cx: &mut Context<Self>) -> Div {
        let qr_box = div()
            .flex_none()
            .size(px(120.))
            .p(px(8.))
            .rounded(px(10.))
            .bg(rgb(tk().card))
            .shadow(vec![BoxShadow {
                color: ui::hsla(0x0D0D0E1A),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(1.),
                inset: false,
            }]);
        let qr_box = match &self.pairing {
            Some((_, Some(qr))) => qr_box.child(qr_view(qr.clone())),
            _ => qr_box
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                .text_color(rgb(tk().muted))
                .child(if self.pairing_busy { "…" } else { "no code" }),
        };
        let code_line: Option<String> = match &self.pairing {
            Some((p, _)) => Some(code_text(&p.code, &self.model.read(cx).clock(p.expires))),
            None => self.pairing_error.as_ref().map(|_| {
                "This daemon cannot pair a phone yet. Pairing comes with the hub.".to_string()
            }),
        };
        let link = self
            .link_shown
            .then(|| self.pairing.as_ref().map(|(p, _)| p.url.clone()))
            .flatten();
        let mut texts = div()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Pair a phone"),
            )
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .line_height(relative(1.45))
                    .text_color(rgb(tk().ink_3))
                    .child(
                        "Open AIVory Code on the phone, tap Me › Pair, and scan this code. \
                         The phone keeps its key in the Secure Enclave or the Android Keystore.",
                    ),
            );
        if let Some(line) = code_line {
            texts = texts.child(
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_2))
                    .child(line),
            );
        }
        if let Some(url) = link {
            texts = texts.child(
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("link copied · {url}")),
            );
        }
        texts = texts.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .mt(px(10.))
                .child(
                    button("pair-new", "Show a new code", true)
                        .on_click(cx.listener(|this, _, _, cx| this.pair(cx))),
                )
                .child(
                    button("pair-link", "Pair by link", false)
                        .on_click(cx.listener(|this, _, _, cx| this.show_link(cx))),
                ),
        );
        card()
            .flex_row()
            .items_center()
            .gap(px(20.))
            .px(px(20.))
            .py(px(18.))
            .child(qr_box)
            .child(texts.flex_1())
    }

    fn devices_section(&self, view: &SettingsView, cx: &mut Context<Self>) -> Div {
        let now = self.now(cx);
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_head(
                "Daemons and devices",
                Some("Daemons run the agents. Phones and other desktops connect to them through the hub."),
            ))
            .child(self.daemon_card(&view.daemons, view.settings.autopilot.max_running, cx))
            .child(self.phone_card(&view.devices, now, cx))
            .child(self.pair_card(cx))
    }

    fn notifications_section(&self, view: &SettingsView, cx: &mut Context<Self>) -> Div {
        let notify = &view.settings.notify;
        let cell = || div().flex_none().w(px(80.)).flex().justify_center();
        let mut head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(12.))
            .text_size(crate::ui::ts(crate::theme::T_CAPTION))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(tk().ink_3))
            .child(div().flex_1().child("EVENT"));
        for (_, label) in CHANNELS {
            head = head.child(cell().child(label.to_uppercase()));
        }
        let mut grid = card().child(head);
        for (event, title, help) in EVENTS {
            let mut line = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(20.))
                .py(px(12.))
                .border_t_1()
                .border_color(rgba(ROW_LINE))
                .child(row_label(title, help).flex_1());
            for (channel, _) in CHANNELS {
                line = line.child(cell().child(self.toggle(
                    format!("nt-{event}-{channel}"),
                    notify_on(notify, event, channel),
                    keys::notify(event, channel),
                    cx,
                )));
            }
            grid = grid.child(line);
        }
        let quiet = &view.settings.quiet_hours;
        let quiet_card = card()
            .child(self.row(
                true,
                row_label(
                    "Quiet hours",
                    format!(
                        "No pushes between {} and {}. Errors in production repos still come through.",
                        quiet.from, quiet.to
                    ),
                ),
                self.toggle("nt-quiet", quiet.on, keys::QUIET_HOURS.to_string(), cx),
            ))
            .child(self.row(
                false,
                row_label(
                    "Coffee reminder",
                    "One small note from me per app start. Never while a decision waits.",
                ),
                self.toggle(
                    "nt-coffee",
                    view.settings.coffee_reminder,
                    keys::COFFEE_REMINDER.to_string(),
                    cx,
                ),
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_head(
                "Notifications",
                Some("Choose where each event reaches you. Phone pushes are end-to-end encrypted."),
            ))
            .child(grid)
            .child(quiet_card)
    }

    fn account_section(&self, a: &Account) -> Div {
        let source = button("about-source", "View the source", false)
            .tooltip(ui::tip("Open the GitHub page"))
            .on_click(|_, _, cx| cx.open_url(crate::notice::SOURCE_PAGE));
        let coffee = button("about-coffee", "Buy Paul a coffee", false)
            .tooltip(ui::tip("Open the coffee page"))
            .on_click(|_, _, cx| cx.open_url(crate::notice::COFFEE_PAGE));
        let mut card = card()
            .child(self.row(
                true,
                row_label(
                    "AIVory Code",
                    format!(
                        "Version {}. Hi, I'm Paul. I built this to keep up with my own agents.",
                        env!("CARGO_PKG_VERSION")
                    ),
                ),
                source.into_any_element(),
            ))
            .child(self.row(
                false,
                row_label("License", "MIT. Free for everyone, no limits."),
                badge("free", Tone::Ok),
            ))
            .child(self.row(
                false,
                row_label(
                    "Support",
                    "It's free and it stays free. If it saves you time, I'd love a coffee :)",
                ),
                coffee.into_any_element(),
            ));
        // A hub shows only when the user connects one.
        if let Some(url) = team_url(a) {
            let (hub_text, hub_tone) = hub_badge(a.hub_state);
            card = card
                .child(self.row(
                    false,
                    row_label("Hub", format!("{} · relay {}", a.hub, a.relay)),
                    badge(hub_text, hub_tone),
                ))
                .child(
                    self.row(
                        false,
                        row_label("Team", a.team.clone()),
                        button("team-manage", "Manage the team", false)
                            .tooltip(ui::tip("Open the team page of the hub"))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                            .into_any_element(),
                    ),
                );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(section_head("About and license", None))
            .child(card)
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = window.viewport_size().width;
        let Some(view) = self.view.clone() else {
            let text = match &self.load_error {
                Some(error) => format!("Cannot read the settings: {error}"),
                None => "Waits for aivoryd".to_string(),
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.header())
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(crate::ui::ts(crate::theme::T_SMALL))
                        .text_color(rgb(tk().muted))
                        .child(text),
                );
        };
        let compact = width < px(COMPACT);
        let narrow = width < NARROW;
        self.narrow = narrow;
        let body = match self.section {
            Section::Providers => self.providers_section(&view, narrow, cx),
            Section::Autopilot => self.autopilot_section(&view, cx),
            Section::Display => {
                self.themes_ready(window, cx);
                self.display_section(&view, cx)
            }
            Section::Devices => self.devices_section(&view, cx),
            Section::Notifications => self.notifications_section(&view, cx),
            Section::Account => self.account_section(&view.account),
        };
        let pad: (Pixels, Pixels) = if narrow {
            (px(16.), px(20.))
        } else {
            (px(32.), px(28.))
        };
        // One scroll id per section: a new section starts at the top.
        let body = div().w_full().max_w(px(CONTENT_MAX)).child(body);
        let content = if tk().look == aivory_protocol::Look::Navigator {
            // The Navigator look shows a visible scroll bar.
            div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(crate::warroom::skins::navigator::panels::scroll_area(
                    SharedString::from(format!("settings-body-{}", self.section.index())),
                    &self.scroll,
                    false,
                    div().px(pad.0).pt(pad.1).pb(px(48.)).child(body),
                ))
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!(
                    "settings-body-{}",
                    self.section.index()
                )))
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .overflow_y_scroll()
                .px(pad.0)
                .pt(pad.1)
                .pb(px(48.))
                .child(body)
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.header())
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .when(compact, |d| d.flex_col())
                    .child(self.nav(&view, compact, cx))
                    .child(content),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::demo::Demo;

    fn demo() -> SettingsView {
        Demo::new().settings
    }

    #[test]
    fn key_paths_follow_the_protocol() {
        assert_eq!(
            keys::provider("claude", "effort"),
            "providers.claude.effort"
        );
        assert_eq!(keys::autopilot("parkAfterMin"), "autopilot.parkAfterMin");
        assert_eq!(keys::display("perRow"), "display.perRow");
        assert_eq!(
            keys::notify("needsAnswer", "phone"),
            "notify.needsAnswer.phone"
        );
        assert_eq!(keys::QUIET_HOURS, "quietHours.on");
    }

    #[test]
    fn every_control_key_exists_in_the_settings() {
        let mut s = demo().settings;
        for id in s.providers.keys().cloned().collect::<Vec<_>>() {
            for (field, value) in [
                ("enabled", json!(false)),
                ("model", json!("x")),
                ("effort", json!(Effort::Max)),
                ("mode", json!(Mode::Plan)),
            ] {
                let key = keys::provider(&id, field);
                assert_eq!(s.set_key(&key, value), Ok(()), "{key}");
            }
        }
        for field in ["on", "startQueued"] {
            assert_eq!(s.set_key(&keys::autopilot(field), json!(false)), Ok(()));
        }
        assert_eq!(
            s.set_key(&keys::autopilot("order"), json!(QueueOrder::SmallestFirst)),
            Ok(())
        );
        for rule in &RULES {
            assert_eq!(
                s.set_key(&keys::autopilot(rule.key), json!(rule.hi)),
                Ok(())
            );
            assert_eq!(rule_value(&s.autopilot, rule.key), rule.hi);
        }
        assert_eq!(
            s.set_key(&keys::display("theme"), json!(Theme::System)),
            Ok(())
        );
        assert_eq!(s.set_key(&keys::display("perRow"), Value::Null), Ok(()));
        assert_eq!(s.display.per_row, None);
        assert_eq!(s.set_key(&keys::display("perRow"), json!(9)), Ok(()));
        assert_eq!(s.display.per_row, Some(9));
        assert_eq!(
            s.set_key(&keys::display("font"), json!(Display::FONTS[2])),
            Ok(())
        );
        assert_eq!(s.set_key(&keys::display("fontSize"), json!(18)), Ok(()));
        assert_eq!(
            s.set_key(&keys::display("background"), json!(ConsoleBackground::Navy)),
            Ok(())
        );
        assert_eq!(s.set_key(keys::QUIET_HOURS, json!(false)), Ok(()));
        assert!(!s.quiet_hours.on);
    }

    #[test]
    fn every_notification_cell_has_a_key_and_a_value() {
        let mut s = demo().settings;
        for (event, _, _) in EVENTS {
            for (channel, _) in CHANNELS {
                let key = keys::notify(event, channel);
                let before = notify_on(&s.notify, event, channel);
                assert_eq!(s.set_key(&key, json!(!before)), Ok(()), "{key}");
                assert_eq!(notify_on(&s.notify, event, channel), !before, "{key}");
            }
        }
    }

    #[test]
    fn demo_notifications_match_the_artboard() {
        let n = demo().settings.notify;
        let on: Vec<String> = EVENTS
            .iter()
            .flat_map(|(e, _, _)| CHANNELS.iter().map(move |(c, _)| (*e, *c)))
            .filter(|(e, c)| notify_on(&n, e, c))
            .map(|(e, c)| keys::notify(e, c))
            .collect();
        assert_eq!(on.len(), 10);
        assert!(on.contains(&"notify.error.email".to_string()));
        assert!(!on.contains(&"notify.autopilot.phone".to_string()));
    }

    #[test]
    fn usage_bar_turns_orange_above_75_percent() {
        assert!(!usage_hot(0));
        assert!(!usage_hot(62));
        assert!(!usage_hot(75));
        assert!(usage_hot(76));
        assert!(usage_hot(100));
    }

    #[test]
    fn steppers_stay_in_their_range() {
        assert_eq!(step_value(15, 5, 120, 5), 20);
        assert_eq!(step_value(5, 5, 120, -5), 5);
        assert_eq!(step_value(120, 5, 120, 5), 120);
        assert_eq!(step_value(9, 9, 18, -1), 9);
        assert_eq!(step_value(18, 9, 18, 1), 18);
    }

    #[test]
    fn rules_cover_the_autopilot_ranges() {
        let a = Autopilot::default();
        assert_eq!(rule_value(&a, "parkAfterMin"), 15);
        assert_eq!(rule_text("parkAfterMin", 15), "15 min");
        assert_eq!(rule_text("maxRunning", 14), "14");
        let [park, running, per] = &RULES;
        assert_eq!((park.lo, park.hi, park.step), (5, 120, 5));
        assert_eq!((running.lo, running.hi), (1, 64));
        assert_eq!((per.lo, per.hi), (1, 16));
        assert_eq!(
            autopilot_text(&a),
            "On. Parks idle sessions after 15 min and keeps at most 14 running."
        );
        assert_eq!(queue_text(6), "6 tasks are in the queue now.");
        assert_eq!(queue_text(1), "1 task is in the queue now.");
    }

    #[test]
    fn qr_matrix_has_the_size_of_its_version() {
        let qr = qr_matrix("aivory://pair?host=desktop&code=AIV-4820").expect("a QR code");
        assert_eq!(qr.dark.len(), qr.width * qr.width);
        // Version v has 4 v + 17 modules per side.
        assert!(
            qr.width >= 21 && (qr.width - 17).is_multiple_of(4),
            "width {}",
            qr.width
        );
        // The three finder patterns have a dark corner module.
        let w = qr.width;
        assert!(qr.dark[0] && qr.dark[w - 1] && qr.dark[w * (w - 1)]);
        // The fourth corner has none.
        assert!(!qr.dark[w * w - 1]);
        // A longer payload needs a larger version.
        let long = qr_matrix(&format!(
            "aivory://pair?host=desktop&code=AIV-4820&{}",
            "k".repeat(80)
        ))
        .expect("a QR code");
        assert!(long.width > qr.width);
    }

    #[test]
    fn qr_runs_cover_exactly_the_dark_modules() {
        let qr = qr_matrix("aivory://pair?host=desktop&code=AIV-4820").expect("a QR code");
        let mut total = 0;
        for y in 0..qr.width {
            for (start, len) in qr.runs(y) {
                assert!(len > 0 && start + len <= qr.width);
                assert!(qr.dark[y * qr.width + start..][..len].iter().all(|d| *d));
                total += len;
            }
        }
        assert_eq!(total, qr.dark.iter().filter(|d| **d).count());
    }

    #[test]
    fn demo_pairing_url_fits_one_code() {
        let url = "aivory://pair?host=desktop&code=AIV-4820";
        assert!(qr_matrix(url).is_some());
        assert_eq!(
            code_text("AIV-4820", "14:50"),
            "code AIV-4820 · valid until 14:50"
        );
    }

    #[test]
    fn nav_counts_match_the_artboard() {
        let view = demo();
        let counts: Vec<String> = Section::ALL.iter().map(|s| s.count(&view)).collect();
        assert_eq!(counts, ["5", "on", "", "5", "", ""]);
        assert_eq!(Section::Account.label(), "About and license");
        assert_eq!(Section::Devices.index(), 3);
    }

    #[test]
    fn a_remote_daemon_has_no_restart_and_no_shell() {
        let local = daemon_buttons(DaemonKind::Local);
        assert!(local.restart && local.shell && local.scale);
        for kind in [DaemonKind::Ssh, DaemonKind::Cloud] {
            let b = daemon_buttons(kind);
            assert!(!b.restart && !b.shell && b.scale);
        }
    }

    #[test]
    fn the_team_button_needs_a_hub() {
        assert_eq!(team_url(&demo().account), None);
        let mut hub = demo().account;
        hub.hub = "hub.example.net".into();
        hub.hub_state = LinkState::Connected;
        assert_eq!(team_url(&hub), Some("https://hub.example.net".to_string()));
        let mut none = hub;
        none.hub_state = LinkState::Off;
        assert_eq!(team_url(&none), None);
        none.hub_state = LinkState::Connected;
        none.hub = "  ".into();
        assert_eq!(team_url(&none), None);
    }

    #[test]
    fn an_import_lists_each_change_with_its_old_value() {
        let change = ImportChange {
            key: "providers.claude.model".into(),
            from: Some(json!("opus-5.5")),
            to: json!("sonnet-5.5"),
        };
        assert_eq!(
            change_line(&change),
            "providers.claude.model: opus-5.5 → sonnet-5.5"
        );
        let first = ImportChange {
            key: "providers.claude.mode".into(),
            from: None,
            to: json!("plan"),
        };
        assert_eq!(change_line(&first), "providers.claude.mode: plan");
        let report = |found, n| ImportReport {
            found,
            changes: vec![change.clone(); n],
            skipped: Vec::new(),
            settings: demo().settings,
        };
        assert!(import_summary(&report(false, 0)).starts_with("No Claude Code"));
        assert!(import_summary(&report(true, 0)).starts_with("Nothing to change"));
        assert_eq!(
            import_summary(&report(true, 1)),
            "Imported 1 value from the Claude Code settings."
        );
        assert_eq!(
            import_summary(&report(true, 3)),
            "Imported 3 values from the Claude Code settings."
        );
    }

    #[test]
    fn an_acp_agent_needs_a_name_and_a_command() {
        let ok = acp_request("Goose", "goose", "acp --stdio").expect("valid");
        assert_eq!(ok.name, "Goose");
        assert_eq!(ok.args, ["acp", "--stdio"]);
        assert!(ok.env.is_none());
        assert!(acp_request("", "goose", "").is_err());
        assert!(acp_request("Goose", "", "").is_err());
        assert!(acp_request("Goose", "goose", "\"open").is_err());
    }

    #[test]
    fn badges_follow_status_and_switch() {
        assert_eq!(
            provider_badge(ProviderStatus::Connected, true),
            ("connected", Tone::Ok)
        );
        assert_eq!(
            provider_badge(ProviderStatus::Acp, true),
            ("ACP", Tone::Off)
        );
        assert_eq!(
            provider_badge(ProviderStatus::Connected, false),
            ("off", Tone::Off)
        );
        assert_eq!(
            provider_badge(ProviderStatus::Off, true),
            ("off", Tone::Off)
        );
        assert_eq!(hub_badge(LinkState::Connected), ("connected", Tone::Ok));
    }

    #[test]
    fn device_lines_read_as_the_artboard() {
        let view = demo();
        let now = aivory_protocol::demo::DEMO_NOW;
        let lines: Vec<String> = view
            .devices
            .iter()
            .map(|d| {
                format!(
                    "{} · paired {} · last seen {}",
                    d.os,
                    date_text(d.paired, false),
                    seen_text(d.last_seen, now)
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                "Android 16 · paired 2026-09-30 · last seen 2 min ago",
                "iOS 26 · paired 2026-10-02 · last seen yesterday"
            ]
        );
        assert_eq!(seen_text(now, now), "just now");
        assert_eq!(seen_text(now - 3 * 3_600_000, now), "3 h ago");
        assert_eq!(seen_text(now - 5 * 86_400_000, now), "5 days ago");
    }

    #[test]
    fn demo_providers_have_usage_bars_as_the_artboard() {
        let view = demo();
        let pct: Vec<Vec<u8>> = view
            .providers
            .iter()
            .map(|p| p.usage.iter().map(|u| u.pct).collect())
            .collect();
        assert_eq!(pct[0], [62, 41]);
        assert_eq!(pct[1], [31, 22]);
        assert_eq!(pct[2], [18, 30]);
        assert!(pct[3].is_empty());
        assert!(
            view.providers
                .iter()
                .all(|p| view.settings.providers.contains_key(&p.id))
        );
    }
}
