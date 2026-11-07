//! Theme tokens (ADR 0024 section 2) on the `Tokens` of the app. The base is Minimal light or
//! Minimal dark, picked by the luminance of `bg`, with `look: Minimal`, so the compiled
//! branches of the looks fall through. About 12 colors give about 40 fields; the other fields
//! come from the tints below. A color that fails the contrast check keeps the Minimal value.
//!
//! `tk()` returns `&'static Tokens`, so a theme leaks one `Tokens` for each archive hash. The
//! interning map holds it; a second load of the same hash leaks nothing.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use aivory_protocol::theme::ThemeFont;
use gpui::px;
use serde_json::Value;

use crate::look::MINIMAL_DARK;
use crate::theme::{MINIMAL, MONO, Tokens};

/// The page is dark when its relative luminance is under this value.
const DARK_BELOW: f32 = 0.18;
/// Corner radii of a theme stay in this range, in px.
const RADIUS_MAX: f32 = 48.;

#[derive(Default)]
struct Interned {
    by_sha: HashMap<String, &'static Tokens>,
    strs: HashMap<String, &'static str>,
    by_id: HashMap<String, &'static Tokens>,
}

fn pool() -> MutexGuard<'static, Interned> {
    static POOL: OnceLock<Mutex<Interned>> = OnceLock::new();
    POOL.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// One `&'static str` for each distinct text (a font family name).
fn leak_str(text: &str) -> &'static str {
    let mut p = pool();
    if let Some(s) = p.strs.get(text) {
        return s;
    }
    let s: &'static str = Box::leak(text.to_string().into_boxed_str());
    p.strs.insert(text.to_string(), s);
    s
}

/// The tokens of an archive hash: built once, then reused. `id` names the theme for
/// [`loaded`].
pub fn intern(id: &str, sha: &str, build: impl FnOnce() -> Tokens) -> &'static Tokens {
    let cached = pool().by_sha.get(sha).copied();
    let t = cached.unwrap_or_else(|| {
        let leaked: &'static Tokens = Box::leak(Box::new(build()));
        pool().by_sha.insert(sha.to_string(), leaked);
        leaked
    });
    pool().by_id.insert(id.to_string(), t);
    t
}

/// Forget the id (the theme was removed or updated). The leaked tokens stay in the sha map.
pub fn forget(id: &str) {
    pool().by_id.remove(id);
}

/// The tokens of a theme that was loaded in this run.
pub fn loaded(id: &str) -> Option<&'static Tokens> {
    pool().by_id.get(id).copied()
}

// ---- color math ----------------------------------------------------------------------------

fn parse_color(text: &str) -> Option<(u32, Option<u8>)> {
    let hex = text.strip_prefix('#')?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some((n, None)),
        8 => Some((n >> 8, Some((n & 0xFF) as u8))),
        _ => None,
    }
}

fn channels(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 0xFF) as f32,
        ((c >> 8) & 0xFF) as f32,
        (c & 0xFF) as f32,
    ]
}

/// `a` toward `b` by `t` (0 to 1).
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let (a, b) = (channels(a), channels(b));
    let ch = |i: usize| (a[i] + (b[i] - a[i]) * t).round().clamp(0., 255.) as u32;
    (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

/// Relative luminance (WCAG).
pub fn luminance(c: u32) -> f32 {
    let lin = |v: f32| {
        let v = v / 255.;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(c);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Contrast ratio (WCAG), 1 to 21.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `0xRRGGBBAA` of a color with an alpha byte.
fn with_alpha(c: u32, alpha: u32) -> u32 {
    (c << 8) | alpha
}

// ---- the overlay ---------------------------------------------------------------------------

type Field = fn(&mut Tokens) -> &mut u32;
/// A contrast check: its name, the field, the background, the minimum ratio, the Minimal value.
type Check = (&'static str, Field, fn(&Tokens) -> u32, f32, u32);

fn color(tokens: &Value, pointer: &str) -> Option<u32> {
    parse_color(tokens.pointer(pointer)?.as_str()?).map(|(c, _)| c)
}

fn family(
    tokens: &Value,
    key: &str,
    fonts: &[ThemeFont],
    notes: &mut Vec<String>,
) -> Option<&'static str> {
    let want = tokens.pointer(&format!("/fonts/{key}"))?.as_str()?;
    match want {
        "sans" => None,
        "mono" => Some(MONO),
        "serif" => {
            notes.push("font serif is not bundled; the sans face stays".to_string());
            None
        }
        file => fonts
            .iter()
            .find(|f| f.file == file)
            .map(|f| leak_str(&f.family))
            .or_else(|| {
                notes.push(format!("font {file} is not in the bundle"));
                None
            }),
    }
}

fn radius(tokens: &Value, name: &str) -> Option<gpui::Pixels> {
    let v = tokens.pointer(&format!("/radii/{name}"))?.as_f64()? as f32;
    Some(px(v.clamp(0., RADIUS_MAX)))
}

/// Build the tokens of a theme from its merged `tokens` JSON. The notes list each check that
/// failed and each token that the overlay did not take.
pub fn overlay(tokens: &Value, fonts: &[ThemeFont]) -> (Tokens, Vec<String>) {
    let mut notes = Vec::new();
    let bg = color(tokens, "/colors/bg");
    let dark = bg.is_some_and(|c| luminance(c) < DARK_BELOW);
    let base = if dark { MINIMAL_DARK } else { MINIMAL };
    let mut t = Tokens {
        look: aivory_protocol::Look::Minimal,
        night: dark,
        ..base
    };
    let get = |p: &str| color(tokens, p);
    let page = get("/colors/bg").unwrap_or(base.page);
    let surface = get("/colors/surface").unwrap_or(base.card);
    let text = get("/colors/text").unwrap_or(base.ink);
    let muted = get("/colors/muted").unwrap_or(base.muted);
    let accent = get("/colors/accent").unwrap_or(base.primary);
    let st = |name: &str, fallback: u32| get(&format!("/colors/state/{name}")).unwrap_or(fallback);
    let needs_you = st("needsYou", base.q);
    let working = st("working", base.work);
    let idle = st("idle", base.idle_ring);
    let parked = st("parked", base.quiet);
    let error = st("error", base.e);
    let done = st("done", base.ok);

    t.page = page;
    t.card = surface;
    t.board_bg = mix(page, surface, 0.5);
    t.ink = text;
    t.ink_2 = mix(text, page, 0.2);
    t.ink_3 = mix(text, page, 0.45);
    t.muted = muted;
    t.logo = text;
    t.track = mix(surface, text, 0.05);
    t.track_2 = mix(surface, text, 0.10);
    t.rail_on = t.track;
    t.menu_hover = t.track;
    t.line = with_alpha(text, 0x24);
    t.line_2 = with_alpha(text, 0x4D);
    t.dark = text;
    t.dark_hover = mix(text, page, 0.15);
    t.primary = accent;
    t.primary_hover = mix(accent, text, 0.15);
    t.link = accent;
    t.switch_off = mix(page, text, 0.25);
    t.switch_on = done;
    t.q = needs_you;
    t.q_text = needs_you;
    t.q_bg = mix(page, needs_you, 0.08);
    t.e = error;
    t.e_text = error;
    t.e_bg = mix(page, error, 0.08);
    t.quiet = parked;
    t.ok = done;
    t.ok_text = done;
    t.ok_bg = mix(page, done, 0.08);
    t.idle_ring = idle;
    t.work = working;
    t.work_text = working;
    t.work_bg = mix(page, working, 0.08);
    t.edge = mix(page, text, 0.2);
    t.field_line = mix(page, text, 0.18);

    // Contrast: (name, foreground, background, minimum ratio). On failure the field keeps the
    // Minimal value.
    let checks: [Check; 12] = [
        ("ink on page", |t| &mut t.ink, |t| t.page, 4.5, base.ink),
        ("ink on card", |t| &mut t.ink, |t| t.card, 4.5, base.ink),
        (
            "ink_2 on page",
            |t| &mut t.ink_2,
            |t| t.page,
            4.5,
            base.ink_2,
        ),
        (
            "ink_3 on page",
            |t| &mut t.ink_3,
            |t| t.page,
            3.0,
            base.ink_3,
        ),
        (
            "muted on page",
            |t| &mut t.muted,
            |t| t.page,
            3.0,
            base.muted,
        ),
        (
            "muted on card",
            |t| &mut t.muted,
            |t| t.card,
            3.0,
            base.muted,
        ),
        ("link on page", |t| &mut t.link, |t| t.page, 3.0, base.link),
        (
            "q_text on q_bg",
            |t| &mut t.q_text,
            |t| t.q_bg,
            4.5,
            base.q_text,
        ),
        (
            "e_text on e_bg",
            |t| &mut t.e_text,
            |t| t.e_bg,
            4.5,
            base.e_text,
        ),
        (
            "ok_text on ok_bg",
            |t| &mut t.ok_text,
            |t| t.ok_bg,
            4.5,
            base.ok_text,
        ),
        (
            "work_text on work_bg",
            |t| &mut t.work_text,
            |t| t.work_bg,
            4.5,
            base.work_text,
        ),
        (
            "primary on card",
            |t| &mut t.primary,
            |t| t.card,
            3.0,
            base.primary,
        ),
    ];
    for (name, field, bg_of, min, fallback) in checks {
        let bg = bg_of(&t);
        let have = *field(&mut t);
        let ratio = contrast(have, bg);
        if ratio < min {
            notes.push(format!(
                "contrast {name}: {ratio:.1} is under {min:.1}; the Minimal value stays"
            ));
            *field(&mut t) = fallback;
        }
    }

    if let Some(f) = family(tokens, "ui", fonts, &mut notes) {
        t.sans = f;
        t.display = f;
        t.display_figure = f;
        t.display_heading = f;
    }
    if let Some(f) = family(tokens, "mono", fonts, &mut notes) {
        t.mono = f;
    }
    for (name, slot) in [
        ("button", &mut t.r_button),
        ("card", &mut t.r_card),
        ("tile", &mut t.r_tile),
        ("pane", &mut t.r_pane),
        ("rail_item", &mut t.r_rail_item),
    ] {
        if let Some(r) = radius(tokens, name) {
            *slot = r;
        }
    }
    (t, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        let files = aivory_theme::sample::files("my-theme", "1.0.0");
        let text = files
            .iter()
            .find(|(p, _)| p == "tokens.toml")
            .map(|(_, b)| String::from_utf8(b.clone()).unwrap())
            .unwrap();
        aivory_theme::tokens::merge(&[("tokens.toml".to_string(), text)])
            .unwrap()
            .1
    }

    #[test]
    fn a_light_theme_sits_on_minimal() {
        let (t, notes) = overlay(&sample(), &[]);
        assert!(!t.night);
        assert_eq!(t.look, aivory_protocol::Look::Minimal);
        assert_eq!(t.page, 0xFFFFFF);
        assert_eq!(t.primary, 0x2563EB);
        assert_eq!(t.q, 0xD97706);
        assert_eq!(t.r_card, px(8.));
        // The radius that the theme leaves out stays as in Minimal.
        assert_eq!(t.r_pane, MINIMAL.r_pane);
        assert!(contrast(t.ink, t.page) >= 4.5, "{notes:?}");
    }

    #[test]
    fn a_dark_page_picks_the_dark_base() {
        let mut v = sample();
        v["colors"]["bg"] = json!("#101114");
        v["colors"]["surface"] = json!("#1b1c21");
        v["colors"]["text"] = json!("#f1f1f4");
        let (t, _) = overlay(&v, &[]);
        assert!(t.night);
        assert_eq!(t.page, 0x101114);
    }

    #[test]
    fn a_failed_contrast_keeps_the_minimal_value_and_logs_it() {
        let mut v = sample();
        // A muted color that is almost the page color.
        v["colors"]["muted"] = json!("#fafafa");
        let (t, notes) = overlay(&v, &[]);
        assert_eq!(t.muted, MINIMAL.muted);
        assert!(
            notes.iter().any(|n| n.contains("muted on page")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_font_file_gives_its_family_and_a_radius_is_clamped() {
        let mut v = sample();
        v["fonts"]["ui"] = json!("assets/fonts/x.ttf");
        v["radii"]["card"] = json!(900);
        let fonts = [ThemeFont {
            file: "assets/fonts/x.ttf".into(),
            family: "Test Family".into(),
        }];
        let (t, _) = overlay(&v, &fonts);
        assert_eq!(t.sans, "Test Family");
        assert_eq!(t.r_card, px(RADIUS_MAX));
        let (again, _) = overlay(&v, &fonts);
        // The same text gives the same leaked str.
        assert!(std::ptr::eq(t.sans, again.sans));
    }

    #[test]
    fn one_archive_hash_leaks_one_tokens() {
        let a = intern("t-one", "sha-1", || overlay(&sample(), &[]).0);
        let b = intern("t-two", "sha-1", || panic!("built twice"));
        assert!(std::ptr::eq(a, b));
        assert!(std::ptr::eq(loaded("t-two").unwrap(), a));
        assert!(loaded("nothing").is_none());
    }
}
