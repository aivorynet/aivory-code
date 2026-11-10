//! Markdown of agent text turned into styled blocks (pulldown-cmark, MIT). The result has no
//! GPUI types: `draw.rs` turns it into elements. The parse of a text is cached, so a frame
//! costs no parse.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use aivory_syntax::{Language, TokenKind};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A text with inline styles. The segments are sorted and do not overlap; text between two
/// segments is plain.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inline {
    pub text: String,
    pub segs: Vec<Seg>,
    /// The targets of the links; `Seg::link` indexes this list.
    pub links: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seg {
    pub range: Range<usize>,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<usize>,
    /// Inline code that names a file: a click opens it in the Lens.
    pub path: Option<String>,
}

impl Seg {
    fn styled(&self) -> bool {
        self.bold
            || self.italic
            || self.code
            || self.strike
            || self.link.is_some()
            || self.path.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MdBlock {
    Heading {
        level: u8,
        text: Inline,
    },
    Para(Inline),
    Code {
        lang: Option<String>,
        text: String,
        spans: Vec<(Range<usize>, TokenKind)>,
    },
    Quote(Vec<MdBlock>),
    List {
        /// The number of the first item of a numbered list.
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
        /// The widest cell of each column in characters.
        widths: Vec<usize>,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// A task list item: `Some(done)`.
    pub task: Option<bool>,
    pub blocks: Vec<MdBlock>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Md {
    pub blocks: Vec<MdBlock>,
}

const CACHE_LIMIT: usize = 4096;

thread_local! {
    static CACHE: RefCell<HashMap<u64, Rc<Md>>> = RefCell::new(HashMap::new());
}

/// The parse of `text`, from the cache when the text was seen before.
pub fn cached(text: &str) -> Rc<Md> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let key = h.finish();
    if let Some(md) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return md;
    }
    let md = Rc::new(parse(text));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CACHE_LIMIT {
            c.clear();
        }
        c.insert(key, md.clone());
    });
    md
}

pub fn parse(text: &str) -> Md {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let text = end_tables(text);
    let events: Vec<Event> = Parser::new_ext(&text, options).collect();
    let mut p = Walker { events, pos: 0 };
    Md { blocks: p.blocks() }
}

/// GFM keeps a table open until a blank line, so a sentence right under a table becomes a
/// one-cell row (and widens the first column). Agents mean a new paragraph there: a line
/// without a `|` after a table row gets a blank line before it. Code fences stay as they are.
fn end_tables(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('|') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let (mut fence, mut in_table) = (false, false);
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            in_table = false;
        } else if !fence {
            if in_table && !t.is_empty() && !t.contains('|') {
                out.push('\n');
            }
            in_table = t.starts_with('|') || (in_table && t.contains('|'));
        }
        out.push_str(line);
    }
    std::borrow::Cow::Owned(out)
}

struct Walker<'a> {
    events: Vec<Event<'a>>,
    pos: usize,
}

impl<'a> Walker<'a> {
    fn peek(&self) -> Option<&Event<'a>> {
        self.events.get(self.pos)
    }

    fn take(&mut self) -> Option<Event<'a>> {
        let e = self.events.get(self.pos).cloned();
        self.pos += 1;
        e
    }

    /// The blocks up to the end of the container (the end event is consumed).
    fn blocks(&mut self) -> Vec<MdBlock> {
        let mut out = Vec::new();
        while let Some(event) = self.peek().cloned() {
            if is_inline(&event) {
                // A tight list item or a stray text has no paragraph around it.
                let text = self.inline();
                if !text.text.is_empty() {
                    out.push(MdBlock::Para(text));
                }
                continue;
            }
            self.pos += 1;
            match event {
                Event::End(_) => break,
                Event::Rule => out.push(MdBlock::Rule),
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        let text = self.inline();
                        self.skip_end();
                        out.push(MdBlock::Para(text));
                    }
                    Tag::Heading { level, .. } => {
                        let text = self.inline();
                        self.skip_end();
                        out.push(MdBlock::Heading {
                            level: heading_level(level),
                            text,
                        });
                    }
                    Tag::CodeBlock(kind) => out.push(self.code(kind)),
                    Tag::BlockQuote(_) => out.push(MdBlock::Quote(self.blocks())),
                    Tag::List(start) => out.push(self.list(start)),
                    Tag::Table(_) => out.push(self.table()),
                    // A container that the console does not draw: keep its content.
                    _ => out.extend(self.blocks()),
                },
                _ => {}
            }
        }
        out
    }

    fn skip_end(&mut self) {
        if matches!(self.peek(), Some(Event::End(_))) {
            self.pos += 1;
        }
    }

    fn code(&mut self, kind: CodeBlockKind) -> MdBlock {
        let info = match kind {
            CodeBlockKind::Fenced(info) => {
                let word = info.split_whitespace().next().unwrap_or("");
                (!word.is_empty()).then(|| word.to_string())
            }
            CodeBlockKind::Indented => None,
        };
        let mut text = String::new();
        while let Some(event) = self.take() {
            match event {
                Event::Text(t) => text.push_str(&t),
                Event::End(_) => break,
                _ => {}
            }
        }
        while text.ends_with('\n') {
            text.pop();
        }
        let spans = info
            .as_deref()
            .and_then(language)
            .and_then(|lang| aivory_syntax::highlight(lang, &text).ok())
            .map(|spans| {
                spans
                    .iter()
                    .map(|s| (s.start_byte as usize..s.end_byte as usize, s.kind))
                    .collect()
            })
            .unwrap_or_default();
        MdBlock::Code {
            lang: info,
            text,
            spans,
        }
    }

    fn list(&mut self, start: Option<u64>) -> MdBlock {
        let mut items = Vec::new();
        while let Some(event) = self.take() {
            match event {
                Event::Start(Tag::Item) => {
                    let mut task = None;
                    if let Some(Event::TaskListMarker(done)) = self.peek() {
                        task = Some(*done);
                        self.pos += 1;
                    }
                    items.push(Item {
                        task,
                        blocks: self.blocks(),
                    });
                }
                Event::End(_) => break,
                _ => {}
            }
        }
        MdBlock::List { start, items }
    }

    fn table(&mut self) -> MdBlock {
        let mut head = Vec::new();
        let mut rows: Vec<Vec<Inline>> = Vec::new();
        let mut in_head = false;
        while let Some(event) = self.take() {
            match event {
                Event::Start(Tag::TableHead) => in_head = true,
                Event::End(TagEnd::TableHead) => in_head = false,
                Event::Start(Tag::TableRow) => rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => {
                    let cell = self.inline();
                    self.skip_end();
                    if in_head {
                        head.push(cell);
                    } else if let Some(row) = rows.last_mut() {
                        row.push(cell);
                    }
                }
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        let cols = rows
            .iter()
            .map(Vec::len)
            .chain([head.len()])
            .max()
            .unwrap_or(0);
        let widths = (0..cols)
            .map(|c| {
                let cell_len = |i: Option<&Inline>| i.map_or(0, |i| i.text.chars().count());
                rows.iter()
                    .map(|r| cell_len(r.get(c)))
                    .chain([cell_len(head.get(c))])
                    .max()
                    .unwrap_or(0)
                    .max(3)
            })
            .collect();
        MdBlock::Table { head, rows, widths }
    }

    /// The inline events from here on, up to the first block event (not consumed).
    fn inline(&mut self) -> Inline {
        let mut out = Inline::default();
        let (mut bold, mut italic, mut strike) = (0u32, 0u32, 0u32);
        let mut link: Vec<usize> = Vec::new();
        let push =
            |out: &mut Inline, s: &str, code: bool, flags: (u32, u32, u32, Option<usize>)| {
                if s.is_empty() {
                    return;
                }
                let start = out.text.len();
                out.text.push_str(s);
                let path = code.then(|| path_of(s)).flatten();
                let seg = Seg {
                    range: start..out.text.len(),
                    bold: flags.0 > 0,
                    italic: flags.1 > 0,
                    code,
                    strike: flags.2 > 0,
                    link: flags.3,
                    path,
                };
                if !seg.styled() {
                    return;
                }
                match out.segs.last_mut() {
                    Some(prev) if prev.range.end == start && same_style(prev, &seg) => {
                        prev.range.end = seg.range.end;
                    }
                    _ => out.segs.push(seg),
                }
            };
        while let Some(event) = self.peek().cloned() {
            match event {
                Event::Text(t) => push(
                    &mut out,
                    &t,
                    false,
                    (bold, italic, strike, link.last().copied()),
                ),
                Event::Code(t) => push(
                    &mut out,
                    &t,
                    true,
                    (bold, italic, strike, link.last().copied()),
                ),
                Event::InlineMath(t) | Event::DisplayMath(t) => {
                    push(&mut out, &t, true, (bold, italic, strike, None))
                }
                Event::Html(t) | Event::InlineHtml(t) => {
                    push(&mut out, &t, false, (bold, italic, strike, None))
                }
                Event::SoftBreak | Event::HardBreak => {
                    push(&mut out, "\n", false, (bold, italic, strike, None))
                }
                Event::FootnoteReference(t) => push(
                    &mut out,
                    &format!("[{t}]"),
                    false,
                    (bold, italic, strike, None),
                ),
                Event::Start(Tag::Strong) => bold += 1,
                Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
                Event::Start(Tag::Emphasis) => italic += 1,
                Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
                Event::Start(Tag::Strikethrough) => strike += 1,
                Event::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
                Event::Start(Tag::Link { dest_url, .. }) => {
                    out.links.push(dest_url.to_string());
                    link.push(out.links.len() - 1);
                }
                Event::End(TagEnd::Link) => {
                    link.pop();
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    out.links.push(dest_url.to_string());
                    link.push(out.links.len() - 1);
                }
                Event::End(TagEnd::Image) => {
                    link.pop();
                }
                _ => break,
            }
            self.pos += 1;
        }
        out
    }
}

fn is_inline(event: &Event) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::Html(_)
        | Event::InlineHtml(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::FootnoteReference(_) => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
        ),
        Event::End(end) => matches!(
            end,
            TagEnd::Strong
                | TagEnd::Emphasis
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image
        ),
        _ => false,
    }
}

fn same_style(a: &Seg, b: &Seg) -> bool {
    a.bold == b.bold
        && a.italic == b.italic
        && a.code == b.code
        && a.strike == b.strike
        && a.link == b.link
        && a.path.is_none()
        && b.path.is_none()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The path that a whole inline code names, if it names one.
fn path_of(code: &str) -> Option<String> {
    let code = code.trim();
    crate::lens::logic::find_path(code).filter(|p| p == code)
}

/// The language of a fence info word.
pub fn language(info: &str) -> Option<Language> {
    let word = info.to_ascii_lowercase();
    if let Some(lang) = Language::from_extension(&word) {
        return Some(lang);
    }
    if let Some(lang) = Language::ALL.iter().copied().find(|l| l.name() == word) {
        return Some(lang);
    }
    match word.as_str() {
        "shell" | "console" | "terminal" | "zsh" | "powershell" => Some(Language::Bash),
        "golang" => Some(Language::Go),
        "c#" => Some(Language::CSharp),
        "rs" => Some(Language::Rust),
        "py" | "python3" => Some(Language::Python),
        "node" => Some(Language::JavaScript),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sentence_under_a_table_is_a_paragraph() {
        let md = super::parse("| a | b |\n|---|---|\n| 1 | 2 |\nSee the notes.");
        let rows = md.blocks.iter().find_map(|b| match b {
            super::MdBlock::Table { rows, .. } => Some(rows.len()),
            _ => None,
        });
        assert_eq!(rows, Some(1));
        assert!(
            md.blocks
                .iter()
                .any(|b| matches!(b, super::MdBlock::Para(t) if t.text == "See the notes."))
        );
    }

    use super::*;

    fn para(md: &Md, i: usize) -> &Inline {
        match &md.blocks[i] {
            MdBlock::Para(t) => t,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn inline_styles_become_segments() {
        let md = parse("a **bold** and *italic* and `code` and ~~gone~~ z");
        let p = para(&md, 0);
        assert_eq!(p.text, "a bold and italic and code and gone z");
        let flags: Vec<_> = p
            .segs
            .iter()
            .map(|s| (&p.text[s.range.clone()], s.bold, s.italic, s.code, s.strike))
            .collect();
        assert_eq!(
            flags,
            [
                ("bold", true, false, false, false),
                ("italic", false, true, false, false),
                ("code", false, false, true, false),
                ("gone", false, false, false, true),
            ]
        );
    }

    #[test]
    fn nested_styles_combine() {
        let md = parse("***both***");
        let p = para(&md, 0);
        assert_eq!(p.segs.len(), 1);
        assert!(p.segs[0].bold && p.segs[0].italic);
    }

    #[test]
    fn headings_have_levels() {
        let md = parse("# One\n\n### Three\n");
        assert!(matches!(&md.blocks[0], MdBlock::Heading { level: 1, text } if text.text == "One"));
        assert!(matches!(&md.blocks[1], MdBlock::Heading { level: 3, .. }));
    }

    #[test]
    fn a_link_keeps_its_target() {
        let md = parse("see [the docs](https://example.com/x) now");
        let p = para(&md, 0);
        assert_eq!(p.text, "see the docs now");
        let seg = &p.segs[0];
        assert_eq!(&p.text[seg.range.clone()], "the docs");
        assert_eq!(p.links[seg.link.unwrap()], "https://example.com/x");
    }

    #[test]
    fn inline_code_with_a_path_is_clickable() {
        let md = parse("edit `src/payment/retry.rs` and `cargo test`");
        let p = para(&md, 0);
        assert_eq!(p.segs[0].path.as_deref(), Some("src/payment/retry.rs"));
        assert_eq!(p.segs[1].path, None);
    }

    #[test]
    fn a_soft_break_stays_a_line_break() {
        let md = parse("first row\nsecond row");
        assert_eq!(para(&md, 0).text, "first row\nsecond row");
    }

    #[test]
    fn a_fenced_block_gets_syntax_spans() {
        let md = parse("text\n```rust\nfn main() { let x = 1; }\n```\n");
        let MdBlock::Code { lang, text, spans } = &md.blocks[1] else {
            panic!("no code block");
        };
        assert_eq!(lang.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() { let x = 1; }");
        assert!(
            spans
                .iter()
                .any(|(r, k)| *k == TokenKind::Keyword && &text[r.clone()] == "fn")
        );
    }

    #[test]
    fn an_unknown_language_has_no_spans() {
        let md = parse("```zzz\nfoo\n```");
        let MdBlock::Code { spans, .. } = &md.blocks[0] else {
            panic!()
        };
        assert!(spans.is_empty());
        assert_eq!(language("ts"), Some(Language::TypeScript));
        assert_eq!(language("shell"), Some(Language::Bash));
        assert_eq!(language("nope"), None);
    }

    #[test]
    fn lists_nest_and_number() {
        let md = parse("1. one\n2. two\n   - inner a\n   - inner b\n3. three\n");
        let MdBlock::List { start, items } = &md.blocks[0] else {
            panic!("no list");
        };
        assert_eq!(*start, Some(1));
        assert_eq!(items.len(), 3);
        let nested = items[1]
            .blocks
            .iter()
            .find_map(|b| match b {
                MdBlock::List { items, start } => Some((items.len(), *start)),
                _ => None,
            })
            .unwrap();
        assert_eq!(nested, (2, None));
    }

    #[test]
    fn a_task_list_has_checks() {
        let md = parse("- [x] done\n- [ ] open\n");
        let MdBlock::List { items, .. } = &md.blocks[0] else {
            panic!()
        };
        assert_eq!([items[0].task, items[1].task], [Some(true), Some(false)]);
    }

    #[test]
    fn quotes_and_rules() {
        let md = parse("> quoted *text*\n\n---\n");
        assert!(matches!(&md.blocks[0], MdBlock::Quote(inner) if inner.len() == 1));
        assert_eq!(md.blocks[1], MdBlock::Rule);
    }

    #[test]
    fn a_table_has_cells_and_widths() {
        let md = parse("| Name | Count |\n|---|---|\n| alpha | 1 |\n| b | 12345 |\n");
        let MdBlock::Table { head, rows, widths } = &md.blocks[0] else {
            panic!("no table");
        };
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1].text, "12345");
        assert_eq!(widths, &[5, 5]);
    }

    #[test]
    fn the_cache_returns_the_same_parse() {
        let a = cached("some **text** here");
        let b = cached("some **text** here");
        assert!(Rc::ptr_eq(&a, &b));
        assert!(!Rc::ptr_eq(&a, &cached("other text")));
    }

    #[test]
    fn plain_text_is_one_paragraph_without_segments() {
        let md = parse("The retry table needs a new column.");
        assert_eq!(md.blocks.len(), 1);
        assert!(para(&md, 0).segs.is_empty());
    }
}
