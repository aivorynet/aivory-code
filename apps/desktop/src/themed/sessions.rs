//! The session components of the catalog (ADR 0024 sections 4 to 6): `sessions` with its
//! density tiers, `decision` and `decision_queue`, `transcript`, `terminal`, `composer` and
//! `settings_section`. The app owns the content of a decision: the question, the reason, the risk
//! flag and the options come from the ask, and a theme cannot change them. Text for an agent
//! comes only from a composer that the user types into.

use aivory_protocol::{ComposerPrefs, Line, LineKind};
use gpui::prelude::*;
use gpui::{AnyElement, ClickEvent, Context, FontWeight, Styled, Window, div, px, rgba};
use gpui_base::input::{InputEvent, TextareaState};
use serde_json::Value;

use super::actions::ComposerInput;
use super::data::{ThemeSnapshot, as_text, tier};
use super::host::{ComposerCfg, Node, ThemedEvent, ThemedView, TierLayout};
use super::render::{Frame, Scope, id, item_key, style};
use crate::actions::Action;
use crate::compose;
use crate::media::{self, MediaStyle};
use crate::theme::tk;
use crate::ui;

impl ThemedView {
    pub(super) fn sessions(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let want = tier(f.snap.live());
        let idx = aivory_theme::layout::TIERS
            .iter()
            .position(|t| *t == want)
            .unwrap_or(0);
        // A tier without a template uses the next smaller tier.
        let pick = (0..=idx)
            .rev()
            .find_map(|i| {
                node.tiers
                    .iter()
                    .find(|t| t.name == aivory_theme::layout::TIERS[i])
            })
            .or(node.tiers.first());
        let Some(tier) = pick else {
            return div().into_any_element();
        };
        let all = f.text(node, "include", sc) == "all";
        let sessions: Vec<&Value> = f
            .snap
            .list("sessions")
            .iter()
            .filter(|s| all || !matches!(s["state"].as_str(), Some("parked" | "done")))
            .collect();
        let base = Self::key_of(node, sc);
        let gap = f.space(node, "gap", sc).unwrap_or(8.);
        match tier.layout {
            TierLayout::List => {
                let mut groups: Vec<(&str, Vec<&Value>)> = Vec::new();
                for s in sessions {
                    let cb = s["codebase"].as_str().unwrap_or("");
                    match groups.iter_mut().find(|(c, _)| *c == cb) {
                        Some((_, v)) => v.push(s),
                        None => groups.push((cb, vec![s])),
                    }
                }
                let many = tier.name == "31+";
                let mut col = div().flex().flex_col().gap(px(gap)).min_w(px(0.));
                for (codebase, members) in groups {
                    let urgent = members
                        .iter()
                        .any(|s| matches!(s["state"].as_str(), Some("needsYou" | "error")));
                    let closed = many && !urgent;
                    let head = div()
                        .flex()
                        .flex_row()
                        .gap(px(8.))
                        .px(px(4.))
                        .text_size(ui::ts(f.theme.types[1]))
                        .text_color(rgba(f.tone("muted")))
                        .child(div().truncate().child(codebase.to_string()))
                        .child(format!("{}", members.len()));
                    let mut group = div().flex().flex_col().gap(px(2.)).child(head);
                    if !closed {
                        for (i, s) in members.iter().enumerate() {
                            group = group.child(self.session_item(
                                &tier.template,
                                f,
                                s,
                                &base,
                                i,
                                window,
                                cx,
                            ));
                        }
                    }
                    col = col.child(group);
                }
                style(col, node, f, sc).into_any_element()
            }
            TierLayout::Grid | TierLayout::Scene => {
                let width = f.num(node, "item_width", sc).map_or(240., |w| w as f32);
                let mut wrap = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(gap))
                    .min_w(px(0.));
                for (i, s) in sessions.iter().enumerate() {
                    wrap = wrap.child(
                        div()
                            .flex_1()
                            .flex_basis(px(width))
                            .min_w(px(width.min(160.)))
                            .child(self.session_item(&tier.template, f, s, &base, i, window, cx)),
                    );
                }
                style(wrap, node, f, sc).into_any_element()
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one call site for list and grid")]
    fn session_item(
        &mut self,
        template: &Node,
        f: &Frame<'_>,
        item: &Value,
        base: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prefix = format!("{base}#{}/", item_key(item, index));
        let inner = Scope {
            item: Some(item),
            prefix: &prefix,
        };
        self.draw(template, f, &inner, window, cx)
    }

    // ---- decisions ----

    /// The session whose ask a decision shows: the prop `session`, else `$item`, else the
    /// dialog, else the selected session, else the oldest ask.
    fn decision_session(&self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> Option<String> {
        let own = f.text(node, "session", sc);
        if !own.is_empty() {
            return Some(own);
        }
        if let Some(i) = sc.item.and_then(|i| i.get("id")).and_then(Value::as_str) {
            return Some(i.to_string());
        }
        self.dialog
            .clone()
            .or_else(|| self.selected.clone())
            .or_else(|| {
                f.snap
                    .list("asks")
                    .first()
                    .and_then(|a| a["id"].as_str().map(str::to_string))
            })
    }

    pub(super) fn decision(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let free = f.flag(node, "free_text", sc);
        let session = self
            .decision_session(node, f, sc)
            .and_then(|sid| f.snap.list("sessions").iter().find(|s| s["id"] == sid));
        let body = self.decision_body(session, &key, free, f, node, window, cx);
        style(div().min_w(px(0.)), node, f, sc)
            .child(body)
            .into_any_element()
    }

    /// The question, the reason, the risk flag, the steps and the options of one ask.
    #[allow(
        clippy::too_many_arguments,
        reason = "shared by decision and decision_queue"
    )]
    fn decision_body(
        &mut self,
        session: Option<&Value>,
        key: &str,
        free: bool,
        f: &Frame<'_>,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sc0 = Scope {
            item: None,
            prefix: "",
        };
        let size = f.ty(node, &sc0, "md");
        let muted = f.tone("muted");
        let Some(session) = session else {
            return div()
                .text_size(size)
                .text_color(rgba(muted))
                .child("No question is open.")
                .into_any_element();
        };
        let ask = &session["ask"];
        if ask.is_null() {
            return div()
                .text_size(size)
                .text_color(rgba(muted))
                .child("No question is open.")
                .into_any_element();
        }
        let sid = as_text(session.get("id"));
        let ask_id = as_text(ask.get("id"));
        let kind = ask["kind"].as_str().unwrap_or("question");
        let label = match kind {
            "permission" => "Permission required",
            "plan" => "Plan approval",
            "error" => "Error",
            _ => "Question",
        };
        let mut col = div().flex().flex_col().gap(px(8.)).min_w(px(0.));
        let mut head = div().flex().flex_row().items_center().gap(px(8.)).child(
            div()
                .text_size(ui::ts(f.theme.types[1]))
                .text_color(rgba(f.state_color("needsYou")))
                .child(label),
        );
        if ask["risky"].as_bool().unwrap_or(false) {
            head = head.child(
                div()
                    .px(px(8.))
                    .py(px(1.))
                    .rounded_full()
                    .bg(rgba(f.state_color("error") & 0xFFFFFF00 | 0x28))
                    .text_color(rgba(f.state_color("error")))
                    .text_size(ui::ts(f.theme.types[0]))
                    .child("Risky"),
            );
        }
        col = col.child(head);
        let mut text = div().text_size(size).text_color(rgba(f.tone("text")));
        if kind == "permission" {
            text = text.font_family(tk().mono);
        }
        col = col.child(text.child(as_text(ask.get("text"))));
        for step in ask["steps"].as_array().into_iter().flatten() {
            col = col.child(
                div()
                    .pl(px(10.))
                    .text_size(f.ty(node, &sc0, "sm"))
                    .text_color(rgba(f.tone("text")))
                    .child(format!("- {}", as_text(Some(step)))),
            );
        }
        if let Some(reason) = ask["reason"].as_str() {
            col = col.child(
                div()
                    .text_size(f.ty(node, &sc0, "sm"))
                    .text_color(rgba(muted))
                    .child(reason.to_string()),
            );
        }
        let mut opts = div().flex().flex_row().flex_wrap().gap(px(8.));
        for (i, opt) in ask["options"].as_array().into_iter().flatten().enumerate() {
            let option = as_text(Some(opt));
            let (sid, ask_id, o2) = (sid.clone(), ask_id.clone(), option.clone());
            opts = opts.child(
                div()
                    .id(id(&format!("{key}/opt{i}")))
                    .px(px(12.))
                    .py(px(6.))
                    .cursor_pointer()
                    .rounded(px(8.))
                    .bg(rgba(if i == 0 {
                        f.tone("accent")
                    } else {
                        f.tone("surface")
                    }))
                    .text_color(rgba(if i == 0 { f.tone("bg") } else { f.tone("text") }))
                    .text_size(size)
                    .hover(|s| s.opacity(0.85))
                    .child(option)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.run(
                            Action::Answer {
                                id: sid.clone(),
                                ask: ask_id.clone(),
                                option: Some(o2.clone()),
                                text: None,
                            },
                            cx,
                        );
                        if this.dialog.is_some() {
                            cx.emit(ThemedEvent::CloseDecision);
                        }
                    })),
            );
        }
        col = col.child(opts);
        if free {
            let ckey = format!("{key}/free");
            self.composers.insert(
                ckey.clone(),
                ComposerCfg {
                    mode: "answer".into(),
                    session: Some(sid.clone()),
                    ask: Some(ask_id.clone()),
                    codebase: None,
                },
            );
            col = col.child(self.composer_box(&ckey, "Or type your own answer", f, window, cx));
        }
        col.into_any_element()
    }

    pub(super) fn decision_queue(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let limit = f.num(node, "limit", sc).map_or(usize::MAX, |n| n as usize);
        let free = f.flag(node, "free_text", sc);
        let scroll = self.scrolls.entry(key.clone()).or_default().clone();
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .gap(px(f.space(node, "gap", sc).unwrap_or(10.)))
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&scroll);
        let asks = f.snap.list("asks");
        if asks.is_empty() {
            d = d.child(
                div()
                    .p(px(8.))
                    .text_size(f.ty(node, sc, "sm"))
                    .text_color(rgba(f.tone("muted")))
                    .child("Nothing waits for you."),
            );
        }
        for (i, ask) in asks.iter().take(limit).enumerate() {
            let sid = as_text(ask.get("id"));
            let session = f.snap.list("sessions").iter().find(|s| s["id"] == sid);
            let item_key = format!("{key}#{sid}");
            let body = self.decision_body(session, &item_key, free && i == 0, f, node, window, cx);
            d = d.child(
                div()
                    .p(px(12.))
                    .rounded(px(8.))
                    .bg(rgba(f.tone("surface")))
                    .border_l(px(3.))
                    .border_color(rgba(f.state_color("needsYou")))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(f.ty(node, sc, "md"))
                                    .child(as_text(ask.get("title"))),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(ui::ts(f.theme.types[1]))
                                    .text_color(rgba(f.tone("muted")))
                                    .child(as_text(ask.get("codebase"))),
                            ),
                    )
                    .child(body),
            );
        }
        style(d, node, f, sc).into_any_element()
    }

    // ---- transcript and terminal ----

    fn lines_session(&self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> Option<String> {
        let own = f.text(node, "session", sc);
        if !own.is_empty() {
            return Some(own);
        }
        if let Some(i) = sc.item.and_then(|i| i.get("id")).and_then(Value::as_str) {
            return Some(i.to_string());
        }
        self.selected.clone().or_else(|| self.dialog.clone())
    }

    pub(super) fn transcript(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        self.lines_view(node, f, sc, false)
    }

    pub(super) fn terminal(&mut self, node: &Node, f: &Frame<'_>, sc: &Scope<'_>) -> AnyElement {
        self.lines_view(node, f, sc, true)
    }

    /// The lines of a session. The scroll box has a stable key, so a re-layout keeps the
    /// position; a new box starts at the newest line.
    fn lines_view(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        terminal: bool,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let fresh = !self.scrolls.contains_key(&key);
        let handle = self.scrolls.entry(key.clone()).or_default().clone();
        if fresh {
            handle.scroll_to_bottom();
        }
        let size = f.ty(node, sc, if terminal { "sm" } else { "md" });
        let mut d = div()
            .id(id(&key))
            .flex()
            .flex_col()
            .gap(px(if terminal { 0. } else { 4. }))
            .min_w(px(0.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&handle)
            .text_size(size)
            .text_color(rgba(f.tone("text")));
        if terminal {
            d = d
                .font_family(tk().mono)
                .bg(rgba(f.tone("bg")))
                .border_1()
                .border_color(rgba(f.tone("muted") & 0xFFFFFF00 | 0x40))
                .p(px(8.));
        }
        let lines: &[Line] = self
            .lines_session(node, f, sc)
            .map_or(&[], |sid| f.snap.tail(&sid));
        if lines.is_empty() {
            d = d.child(
                div()
                    .text_color(rgba(f.tone("muted")))
                    .child("No lines yet."),
            );
        }
        let media_style = MediaStyle {
            card: rgba(f.tone("surface")),
            text: rgba(f.tone("text")),
            muted: rgba(f.tone("muted")),
            radius: 8.,
        };
        for (n, l) in lines.iter().enumerate() {
            d = d.child(line_row(l, f, terminal));
            if let Some(m) = &l.media {
                d = d.child(div().flex_none().py(px(4.)).child(media::preview(
                    m,
                    ("themed-media", n),
                    media_style,
                )));
            }
        }
        style(d, node, f, sc).into_any_element()
    }

    // ---- composer ----

    /// A text box with the Send button of the app. The text stays under the key of the node when
    /// the layout changes.
    fn composer_box(
        &mut self,
        key: &str,
        placeholder: &str,
        f: &Frame<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = match self.inputs.get(key) {
            Some(i) => i.clone(),
            None => {
                let ph = placeholder.to_string();
                let input = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .placeholder(ph)
                        .submit_on_enter(true)
                        .auto_grow(1, 5)
                });
                let k = key.to_string();
                let weak = cx.weak_entity();
                let ck = key.to_string();
                crate::complete::attach(
                    &input,
                    self.model.clone(),
                    move |cx| {
                        weak.upgrade().and_then(|v| {
                            v.read(cx)
                                .composers
                                .get(&ck)
                                .and_then(|c| c.session.clone())
                        })
                    },
                    |_| (None, None),
                    cx,
                );
                let sub = cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, ev: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { shift: false, .. } = ev {
                            this.submit_composer(&k, window, cx);
                        }
                    },
                );
                self.subs.push(sub);
                self.inputs.insert(key.to_string(), input.clone());
                input
            }
        };
        let k = key.to_string();
        let sends = self.composers.get(key).is_some_and(|c| c.mode == "send");
        let attach = sends.then(|| {
            div()
                .id(id(&format!("{key}/attach")))
                .flex_none()
                .px(px(10.))
                .py(px(6.))
                .cursor_pointer()
                .rounded(px(8.))
                .text_color(rgba(f.tone("muted")))
                .text_size(ui::ts(f.theme.types[1]))
                .hover(|s| s.text_color(rgba(f.tone("text"))))
                .child("Attach")
                .on_click(cx.listener(|_, _: &ClickEvent, window, cx| {
                    compose::attach(|this: &mut Self| Some(&mut this.tools), window, cx)
                }))
        });
        let frame = div()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(8.))
            .p(px(8.))
            .rounded(px(8.))
            .bg(rgba(f.tone("surface")))
            .border_1()
            .border_color(rgba(f.tone("muted") & 0xFFFFFF00 | 0x40))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(ui::ts(f.theme.types[2]))
                    .text_color(rgba(f.tone("text")))
                    .child(input),
            )
            .children(attach)
            .child(
                div()
                    .id(id(&format!("{key}/send")))
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .cursor_pointer()
                    .rounded(px(8.))
                    .bg(rgba(f.tone("accent")))
                    .text_color(rgba(f.tone("bg")))
                    .text_size(ui::ts(f.theme.types[1]))
                    .child("Send")
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.submit_composer(&k, window, cx);
                    })),
            );
        frame.into_any_element()
    }

    pub(super) fn composer(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key_of(node, sc);
        let session = {
            let own = f.text(node, "session", sc);
            if !own.is_empty() {
                Some(own)
            } else {
                sc.item
                    .and_then(|i| i.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.selected.clone())
            }
        };
        let mut mode = f.text(node, "mode", sc);
        if mode.is_empty() {
            mode = if session.is_some() {
                "send"
            } else {
                "dispatch"
            }
            .to_string();
        }
        let codebase = f.text(node, "codebase", sc);
        self.composers.insert(
            key.clone(),
            ComposerCfg {
                mode,
                session,
                ask: None,
                codebase: (!codebase.is_empty()).then_some(codebase),
            },
        );
        let ph = {
            let p = f.text(node, "placeholder", sc);
            if p.is_empty() {
                "Type a message".to_string()
            } else {
                p
            }
        };
        let body = self.composer_box(&key, &ph, f, window, cx);
        // A composer of a session shows the controls of that session above the text box.
        let pane = self
            .composers
            .get(&key)
            .filter(|c| c.mode == "send")
            .and_then(|c| c.session.clone())
            .and_then(|id| self.pane(&id, cx));
        let controls = pane.map(|pane| {
            self.controls.sync(&pane);
            let providers = self.model.read(cx).providers.clone();
            let style = crate::compose_controls::ControlStyle {
                pill: rgba(f.tone("surface")),
                pill_hover: rgba(f.tone("muted") & 0xFFFFFF00 | 0x60),
                pill_text: rgba(f.tone("text")),
                on: rgba(f.tone("accent")),
                on_text: rgba(f.tone("bg")),
                menu: rgba(f.tone("surface")),
                menu_text: rgba(f.tone("text")),
                menu_hover: rgba(f.tone("muted") & 0xFFFFFF00 | 0x60),
                font: None,
                radius: Some(8.),
                text: None,
            };
            self.controls.row(
                "themed-ctl",
                &pane,
                &providers,
                &style,
                |this: &mut Self| &mut this.controls,
                cx,
            )
        });
        let chips = (self.composers.get(&key).is_some_and(|c| c.mode == "send"))
            .then(|| {
                let palette = compose::Palette {
                    text: f.tone("text"),
                    dim: f.tone("muted"),
                    line: f.tone("muted") & 0xFFFFFF00 | 0x40,
                    chip_bg: f.tone("surface"),
                    hover: f.tone("muted") & 0xFFFFFF00 | 0x60,
                    fail: f.state_color("error"),
                    ok: f.state_color("done"),
                };
                compose::chips(&self.tools, &key, palette, |i| {
                    Box::new(cx.listener(move |this, _, _, cx| {
                        this.tools.remove(i);
                        cx.notify();
                    }))
                })
            })
            .flatten();
        let column = style(
            div().min_w(px(0.)).flex().flex_col().gap(px(6.)),
            node,
            f,
            sc,
        );
        let column = if self.composers.get(&key).is_some_and(|c| c.mode == "send") {
            compose::media_target(column, |this: &mut Self| Some(&mut this.tools), cx)
        } else {
            column
        };
        column
            .children(controls)
            .children(chips)
            .child(body)
            .into_any_element()
    }

    /// Send the text of a composer. The mode of the composer picks the action.
    pub(super) fn submit_composer(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(cfg), Some(input)) = (
            self.composers.get(key).cloned(),
            self.inputs.get(key).cloned(),
        ) else {
            return;
        };
        let typed = input.read(cx).value().trim().to_string();
        let sends = cfg.mode == "send";
        let images = if sends {
            self.tools.images()
        } else {
            Vec::new()
        };
        let text = if sends {
            self.tools.message(&typed, None)
        } else {
            typed
        };
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let snap = self.model.read(cx).snapshot.clone();
        let action = match cfg.mode.as_str() {
            "answer" => match (cfg.session, cfg.ask) {
                (Some(id), Some(ask)) => Action::Answer {
                    id,
                    ask,
                    option: None,
                    text: Some(text),
                },
                _ => return,
            },
            "send" => match cfg.session {
                // A known session sends with the picked controls (a handoff when the provider
                // changed); an unknown id goes the theme way.
                Some(id) => match self.pane(&id, cx) {
                    Some(pane) => {
                        let prefs = self.controls.current(&pane);
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        crate::compose_controls::submit(
                            &self.model,
                            &pane,
                            text,
                            images,
                            prefs,
                            cx,
                        );
                        self.tools.sent();
                        return;
                    }
                    None => Action::Send {
                        id,
                        text,
                        composer: None,
                    },
                },
                None => return,
            },
            "newSession" => match (cfg.codebase, default_prefs(&snap, self.selected.as_deref())) {
                (Some(codebase), Some(composer)) => Action::NewSession {
                    codebase,
                    text,
                    composer,
                },
                _ => return,
            },
            _ => Action::Dispatch {
                codebase: cfg.codebase,
                text,
            },
        };
        input.update(cx, |s, cx| s.set_value("", window, cx));
        self.run(action, cx);
    }

    /// The pane of a session in the war room.
    fn pane(&self, id: &str, cx: &Context<Self>) -> Option<aivory_protocol::Pane> {
        self.model
            .read(cx)
            .war
            .as_ref()?
            .sessions
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// The text of a composer, for a control that sends it (`dispatch`, `newSession`).
    pub(super) fn composer_input(
        &self,
        key: &str,
        snap: &ThemeSnapshot,
        cx: &Context<Self>,
    ) -> Option<ComposerInput> {
        let input = self.inputs.get(key)?;
        Some(ComposerInput {
            text: input.read(cx).value().to_string(),
            prefs: default_prefs(snap, self.selected.as_deref()),
        })
    }

    /// Empty a composer after its text went out.
    pub(super) fn clear_input(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.inputs.get(key) {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    pub(super) fn settings_section(
        &mut self,
        node: &Node,
        f: &Frame<'_>,
        sc: &Scope<'_>,
    ) -> AnyElement {
        style(
            div().flex().flex_col().min_w(px(0.)).min_h(px(0.)),
            node,
            f,
            sc,
        )
        .child(div().flex_1().min_h(px(0.)).child(self.settings.clone()))
        .into_any_element()
    }
}

/// The dropdowns of a new session: those of the selected session, else of the first one.
fn default_prefs(snap: &ThemeSnapshot, selected: Option<&str>) -> Option<ComposerPrefs> {
    let list = snap.list("sessions");
    let pick = selected
        .and_then(|id| list.iter().find(|s| s["id"] == id))
        .or_else(|| list.first())?;
    serde_json::from_value(pick["composer"].clone()).ok()
}

/// One line of a transcript or terminal, with the marks of the line kinds.
fn line_row(l: &Line, f: &Frame<'_>, terminal: bool) -> AnyElement {
    let text = f.tone("text");
    let muted = f.tone("muted");
    let (mark, color, mono) = match l.kind {
        LineKind::User => ("› ", f.tone("accent"), false),
        LineKind::Agent => ("", text, false),
        LineKind::Tool => ("● ", muted, true),
        LineKind::Output => ("  └ ", muted, true),
        LineKind::Added => ("+ ", f.state_color("done"), true),
        LineKind::Removed => ("- ", f.state_color("error"), true),
        LineKind::Subagent => ("↳ ", f.tone("analysis"), false),
        LineKind::Error => ("", f.state_color("error"), false),
        LineKind::Turn => ("", muted, false),
        LineKind::Meta => ("", muted, false),
    };
    let body = match &l.label {
        Some(label) => format!("{mark}{label}  {}", l.text),
        None => format!("{mark}{}", l.text),
    };
    let mut d = div().text_color(rgba(color));
    if mono || terminal {
        d = d.font_family(tk().mono);
    }
    if l.kind == LineKind::User {
        d = d.font_weight(FontWeight::SEMIBOLD);
    }
    if terminal {
        d = d.whitespace_nowrap().overflow_hidden().text_ellipsis();
    }
    d.child(body).into_any_element()
}
