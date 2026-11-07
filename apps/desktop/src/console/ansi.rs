//! ANSI SGR colors in tool output. The parser keeps the colors, bold, dim, italic and
//! underline, and it strips every other escape code.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// 0-7 normal, 8-15 bright.
    Palette(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Style {
    fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// The text without escape codes and the styled ranges of it (plain text has no range).
pub fn parse(input: &str) -> (String, Vec<(Range<usize>, Style)>) {
    let mut text = String::with_capacity(input.len());
    let mut runs: Vec<(Range<usize>, Style)> = Vec::new();
    let mut style = Style::default();
    let mut start = 0;
    let mut chars = input.chars().peekable();
    let close = |text: &String, start: &mut usize, style: &Style, runs: &mut Vec<_>| {
        if text.len() > *start && !style.is_plain() {
            runs.push((*start..text.len(), *style));
        }
        *start = text.len();
    };
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    let mut params = String::new();
                    let mut last = ' ';
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            last = n;
                            break;
                        }
                        params.push(n);
                    }
                    if last == 'm' {
                        close(&text, &mut start, &style, &mut runs);
                        apply(&mut style, &params);
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // A character set (`ESC ( B`) has one more character.
                Some('(' | ')' | '*' | '+' | '#' | '%') => {
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {}
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => text.push(c),
        }
    }
    close(&text, &mut start, &style, &mut runs);
    (text, runs)
}

fn apply(style: &mut Style, params: &str) {
    if params.is_empty() {
        *style = Style::default();
        return;
    }
    let codes: Vec<u32> = params
        .split([';', ':'])
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            c @ 30..=37 => style.fg = Some(Color::Palette((c - 30) as u8)),
            39 => style.fg = None,
            c @ 90..=97 => style.fg = Some(Color::Palette((c - 90 + 8) as u8)),
            38 => match codes.get(i + 1) {
                Some(5) => {
                    if let Some(n) = codes.get(i + 2) {
                        style.fg = Some(xterm(*n));
                    }
                    i += 2;
                }
                Some(2) => {
                    if let [r, g, b] = codes[(i + 2).min(codes.len())..]
                        .iter()
                        .take(3)
                        .copied()
                        .collect::<Vec<_>>()[..]
                    {
                        style.fg = Some(Color::Rgb(r as u8, g as u8, b as u8));
                    }
                    i += 4;
                }
                _ => {}
            },
            // The background colors stay out: the console stays calm.
            48 => match codes.get(i + 1) {
                Some(5) => i += 2,
                Some(2) => i += 4,
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
}

/// An xterm 256 color: 0-15 are the palette, 16-231 the color cube, 232-255 the grey ramp.
fn xterm(n: u32) -> Color {
    match n {
        0..=15 => Color::Palette(n as u8),
        16..=231 => {
            let n = n - 16;
            let level = |v: u32| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            Color::Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        _ => {
            let v = (8 + (n.min(255) - 232) * 10) as u8;
            Color::Rgb(v, v, v)
        }
    }
}

/// The color of a palette entry as `0xRRGGBB`, or `None` for the text color of the console.
pub fn palette(index: u8, dark: bool) -> Option<u32> {
    const LIGHT: [u32; 16] = [
        0x1A1A2E, 0xB91C1C, 0x15803D, 0xA16207, 0x1D4ED8, 0x7E22CE, 0x0E7490, 0x6B7280, 0x4B5563,
        0xDC2626, 0x16A34A, 0xB45309, 0x2563EB, 0x9333EA, 0x0891B2, 0x374151,
    ];
    const DARK: [u32; 16] = [
        0x9AA2B4, 0xF87171, 0x4ADE80, 0xFACC15, 0x7CA7FF, 0xC084FC, 0x22D3EE, 0xD1D5DB, 0x9CA3AF,
        0xFCA5A5, 0x86EFAC, 0xFDE047, 0xA5C0FF, 0xD8B4FE, 0x67E8F9, 0xF3F4F6,
    ];
    let table = if dark { &DARK } else { &LIGHT };
    table.get(usize::from(index)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_become_ranges_and_codes_leave_the_text() {
        let (text, runs) = parse("ok \u{1b}[31mFAILED\u{1b}[0m done");
        assert_eq!(text, "ok FAILED done");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, 3..9);
        assert_eq!(runs[0].1.fg, Some(Color::Palette(1)));
    }

    #[test]
    fn bold_bright_and_reset_work() {
        let (text, runs) = parse("\u{1b}[1;92mpass\u{1b}[22m x\u{1b}[m y");
        assert_eq!(text, "pass x y");
        assert_eq!(runs[0].0, 0..4);
        assert!(runs[0].1.bold);
        assert_eq!(runs[0].1.fg, Some(Color::Palette(10)));
        assert_eq!(runs[1].0, 4..6);
        assert!(!runs[1].1.bold);
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn extended_colors_parse() {
        let (_, runs) = parse("\u{1b}[38;5;196ma\u{1b}[38;2;1;2;3mb");
        assert_eq!(runs[0].1.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(runs[1].1.fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn other_codes_are_stripped() {
        let (text, runs) = parse("a\u{1b}[2K\u{1b}[1Ab\u{1b}]0;title\u{7}c\r\nd\u{1b}(Be");
        assert_eq!(text, "abc\nde");
        assert!(runs.is_empty());
    }

    #[test]
    fn a_background_code_adds_no_range() {
        let (text, runs) = parse("\u{1b}[41mred bg\u{1b}[0m");
        assert_eq!(text, "red bg");
        assert!(runs.is_empty());
    }

    #[test]
    fn the_palette_follows_the_theme() {
        assert_eq!(palette(1, false), Some(0xB91C1C));
        assert_eq!(palette(1, true), Some(0xF87171));
        assert_eq!(palette(16, false), None);
    }
}
