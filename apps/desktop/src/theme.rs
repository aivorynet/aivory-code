//! Design tokens of the accepted design (docs/design/screens/README.md, "Shared tokens").
//! Colors are `0xRRGGBB` for `rgb()` or `0xRRGGBBAA` for `rgba()`.

use gpui::{Pixels, px};

pub const SANS: &str = "IBM Plex Sans";

// ---- type scale -----------------------------------------------------------------------------
// The only text sizes of the interface, in px at 100 %. `ui::ts()` scales them by the interface
// text size (Settings → Display). IBM Plex Sans for all interface text; JetBrains Mono only for
// code, commands, file paths, and numbers that line up in columns.

/// Meta lines, hints, counts, the status bar.
pub const T_CAPTION: f32 = 12.;
/// Secondary text: chips, pickers, buttons, table cells, labels.
pub const T_SMALL: f32 = 13.;
/// Body text, transcript prose, inputs.
pub const T_BODY: f32 = 14.;
/// Card and pane titles, section titles.
pub const T_TITLE: f32 = 16.;
/// Screen titles.
pub const T_HEADING: f32 = 20.;
pub const MONO: &str = "JetBrains Mono";
// The brand face of the Scipio app (scipio-apps, BricolageGrotesque.ttf): weight 600, the
// optical size at the font size, tight tracking. Static cuts, because GPUI sets no font axes
// and no letter spacing; use them with FontWeight::NORMAL (the weight is in the cut).
/// Screen titles and stat numbers (opsz 26, -0.022 em).
pub const DISPLAY: &str = "Bricolage Grotesque Title";
/// The big number of the Overview (opsz 46, -0.03 em).
pub const DISPLAY_FIGURE: &str = "Bricolage Grotesque Figure";
/// Section, group and column headings (opsz 18, -0.02 em).
pub const DISPLAY_HEADING: &str = "Bricolage Grotesque Heading";
/// Screen titles in the display face.
pub const T_DISPLAY: f32 = 26.;

/// UI text size of the desktop app.
pub const TEXT: Pixels = px(13.);

// Ink (Scipio graphite).
pub const INK: u32 = 0x16171B;
pub const INK_2: u32 = 0x3A3A3F;
pub const INK_3: u32 = 0x6E6E73;
pub const MUTED: u32 = 0x8A93A6;
pub const LOGO: u32 = 0x0D0D0E;

// Surfaces.
pub const PAGE: u32 = 0xFFFFFF;
pub const CARD: u32 = 0xFFFFFF;
/// The soft grey of secondary buttons, tracks, fields and chips (`--sub`).
pub const TRACK: u32 = 0xF2F2F7;
/// The hover step of [`TRACK`] (`--sub2`).
pub const TRACK_2: u32 = 0xE5E5EA;
pub const RAIL_ON: u32 = 0xF2F2F7;
/// rgba(60,60,67,.14): hairlines.
pub const LINE: u32 = 0x3C3C4324;
/// rgba(60,60,67,.30): control borders.
pub const LINE_2: u32 = 0x3C3C434D;

// Actions.
pub const DARK: u32 = 0x16171B;
pub const DARK_HOVER: u32 = 0x2A2A3A;
pub const SWITCH_OFF: u32 = 0xC9CED8;
pub const SWITCH_ON: u32 = 0x34C759;

// States: crimson only for "needs you", amber for an error.
pub const Q: u32 = 0xD20025;
pub const Q_TEXT: u32 = 0xD20025;
/// rgba(210,0,37,.07) on white.
pub const Q_BG: u32 = 0xFCEDF0;
pub const E: u32 = 0x9A5200;
pub const E_TEXT: u32 = 0x9A5200;
pub const E_BG: u32 = 0xFFF3E2;
pub const QUIET: u32 = 0xA0AEC0;
pub const OK: u32 = 0x16A34A;

// Radii: chips 6, inputs 10-14, cards 12-24, session panes 18, buttons are pills.
pub const R_BUTTON: Pixels = px(8.);
/// Decision tiles and the large surfaces of the war room.
pub const R_TILE: Pixels = px(24.);
/// Session panes of the war room grid.
pub const R_PANE: Pixels = px(18.);
pub const R_RAIL_ITEM: Pixels = px(10.);

// Shell geometry.
pub const RAIL_W: Pixels = px(76.);
/// The rail under [`NARROW`]: icons only.
pub const RAIL_W_COMPACT: Pixels = px(52.);
pub const HEADER_H: Pixels = px(72.);
pub const STATUS_H: Pixels = px(30.);
/// Under this window width the rail shrinks to icons.
pub const NARROW: Pixels = px(720.);

// Shell surfaces used by the war room. The main action is graphite (Scipio).
pub const PRIMARY: u32 = 0x16171B;
pub const PRIMARY_HOVER: u32 = 0x2A2A3A;
/// Links and the focus ring of a picked chip.
pub const LINK: u32 = 0x1F4FB8;
pub const IDLE_RING: u32 = 0x8A96A8;
pub const MENU_HOVER: u32 = 0xF2F2F7;
pub const R_CARD: Pixels = px(12.);

// ---- run-time tokens (ADR 0022) -------------------------------------------------------------
// The consts above are the Minimal light values. The screens read `tk()`, so a look switch
// redraws the whole app with other colors, faces and radii.

/// The style tokens of one look. Field names are the const names in lower case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub look: aivory_protocol::Look,
    /// True for a dark page: panes, charts and menus pick their dark variants.
    pub night: bool,
    pub sans: &'static str,
    pub mono: &'static str,
    pub display: &'static str,
    pub display_figure: &'static str,
    pub display_heading: &'static str,
    pub ink: u32,
    pub ink_2: u32,
    pub ink_3: u32,
    pub muted: u32,
    pub logo: u32,
    pub page: u32,
    pub card: u32,
    pub track: u32,
    pub track_2: u32,
    pub rail_on: u32,
    pub line: u32,
    pub line_2: u32,
    pub dark: u32,
    pub dark_hover: u32,
    pub switch_off: u32,
    pub switch_on: u32,
    pub q: u32,
    pub q_text: u32,
    pub q_bg: u32,
    pub e: u32,
    pub e_text: u32,
    pub e_bg: u32,
    pub quiet: u32,
    pub ok: u32,
    pub primary: u32,
    pub primary_hover: u32,
    pub link: u32,
    pub idle_ring: u32,
    pub menu_hover: u32,
    /// The page behind the cards of the war room board.
    pub board_bg: u32,
    /// A working agent: lines, dots and the Working gate.
    pub work: u32,
    /// The tint of the Working gate header.
    pub work_bg: u32,
    /// Text on [`Tokens::work_bg`].
    pub work_text: u32,
    /// The tint of a status pill that says "connected" or "done".
    pub ok_bg: u32,
    /// Text on [`Tokens::ok_bg`], and a passed gate.
    pub ok_text: u32,
    /// The border of a button.
    pub edge: u32,
    /// The border of a text field.
    pub field_line: u32,
    pub r_button: Pixels,
    pub r_tile: Pixels,
    pub r_pane: Pixels,
    pub r_rail_item: Pixels,
    pub r_card: Pixels,
}

/// Minimal, light: the reference look.
pub const MINIMAL: Tokens = Tokens {
    look: aivory_protocol::Look::Minimal,
    night: false,
    sans: SANS,
    mono: MONO,
    display: DISPLAY,
    display_figure: DISPLAY_FIGURE,
    display_heading: DISPLAY_HEADING,
    ink: INK,
    ink_2: INK_2,
    ink_3: INK_3,
    muted: MUTED,
    logo: LOGO,
    page: PAGE,
    card: CARD,
    track: TRACK,
    track_2: TRACK_2,
    rail_on: RAIL_ON,
    line: LINE,
    line_2: LINE_2,
    dark: DARK,
    dark_hover: DARK_HOVER,
    switch_off: SWITCH_OFF,
    switch_on: SWITCH_ON,
    q: Q,
    q_text: Q_TEXT,
    q_bg: Q_BG,
    e: E,
    e_text: E_TEXT,
    e_bg: E_BG,
    quiet: QUIET,
    ok: OK,
    primary: PRIMARY,
    primary_hover: PRIMARY_HOVER,
    link: LINK,
    idle_ring: IDLE_RING,
    menu_hover: MENU_HOVER,
    board_bg: 0xF6F7F9,
    work: 0x3574F0,
    work_bg: 0xEEF3FE,
    work_text: 0x2A5BC4,
    ok_bg: 0xEAF7EE,
    ok_text: 0x15803D,
    edge: 0xC9CCD6,
    field_line: 0xCDD2DA,
    r_button: R_BUTTON,
    r_tile: R_TILE,
    r_pane: R_PANE,
    r_rail_item: R_RAIL_ITEM,
    r_card: R_CARD,
};

/// A surface color: `light` on a light page, `dark` on a dark page. For a literal that has no token.
pub fn pick(light: u32, dark: u32) -> u32 {
    if tk().night { dark } else { light }
}

thread_local! {
    static CURRENT: std::cell::Cell<&'static Tokens> = const { std::cell::Cell::new(&MINIMAL) };
}

/// The tokens of the current look. GPUI draws on one thread, so a thread-local is enough.
pub fn tk() -> &'static Tokens {
    CURRENT.with(|c| c.get())
}

/// Switch the look of this thread. The caller refreshes the windows.
pub fn set_tokens(tokens: &'static Tokens) {
    CURRENT.with(|c| c.set(tokens));
}

/// Provider colors.
pub fn provider_color(provider: &str) -> u32 {
    match provider {
        "claude" => 0xD97757,
        // A dark look needs a light tile: the navy of Minimal would vanish on black.
        "codex" if tk().night => 0x8C8CD8,
        "codex" => 0x1A1A2E,
        "gemini" => 0x4B5BDB,
        "opencode" => 0x7C3AED,
        "goose" => 0x0D9488,
        _ => MUTED,
    }
}

/// The console theme of the session panes (Display popover: White, Grey, Dark, Navy).
/// Colors as `0xRRGGBBAA` for `rgba()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneTheme {
    pub bg: u32,
    pub text: u32,
    pub dim: u32,
    pub line: u32,
    pub hover: u32,
    pub user_bg: u32,
    pub add: u32,
    pub add_bg: u32,
    pub del: u32,
    pub del_bg: u32,
    pub sub: u32,
    pub tool: u32,
    pub field_bg: u32,
    pub field_border: u32,
    pub ghost_hover: u32,
    pub accent: u32,
    pub ask_bg: u32,
    pub ask_border: u32,
    pub ask_head_bg: u32,
    pub ask_head: u32,
    pub err_bg: u32,
    pub err_border: u32,
    pub err_head_bg: u32,
    pub err_head: u32,
    pub sub_on_bg: u32,
    pub sub_on_border: u32,
    pub sub_on_text: u32,
    /// The tint of code blocks and inline code.
    pub code_bg: u32,
    /// Links.
    pub link: u32,
    /// The bar at the left of a prompt and a quote.
    pub bar: u32,
    /// The header row and other chrome around the transcript.
    pub top: u32,
    /// The raised card of a permission or a question (Genie `BG_ELEVATED`).
    pub card: u32,
    pub card_border: u32,
    pub card_head: u32,
    /// The media row at the top of the input box.
    pub media_bg: u32,
    /// The text field inside the input box.
    pub input_bg: u32,
    /// A plain button (Allow is the blue default).
    pub button_bg: u32,
    pub button_border: u32,
}

const fn opaque(rgb: u32) -> u32 {
    (rgb << 8) | 0xFF
}

impl PaneTheme {
    /// The light console: a white card with dark ink (war room v2).
    pub const WHITE: Self = Self {
        bg: opaque(0xFFFFFF),
        text: opaque(0x1A1A2E),
        dim: opaque(0x5A6578),
        line: 0x0D0D0E14,
        hover: 0x0D0D0E2E,
        user_bg: opaque(0xF3F5F8),
        add: opaque(0x15803D),
        add_bg: opaque(0xF0FAF3),
        del: opaque(0xB91C1C),
        del_bg: opaque(0xFEF3F3),
        sub: opaque(0x6D28D9),
        tool: opaque(0x16A34A),
        field_bg: opaque(0xF2F2F7),
        field_border: opaque(0xF2F2F7),
        ghost_hover: opaque(0xF2F2F7),
        accent: opaque(0x7C3AED),
        ask_bg: opaque(0xFFFBF4),
        ask_border: opaque(0xF1D9B5),
        ask_head_bg: opaque(0xFDF1DE),
        ask_head: opaque(0x92400E),
        err_bg: opaque(0xFFF8F8),
        err_border: opaque(0xF2C7C7),
        err_head_bg: opaque(0xFCE8E8),
        err_head: opaque(0x991B1B),
        sub_on_bg: opaque(0xF3EEFF),
        sub_on_border: opaque(0xC4B5FD),
        sub_on_text: opaque(0x6D28D9),
        code_bg: opaque(0xF7F8FA),
        link: opaque(0x2F5BD3),
        bar: opaque(0xC9CCD6),
        top: opaque(0xFFFFFF),
        card: opaque(0xFFFFFF),
        card_border: opaque(0xD20025),
        card_head: opaque(0xD20025),
        media_bg: opaque(0xEEF0F3),
        input_bg: opaque(0xF2F2F7),
        button_bg: opaque(0xF2F2F7),
        button_border: opaque(0xF2F2F7),
    };

    pub const GREY: Self = Self {
        bg: opaque(0xF7F8FA),
        user_bg: opaque(0xEBEEF2),
        code_bg: opaque(0xE9ECF0),
        bar: opaque(0xC3C9D3),
        line: 0x0D0D0E17,
        top: opaque(0xF7F8FA),
        card: opaque(0xFFFFFF),
        media_bg: opaque(0xE9EBEF),
        field_border: opaque(0xC4C8D1),
        button_border: opaque(0xC4C8D1),
        ..Self::WHITE
    };

    pub const DARK: Self = Self {
        bg: opaque(0x15161B),
        text: opaque(0xECEDF1),
        dim: opaque(0xB8BECB),
        line: 0xFFFFFF14,
        hover: 0xFFFFFF3D,
        user_bg: 0xFFFFFF12,
        add: opaque(0x86E3AE),
        add_bg: 0x86E3AE14,
        del: opaque(0xF7A8A8),
        del_bg: 0xF7A8A814,
        sub: opaque(0xC4B5FD),
        tool: opaque(0x4ADE80),
        field_bg: opaque(0x2B2D30),
        field_border: opaque(0x43454A),
        ghost_hover: 0xFFFFFF14,
        accent: opaque(0xC4B5FD),
        ask_bg: 0xFBBF240D,
        ask_border: 0xFBBF2447,
        ask_head_bg: 0xFBBF241F,
        ask_head: opaque(0xFCD34D),
        err_bg: 0xF871710D,
        err_border: 0xF871714D,
        err_head_bg: 0xF871711F,
        err_head: opaque(0xFCA5A5),
        sub_on_bg: 0xC4B5FD24,
        sub_on_border: 0xC4B5FD66,
        sub_on_text: opaque(0xDDD6FE),
        code_bg: 0xFFFFFF0F,
        link: opaque(0x8AB4FF),
        bar: 0xFFFFFF3D,
        top: opaque(0x101115),
        card: opaque(0x393B40),
        card_border: opaque(0xFF8A9A),
        card_head: opaque(0xFF8A9A),
        media_bg: opaque(0x313338),
        input_bg: opaque(0x1E1F22),
        button_bg: opaque(0x2B2D30),
        button_border: opaque(0x4E5157),
    };

    pub const NAVY: Self = Self {
        bg: opaque(0x0D1624),
        field_bg: opaque(0x17243A),
        field_border: opaque(0x2C3D5A),
        line: 0xA7AABC1F,
        top: opaque(0x09111D),
        card: opaque(0x1F2E48),
        media_bg: opaque(0x1F2E48),
        input_bg: opaque(0x0D1624),
        button_bg: opaque(0x17243A),
        button_border: opaque(0x3A4E72),
        ..Self::DARK
    };

    /// The Bridge console: black ground, warm white text, orange, pink and purple chrome
    /// (ADR 0022). One meaning per color: pink is data, yellow a highlight, red an alert.
    pub const BRIDGE: Self = Self {
        bg: opaque(0x000000),
        text: opaque(0xF2EEE4),
        dim: opaque(0xC8C0D0),
        line: 0xCE9CCE40,
        hover: 0xCE9CCE40,
        user_bg: opaque(0x1C141C),
        add: opaque(0xCE9CCE),
        add_bg: 0xCE9CCE1A,
        del: opaque(0xFF8C84),
        del_bg: 0xE06A621A,
        sub: opaque(0xCE9CCE),
        tool: opaque(0xFFCE63),
        field_bg: opaque(0x101016),
        field_border: opaque(0xCE9CCE),
        ghost_hover: opaque(0x18181F),
        accent: opaque(0xFF9C00),
        ask_bg: 0xFF9C001A,
        ask_border: opaque(0xFF9C00),
        ask_head_bg: 0xFF9C0033,
        ask_head: opaque(0xFFCE63),
        err_bg: 0xE06A621A,
        err_border: opaque(0xE06A62),
        err_head_bg: 0xE06A6233,
        err_head: opaque(0xFF8C84),
        sub_on_bg: 0xCE9CCE22,
        sub_on_border: opaque(0xCE9CCE),
        sub_on_text: opaque(0xF2EEE4),
        code_bg: opaque(0x14101A),
        link: opaque(0xFFCE63),
        bar: opaque(0xFF9C00),
        top: opaque(0x000000),
        card: opaque(0x101016),
        card_border: opaque(0xE06A62),
        card_head: opaque(0xE06A62),
        media_bg: opaque(0x1C141C),
        input_bg: opaque(0x101016),
        button_bg: opaque(0x24242E),
        button_border: opaque(0xCE9CCE),
    };

    /// The console theme for the Display background in the current look: Bridge has its own
    /// console, and a dark look never shows a white pane.
    pub fn for_look(background: aivory_protocol::ConsoleBackground) -> Self {
        use aivory_protocol::ConsoleBackground as B;
        let t = tk();
        if t.look == aivory_protocol::Look::Bridge {
            return Self::BRIDGE;
        }
        match background {
            B::White | B::Grey if t.night => Self::DARK,
            b => Self::of(b),
        }
    }

    /// A dark console: light text on a near-black background.
    pub fn is_dark(&self) -> bool {
        luminance(self.bg >> 8) < 0.2
    }

    /// The theme for the pane header: the console theme on its `top` surface (Genie toolbar).
    pub fn header(&self) -> Self {
        Self {
            bg: self.top,
            ..*self
        }
    }

    pub fn of(background: aivory_protocol::ConsoleBackground) -> Self {
        use aivory_protocol::ConsoleBackground as B;
        match background {
            B::White => Self::WHITE,
            B::Grey => Self::GREY,
            B::Dark => Self::DARK,
            B::Navy => Self::NAVY,
        }
    }
}

// ---- contrast -------------------------------------------------------------------------------

fn channel(c: u32) -> f64 {
    let c = f64::from(c & 0xFF) / 255.;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The relative luminance (WCAG) of `0xRRGGBB`.
pub fn luminance(rgb: u32) -> f64 {
    0.2126 * channel(rgb >> 16) + 0.7152 * channel(rgb >> 8) + 0.0722 * channel(rgb)
}

/// The WCAG contrast ratio of two `0xRRGGBB` colors, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// `0xRRGGBBAA` over the opaque `0xRRGGBB`: the color that the eye sees.
#[cfg(test)]
pub fn composite(fg: u32, bg: u32) -> u32 {
    let a = f64::from(fg & 0xFF) / 255.;
    let mix = |shift: u32| {
        let f = f64::from((fg >> (shift + 8)) & 0xFF);
        let b = f64::from((bg >> shift) & 0xFF);
        ((f * a + b * (1. - a)).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// A text color that a program chose (ANSI 24-bit): moved toward white on a dark background
/// and toward black on a light one, until it reaches the contrast `min` against `bg`.
pub fn fit_contrast(fg: u32, bg: u32, min: f64) -> u32 {
    let toward = if luminance(bg) < 0.5 { 0xFF } else { 0x00 };
    let mut color = fg;
    for step in 1..=20 {
        if contrast(color, bg) >= min {
            break;
        }
        let t = f64::from(step) / 20.;
        let mix = |shift: u32| {
            let c = f64::from((fg >> shift) & 0xFF);
            ((c + (f64::from(toward) - c) * t).round() as u32) << shift
        };
        color = mix(16) | mix(8) | mix(0);
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text color of a dark console against every surface it sits on.
    fn check(t: &PaneTheme, name: &str) {
        let bg = t.bg >> 8;
        let surface = |c: u32| composite(c, bg);
        let surfaces = [
            ("bg", bg),
            ("prompt", surface(t.user_bg)),
            ("code", surface(t.code_bg)),
            ("field", t.field_bg >> 8),
            ("add", surface(t.add_bg)),
            ("del", surface(t.del_bg)),
            ("ask", surface(t.ask_bg)),
            ("ask head", surface(t.ask_head_bg)),
            ("error", surface(t.err_bg)),
            ("error head", surface(t.err_head_bg)),
            ("subagent", surface(t.sub_on_bg)),
            ("hover", surface(t.ghost_hover)),
        ];
        for (surface_name, s) in surfaces {
            // Body text and the grey of results and meta lines.
            for (what, c) in [("text", t.text), ("dim", t.dim)] {
                let ratio = contrast(c >> 8, s);
                assert!(
                    ratio >= 7.,
                    "{name}: {what} on {surface_name} is {ratio:.2}:1"
                );
            }
            // Colors with a meaning.
            for (what, c) in [
                ("add", t.add),
                ("del", t.del),
                ("sub", t.sub),
                ("tool", t.tool),
                ("accent", t.accent),
                ("link", t.link),
                ("ask head", t.ask_head),
                ("error head", t.err_head),
                ("subagent", t.sub_on_text),
            ] {
                let ratio = contrast(c >> 8, s);
                assert!(
                    ratio >= 4.5,
                    "{name}: {what} on {surface_name} is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn the_dark_console_reads_well() {
        check(&PaneTheme::DARK, "dark");
    }

    #[test]
    fn the_navy_console_reads_well() {
        check(&PaneTheme::NAVY, "navy");
    }

    #[test]
    fn the_bridge_console_reads_well() {
        check(&PaneTheme::BRIDGE, "bridge");
        assert!(PaneTheme::BRIDGE.is_dark());
    }

    #[test]
    fn the_header_sits_on_the_top_surface_of_its_console() {
        assert!(PaneTheme::DARK.is_dark() && PaneTheme::NAVY.is_dark());
        assert!(!PaneTheme::WHITE.is_dark() && !PaneTheme::GREY.is_dark());
        for t in [
            PaneTheme::WHITE,
            PaneTheme::GREY,
            PaneTheme::DARK,
            PaneTheme::NAVY,
        ] {
            let h = t.header();
            assert_eq!(h.bg, t.top);
            assert!(contrast(h.text >> 8, h.bg >> 8) >= 7.);
            assert!(contrast(h.dim >> 8, h.bg >> 8) >= 4.5);
            assert!(contrast(t.card_head >> 8, t.card >> 8) >= 4.5);
        }
    }

    #[test]
    fn contrast_of_black_and_white_is_21() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.).abs() < 0.01);
        assert!((contrast(0x777777, 0x777777) - 1.).abs() < 0.001);
    }

    #[test]
    fn a_program_color_moves_until_it_is_readable() {
        let bg = PaneTheme::DARK.bg >> 8;
        assert!(contrast(0x010203, bg) < 2.);
        let fitted = fit_contrast(0x010203, bg, 4.5);
        assert!(contrast(fitted, bg) >= 4.5);
        // A readable color stays as it is, and a light background darkens.
        assert_eq!(fit_contrast(0xFFFFFF, bg, 4.5), 0xFFFFFF);
        let paper = PaneTheme::WHITE.bg >> 8;
        assert!(contrast(fit_contrast(0xF0F0F0, paper, 4.5), paper) >= 4.5);
    }
}
