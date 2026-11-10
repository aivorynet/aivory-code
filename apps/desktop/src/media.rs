//! The preview of a media line in a transcript: an image as a thumbnail, a video, a PDF, or a
//! link as a small card. A click opens the file in the viewer of the system, or the link in
//! the browser. Each look renders its transcript lines and calls [`preview`] for a line with
//! `media`.

use std::path::PathBuf;

use aivory_protocol::{Media, MediaKind};
use gpui::prelude::*;
use gpui::{AnyElement, ElementId, ObjectFit, Rgba, div, img, px};

use crate::theme::T_CAPTION;
use crate::ui;

/// The largest thumbnail edge.
pub const THUMB: f32 = 240.;

/// The colors of a card in one look.
#[derive(Debug, Clone, Copy)]
pub struct MediaStyle {
    pub card: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub radius: f32,
}

/// The preview of `media`. `id` must be unique in the transcript (the line index).
pub fn preview(media: &Media, id: impl Into<ElementId>, style: MediaStyle) -> AnyElement {
    let target = media.clone();
    let open = move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App| {
        if target.src.starts_with("http://") || target.src.starts_with("https://") {
            cx.open_url(&target.src);
        } else if target.kind == MediaKind::Image {
            // A local image opens large in the app.
            cx.set_global(Viewer(Some(target.clone())));
            cx.refresh_windows();
        } else {
            cx.open_with_system(&PathBuf::from(&target.src));
        }
    };
    let local = !media.src.starts_with("http");
    if media.kind == MediaKind::Image && local && std::path::Path::new(&media.src).is_file() {
        return div()
            .id(id)
            .flex_none()
            .max_w(px(THUMB))
            .max_h(px(THUMB))
            .rounded(px(style.radius))
            .overflow_hidden()
            .cursor_pointer()
            .tooltip(ui::tip(format!("Show {} large", media.name())))
            .on_click(open)
            .child(
                img(PathBuf::from(&media.src))
                    .max_w(px(THUMB))
                    .max_h(px(THUMB))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element();
    }
    let (icon, word) = match media.kind {
        MediaKind::Image => ("icons/capture.svg", "Image"),
        MediaKind::Video => ("icons/play.svg", "Video"),
        MediaKind::Pdf => ("icons/file.svg", "PDF"),
        MediaKind::Link => ("icons/link.svg", "Link"),
    };
    div()
        .id(id)
        .flex_none()
        .max_w(px(THUMB + 120.))
        .px(px(10.))
        .py(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(style.radius))
        .bg(style.card)
        .cursor_pointer()
        .tooltip(ui::tip(format!("Open {}", media.src)))
        .on_click(open)
        .child(ui::icon(icon, 14., style.muted))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .text_size(ui::ts(T_CAPTION))
                .child(div().text_color(style.muted).child(word))
                .child(
                    div()
                        .text_color(style.text)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(media.name().to_string()),
                ),
        )
        .into_any_element()
}

/// The image that the large view shows over the app.
#[derive(Default)]
pub struct Viewer(pub Option<Media>);

impl gpui::Global for Viewer {}

/// Close the large view. `true`: it was open (Esc).
pub fn close_viewer(cx: &mut gpui::App) -> bool {
    let open = cx.try_global::<Viewer>().is_some_and(|v| v.0.is_some());
    if open {
        cx.set_global(Viewer(None));
        cx.refresh_windows();
    }
    open
}

/// The large view of an image over the whole window: the image fits the window, a click on
/// the dark area or Esc closes it, and a button opens it in the viewer of the system.
pub fn viewer(cx: &gpui::App) -> Option<AnyElement> {
    let media = cx.try_global::<Viewer>()?.0.clone()?;
    let path = PathBuf::from(&media.src);
    let system = path.clone();
    let t = crate::theme::tk();
    Some(
        div()
            .id("media-viewer")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .p(px(40.))
            .bg(gpui::rgba(0x0000_00CC))
            .occlude()
            .on_click(|_, _, cx| {
                close_viewer(cx);
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(img(path).size_full().object_fit(ObjectFit::Contain)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(ui::ts(T_CAPTION))
                    .text_color(gpui::rgb(0xFFFFFF))
                    .child(media.name().to_string())
                    .child(
                        div()
                            .id("media-viewer-open")
                            .px(px(10.))
                            .py(px(4.))
                            .rounded_full()
                            .bg(gpui::rgb(t.card))
                            .text_color(gpui::rgb(t.ink))
                            .cursor_pointer()
                            .child("Open in viewer")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                cx.open_with_system(&system);
                            }),
                    )
                    .child(
                        div()
                            .text_color(gpui::rgba(0xFFFF_FFAA))
                            .child("Esc closes"),
                    ),
            )
            .into_any_element(),
    )
}

/// The page style of the Minimal look.
pub fn page_style() -> MediaStyle {
    let t = crate::theme::tk();
    MediaStyle {
        card: gpui::rgb(t.track),
        text: gpui::rgb(t.ink),
        muted: gpui::rgb(t.ink_3),
        radius: 8.,
    }
}
