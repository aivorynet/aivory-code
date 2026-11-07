//! The looks of the whole app (ADR 0022): the token set of each look, and the switch.
//! Minimal is the reference; the other looks start with their own tokens on the Minimal
//! elements and replace elements one by one.

use aivory_protocol::{Display, Look, Theme};
use gpui::px;

use crate::theme::{MINIMAL, Tokens, set_tokens, tk};

/// The looks in menu order, with their names.
pub const LOOKS: [(Look, &str); 8] = [
    (Look::Minimal, "Minimal"),
    (Look::Bridge, "Bridge"),
    (Look::Operator, "Operator"),
    (Look::Neon, "Neon"),
    (Look::Navigator, "Navigator"),
    (Look::Holo, "Holo"),
    (Look::Tactical, "Tactical"),
    (Look::Command, "Command"),
];

/// The name of a look.
pub fn name(look: Look) -> &'static str {
    LOOKS
        .iter()
        .find(|(l, _)| *l == look)
        .map_or("Minimal", |(_, n)| n)
}

/// Minimal on a dark page (Display theme Dark).
pub const MINIMAL_DARK: Tokens = Tokens {
    night: true,
    ink: 0xF2F2F5,
    ink_2: 0xC9CBD2,
    ink_3: 0x9A9DA6,
    muted: 0x7E8494,
    logo: 0xF2F2F5,
    page: 0x121316,
    card: 0x1C1D22,
    track: 0x26272E,
    track_2: 0x30323A,
    rail_on: 0x26272E,
    line: 0xFFFFFF1F,
    line_2: 0xFFFFFF40,
    dark: 0x3A3D47,
    dark_hover: 0x4A4E5A,
    switch_off: 0x4A4E5A,
    q: 0xFF4D6A,
    q_text: 0xFF6B83,
    q_bg: 0x3A1820,
    e: 0xF0A040,
    e_text: 0xF5B35C,
    e_bg: 0x3A2A10,
    quiet: 0x6B7385,
    ok: 0x34C759,
    primary: 0x3A3D47,
    primary_hover: 0x4A4E5A,
    link: 0x6EA0FF,
    idle_ring: 0x7E8494,
    menu_hover: 0x26272E,
    board_bg: 0x15161A,
    work: 0x5B8DEF,
    work_bg: 0x1A2540,
    work_text: 0x8DB0FF,
    ok_bg: 0x12301C,
    ok_text: 0x5FD98A,
    edge: 0x4A4E5A,
    field_line: 0x3A3D47,
    ..MINIMAL
};

/// A starship console: black page, orange and purple blocks, Antonio for titles and labels,
/// IBM Plex Sans for all content text, pill shapes. A calm set of colors, one meaning each.
pub const BRIDGE: Tokens = Tokens {
    look: Look::Bridge,
    night: true,
    sans: "Barlow Semi Condensed",
    mono: "IBM Plex Mono",
    display: "Antonio SB",
    display_figure: "Antonio SB",
    display_heading: "Antonio SB",
    ink: 0xF2EEE4,
    ink_2: 0xE4DCCC,
    ink_3: 0xCE9CCE,
    muted: 0xB4B0BC,
    logo: 0xFF9C00,
    page: 0x000000,
    card: 0x101016,
    track: 0x24242E,
    track_2: 0x34343F,
    rail_on: 0x34343F,
    line: 0xCE9CCE59,
    line_2: 0xCE9CCE99,
    dark: 0xB86E00,
    dark_hover: 0xFF9C00,
    switch_off: 0x34343F,
    switch_on: 0xFF9C00,
    q: 0xE06A62,
    q_text: 0xFF8C84,
    q_bg: 0x3A1614,
    e: 0xFFCE63,
    e_text: 0xFFCE63,
    e_bg: 0x332A10,
    quiet: 0x848484,
    ok: 0xCE9CCE,
    primary: 0xB86E00,
    primary_hover: 0xFF9C00,
    link: 0xFFCE63,
    idle_ring: 0x848484,
    menu_hover: 0x24242E,
    r_button: px(18.),
    r_card: px(18.),
    r_tile: px(24.),
    r_pane: px(0.),
    r_rail_item: px(18.),
    board_bg: 0x000000,
    work: 0xCE9CCE,
    work_bg: 0x2A1A2A,
    work_text: 0xE8C4E8,
    ok_bg: 0x2A1A2A,
    ok_text: 0xE8C4E8,
    edge: 0x848484,
    field_line: 0x848484,
};

/// An operator console: black page, phosphor green, mono text, square edges.
pub const OPERATOR: Tokens = Tokens {
    look: Look::Operator,
    night: true,
    sans: crate::theme::MONO,
    display: "VT323",
    display_figure: "VT323",
    display_heading: "VT323",
    ink: 0xC8FFD4,
    ink_2: 0x8FE0A0,
    ink_3: 0x5FA070,
    muted: 0x3F7A4C,
    logo: 0x00FF41,
    page: 0x000000,
    card: 0x020A04,
    track: 0x062010,
    track_2: 0x0B3018,
    rail_on: 0x0B3018,
    line: 0x00FF4126,
    line_2: 0x00FF4155,
    dark: 0x0B5A1E,
    dark_hover: 0x117A2A,
    switch_off: 0x0B3018,
    switch_on: 0x00C832,
    q: 0xFF4D4D,
    q_text: 0xFF6B6B,
    q_bg: 0x2A0808,
    e: 0xE0E040,
    e_text: 0xE0E040,
    e_bg: 0x202008,
    quiet: 0x2F5A38,
    ok: 0x00FF41,
    primary: 0x0B5A1E,
    primary_hover: 0x117A2A,
    link: 0x00FF41,
    idle_ring: 0x3F7A4C,
    menu_hover: 0x062010,
    r_button: px(2.),
    r_card: px(2.),
    r_tile: px(2.),
    r_pane: px(0.),
    r_rail_item: px(2.),
    board_bg: 0x000000,
    work: 0x00FF41,
    work_bg: 0x06200F,
    work_text: 0x6CFF8E,
    ok_bg: 0x04280F,
    ok_text: 0x00FF41,
    edge: 0x1A5A30,
    field_line: 0x0F4A25,
    ..MINIMAL
};

/// A dark neon interface: night page, neon cyan and magenta, Chakra Petch, hard edges.
pub const NEON: Tokens = Tokens {
    look: Look::Neon,
    night: true,
    sans: "Chakra Petch",
    display: "Orbitron",
    display_figure: "Orbitron",
    display_heading: "Chakra Petch",
    ink: 0xE8F6FF,
    ink_2: 0xB0C4D8,
    ink_3: 0x7F92A8,
    muted: 0x5E6C80,
    logo: 0x00F0FF,
    page: 0x07060D,
    card: 0x0F0D1A,
    track: 0x1A1630,
    track_2: 0x241E40,
    rail_on: 0x241E40,
    line: 0x00F0FF26,
    line_2: 0x00F0FF55,
    dark: 0xD81B5A,
    dark_hover: 0xFF2A6D,
    switch_off: 0x241E40,
    switch_on: 0x05FFA1,
    q: 0xFF2A6D,
    q_text: 0xFF4F88,
    q_bg: 0x2E0A1A,
    e: 0xFCEE0A,
    e_text: 0xFCEE0A,
    e_bg: 0x2A2808,
    quiet: 0x4A5468,
    ok: 0x05FFA1,
    primary: 0xD81B5A,
    primary_hover: 0xFF2A6D,
    link: 0x00F0FF,
    idle_ring: 0x5E6C80,
    menu_hover: 0x1A1630,
    r_button: px(2.),
    r_card: px(2.),
    r_tile: px(2.),
    r_pane: px(0.),
    r_rail_item: px(2.),
    board_bg: 0x07060D,
    work: 0x00F0FF,
    work_bg: 0x0A2430,
    work_text: 0x5FF6FF,
    ok_bg: 0x06301F,
    ok_text: 0x05FFA1,
    edge: 0x3A3270,
    field_line: 0x2E2858,
    ..MINIMAL
};

/// A 3D workstation control system: grey bevelled windows with black text on a blue desktop.
/// The screens inside the grey windows read these tokens, so they are light and high contrast.
pub const NAVIGATOR: Tokens = Tokens {
    look: Look::Navigator,
    night: false,
    ink: 0x111111,
    ink_2: 0x2A2A2A,
    ink_3: 0x444444,
    muted: 0x555555,
    logo: 0x111111,
    page: 0xBDBDBD,
    card: 0xC8C8C8,
    track: 0xA8A8A8,
    track_2: 0x989898,
    rail_on: 0xA8A8A8,
    line: 0x6A6A6A66,
    line_2: 0x6A6A6AAA,
    dark: 0x3A3A3A,
    dark_hover: 0x555555,
    switch_off: 0x8A8A8A,
    switch_on: 0x2FA84A,
    q: 0xC4262D,
    q_text: 0x9A1B21,
    q_bg: 0xE8C4C0,
    e: 0xA86A12,
    e_text: 0x6A4208,
    e_bg: 0xEAD6A8,
    quiet: 0x6A6A6A,
    ok: 0x1E8E3A,
    primary: 0x3D5BD8,
    primary_hover: 0x4B69E6,
    link: 0x1F3FB0,
    idle_ring: 0x6A6A6A,
    menu_hover: 0xD8D8D8,
    r_button: px(2.),
    r_card: px(2.),
    r_tile: px(2.),
    r_pane: px(0.),
    r_rail_item: px(2.),
    board_bg: 0xBDBDBD,
    work: 0x3D5BD8,
    work_bg: 0xD0D8F0,
    work_text: 0x2A43A8,
    ok_bg: 0xCDE8D2,
    ok_text: 0x1B6B2E,
    edge: 0x6A6A6A,
    field_line: 0x6A6A6A,
    ..MINIMAL
};

/// Holo: see-through glass over a deep blue page, cyan and violet light, orange for a wait.
pub const HOLO: Tokens = Tokens {
    look: Look::Holo,
    night: true,
    sans: "Rajdhani",
    ink: 0xDDF8FF,
    ink_2: 0xBFE6F7,
    ink_3: 0x8FBBD4,
    muted: 0x5F95B8,
    logo: 0x4FD8FF,
    page: 0x04101C,
    card: 0x0A1E30,
    track: 0x0E2A40,
    track_2: 0x143650,
    rail_on: 0x143650,
    line: 0x4FD8FF2E,
    line_2: 0x4FD8FF66,
    dark: 0x1D6E9E,
    dark_hover: 0x2A8CC4,
    switch_off: 0x143650,
    switch_on: 0x2AA9E0,
    q: 0xFF9A4D,
    q_text: 0xFFB27A,
    q_bg: 0x2A1608,
    e: 0xFF9A4D,
    e_text: 0xFFB27A,
    e_bg: 0x2A1608,
    quiet: 0x3A5A72,
    ok: 0x4FD8FF,
    primary: 0x1D6E9E,
    primary_hover: 0x2A8CC4,
    link: 0x4FD8FF,
    idle_ring: 0x5F95B8,
    menu_hover: 0x0E2A40,
    r_button: px(4.),
    r_card: px(4.),
    r_tile: px(4.),
    r_pane: px(0.),
    r_rail_item: px(4.),
    board_bg: 0x04101C,
    work: 0x4FD8FF,
    work_bg: 0x0A2A3E,
    work_text: 0x8CE8FF,
    ok_bg: 0x0A2A3E,
    ok_text: 0x4FD8FF,
    edge: 0x1D6E9E,
    field_line: 0x1D6E9E,
    ..MINIMAL
};

/// Tactical: a phosphor-green radar monitor, mono text, square edges, white only for a wait.
pub const TACTICAL: Tokens = Tokens {
    look: Look::Tactical,
    night: true,
    sans: "B612",
    ink: 0xCFFFD8,
    ink_2: 0x7BE08F,
    ink_3: 0x4FB866,
    muted: 0x2E8A45,
    logo: 0x39FF6A,
    page: 0x000400,
    card: 0x001A06,
    track: 0x00240A,
    track_2: 0x0B4A1E,
    rail_on: 0x0B4A1E,
    line: 0x39FF6A26,
    line_2: 0x39FF6A55,
    dark: 0x128A35,
    dark_hover: 0x1FA845,
    switch_off: 0x0B4A1E,
    switch_on: 0x22C24E,
    q: 0xE8FFE8,
    q_text: 0xFFFFFF,
    q_bg: 0x0B4A1E,
    e: 0xE8FFE8,
    e_text: 0xE8FFE8,
    e_bg: 0x0B4A1E,
    quiet: 0x1F6B33,
    ok: 0x39FF6A,
    primary: 0x128A35,
    primary_hover: 0x1FA845,
    link: 0x39FF6A,
    idle_ring: 0x2E8A45,
    menu_hover: 0x00240A,
    r_button: px(0.),
    r_card: px(0.),
    r_tile: px(0.),
    r_pane: px(0.),
    r_rail_item: px(0.),
    board_bg: 0x000400,
    work: 0x39FF6A,
    work_bg: 0x00240A,
    work_text: 0x7BE08F,
    ok_bg: 0x00240A,
    ok_text: 0x39FF6A,
    edge: 0x128A35,
    field_line: 0x128A35,
    ..MINIMAL
};

/// Command: an RTS command console. Olive panels on a dark hatched field, a command sidebar,
/// green for work and the selection, amber for a wait, red for a fault (board `CommandWarRoom`).
pub const COMMAND: Tokens = Tokens {
    look: Look::Command,
    night: true,
    sans: "VT323",
    mono: "VT323",
    display: "VT323",
    display_figure: "VT323",
    display_heading: "VT323",
    ink: 0xE6E8DA,
    ink_2: 0xD2D6C0,
    ink_3: 0xA8AE96,
    muted: 0x7E8A70,
    logo: 0x8FE05A,
    page: 0x1E2124,
    card: 0x24282B,
    track: 0x0A0C0A,
    track_2: 0x2C3135,
    rail_on: 0x2C3135,
    line: 0x5A5E58AA,
    line_2: 0x6E767CFF,
    dark: 0x3C4247,
    dark_hover: 0x4B5157,
    switch_off: 0x3C4247,
    switch_on: 0x5EA634,
    q: 0xFFC21A,
    q_text: 0xFFC21A,
    q_bg: 0x2E2408,
    e: 0xFF5A43,
    e_text: 0xFF5A43,
    e_bg: 0x2E140F,
    quiet: 0x7E8A70,
    ok: 0x8FE05A,
    primary: 0x5EA634,
    primary_hover: 0x6EB844,
    link: 0x8FE05A,
    idle_ring: 0x7E8A70,
    menu_hover: 0x2C3135,
    r_button: px(0.),
    r_card: px(0.),
    r_tile: px(0.),
    r_pane: px(0.),
    r_rail_item: px(0.),
    board_bg: 0x1E2124,
    work: 0x59E03A,
    work_bg: 0x1E2A16,
    work_text: 0x8FE05A,
    ok_bg: 0x1E2A16,
    ok_text: 0x8FE05A,
    edge: 0x6A7076,
    field_line: 0x3A4038,
};

/// The tokens of a look. `theme` picks light or dark for Minimal only.
pub fn tokens(look: Look, theme: Theme, system_dark: bool) -> &'static Tokens {
    match look {
        Look::Minimal => match theme {
            Theme::Dark => &MINIMAL_DARK,
            Theme::System if system_dark => &MINIMAL_DARK,
            _ => &MINIMAL,
        },
        Look::Bridge => &BRIDGE,
        Look::Operator => &OPERATOR,
        Look::Neon => &NEON,
        Look::Navigator => &NAVIGATOR,
        Look::Holo => &HOLO,
        Look::Tactical => &TACTICAL,
        Look::Command => &COMMAND,
    }
}

/// `AIVORY_LOOK=<look>` forces a look (screenshot runs). The old `AIVORY_SKIN` works too.
pub fn forced() -> Option<Look> {
    let v = std::env::var("AIVORY_LOOK")
        .or_else(|_| std::env::var("AIVORY_SKIN"))
        .ok()?;
    match v.as_str() {
        "minimal" => Some(Look::Minimal),
        "bridge" | "lcars" => Some(Look::Bridge),
        "operator" | "matrix" => Some(Look::Operator),
        "neon" | "nightcity" | "cyber" => Some(Look::Neon),
        "navigator" => Some(Look::Navigator),
        "holo" => Some(Look::Holo),
        "tactical" => Some(Look::Tactical),
        "command" => Some(Look::Command),
        _ => None,
    }
}

/// The tokens of a Display setting. The order is forced (`AIVORY_LOOK`), then the custom theme,
/// then the look. A custom theme that is not loaded (or failed) gives Minimal.
fn resolve(display: &Display, system_dark: bool) -> &'static Tokens {
    if let Some(look) = forced() {
        crate::themed::set_active(false);
        crate::themed::host::set_current(None);
        return tokens(look, display.theme, system_dark);
    }
    if let Some(id) = &display.custom_theme {
        if let Some(t) = crate::themed::overlay::loaded(id) {
            crate::themed::set_active(true);
            crate::themed::host::set_current(Some(id));
            return t;
        }
        crate::themed::set_active(false);
        crate::themed::host::set_current(None);
        return tokens(Look::Minimal, display.theme, system_dark);
    }
    crate::themed::set_active(false);
    crate::themed::host::set_current(None);
    tokens(display.look, display.theme, system_dark)
}

/// Leave the custom theme for a built-in look (the look switch of a theme layout). The theme
/// is cleared and the look is set; both are daemon settings, and the windows read them again.
pub fn leave_theme(model: &gpui::Entity<crate::model::Model>, look: Look, cx: &mut gpui::App) {
    use aivory_protocol::rpc::{SettingsSetParams, method};
    let set = |key: &str, value: serde_json::Value, cx: &mut gpui::App| {
        model.read(cx).call::<method::SettingsSet>(
            SettingsSetParams {
                key: key.into(),
                value,
            },
            cx,
        )
    };
    let clear = set("display.customTheme", serde_json::Value::Null, cx);
    let pick = set("display.look", serde_json::json!(look), cx);
    let model = model.clone();
    cx.spawn(async move |cx| {
        let _ = clear.await;
        let _ = pick.await;
        cx.update(|cx| model.update(cx, |m, cx| m.refresh_display(cx)));
    })
    .detach();
}

/// Apply the look of a Display setting. True when the tokens changed (refresh the windows).
pub fn apply(display: &Display, system_dark: bool) -> bool {
    let next = resolve(display, system_dark);
    if tk() == next {
        return false;
    }
    set_tokens(next);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_look_has_its_tokens() {
        for (look, _) in LOOKS {
            assert_eq!(tokens(look, Theme::Light, false).look, look);
        }
        assert!(!tokens(Look::Minimal, Theme::Light, true).night);
        assert!(tokens(Look::Minimal, Theme::System, true).night);
        assert!(tokens(Look::Operator, Theme::Light, false).night);
    }

    #[test]
    fn the_order_is_forced_then_custom_theme_then_look() {
        if forced().is_some() {
            return;
        }
        let mut d = Display {
            look: Look::Bridge,
            custom_theme: Some("look-test-none".into()),
            ..Display::default()
        };
        // A theme that is not loaded gives Minimal, not the look.
        assert_eq!(resolve(&d, false).look, Look::Minimal);
        assert!(!crate::themed::active());
        let on = crate::themed::overlay::intern("look-test", "look-test-sha", || {
            crate::themed::overlay::overlay(&serde_json::json!({}), &[]).0
        });
        d.custom_theme = Some("look-test".into());
        assert!(std::ptr::eq(resolve(&d, false), on));
        assert!(crate::themed::active());
        d.custom_theme = None;
        assert_eq!(resolve(&d, false).look, Look::Bridge);
        assert!(!crate::themed::active());
    }

    #[test]
    fn names_follow_the_menu() {
        assert_eq!(name(Look::Neon), "Neon");
        assert_eq!(name(Look::Bridge), "Bridge");
    }
}
