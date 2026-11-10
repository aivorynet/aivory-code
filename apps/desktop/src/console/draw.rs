//! The console look: blocks and markdown as GPUI elements. One mono font for everything
//! inside the body, a line height of 1.5, and a hierarchy by indent and weight. The war
//! room panes, the deep dive transcript and the inbox tail all draw from here.

use std::ops::Range;
use std::rc::Rc;

use aivory_protocol::{Line, LineKind};
use aivory_syntax::TokenKind;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Div, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText,
    SharedString, StrikethroughStyle, StyledText, UnderlineStyle, Window, div, px, relative, rgba,
};

use super::ansi;
use super::blocks::{self, Block, BlockKind, DiffRow, DiffSign, ToolState};
use super::markdown::{self, Inline, Item, MdBlock};
use super::todo::{self, TodoState};
use crate::theme::*;
use crate::ui;
use crate::warroom::Geom;
use aivory_protocol::layout::PaneDensity;

/// The sizes and colors of one console.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub font: &'static str,
    pub size: f32,
    /// The size of prompts and agent prose (sans).
    pub prose: f32,
    /// The line height as a multiple of the size.
    pub line: f32,
    pub theme: PaneTheme,
    pub pad_x: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
}

impl Style {
    /// The console of a war room pane: the Display font and size, the Display background.
    pub fn of(g: &Geom) -> Self {
        let (size, pad_x, pad_y) = match g.density {
            // A compact pane keeps the code size of the type ramp (body − 1) at most.
            PaneDensity::Normal if g.compact => (g.font_size.min(13.), 10., 8.),
            PaneDensity::Normal => (g.font_size, 14., 12.),
            PaneDensity::Mini => (g.font_size - 2., 10., 8.),
            PaneDensity::Micro => (g.font_size - 3., 8., 6.),
        };
        // Prose follows the pane: one step under the body size in a compact or mini pane.
        let prose = if g.compact || g.density != PaneDensity::Normal {
            T_BODY - 1.
        } else {
            T_BODY
        };
        Self {
            font: g.font,
            size,
            prose,
            line: 1.5,
            theme: g.theme,
            pad_x,
            pad_top: pad_y,
            pad_bottom: pad_y,
        }
    }

    /// A console with the default font, for the deep dive and the inbox.
    pub fn plain(size: f32, theme: PaneTheme) -> Self {
        Self {
            font: tk().mono,
            size,
            prose: T_BODY,
            line: 1.5,
            theme,
            pad_x: 14.,
            pad_top: 12.,
            pad_bottom: 12.,
        }
    }

    /// The width of one character. All console fonts are 0.6 em wide.
    pub fn ch(&self) -> f32 {
        self.size * 0.6
    }

    /// The height of one line.
    pub fn lh(&self) -> f32 {
        self.size * self.line
    }

    pub fn dark(&self) -> bool {
        self.theme.is_dark()
    }

    /// A `div` in the console font, size and line height.
    pub fn base(&self) -> Div {
        div()
            .font_family(self.font)
            .text_size(px(self.size))
            .line_height(relative(self.line))
            .text_color(rgba(self.theme.text))
    }
}

type FnToggle = Rc<dyn Fn(u32, &mut Window, &mut App)>;
type FnPath = Rc<dyn Fn(String, &mut Window, &mut App)>;
type FnLoad = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a click on a block does. A screen that has no handler leaves the field empty.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Fold or unfold the result rows of the block that starts at this line.
    pub toggle: Option<FnToggle>,
    /// Open a file in the Lens.
    pub open_file: Option<FnPath>,
    /// Load the older lines.
    pub load_older: Option<FnLoad>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks").finish_non_exhaustive()
    }
}

/// Everything that drawing a block needs.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    pub style: &'a Style,
    pub hooks: &'a Hooks,
    /// Names the elements of this transcript: they must not clash with other panes.
    pub id: &'a SharedString,
    /// The animation step (4 per second).
    pub tick: u32,
}

fn hsla(rgba_hex: u32) -> gpui::Hsla {
    ui::hsla(rgba_hex)
}

fn rgb_hex(rgb: u32) -> u32 {
    (rgb << 8) | 0xFF
}

fn underline(color: Option<gpui::Hsla>) -> UnderlineStyle {
    UnderlineStyle {
        thickness: px(1.),
        color,
        wavy: false,
    }
}

// ---- list items -----------------------------------------------------------------------

/// The first row of a transcript: the padding, or the marker of the older lines.
pub fn header_item(start: u32, f: &Frame) -> AnyElement {
    let s = f.style;
    match blocks::earlier_marker(start) {
        None => div().h(px(s.pad_top)).into_any_element(),
        Some(text) => {
            let load = f.hooks.load_older.clone();
            let t = s.theme;
            s.base()
                .id(SharedString::from(format!("{}-earlier", f.id)))
                .w_full()
                .px(px(s.pad_x))
                .pt(px(s.pad_top))
                .pb(px(2.))
                .text_color(rgba(t.dim))
                .when(load.is_some(), |d| {
                    d.cursor_pointer().hover(|st| st.text_color(rgba(t.text)))
                })
                .when_some(load, |d, load| {
                    d.on_click(move |_, window, cx| load(window, cx))
                })
                .child(text)
                .into_any_element()
        }
    }
}

/// The last row of a transcript: the bottom padding.
pub fn footer_item(f: &Frame) -> AnyElement {
    div().h(px(f.style.pad_bottom)).into_any_element()
}

/// Lines as a column of blocks, for a place that has no list (a conflict side).
pub fn stack(lines: &[Line], f: &Frame) -> Div {
    let blocks = blocks::group(lines, 0, blocks::Pending::No, &Default::default());
    let mut col = div().flex().flex_col();
    for b in &blocks {
        col = col.child(block_item(b, f));
    }
    col
}

/// One block with the side padding of the console.
pub fn block_item(b: &Block, f: &Frame) -> AnyElement {
    let inner = match b.kind {
        BlockKind::Meta => meta(b, f),
        BlockKind::User => user(b, f),
        BlockKind::Agent => agent(b, f),
        BlockKind::Subagent => subagent(b, f),
        BlockKind::Turn => turn(b, f),
        BlockKind::Error => error(b, f),
        BlockKind::Call => call(b, f),
    };
    div()
        .w_full()
        .px(px(f.style.pad_x))
        .child(inner)
        .into_any_element()
}

fn joined(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn meta(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let text = match b.lines[0].label.as_deref() {
        Some(label) => format!("{label} · {}", b.lines[0].text),
        None => b.lines[0].text.clone(),
    };
    let row = s.base().mt(px(8.)).text_color(rgba(s.theme.dim));
    if let Some(m) = &b.lines[0].media {
        let style = crate::media::MediaStyle {
            card: rgba(s.theme.user_bg),
            text: rgba(s.theme.text),
            muted: rgba(s.theme.dim),
            radius: 0.,
        };
        return row.child(crate::media::preview(
            m,
            ElementId::from((f.id.clone(), b.abs as usize)),
            style,
        ));
    }
    row.child(text)
}

fn turn(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let l = &b.lines[0];
    let text = match l.label.as_deref() {
        Some(label) => format!("{label} · {}", l.text),
        None => l.text.clone(),
    };
    s.base()
        .mt(px(18.))
        .pt(px(10.))
        .border_t_1()
        .border_color(rgba(s.theme.line))
        .text_color(rgba(s.theme.dim))
        .font_weight(FontWeight::BOLD)
        .child(text)
}

fn error(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    s.base()
        .mt(px(6.))
        .text_color(rgba(s.theme.del))
        .child(joined(&b.lines))
}

fn subagent(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let l = &b.lines[0];
    let mut text = String::from("↳ ");
    let mut spans = Vec::new();
    if let Some(label) = &l.label {
        let start = text.len();
        text.push_str(label);
        spans.push((
            start..text.len(),
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        ));
        text.push_str("  ");
    }
    text.push_str(&l.text);
    s.base()
        .mt(px(5.))
        .text_color(rgba(s.theme.sub))
        .child(StyledText::new(text).with_highlights(spans))
}

/// A prompt: the user's text in the sans font on a grey band with round corners.
fn user(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    s.base()
        .mt(px(14.))
        .mb(px(2.))
        .font_family(tk().sans)
        .text_size(crate::ui::ts(s.prose))
        .line_height(relative(1.55))
        .bg(rgba(t.user_bg))
        .rounded(px(8.))
        .py(px(8.))
        .px(px(12.))
        .child(joined(&b.lines))
}

// ---- agent text (markdown) --------------------------------------------------------------

fn agent(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let md = markdown::cached(&joined(&b.lines));
    let key = format!("{}-a{}", f.id, b.abs);
    // Agent prose reads in the sans font; code blocks keep the console font.
    s.base()
        .mt(px(10.))
        .font_family(tk().sans)
        .text_size(crate::ui::ts(s.prose))
        .line_height(relative(1.6))
        .child(md_blocks(&md.blocks, f, &key))
}

fn md_blocks(blocks: &[MdBlock], f: &Frame, key: &str) -> Div {
    let mut col = div().flex().flex_col().gap(px(f.style.size * 0.35));
    for (i, b) in blocks.iter().enumerate() {
        col = col.child(md_block(b, f, &format!("{key}.{i}")));
    }
    col
}

fn md_block(b: &MdBlock, f: &Frame, key: &str) -> AnyElement {
    let s = f.style;
    let t = s.theme;
    match b {
        MdBlock::Para(text) => inline(text, f, key).into_any_element(),
        MdBlock::Heading { level, text } => {
            let size = match level {
                1 => s.size * 1.2,
                2 => s.size * 1.1,
                _ => s.size,
            };
            div()
                .mt(px(4.))
                .text_size(px(size))
                .font_weight(FontWeight::BOLD)
                .when(*level >= 4, |d| d.text_color(rgba(t.dim)))
                .child(inline(text, f, key))
                .into_any_element()
        }
        MdBlock::Code { text, spans, .. } => code_block(text, spans, f).into_any_element(),
        MdBlock::Quote(inner) => div()
            .flex()
            .border_l_2()
            .border_color(rgba(t.bar))
            .pl(px(s.ch() * 1.5))
            .text_color(rgba(t.dim))
            .child(div().flex_1().min_w(px(0.)).child(md_blocks(inner, f, key)))
            .into_any_element(),
        MdBlock::List { start, items } => list(*start, items, f, key).into_any_element(),
        MdBlock::Table { head, rows, widths } => table(head, rows, widths, f, key),
        MdBlock::Rule => div()
            .my(px(4.))
            .h(px(1.))
            .w_full()
            .bg(rgba(t.line))
            .into_any_element(),
    }
}

fn code_block(text: &str, spans: &[(Range<usize>, TokenKind)], f: &Frame) -> Div {
    let s = f.style;
    let dark = s.dark();
    let runs: Vec<(Range<usize>, HighlightStyle)> = spans
        .iter()
        .filter_map(|(range, kind)| {
            let color = syntax_color(*kind, dark)?;
            Some((
                range.clone(),
                HighlightStyle {
                    color: Some(hsla(rgb_hex(color))),
                    font_style: (*kind == TokenKind::Comment).then_some(FontStyle::Italic),
                    ..Default::default()
                },
            ))
        })
        .collect();
    div()
        .w_full()
        .font_family(s.font)
        .text_size(px(s.size))
        .line_height(relative(s.line))
        .bg(rgba(s.theme.code_bg))
        .rounded(px(8.))
        .px(px(12.))
        .py(px(8.))
        .child(StyledText::new(text.to_string()).with_highlights(runs))
}

/// The syntax colors of a code block. They sit apart from the state colors.
pub fn syntax_color(kind: TokenKind, dark: bool) -> Option<u32> {
    let (light, dim) = match kind {
        TokenKind::Keyword => (0x4B5BDB, 0x9DB0FF),
        TokenKind::Type => (0x0D9488, 0x5EEAD4),
        TokenKind::Function => (0x6D28D9, 0xC4B5FD),
        TokenKind::String => (0x15803D, 0x86E3AE),
        TokenKind::Number | TokenKind::Constant => (0xC2410C, 0xFDBA74),
        TokenKind::Comment => (0x6B7488, 0xA3ABBD),
        TokenKind::Attribute | TokenKind::Macro => (0x5A6578, 0xA6AEC0),
        TokenKind::Variable | TokenKind::Operator | TokenKind::Punctuation => return None,
    };
    Some(if dark { dim } else { light })
}

fn list(start: Option<u64>, items: &[Item], f: &Frame, key: &str) -> Div {
    let s = f.style;
    let t = s.theme;
    let mut col = div().flex().flex_col().gap(px(2.));
    for (i, item) in items.iter().enumerate() {
        let marker = match (start, item.task) {
            (_, Some(done)) => checkbox(done, false, f).into_any_element(),
            (Some(n), None) => div()
                .text_color(rgba(t.dim))
                .child(format!("{}.", n + i as u64))
                .into_any_element(),
            (None, None) => div().text_color(rgba(t.dim)).child("•").into_any_element(),
        };
        let digits = start.map_or(1, |n| (n + items.len() as u64).to_string().len());
        let width = s.ch() * (digits as f32 + 2.);
        col = col.child(
            div()
                .flex()
                .child(
                    div()
                        .w(px(width))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .pr(px(s.ch()))
                        .child(marker),
                )
                .child(div().flex_1().min_w(px(0.)).child(md_blocks(
                    &item.blocks,
                    f,
                    &format!("{key}.{i}"),
                ))),
        );
    }
    col
}

/// A table as a grid: the columns share the width by their widest cell.
fn table(
    head: &[Inline],
    rows: &[Vec<Inline>],
    widths: &[usize],
    f: &Frame,
    key: &str,
) -> AnyElement {
    let s = f.style;
    let t = s.theme;
    let row = |cells: &[Inline], bold: bool, tag: String| {
        let mut r = div().flex().when(bold, |d| d.font_weight(FontWeight::BOLD));
        for (c, w) in widths.iter().enumerate() {
            // Natural width in characters; a narrow pane shrinks the columns together.
            let cell = div()
                .w(px(s.ch() * (*w as f32 + 3.)))
                .min_w(px(0.))
                .pr(px(s.ch() * 1.5));
            r = r.child(match cells.get(c) {
                Some(text) => cell.child(inline(text, f, &format!("{tag}.{c}"))),
                None => cell,
            });
        }
        r
    };
    let mut col = div().flex().flex_col().w_full();
    if !head.is_empty() {
        col = col.child(
            row(head, true, format!("{key}.h"))
                .pb(px(2.))
                .mb(px(2.))
                .border_b_1()
                .border_color(rgba(t.line)),
        );
    }
    for (i, r) in rows.iter().enumerate() {
        col = col.child(row(r, false, format!("{key}.r{i}")));
    }
    col.into_any_element()
}

/// The text of an inline run with its styles and the click targets of links and paths.
fn inline(text: &Inline, f: &Frame, key: &str) -> AnyElement {
    let t = f.style.theme;
    let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut targets: Vec<Target> = Vec::new();
    for seg in &text.segs {
        let mut st = HighlightStyle::default();
        if seg.bold {
            st.font_weight = Some(FontWeight::BOLD);
        }
        if seg.italic {
            st.font_style = Some(FontStyle::Italic);
        }
        if seg.code {
            st.background_color = Some(hsla(t.code_bg));
        }
        if seg.strike {
            st.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
        }
        if let Some(link) = seg.link {
            st.color = Some(hsla(t.link));
            st.underline = Some(underline(None));
            ranges.push(seg.range.clone());
            targets.push(Target::Url(text.links[link].clone()));
        } else if let Some(path) = &seg.path
            && f.hooks.open_file.is_some()
        {
            st.underline = Some(underline(Some(hsla(t.bar))));
            ranges.push(seg.range.clone());
            targets.push(Target::Path(path.clone()));
        }
        runs.push((seg.range.clone(), st));
    }
    let styled = StyledText::new(text.text.clone()).with_highlights(runs);
    if ranges.is_empty() {
        return styled.into_any_element();
    }
    let open = f.hooks.open_file.clone();
    InteractiveText::new(ElementId::Name(SharedString::from(key.to_string())), styled)
        .on_click(ranges, move |ix, window, cx| match targets.get(ix) {
            Some(Target::Url(url)) => cx.open_url(url),
            Some(Target::Path(path)) => {
                if let Some(open) = &open {
                    open(path.clone(), window, cx);
                }
            }
            None => {}
        })
        .into_any_element()
}

enum Target {
    Url(String),
    Path(String),
}

/// A checkbox: a square, filled when done, with a dot when active.
fn checkbox(done: bool, active: bool, f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    let side = (s.size * 0.8).round();
    let color = if done { t.tool } else { t.dim };
    let b = div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(2.))
        .border_1()
        .border_color(rgba(color));
    if done {
        b.bg(rgba(t.tool))
            .text_color(rgba(t.bg))
            .text_size(px(side * 0.8))
            .line_height(relative(1.))
            .child("✓")
    } else if active {
        b.child(div().size(px(side * 0.4)).bg(rgba(t.accent)))
    } else {
        b
    }
}

// ---- tool calls -------------------------------------------------------------------------

fn call(b: &Block, f: &Frame) -> Div {
    let s = f.style;
    let mut col = div().mt(px(8.)).flex().flex_col();
    let head = b.head();
    let results = b.results();
    let tool = head.and_then(|h| h.label.as_deref());
    // A todo list shows as checkboxes; the call row names its progress.
    let todos = if tool == Some("TodoWrite") {
        todo::parse_todos(head.map_or("", |h| h.text.as_str()), results)
    } else {
        Vec::new()
    };
    if let Some(h) = head {
        let arg = (!todos.is_empty()).then(|| todo::progress(&todos));
        col = col.child(tool_head(b, h, arg, f));
    }
    if !todos.is_empty() {
        return col.child(todo_list(&todos, f));
    }
    if results.is_empty() {
        return col;
    }
    let fold = blocks::fold(results, b.expanded);
    let (rows, _) = blocks::split_cut(results);
    let rows = &rows[..fold.shown.min(rows.len())];
    let is_diff = rows
        .iter()
        .any(|l| matches!(l.kind, LineKind::Added | LineKind::Removed));
    let first_gutter = head.is_some();
    if is_diff {
        let diff = blocks::diff_rows(tool, rows);
        let digits = diff
            .iter()
            .filter_map(|r| r.number)
            .max()
            .map_or(0, |n| n.to_string().len());
        for (i, row) in diff.iter().enumerate() {
            col = col.child(indented(
                f,
                i == 0 && first_gutter,
                diff_row(row, digits, f),
            ));
        }
    } else {
        for (i, row) in rows.iter().enumerate() {
            col = col.child(indented(f, i == 0 && first_gutter, result_row(row, f)));
        }
    }
    if let Some(label) = fold.label() {
        let t = s.theme;
        let clickable = fold.foldable && f.hooks.toggle.is_some();
        let toggle = f.hooks.toggle.clone();
        let abs = b.abs;
        let marker = s
            .base()
            .id(SharedString::from(format!("{}-fold{}", f.id, b.abs)))
            .pl(px(s.ch() * 2.))
            .text_color(rgba(t.dim))
            .when(clickable, |d| {
                d.cursor_pointer().hover(|st| st.text_color(rgba(t.text)))
            })
            .when_some(toggle.filter(|_| clickable), |d, toggle| {
                d.on_click(move |_, window, cx| toggle(abs, window, cx))
            })
            .child(label);
        col = col.child(marker);
    }
    col
}

fn tool_head(b: &Block, h: &Line, arg: Option<String>, f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    let name = h.label.as_deref().unwrap_or("Tool");
    let state = b.tool_state();
    let mut text = String::from(name);
    let mut runs = vec![(
        0..text.len(),
        HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
    )];
    let dim = HighlightStyle {
        color: Some(hsla(t.dim)),
        ..Default::default()
    };
    let mut path_range = None;
    let mut path = None;
    let shown = Line {
        text: arg.unwrap_or_else(|| h.text.clone()),
        ..h.clone()
    };
    let h = &shown;
    if !h.text.is_empty() {
        let at = text.len();
        text.push('(');
        runs.push((at..text.len(), dim));
        let arg_start = text.len();
        text.push_str(&h.text);
        if f.hooks.open_file.is_some()
            && let Some(p) = crate::lens::logic::find_path(&h.text)
            && let Some(at) = h.text.find(&p)
        {
            let range = arg_start + at..arg_start + at + p.len();
            runs.push((
                range.clone(),
                HighlightStyle {
                    underline: Some(underline(Some(hsla(t.bar)))),
                    ..Default::default()
                },
            ));
            path_range = Some(range);
            path = Some(p);
        }
        let at = text.len();
        text.push(')');
        runs.push((at..text.len(), dim));
    }
    let styled = StyledText::new(text).with_highlights(runs);
    let content: AnyElement = match (path_range, path) {
        (Some(range), Some(path)) => {
            let open = f.hooks.open_file.clone();
            InteractiveText::new(
                ElementId::Name(SharedString::from(format!("{}-p{}", f.id, b.abs))),
                styled,
            )
            .on_click(vec![range], move |_, window, cx| {
                if let Some(open) = &open {
                    open(path.clone(), window, cx);
                }
            })
            .into_any_element()
        }
        _ => styled.into_any_element(),
    };
    s.base()
        .flex()
        .child(
            div()
                .w(px(s.ch() * 2.))
                .h(px(s.lh()))
                .flex_none()
                .flex()
                .items_center()
                .child(state_dot(state, f)),
        )
        .child(div().flex_1().min_w(px(0.)).child(content))
}

/// The dot of a tool call: green when done, red on an error, orange when it waits for an
/// answer, and it pulses while the call runs.
fn state_dot(state: ToolState, f: &Frame) -> Div {
    let t = f.style.theme;
    let size = (f.style.size * 0.6).round().max(5.);
    let (color, opacity) = match state {
        ToolState::Done => (t.tool, 1.),
        ToolState::Failed => (t.del, 1.),
        ToolState::Waiting => (rgb_hex(tk().q), 1.),
        ToolState::Running => (t.dim, [1., 0.65, 0.3, 0.65][(f.tick % 4) as usize]),
    };
    div()
        .size(px(size))
        .rounded_full()
        .bg(rgba(color))
        .opacity(opacity)
}

/// A result row under the head: 2 characters in, and the first row has the `⎿` bend.
fn indented(f: &Frame, bend: bool, content: Div) -> Div {
    let s = f.style;
    let t = s.theme;
    let gutter = div()
        .w(px(s.ch() * 2.))
        .h(px(s.lh()))
        .flex_none()
        .relative()
        .when(bend, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(s.ch() * 0.5))
                    .top(px(s.lh() * 0.12))
                    .w(px(s.ch() * 0.8))
                    .h(px(s.lh() * 0.42))
                    .border_l_1()
                    .border_b_1()
                    .border_color(rgba(t.dim)),
            )
        });
    div()
        .flex()
        .child(gutter)
        .child(content.flex_1().min_w(px(0.)))
}

fn result_row(row: &Line, f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    let base = if row.kind == LineKind::Error {
        t.del
    } else {
        t.dim
    };
    if !row.text.contains('\u{1b}') {
        return s.base().text_color(rgba(base)).child(row.text.clone());
    }
    let (text, runs) = ansi::parse(&row.text);
    let dark = s.dark();
    let runs: Vec<(Range<usize>, HighlightStyle)> = runs
        .into_iter()
        .map(|(range, st)| {
            let color = match st.fg {
                Some(ansi::Color::Palette(i)) => ansi::palette(i, dark).map(rgb_hex),
                // A program color moves until the console can read it.
                Some(ansi::Color::Rgb(r, g, b)) => Some(rgb_hex(fit_contrast(
                    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
                    s.theme.bg >> 8,
                    4.5,
                ))),
                None => None,
            };
            (
                range,
                HighlightStyle {
                    color: color.map(hsla),
                    font_weight: st.bold.then_some(FontWeight::BOLD),
                    font_style: st.italic.then_some(FontStyle::Italic),
                    underline: st.underline.then(|| underline(None)),
                    // Dim text fades on a light console; on a dark one it would not read.
                    fade_out: (st.dim && !dark).then_some(0.4),
                    ..Default::default()
                },
            )
        })
        .collect();
    s.base()
        .text_color(rgba(base))
        .child(StyledText::new(text).with_highlights(runs))
}

fn diff_row(row: &DiffRow, digits: usize, f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    let (sign, color, bg) = match row.sign {
        DiffSign::Added => ("+", Some(t.add), Some(t.add_bg)),
        DiffSign::Removed => ("-", Some(t.del), Some(t.del_bg)),
        DiffSign::Context => (" ", None, None),
        DiffSign::Hunk => ("", None, None),
    };
    if row.sign == DiffSign::Hunk {
        return s
            .base()
            .mt(px(2.))
            .text_color(rgba(t.dim))
            .child(row.text.clone());
    }
    s.base()
        .flex()
        .when_some(bg, |d, bg| d.bg(rgba(bg)))
        .when(digits > 0, |d| {
            d.child(
                div()
                    .w(px(s.ch() * (digits as f32 + 1.)))
                    .flex_none()
                    .text_right()
                    .pr(px(s.ch() * 0.5))
                    .text_color(rgba(t.dim))
                    .child(row.number.map(|n| n.to_string()).unwrap_or_default()),
            )
        })
        .child(
            div()
                .w(px(s.ch() * 1.5))
                .flex_none()
                .pl(px(s.ch() * 0.3))
                .text_color(rgba(color.unwrap_or(t.dim)))
                .child(sign),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_color(rgba(color.unwrap_or(t.dim)))
                .child(row.text.clone()),
        )
}

fn todo_list(todos: &[todo::Todo], f: &Frame) -> Div {
    let s = f.style;
    let t = s.theme;
    let mut col = div().flex().flex_col();
    for (i, item) in todos.iter().enumerate() {
        let done = item.state == TodoState::Done;
        let active = item.state == TodoState::Active;
        let row = div()
            .flex()
            .gap(px(s.ch()))
            .items_start()
            .child(
                div()
                    .h(px(s.lh()))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(checkbox(done, active, f)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(rgba(if done { t.dim } else { t.text }))
                    .when(active, |d| d.font_weight(FontWeight::BOLD))
                    .when(done, |d| d.line_through())
                    .child(item.text.clone()),
            );
        col = col.child(indented(f, i == 0, row));
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_colors_differ_by_theme_and_skip_plain_tokens() {
        assert_ne!(
            syntax_color(TokenKind::Keyword, false),
            syntax_color(TokenKind::Keyword, true)
        );
        assert_eq!(syntax_color(TokenKind::Operator, false), None);
    }

    /// The colors that only the console draws: code, program output, the check mark.
    #[test]
    fn the_dark_console_colors_reach_the_contrast_floor() {
        use crate::theme::{composite, contrast};
        for theme in [PaneTheme::DARK, PaneTheme::NAVY] {
            let bg = theme.bg >> 8;
            let code = composite(theme.code_bg, bg);
            for kind in [
                TokenKind::Keyword,
                TokenKind::Type,
                TokenKind::Function,
                TokenKind::String,
                TokenKind::Number,
                TokenKind::Constant,
                TokenKind::Comment,
                TokenKind::Attribute,
                TokenKind::Macro,
            ] {
                let color = syntax_color(kind, true).expect("a colored token");
                for surface in [bg, code, composite(theme.user_bg, bg)] {
                    let ratio = contrast(color, surface);
                    assert!(ratio >= 4.5, "{kind:?} is {ratio:.2}:1");
                }
            }
            for index in 0..16 {
                let color = ansi::palette(index, true).expect("a palette color");
                let ratio = contrast(color, bg);
                assert!(ratio >= 4.5, "ANSI {index} is {ratio:.2}:1");
            }
            // The check mark of a done todo sits on the green box.
            assert!(contrast(theme.bg >> 8, theme.tool >> 8) >= 4.5);
            // A program color that is too dark moves until it reads.
            let moved = fit_contrast(0x101010, bg, 4.5);
            assert!(contrast(moved, bg) >= 4.5);
        }
    }

    #[test]
    fn the_style_knows_its_cell() {
        let s = Style::plain(12., PaneTheme::WHITE);
        assert!((s.ch() - 7.2).abs() < 0.001);
        assert!((s.lh() - 18.).abs() < 0.001);
        assert!(!s.dark());
        assert!(Style::plain(12., PaneTheme::DARK).dark());
        assert!(Style::plain(12., PaneTheme::NAVY).dark());
        assert!(!Style::plain(12., PaneTheme::GREY).dark());
    }
}
