//! Settings > Display > Themes (ADR 0024 section 9): add a theme from a GitHub link, confirm it,
//! see the installed themes, use, update and remove them. Every action is a daemon call. A
//! download is a job: the app polls `theme/job` each 500 ms. There is no background check; the
//! update badge shows only after the user presses "Check for updates".

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use aivory_protocol::rpc::{Empty, Method, method};
use aivory_protocol::theme::{
    DisplayClass, Inspected, InstalledTheme, JobRef, JobState, ThemeAddParams, ThemeAssetParams,
    ThemeGetParams, ThemeRef, ThemeSourceParams, UpdateAvailable, UpdateList, UpdateResult,
};
use base64::Engine as _;
use gpui::prelude::*;
use gpui::{
    Context, Div, Entity, FontWeight, ObjectFit, RenderImage, SharedString, Stateful, Subscription,
    Window, div, img, px, rgb, rgba,
};
use gpui_base::input::{InputEvent, InputState};
use serde_json::{Value, json};

use super::{SettingsScreen, Tone, badge, button, keys, row_label};
use crate::forms;
use crate::notice::{self, Draft};
use crate::theme::*;

/// The inset box of the install question.
fn card_edge() -> Div {
    div()
        .w_full()
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgba(tk().line))
        .bg(rgb(tk().track))
}

/// The width of a theme card.
const CARD_W: f32 = 184.;
/// The preview is cut to this width, so the card keeps a small image in memory.
const PREVIEW_W: u32 = 368;

/// What a theme card shows: a preview image, or a strip of the colors of its tokens.
#[derive(Clone)]
pub(super) struct CardArt {
    strip: Vec<u32>,
    image: Option<Arc<RenderImage>>,
}

#[derive(Default)]
pub(super) struct ThemesPanel {
    input: Option<Entity<InputState>>,
    _events: Vec<Subscription>,
    listed: bool,
    list: Vec<InstalledTheme>,
    /// By `id@version`.
    art: HashMap<String, CardArt>,
    /// The text of a running job.
    busy: Option<String>,
    ticks: u32,
    inspected: Option<Inspected>,
    error: Option<String>,
    /// The result of the last "Check for updates". `None` until the user presses it.
    updates: Option<Vec<UpdateAvailable>>,
    /// Themes with a running update or remove.
    working: HashSet<String>,
}

/// `1.4 MB`, `820 KB`, `12 B`.
fn size_text(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// The part of a source that a card shows: `github:owner/repo`, or `local`.
fn source_text(source: &str) -> String {
    if source.starts_with("local:") {
        "local folder".to_string()
    } else if source.is_empty() {
        "unknown source".to_string()
    } else {
        source.to_string()
    }
}

fn art_key(t: &InstalledTheme) -> String {
    format!("{}@{}", t.id, t.version)
}

/// Decode and shrink a preview in the background.
fn decode_preview(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
    let rgba = if rgba.width() > PREVIEW_W {
        let h = (u64::from(rgba.height()) * u64::from(PREVIEW_W) / u64::from(rgba.width())).max(1);
        image::imageops::thumbnail(&rgba, PREVIEW_W, h as u32)
    } else {
        rgba
    };
    Some(crate::rts::to_render(&rgba))
}

impl SettingsScreen {
    /// Called by the screen on each draw of Display: build the field once, list once.
    pub(super) fn themes_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.themes.input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("GitHub link or github:owner/repo")
            });
            let events =
                vec![
                    cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            this.add_theme(cx);
                        }
                    }),
                ];
            self.themes.input = Some(input);
            self.themes._events = events;
        }
        if !self.themes.listed {
            self.themes.listed = true;
            self.refresh_themes(cx);
        }
    }

    fn refresh_themes(&mut self, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::ThemeListMethod>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.themes.list = list.themes;
                        let want: Vec<InstalledTheme> = this
                            .themes
                            .list
                            .iter()
                            .filter(|t| {
                                t.error.is_none() && !this.themes.art.contains_key(&art_key(t))
                            })
                            .cloned()
                            .collect();
                        for t in want {
                            this.load_art(t, cx);
                        }
                    }
                    Err(e) => eprintln!("aivory: theme/list failed: {e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The preview of a theme: `theme/get` for the colors, `theme/asset` for the image.
    fn load_art(&mut self, theme: InstalledTheme, cx: &mut Context<Self>) {
        let endpoint = self.model.read(cx).endpoint();
        let key = art_key(&theme);
        let id = theme.id.clone();
        let task = cx.background_executor().spawn(async move {
            let bundle = crate::daemon::call::<method::ThemeGet>(
                &endpoint,
                &ThemeGetParams {
                    id: id.clone(),
                    display: DisplayClass::Desktop,
                },
            )
            .ok()?;
            let (t, _) = crate::themed::overlay::overlay(&bundle.tokens, &bundle.fonts);
            let strip = vec![t.page, t.card, t.track, t.primary, t.q, t.work, t.ok];
            let image = bundle.preview.as_ref().and_then(|path| {
                let asset = crate::daemon::call::<method::ThemeAssetMethod>(
                    &endpoint,
                    &ThemeAssetParams {
                        id: id.clone(),
                        path: path.clone(),
                    },
                )
                .ok()?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(asset.base64)
                    .ok()?;
                decode_preview(&bytes)
            });
            Some(CardArt { strip, image })
        });
        cx.spawn(async move |this, cx| {
            if let Some(art) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.themes.art.insert(key, art);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Start a job and poll `theme/job` each 500 ms. `done` gets the result of the job.
    fn run_job<M>(
        &mut self,
        params: M::Params,
        label: String,
        done: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        M: Method<Result = JobRef> + 'static,
        M::Params: Send + 'static,
    {
        self.themes.busy = Some(label);
        self.themes.ticks = 0;
        self.themes.error = None;
        let start = self.model.read(cx).call::<M>(params, cx);
        cx.spawn(async move |this, cx| {
            let outcome: Result<Value, String> = async {
                let job = start.await.map_err(|e| notice::reason(&e))?;
                loop {
                    let call = this
                        .update(cx, |t, cx| {
                            t.themes.ticks += 1;
                            cx.notify();
                            t.model.read(cx).call::<method::ThemeJob>(
                                JobRef {
                                    job: job.job.clone(),
                                },
                                cx,
                            )
                        })
                        .map_err(|e| e.to_string())?;
                    let st = call.await.map_err(|e| notice::reason(&e))?;
                    match st.state {
                        JobState::Pending => {
                            cx.background_executor()
                                .timer(Duration::from_millis(500))
                                .await;
                        }
                        JobState::Done => break Ok(st.result.unwrap_or(Value::Null)),
                        JobState::Failed => {
                            break Err(st.error.unwrap_or_else(|| "The job failed.".to_string()));
                        }
                    }
                }
            }
            .await;
            let _ = this.update(cx, |t, cx| {
                t.themes.busy = None;
                done(t, outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The Add button: download and check the theme, then show the confirm.
    fn add_theme(&mut self, cx: &mut Context<Self>) {
        if self.themes.busy.is_some() {
            return;
        }
        let source = self
            .themes
            .input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if source.is_empty() {
            self.themes.error = Some("Paste a GitHub link or github:owner/repo.".to_string());
            cx.notify();
            return;
        }
        self.themes.inspected = None;
        self.run_job::<method::ThemeInspect>(
            ThemeSourceParams { source },
            "Downloads and checks the theme".to_string(),
            |this, result, _| match result
                .and_then(|v| serde_json::from_value::<Inspected>(v).map_err(|e| e.to_string()))
            {
                Ok(i) => this.themes.inspected = Some(i),
                Err(e) => this.themes.error = Some(e),
            },
            cx,
        );
    }

    fn install_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.themes.inspected.take() else {
            return;
        };
        // The link is done with: the field starts empty for the next add.
        if let Some(input) = &self.themes.input {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let call = self
            .model
            .read(cx)
            .call::<method::ThemeAdd>(ThemeAddParams { token: i.token }, cx);
        self.themes.busy = Some("Installs the theme".to_string());
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.themes.busy = None;
                match result {
                    Ok(t) => {
                        this.toast(Draft::info(format!("Installed the theme {}.", t.name)), cx);
                        this.refresh_themes(cx);
                    }
                    Err(e) => this.themes.error = Some(notice::reason(&e)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn use_theme(&mut self, id: &str, cx: &mut Context<Self>) {
        self.save(keys::display("customTheme"), json!(id), cx);
    }

    fn remove_theme(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.themes.working.insert(id.clone()) {
            return;
        }
        let call = self
            .model
            .read(cx)
            .call::<method::ThemeRemove>(ThemeRef { id: id.clone() }, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.themes.working.remove(&id);
                match result {
                    Ok(r) => {
                        crate::themed::forget(&id);
                        this.themes.list.retain(|t| t.id != id);
                        if r.cleared_current {
                            this.refresh(cx);
                            this.model.update(cx, |m, cx| m.refresh_display(cx));
                        }
                        this.toast(Draft::info(format!("Removed the theme {id}.")), cx);
                    }
                    Err(e) => this.themes.error = Some(notice::reason(&e)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The user presses "Check for updates". The only check there is.
    fn check_theme_updates(&mut self, cx: &mut Context<Self>) {
        if self.themes.busy.is_some() {
            return;
        }
        self.run_job::<method::ThemeCheckUpdates>(
            Empty {},
            "Asks the sources for newer versions".to_string(),
            |this, result, cx| match result
                .and_then(|v| serde_json::from_value::<UpdateList>(v).map_err(|e| e.to_string()))
            {
                Ok(list) => {
                    let text = match list.updates.len() {
                        0 => "All themes are current.".to_string(),
                        n => format!("{n} theme update(s) found."),
                    };
                    this.themes.updates = Some(list.updates);
                    if let Some(e) = list.errors.first() {
                        this.themes.error = Some(format!("Check failed for {e}"));
                    }
                    this.toast(Draft::info(text), cx);
                }
                Err(e) => this.themes.error = Some(e),
            },
            cx,
        );
    }

    fn update_theme(&mut self, id: String, cx: &mut Context<Self>) {
        if self.themes.busy.is_some() {
            return;
        }
        let who = id.clone();
        self.run_job::<method::ThemeUpdate>(
            ThemeRef { id },
            "Updates the theme".to_string(),
            move |this, result, cx| match result
                .and_then(|v| serde_json::from_value::<UpdateResult>(v).map_err(|e| e.to_string()))
            {
                Ok(r) => {
                    crate::themed::forget(&who);
                    if let Some(u) = &mut this.themes.updates {
                        u.retain(|x| x.id != who);
                    }
                    let kept = if r.kept.is_empty() {
                        String::new()
                    } else {
                        format!(" {} edited file(s) stay.", r.kept.len())
                    };
                    this.toast(
                        Draft::info(format!("Updated {} to {}.{kept}", r.id, r.version)),
                        cx,
                    );
                    this.refresh_themes(cx);
                    this.model.update(cx, |m, cx| m.refresh_display(cx));
                }
                Err(e) => this.themes.error = Some(e),
            },
            cx,
        );
    }

    // ---- drawing ----

    /// The Themes block of the Display card. `current` is `display.customTheme`.
    pub(super) fn themes_block(&self, current: Option<&str>, cx: &mut Context<Self>) -> Div {
        let p = &self.themes;
        let busy = p.busy.is_some();
        let mut block = div()
            .px(px(20.))
            .py(px(14.))
            .border_t_1()
            .border_color(rgba(super::ROW_LINE))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(row_label(
                "Themes",
                "Install a theme from GitHub. It sets colors, fonts and corners on every screen.",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(forms::boxed().flex_1().children(p.input.clone()))
                    .child(
                        forms::button("theme-add", "Add", true, busy)
                            .on_click(cx.listener(|this, _, _, cx| this.add_theme(cx))),
                    ),
            );
        if let Some(text) = &p.busy {
            block = block.child(
                div()
                    .text_size(crate::ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child(format!("{text}... {} s", p.ticks / 2)),
            );
        }
        if let Some(e) = &p.error {
            block = block.child(forms::error_line(e.clone()));
        }
        if let Some(i) = &p.inspected {
            block = block.child(self.theme_confirm(i, cx));
        }
        let mut cards = div().flex().flex_wrap().gap(px(8.));
        for t in &p.list {
            cards = cards.child(self.theme_card(t, current == Some(t.id.as_str()), cx));
        }
        if p.list.is_empty() {
            block = block.child(
                div()
                    .text_size(crate::ui::ts(T_SMALL))
                    .text_color(rgb(tk().ink_3))
                    .child("No theme is installed. The look cards above stay."),
            );
        } else {
            block = block.child(cards).child(
                div().flex().items_center().gap(px(10.)).child(
                    forms::button("theme-check", "Check for updates", false, busy)
                        .on_click(cx.listener(|this, _, _, cx| this.check_theme_updates(cx))),
                ),
            );
        }
        block
    }

    /// The question before the install: what the daemon found in the archive.
    fn theme_confirm(&self, i: &Inspected, cx: &mut Context<Self>) -> Div {
        let line = |label: &'static str, value: String| {
            div()
                .flex()
                .gap(px(10.))
                .text_size(crate::ui::ts(T_SMALL))
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_color(rgb(tk().ink_3))
                        .child(label),
                )
                .child(div().min_w(px(0.)).text_color(rgb(tk().ink)).child(value))
        };
        let installed = i.installed;
        card_edge()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(crate::ui::ts(T_BODY))
                    .child(format!("Install {} {}?", i.name, i.version)),
            )
            .child(line("Name", i.name.clone()))
            .child(line("Author", i.author.clone()))
            .child(line("License", i.license.clone()))
            .child(line(
                "Size",
                format!("{} in {} files", size_text(i.size), i.files),
            ))
            .child(line("SHA-256", i.sha256.clone()).font_family(tk().mono))
            .when(installed, |d| {
                d.child(forms::error_line(
                    "This theme is installed. Cancel, then use Update on its card.",
                ))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        forms::button("theme-cancel", "Cancel", false, false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.themes.inspected = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        forms::button("theme-install", "Install", true, installed).on_click(
                            cx.listener(move |this, _, window, cx| {
                                if !installed {
                                    this.install_theme(window, cx);
                                }
                            }),
                        ),
                    ),
            )
    }

    /// One theme: preview, name, source and version, and its buttons. The style follows the
    /// look cards.
    fn theme_card(&self, t: &InstalledTheme, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let art = self.themes.art.get(&art_key(t));
        let update = self
            .themes
            .updates
            .as_ref()
            .and_then(|u| u.iter().find(|x| x.id == t.id))
            .cloned();
        let working = self.themes.working.contains(&t.id);
        let (id_use, id_rm, id_up) = (t.id.clone(), t.id.clone(), t.id.clone());
        let preview = match art {
            Some(CardArt {
                image: Some(image), ..
            }) => div()
                .h(px(96.))
                .rounded(px(6.))
                .overflow_hidden()
                .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover)),
            Some(CardArt { strip, .. }) => {
                let mut row = div().h(px(96.)).rounded(px(6.)).overflow_hidden().flex();
                for c in strip {
                    row = row.child(div().flex_1().h_full().bg(rgb(*c)));
                }
                row
            }
            None => div().h(px(96.)).rounded(px(6.)).bg(rgb(tk().track)),
        };
        div()
            .id(SharedString::from(format!("theme-{}", t.id)))
            .w(px(CARD_W))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(4.))
            .rounded(px(10.))
            .border_2()
            .border_color(if on { rgb(tk().ink) } else { rgba(0) })
            .child(preview.border_1().border_color(rgba(tk().line)))
            .child(
                div()
                    .px(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(crate::ui::ts(T_SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if on { tk().ink } else { tk().ink_2 }))
                                    .child(t.name.clone()),
                            )
                            .when_some(update.clone(), |d, u| {
                                d.child(badge(&format!("Update {}", u.latest), Tone::Warn))
                            }),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(crate::ui::ts(T_CAPTION))
                            .text_color(rgb(tk().ink_3))
                            .child(format!("{} · {}", source_text(&t.source), t.version)),
                    )
                    .when_some(t.error.clone(), |d, e| {
                        d.child(
                            div()
                                .text_size(crate::ui::ts(T_CAPTION))
                                .text_color(rgb(tk().e_text))
                                .child(e),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .px(px(2.))
                    .pb(px(2.))
                    .child(
                        button(
                            format!("theme-use-{}", t.id),
                            if on { "In use" } else { "Use" },
                            !on,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !on {
                                this.use_theme(&id_use, cx);
                            }
                        })),
                    )
                    .when(update.is_some(), |d| {
                        d.child(
                            button(format!("theme-up-{}", t.id), "Update", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.update_theme(id_up.clone(), cx);
                                }),
                            ),
                        )
                    })
                    .child(
                        button(format!("theme-rm-{}", t.id), "Remove", false)
                            .opacity(if working { 0.6 } else { 1. })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_theme(id_rm.clone(), cx);
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_short() {
        assert_eq!(size_text(12), "12 B");
        assert_eq!(size_text(2048), "2 KB");
        assert_eq!(size_text(3 * 1_048_576), "3.0 MB");
    }

    #[test]
    fn a_local_source_has_a_plain_name() {
        assert_eq!(source_text("local:C:/x"), "local folder");
        assert_eq!(source_text("github:a/b"), "github:a/b");
        assert_eq!(source_text(""), "unknown source");
    }

    #[test]
    fn a_card_key_has_the_version() {
        let t = InstalledTheme {
            id: "my-theme".into(),
            name: "My".into(),
            version: "1.0.0".into(),
            author: String::new(),
            license: String::new(),
            source: String::new(),
            edited: Vec::new(),
            error: None,
        };
        assert_eq!(art_key(&t), "my-theme@1.0.0");
    }
}
