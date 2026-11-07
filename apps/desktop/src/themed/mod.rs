//! Installed theme packages in the desktop app (ADR 0024). The daemon checks a theme and serves
//! it as JSON (`theme/get`, `theme/asset`). This module loads the bundle, registers its fonts,
//! and builds the tokens once for each archive hash. It compiles the layouts of the bundle once.
//!
//! - [`overlay`]: the tokens of a theme on the `Tokens` of Minimal.
//! - [`data`]: the snapshot that layouts read.
//! - [`actions`]: the controls of a theme and the confirm that the app owns.
//! - [`host`]: the compiled layouts and the view that keeps the state of the components.
//! - [`render`], `content`, `sessions`: the components of the catalog.

pub mod actions;
mod content;
pub mod data;
pub mod host;
pub mod overlay;
pub mod render;
mod sessions;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use aivory_protocol::rpc::method;
use aivory_protocol::theme::{DisplayClass, ThemeAssetParams, ThemeBundle, ThemeGetParams};
use aivoryd::Endpoint;
use base64::Engine as _;
use gpui::{App, Entity};

use crate::daemon;
use crate::model::Model;
use crate::notice::Draft;

#[derive(Default)]
struct State {
    /// Loads in flight.
    pending: HashSet<String>,
    /// Themes that failed to load in this run. One notice each.
    failed: HashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// True while the app draws a custom theme. The model builds the snapshot only then.
pub fn active() -> bool {
    ACTIVE.with(Cell::get)
}

pub fn set_active(on: bool) {
    ACTIVE.with(|a| a.set(on));
}

/// The theme was removed or updated: load it again on the next [`sync`].
pub fn forget(id: &str) {
    overlay::forget(id);
    host::drop_theme(id);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.failed.remove(id);
        s.pending.remove(id);
    });
}

/// A bundle and the bytes of its font files.
struct Fetched {
    bundle: ThemeBundle,
    fonts: Vec<Vec<u8>>,
}

fn fetch(endpoint: &Endpoint, id: &str) -> anyhow::Result<Fetched> {
    let bundle = daemon::call::<method::ThemeGet>(
        endpoint,
        &ThemeGetParams {
            id: id.to_string(),
            display: DisplayClass::Desktop,
        },
    )?;
    let mut fonts = Vec::new();
    for f in &bundle.fonts {
        let asset = daemon::call::<method::ThemeAssetMethod>(
            endpoint,
            &ThemeAssetParams {
                id: id.to_string(),
                path: f.file.clone(),
            },
        )?;
        fonts.push(base64::engine::general_purpose::STANDARD.decode(asset.base64)?);
    }
    Ok(Fetched { bundle, fonts })
}

/// Load the theme `custom` (the `display.customTheme` setting) when it is not loaded yet. A
/// failure shows one notice, and the app stays on Minimal. The windows refresh when it is done.
pub fn sync(model: &Entity<Model>, endpoint: Endpoint, custom: Option<&str>, cx: &mut App) {
    let Some(id) = custom.map(str::to_string) else {
        return;
    };
    let start = overlay::loaded(&id).is_none()
        && STATE.with(|s| {
            let mut s = s.borrow_mut();
            !s.failed.contains(&id) && s.pending.insert(id.clone())
        });
    if !start {
        return;
    }
    let task = cx.background_executor().spawn({
        let id = id.clone();
        async move { fetch(&endpoint, &id) }
    });
    let model = model.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            STATE.with(|s| s.borrow_mut().pending.remove(&id));
            match result {
                Ok(f) => finish(&model, &id, f, cx),
                Err(e) => fail(&model, &id, &format!("{e:#}"), cx),
            }
        });
    })
    .detach();
}

fn fail(model: &Entity<Model>, id: &str, why: &str, cx: &mut App) {
    eprintln!("aivory: theme {id} cannot load: {why}");
    STATE.with(|s| s.borrow_mut().failed.insert(id.to_string()));
    model.update(cx, |m, cx| {
        m.notify_user(
            Draft::failure(format!("The theme \"{id}\" cannot load. Minimal is on.")),
            cx,
        );
    });
}

fn finish(model: &Entity<Model>, id: &str, f: Fetched, cx: &mut App) {
    // Fonts stay until the app restarts (GPUI has no way to drop one).
    if !f.fonts.is_empty()
        && let Err(e) = cx
            .text_system()
            .add_fonts(f.fonts.into_iter().map(Cow::Owned).collect())
    {
        return fail(model, id, &format!("a font does not load: {e:#}"), cx);
    }
    let b = &f.bundle;
    match host::compile(b) {
        Ok(c) => host::store(c),
        Err(e) => return fail(model, id, &format!("the layouts do not load: {e}"), cx),
    }
    overlay::intern(id, &b.sha256, || {
        let (tokens, notes) = overlay::overlay(&b.tokens, &b.fonts);
        for n in notes {
            eprintln!("aivory: theme {id}: {n}");
        }
        tokens
    });
    model.update(cx, |m, cx| m.refresh_display(cx));
}
