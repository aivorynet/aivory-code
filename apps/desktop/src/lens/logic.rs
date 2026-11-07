//! The pure parts of the Lens: viewport math, the blame scale, the token palette, line segments
//! with underlines, claim heat, the line diff, the timeline, the outline, and small texts.

use std::collections::HashSet;
use std::ops::Range;

use aivory_protocol::code::{
    BlameHunk, BlameTurn, CommitRef, Finding, FindingLevel, Highlight, LineRange, OutlineNode,
    Place, SymbolKind, TimelineEvent, TokenKind,
};
use aivory_protocol::coord::{Claim, ClaimState, MailItem, PingKind, RadarHit};
use aivory_protocol::{FileNode, UnixMs};

pub const LINE_H: f32 = 22.;
/// Lines to highlight before and after the visible lines.
pub const MARGIN: u32 = 80;

// ---- Viewport ----

/// The lines to ask `code/file` for when `first..end` (0-based, `end` excluded) are visible:
/// the visible lines plus a margin on both sides, cut to the file. 1-based, both ends included.
pub fn request_range(first: usize, end: usize, total: u32, margin: u32) -> LineRange {
    let total = total.max(1);
    let from = (first as u32 + 1).saturating_sub(margin).max(1);
    let to = (end as u32)
        .saturating_add(margin)
        .clamp(from, total.max(from));
    LineRange { from, to }
}

/// True when every line of `want` lies in one of the ranges.
pub fn covers(loaded: &[LineRange], want: LineRange) -> bool {
    let mut at = want.from;
    let mut sorted: Vec<LineRange> = loaded.to_vec();
    sorted.sort_by_key(|r| r.from);
    for r in sorted {
        if r.from > at {
            break;
        }
        at = at.max(r.to.saturating_add(1));
        if at > want.to {
            return true;
        }
    }
    at > want.to
}

/// Add a range and merge the ranges that touch.
pub fn add_range(loaded: &mut Vec<LineRange>, range: LineRange) {
    loaded.push(range);
    loaded.sort_by_key(|r| r.from);
    let mut merged: Vec<LineRange> = Vec::with_capacity(loaded.len());
    for r in loaded.drain(..) {
        match merged.last_mut() {
            Some(last) if r.from <= last.to.saturating_add(1) => last.to = last.to.max(r.to),
            _ => merged.push(r),
        }
    }
    *loaded = merged;
}

/// The range to request for the visible items `first..end`, or `None` when the highlights of
/// the visible lines are loaded.
pub fn fetch_for(
    loaded: &[LineRange],
    first: usize,
    end: usize,
    total: u32,
    margin: u32,
) -> Option<LineRange> {
    if end <= first || total == 0 {
        return None;
    }
    let visible = LineRange {
        from: first as u32 + 1,
        to: (end as u32).min(total),
    };
    if visible.to < visible.from || covers(loaded, visible) {
        return None;
    }
    Some(request_range(first, end, total, margin))
}

/// Does a card of height `card_h` fit under the line? Otherwise it opens above the line, when
/// it fits there.
pub fn card_opens_up(line_top: f32, view_h: f32, card_h: f32) -> bool {
    let below = line_top + LINE_H + 2. + card_h;
    below > view_h && line_top - 2. - card_h >= 0.
}

/// The new selection after an arrow key.
pub fn step(sel: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (sel as i64 + i64::from(delta)).clamp(0, len as i64 - 1) as usize
}

// ---- Blame ----

/// The turn blame scale: rank 0 is the newest turn of the file (README section 8).
pub const BLAME_COLORS: [u32; 5] = [0x3574F0, 0x6B8AFF, 0xA9BCFA, 0xCBD5F5, 0xE3E7EF];

/// The step of the scale: rank 4 and older, commits, and unknown lines share the last step.
pub fn blame_bucket(rank: Option<usize>) -> usize {
    rank.map_or(BLAME_COLORS.len() - 1, |r| r.min(BLAME_COLORS.len() - 1))
}

pub fn blame_color(rank: Option<usize>) -> u32 {
    BLAME_COLORS[blame_bucket(rank)]
}

/// What wrote a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlameKind<'a> {
    Turn(&'a BlameTurn),
    Commit(&'a CommitRef),
    Unknown,
}

/// The hunks of `ledger/blame` with a line index and the age rank of each turn.
#[derive(Debug, Clone, Default)]
pub struct BlameMap {
    hunks: Vec<BlameHunk>,
    /// `(session, n)` newest first.
    ranks: Vec<(String, u32)>,
    /// The hunk of each line (index 0 is line 1).
    by_line: Vec<Option<u32>>,
}

impl BlameMap {
    pub fn new(hunks: Vec<BlameHunk>, total: u32) -> Self {
        let mut turns: Vec<(&BlameTurn, UnixMs)> = hunks
            .iter()
            .filter_map(|h| h.turn.as_ref().map(|t| (t, t.time)))
            .collect();
        turns.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.n.cmp(&a.0.n)));
        let mut ranks: Vec<(String, u32)> = Vec::new();
        for (t, _) in turns {
            let key = (t.session.clone(), t.n);
            if !ranks.contains(&key) {
                ranks.push(key);
            }
        }
        let mut by_line = vec![None; total as usize];
        for (i, h) in hunks.iter().enumerate() {
            for line in h.line..h.line.saturating_add(h.count) {
                if let Some(slot) = by_line.get_mut(line.saturating_sub(1) as usize) {
                    *slot = Some(i as u32);
                }
            }
        }
        Self {
            hunks,
            ranks,
            by_line,
        }
    }

    pub fn hunk_at(&self, line: u32) -> Option<&BlameHunk> {
        let i = (*self.by_line.get(line.checked_sub(1)? as usize)?)?;
        self.hunks.get(i as usize)
    }

    pub fn kind_at(&self, line: u32) -> BlameKind<'_> {
        match self.hunk_at(line) {
            Some(h) => match (&h.turn, &h.commit) {
                (Some(t), _) => BlameKind::Turn(t),
                (None, Some(c)) => BlameKind::Commit(c),
                _ => BlameKind::Unknown,
            },
            None => BlameKind::Unknown,
        }
    }

    pub fn rank_of(&self, turn: &BlameTurn) -> usize {
        self.ranks
            .iter()
            .position(|(s, n)| *s == turn.session && *n == turn.n)
            .unwrap_or(BLAME_COLORS.len() - 1)
    }

    pub fn color_at(&self, line: u32) -> u32 {
        match self.kind_at(line) {
            BlameKind::Turn(t) => blame_color(Some(self.rank_of(t))),
            _ => blame_color(None),
        }
    }
}

// ---- Tokens, marks, segments ----

/// The color of a token kind (README section 8, "New tokens"). `None` is the ink of the code.
pub fn token_color(kind: TokenKind) -> Option<u32> {
    match kind {
        TokenKind::Keyword => Some(0x4B5BDB),
        TokenKind::Type => Some(0x0D9488),
        TokenKind::Function => Some(0x6D28D9),
        TokenKind::String => Some(0x15803D),
        TokenKind::Number => Some(0xC2410C),
        TokenKind::Comment => Some(0x8A93A6),
        TokenKind::Attribute | TokenKind::Macro => Some(0x5A6578),
        TokenKind::Variable
        | TokenKind::Operator
        | TokenKind::Punctuation
        | TokenKind::Constant => None,
    }
}

/// The severity of a diagnostic mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    Hint,
    Warning,
    Error,
}

impl Sev {
    pub fn of(level: FindingLevel) -> Option<Self> {
        match level {
            FindingLevel::Error => Some(Self::Error),
            FindingLevel::Warning => Some(Self::Warning),
            FindingLevel::Note => Some(Self::Hint),
            FindingLevel::None => None,
        }
    }

    /// The dot and underline color.
    pub fn color(self) -> u32 {
        match self {
            Self::Error => 0xDC2626,
            Self::Warning => 0xF59E0B,
            Self::Hint => 0x8A96A8,
        }
    }
}

/// An underlined range of a line, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub sev: Sev,
}

/// A styled piece of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub range: Range<usize>,
    pub color: Option<u32>,
    pub under: Option<Sev>,
    pub bg: bool,
}

fn floor_boundary(text: &str, mut x: usize) -> usize {
    x = x.min(text.len());
    while x > 0 && !text.is_char_boundary(x) {
        x -= 1;
    }
    x
}

/// Cut a line at the borders of the tokens, the marks and the symbol highlight.
pub fn line_segments(
    text: &str,
    tokens: &[Highlight],
    marks: &[Mark],
    bg: Option<Range<usize>>,
) -> Vec<Seg> {
    let cut = |x: u32| floor_boundary(text, x as usize);
    let mut cuts = vec![0, text.len()];
    for t in tokens {
        cuts.push(cut(t.start));
        cuts.push(cut(t.end));
    }
    for m in marks {
        cuts.push(floor_boundary(text, m.start));
        cuts.push(floor_boundary(text, m.end));
    }
    if let Some(b) = &bg {
        cuts.push(floor_boundary(text, b.start));
        cuts.push(floor_boundary(text, b.end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<Seg> = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let color = tokens
            .iter()
            .find(|t| cut(t.start) <= a && b <= cut(t.end))
            .and_then(|t| token_color(t.kind));
        let under = marks
            .iter()
            .filter(|m| floor_boundary(text, m.start) <= a && b <= floor_boundary(text, m.end))
            .map(|m| m.sev)
            .max();
        let has_bg = bg.as_ref().is_some_and(|r| {
            floor_boundary(text, r.start) <= a && b <= floor_boundary(text, r.end)
        });
        if color.is_none() && under.is_none() && !has_bg {
            continue;
        }
        match out.last_mut() {
            Some(last)
                if last.range.end == a
                    && last.color == color
                    && last.under == under
                    && last.bg == has_bg =>
            {
                last.range.end = b;
            }
            _ => out.push(Seg {
                range: a..b,
                color,
                under,
                bg: has_bg,
            }),
        }
    }
    out
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The identifier that contains byte `ix`, or `None` for a space, a digit start, and so on.
pub fn word_at(text: &str, ix: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    if ix >= bytes.len() || !is_ident(bytes[ix]) {
        return None;
    }
    let mut start = ix;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = ix;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }
    (!bytes[start].is_ascii_digit()).then_some(start..end)
}

/// A hover on a keyword, a comment, a string or a number gives no doc card.
pub fn is_symbol_at(tokens: &[Highlight], ix: usize) -> bool {
    !tokens
        .iter()
        .find(|t| t.start as usize <= ix && ix < t.end as usize)
        .is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::Keyword
                    | TokenKind::Comment
                    | TokenKind::String
                    | TokenKind::Number
                    | TokenKind::Operator
                    | TokenKind::Punctuation
            )
        })
}

/// The underlines of the findings on one line (1-based).
pub fn marks_for_line(findings: &[Finding], line: u32, text: &str) -> Vec<Mark> {
    findings
        .iter()
        .filter_map(|f| {
            let sev = Sev::of(f.level)?;
            let last = f.end_line.unwrap_or(f.line);
            if line < f.line || line > last {
                return None;
            }
            let start = if line == f.line { f.col as usize } else { 0 };
            let mut end = if line == last {
                f.end_col.map_or(start, |c| c as usize)
            } else {
                text.len()
            };
            if end <= start {
                end = word_at(text, start).map_or(start + 1, |w| w.end);
            }
            Some(Mark {
                start: start.min(text.len()),
                end: end.min(text.len()),
                sev,
            })
        })
        .filter(|m| m.end > m.start)
        .collect()
}

/// The dot of the gutter: the worst severity on the line.
pub fn gutter_sev(findings: &[Finding], line: u32) -> Option<Sev> {
    findings
        .iter()
        .filter(|f| f.line <= line && line <= f.end_line.unwrap_or(f.line))
        .filter_map(|f| Sev::of(f.level))
        .max()
}

// ---- Heat ----

/// The claim of another session on a range of this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heat {
    pub from: u32,
    pub to: u32,
    pub session: String,
    pub state: ClaimState,
    pub lease_s: u32,
}

/// The claims with a range on `path`, without the claims of `own` and the landed ones.
pub fn heat_ranges(claims: &[Claim], path: &str, own: Option<&str>) -> Vec<Heat> {
    claims
        .iter()
        .filter(|c| c.path.as_deref() == Some(path))
        .filter(|c| c.state != ClaimState::Landed && Some(c.session.as_str()) != own)
        .filter_map(|c| {
            let r = c.range?;
            Some(Heat {
                from: r.from,
                to: r.to,
                session: c.session.clone(),
                state: c.state,
                lease_s: c.lease_s,
            })
        })
        .collect()
}

/// The heat of a line: an editing claim wins over an intent claim.
pub fn heat_at(heat: &[Heat], line: u32) -> Option<&Heat> {
    let on = |h: &&Heat| h.from <= line && line <= h.to;
    heat.iter()
        .filter(on)
        .find(|h| h.state == ClaimState::Editing)
        .or_else(|| heat.iter().find(on))
}

pub fn state_word(state: ClaimState) -> &'static str {
    match state {
        ClaimState::Intent => "intent",
        ClaimState::Editing => "editing",
        ClaimState::Landed => "landed",
    }
}

/// `8a22 editing`.
pub fn heat_tag(h: &Heat) -> String {
    format!("{} {}", h.session, state_word(h.state))
}

/// `8a22 · editing · lease 7 min`.
pub fn heat_hover(h: &Heat) -> String {
    format!(
        "{} · {} · {}",
        h.session,
        state_word(h.state),
        lease_text(h.lease_s)
    )
}

/// `lease 7 min`, `lease 30 s`.
pub fn lease_text(lease_s: u32) -> String {
    if lease_s < 60 {
        format!("lease {lease_s} s")
    } else {
        format!("lease {} min", lease_s.div_ceil(60))
    }
}

/// `L15–24`, `L8`.
pub fn lines_text(from: u32, to: u32) -> String {
    if from == to {
        format!("L{from}")
    } else {
        format!("L{from}–{to}")
    }
}

// ---- Outline ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineRow {
    pub depth: u8,
    pub name: String,
    pub kind: SymbolKind,
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

pub fn flatten_outline(nodes: &[OutlineNode]) -> Vec<OutlineRow> {
    fn walk(nodes: &[OutlineNode], depth: u8, out: &mut Vec<OutlineRow>) {
        for n in nodes {
            out.push(OutlineRow {
                depth,
                name: n.name.clone(),
                kind: n.kind,
                line: n.line,
                start: n.start_line,
                end: n.end_line.max(n.start_line),
            });
            walk(&n.children, depth.saturating_add(1), out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

/// The glyph of the outline rows.
pub fn kind_glyph(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Module | SymbolKind::Namespace => "M",
        SymbolKind::Class | SymbolKind::Struct => "S",
        SymbolKind::Enum => "E",
        SymbolKind::Interface | SymbolKind::Trait => "T",
        SymbolKind::Impl => "I",
        SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor => "fn",
        SymbolKind::Variant | SymbolKind::Field | SymbolKind::Property => "·",
        SymbolKind::Variable => "v",
        SymbolKind::Constant => "C",
        SymbolKind::TypeAlias => "t",
        SymbolKind::Macro => "m!",
        SymbolKind::Section => "§",
    }
}

/// The rows from the root to the innermost declaration that contains the line.
pub fn chain_at(rows: &[OutlineRow], line: u32) -> Vec<&OutlineRow> {
    let mut chain: Vec<&OutlineRow> = Vec::new();
    for r in rows {
        // Rows come in tree order. A row below a depth that no row matched, or beside a
        // match, cannot contain the line.
        if r.depth as usize == chain.len() && r.start <= line && line <= r.end {
            chain.push(r);
        }
    }
    chain
}

/// `Invoice › total`: the names from the root to the declaration at the cursor.
pub fn symbol_path(rows: &[OutlineRow], line: u32) -> String {
    chain_at(rows, line)
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>()
        .join(" › ")
}

/// The function at the cursor, or the first function of the file: the symbol of the
/// "Callers" and "Tests" sections.
pub fn focus_symbol(rows: &[OutlineRow], line: Option<u32>) -> Option<&OutlineRow> {
    let is_fn = |r: &&OutlineRow| {
        matches!(
            r.kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
        )
    };
    line.and_then(|l| chain_at(rows, l).into_iter().rev().find(is_fn))
        .or_else(|| rows.iter().find(is_fn))
}

/// The name of an outline row without its signature: `total(&self, on)` -> `total`.
pub fn bare_name(name: &str) -> &str {
    let head = name.split(['(', '<']).next().unwrap_or(name).trim();
    head.rsplit(' ').next().unwrap_or(head)
}

// ---- Diff ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineDiff {
    pub rows: Vec<DiffRow>,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Copy)]
enum Op {
    Eq(usize, usize),
    Del(usize),
    Ins(usize),
}

/// The largest table of the line diff; a bigger change shows as one replaced block.
const LCS_LIMIT: usize = 4_000_000;

fn edit_ops(old: &[String], new: &[String]) -> Vec<Op> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (n, m) = (old.len() - prefix - suffix, new.len() - prefix - suffix);
    let mut ops: Vec<Op> = (0..prefix).map(|i| Op::Eq(i, i)).collect();
    if n * m <= LCS_LIMIT {
        let a = &old[prefix..prefix + n];
        let b = &new[prefix..prefix + m];
        let w = m + 1;
        let mut dp = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * w + j] = if a[i] == b[j] {
                    dp[(i + 1) * w + j + 1] + 1
                } else {
                    dp[(i + 1) * w + j].max(dp[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && a[i] == b[j] {
                ops.push(Op::Eq(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if j >= m || (i < n && dp[(i + 1) * w + j] >= dp[i * w + j + 1]) {
                ops.push(Op::Del(prefix + i));
                i += 1;
            } else {
                ops.push(Op::Ins(prefix + j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..n).map(|i| Op::Del(prefix + i)));
        ops.extend((0..m).map(|j| Op::Ins(prefix + j)));
    }
    ops.extend((0..suffix).map(|k| Op::Eq(old.len() - suffix + k, new.len() - suffix + k)));
    ops
}

/// The unified diff of two texts with `context` unchanged lines around each change.
pub fn diff_lines(old: &[String], new: &[String], context: usize) -> LineDiff {
    let ops = edit_ops(old, new);
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| !matches!(o, Op::Eq(..)))
        .map(|(i, _)| i)
        .collect();
    let mut out = LineDiff::default();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let (s, e) = (i.saturating_sub(context), (i + context + 1).min(ops.len()));
        match ranges.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => ranges.push((s, e)),
        }
    }
    for (s, e) in ranges {
        let mut rows: Vec<DiffRow> = Vec::new();
        for op in &ops[s..e] {
            match *op {
                Op::Eq(i, j) => rows.push(DiffRow {
                    kind: DiffKind::Context,
                    old: Some(i as u32 + 1),
                    new: Some(j as u32 + 1),
                    text: old[i].clone(),
                }),
                Op::Del(i) => {
                    out.removed += 1;
                    rows.push(DiffRow {
                        kind: DiffKind::Removed,
                        old: Some(i as u32 + 1),
                        new: None,
                        text: old[i].clone(),
                    });
                }
                Op::Ins(j) => {
                    out.added += 1;
                    rows.push(DiffRow {
                        kind: DiffKind::Added,
                        old: None,
                        new: Some(j as u32 + 1),
                        text: new[j].clone(),
                    });
                }
            }
        }
        let old_n = rows.iter().filter(|r| r.old.is_some()).count();
        let new_n = rows.iter().filter(|r| r.new.is_some()).count();
        let old_at = rows.iter().find_map(|r| r.old).unwrap_or(0);
        let new_at = rows.iter().find_map(|r| r.new).unwrap_or(0);
        out.rows.push(DiffRow {
            kind: DiffKind::Hunk,
            old: None,
            new: None,
            text: format!("@@ -{old_at},{old_n} +{new_at},{new_n} @@"),
        });
        out.rows.extend(rows);
    }
    out
}

/// The lines that a turn wrote and that the file still has, as added rows with `context` lines
/// around each run. This stands in for the diff of a turn when the daemon has no snapshot.
pub fn turn_lines(
    lines: &[String],
    blame: &BlameMap,
    session: &str,
    n: u32,
    context: usize,
) -> LineDiff {
    let mine: Vec<usize> = (0..lines.len())
        .filter(|i| {
            matches!(blame.kind_at(*i as u32 + 1), BlameKind::Turn(t) if t.session == session && t.n == n)
        })
        .collect();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &i in &mine {
        let (s, e) = (
            i.saturating_sub(context),
            (i + context + 1).min(lines.len()),
        );
        match ranges.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => ranges.push((s, e)),
        }
    }
    let mut out = LineDiff::default();
    for (s, e) in ranges {
        out.rows.push(DiffRow {
            kind: DiffKind::Hunk,
            old: None,
            new: None,
            text: format!("@@ lines {}–{} @@", s + 1, e),
        });
        for (i, text) in lines.iter().enumerate().take(e).skip(s) {
            let added = mine.binary_search(&i).is_ok();
            if added {
                out.added += 1;
            }
            out.rows.push(DiffRow {
                kind: if added {
                    DiffKind::Added
                } else {
                    DiffKind::Context
                },
                old: None,
                new: Some(i as u32 + 1),
                text: text.clone(),
            });
        }
    }
    out
}

/// The text of a file as lines (no line ends).
pub fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

// ---- Timeline ----

#[derive(Debug, Clone, PartialEq)]
pub enum PointKind {
    Turn { session: String, n: u32 },
    Commit { id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub kind: PointKind,
    /// `8a22 · turn 5`, or the short commit id.
    pub who: String,
    pub time: UnixMs,
    pub text: String,
    pub detail: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    /// The blame color of a turn.
    pub color: Option<u32>,
    pub model: String,
    pub cost: Option<f64>,
}

/// The points of the timeline, newest first. A turn gets the blame color of its age rank among
/// the turns of the file.
pub fn timeline_points(events: &[TimelineEvent]) -> Vec<Point> {
    let mut turns: Vec<(&str, u32, UnixMs)> = events
        .iter()
        .filter_map(|e| match e {
            TimelineEvent::Turn {
                session, n, time, ..
            } => Some((session.as_str(), *n, *time)),
            TimelineEvent::Commit { .. } => None,
        })
        .collect();
    turns.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    let mut points: Vec<Point> = events
        .iter()
        .map(|e| match e {
            TimelineEvent::Turn {
                session,
                n,
                prompt,
                model,
                time,
                cost,
                added,
                removed,
            } => {
                let rank = turns.iter().position(|t| t.0 == session && t.1 == *n);
                let mut detail = model.clone();
                if let Some(c) = cost {
                    if !detail.is_empty() {
                        detail.push_str(" · ");
                    }
                    detail.push_str(&cost_text(*c));
                }
                Point {
                    kind: PointKind::Turn {
                        session: session.clone(),
                        n: *n,
                    },
                    who: format!("{session} · turn {n}"),
                    time: *time,
                    text: prompt.clone(),
                    detail,
                    added: Some(*added),
                    removed: Some(*removed),
                    color: Some(blame_color(rank)),
                    model: model.clone(),
                    cost: *cost,
                }
            }
            TimelineEvent::Commit {
                id,
                summary,
                author,
                time,
                ..
            } => Point {
                kind: PointKind::Commit { id: id.clone() },
                who: id.chars().take(7).collect(),
                time: *time,
                text: summary.clone(),
                detail: if author.is_empty() {
                    "committed".to_string()
                } else {
                    format!("{author} · committed")
                },
                added: None,
                removed: None,
                color: None,
                model: String::new(),
                cost: None,
            },
        })
        .collect();
    points.sort_by_key(|p| std::cmp::Reverse(p.time));
    points
}

/// The `code/file` revisions to compare for a point: `(old, new)`.
pub fn revs_of(kind: &PointKind) -> (String, String) {
    match kind {
        PointKind::Turn { n, .. } => (format!("turn:{}", n.saturating_sub(1)), format!("turn:{n}")),
        PointKind::Commit { id } => (format!("{id}^"), id.clone()),
    }
}

// ---- Texts ----

/// `$0.31`.
pub fn cost_text(cost: f64) -> String {
    format!("${cost:.2}")
}

/// `Rust`, `TypeScript`.
pub fn lang_label(language: &str) -> String {
    match language {
        "" => "Text".to_string(),
        "rust" => "Rust".to_string(),
        "typescript" => "TypeScript".to_string(),
        "javascript" => "JavaScript".to_string(),
        "python" => "Python".to_string(),
        "go" => "Go".to_string(),
        "java" => "Java".to_string(),
        "kotlin" => "Kotlin".to_string(),
        "csharp" => "C#".to_string(),
        "cpp" => "C++".to_string(),
        "c" => "C".to_string(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or(String::new(), |c| {
                c.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

/// The file name of a path.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// `invoice.rs:17`.
pub fn place_text(path: &str, line: u32) -> String {
    format!("{}:{line}", file_name(path))
}

/// The pieces of a hover card from the markdown of `code/hover`: no code block, no ticks.
pub fn doc_text(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(&line.replace('`', ""));
    }
    let text = out.trim().to_string();
    if text.chars().count() > 300 {
        let cut: String = text.chars().take(299).collect();
        format!("{cut}…")
    } else {
        text
    }
}

const SIGNATURE_KEYWORDS: [&str; 24] = [
    "pub",
    "fn",
    "let",
    "mut",
    "self",
    "impl",
    "struct",
    "enum",
    "trait",
    "async",
    "const",
    "static",
    "use",
    "mod",
    "crate",
    "where",
    "for",
    "def",
    "class",
    "function",
    "export",
    "interface",
    "type",
    "return",
];

/// The colors of a signature (keywords, types, function names), in bytes.
pub fn signature_segments(sig: &str) -> Vec<(Range<usize>, u32)> {
    let bytes = sig.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && is_ident(bytes[i]) {
                i += 1;
            }
            let word = &sig[start..i];
            let next = sig[i..].trim_start().chars().next();
            let color = if SIGNATURE_KEYWORDS.contains(&word) {
                Some(0x4B5BDB)
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Some(0x0D9488)
            } else if next == Some('(') {
                Some(0x6D28D9)
            } else {
                None
            };
            if let Some(c) = color {
                out.push((start..i, c));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A path in a transcript line: `Edit  crates/orch/src/budget.rs`, `src/a.rs:12`.
pub fn find_path(text: &str) -> Option<String> {
    const EXTENSIONS: [&str; 16] = [
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "c", "h", "cpp", "toml", "md",
        "json", "dart",
    ];
    text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']'))
        .map(|t| t.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '.')))
        .map(|t| {
            // `src/a.rs:12` and `src/a.rs:12:3`.
            let mut parts = t.split(':');
            let head = parts.next().unwrap_or(t);
            if parts.all(|p| p.chars().all(|c| c.is_ascii_digit())) {
                head
            } else {
                t
            }
        })
        .find(|t| {
            !t.contains("://")
                && t.rsplit_once('.')
                    .is_some_and(|(stem, ext)| !stem.is_empty() && EXTENSIONS.contains(&ext))
                && t.chars().all(|c| c.is_alphanumeric() || "/._-".contains(c))
        })
        .map(str::to_string)
}

// ---- Tree ----

/// The indent of a tree row.
pub fn indent(depth: u8) -> f32 {
    14. + f32::from(depth) * 18.
}

/// The rows to show: all rows below a collapsed folder are hidden. A query shows the files
/// whose path has every word, and the folders above them.
pub fn visible_rows(files: &[FileNode], collapsed: &HashSet<String>, query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        let mut out = Vec::new();
        let mut skip: Option<&str> = None;
        for (i, f) in files.iter().enumerate() {
            if skip.is_some_and(|s| f.path.starts_with(s) && f.path != s) {
                continue;
            }
            skip = None;
            out.push(i);
            if f.dir && collapsed.contains(&f.path) {
                skip = Some(&f.path);
            }
        }
        return out;
    }
    let hit = |f: &FileNode| {
        let path = f.path.to_lowercase();
        words.iter().all(|w| path.contains(w.as_str()))
    };
    let shown: Vec<&str> = files
        .iter()
        .filter(|f| !f.dir && hit(f))
        .map(|f| f.path.as_str())
        .collect();
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            if f.dir {
                shown.iter().any(|p| p.starts_with(f.path.as_str()))
            } else {
                shown.contains(&f.path.as_str())
            }
        })
        .map(|(i, _)| i)
        .collect()
}

/// The part of a path: the part tag of the closest folder above it.
pub fn part_of<'a>(files: &'a [FileNode], path: &str) -> Option<&'a str> {
    files
        .iter()
        .filter(|f| f.dir && f.part.is_some() && path.starts_with(f.path.as_str()))
        .max_by_key(|f| f.path.len())
        .and_then(|f| f.part.as_deref())
}

// ---- Usages ----

/// The places of a usages list grouped by file, in the order of the first place of each file.
pub fn group_places(items: &[Place]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, p) in items.iter().enumerate() {
        match groups.iter_mut().find(|g| g.0 == p.path) {
            Some(g) => g.1.push(i),
            None => groups.push((p.path.clone(), vec![i])),
        }
    }
    groups
}

/// The byte range of `name` in a preview (whole word), for the highlight.
pub fn name_range(preview: &str, name: &str) -> Option<Range<usize>> {
    if name.is_empty() {
        return None;
    }
    let mut from = 0;
    while let Some(at) = preview[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        let before = preview[..start]
            .bytes()
            .next_back()
            .is_none_or(|b| !is_ident(b));
        let after = preview[end..].bytes().next().is_none_or(|b| !is_ident(b));
        if before && after {
            return Some(start..end);
        }
        from = end;
    }
    None
}

/// `6 in 3 files`.
pub fn usages_meta(total: u32, files: usize) -> String {
    format!(
        "{total} in {files} {}",
        if files == 1 { "file" } else { "files" }
    )
}

// ---- Problems and radar ----

/// A stage of the diagnostics run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub name: String,
    pub on: bool,
}

/// The stages and the free notes of `DiagnosticsResult::notes`. The daemon writes
/// `stages: syntax, rules, linters (clippy); lsp is not running`.
pub fn stage_rows(notes: &[String]) -> (Vec<Stage>, Vec<String>) {
    let mut stages = Vec::new();
    let mut rest = Vec::new();
    for note in notes {
        let Some(list) = note.strip_prefix("stages:") else {
            rest.push(note.clone());
            continue;
        };
        for (i, part) in list.split(';').enumerate() {
            if i == 0 {
                // Split on commas outside of parentheses.
                let mut depth = 0;
                let mut cur = String::new();
                for c in part.chars() {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if c == ',' && depth == 0 {
                        stages.push(Stage {
                            name: cur.trim().to_string(),
                            on: true,
                        });
                        cur.clear();
                    } else {
                        cur.push(c);
                    }
                }
                if !cur.trim().is_empty() {
                    stages.push(Stage {
                        name: cur.trim().to_string(),
                        on: true,
                    });
                }
            } else {
                let name = part.trim();
                let name = name.strip_suffix("is not running").unwrap_or(name).trim();
                if !name.is_empty() {
                    stages.push(Stage {
                        name: name.to_string(),
                        on: false,
                    });
                }
            }
        }
    }
    (stages, rest)
}

/// The hits that touch a file: the key is the path, a folder above it, or a symbol that a claim
/// on this file holds; or the trial merge lists the file.
pub fn hits_for_file<'a>(hits: &'a [RadarHit], claims: &[Claim], path: &str) -> Vec<&'a RadarHit> {
    hits.iter()
        .filter(|h| {
            h.conflict_files.iter().any(|f| f == path)
                || h.keys.iter().any(|k| key_touches(k, claims, path))
        })
        .collect()
}

fn key_touches(key: &str, claims: &[Claim], path: &str) -> bool {
    if let Some(p) = key.strip_prefix("path:") {
        return p == path;
    }
    if let Some(dir) = key.strip_prefix("mod:") {
        return path.starts_with(&format!("{}/", dir.trim_end_matches('/')));
    }
    claims
        .iter()
        .any(|c| c.key == key && c.path.as_deref() == Some(path))
}

/// The claims on a file (any state but landed).
pub fn claims_on<'a>(claims: &'a [Claim], path: &str) -> Vec<&'a Claim> {
    claims
        .iter()
        .filter(|c| c.path.as_deref() == Some(path) && c.state != ClaimState::Landed)
        .collect()
}

pub fn ping_label(kind: PingKind) -> &'static str {
    match kind {
        PingKind::Fyi => "FYI",
        PingKind::Question => "Question",
        PingKind::Request => "Request",
        PingKind::Block => "Block",
    }
}

/// A ping of the last hour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ping {
    pub to: String,
    pub kind: PingKind,
    pub at: UnixMs,
    pub text: String,
}

/// The mail items of the last hour, newest first.
pub fn recent_pings(mail: &[(String, Vec<MailItem>)], now: UnixMs) -> Vec<Ping> {
    let mut out: Vec<Ping> = mail
        .iter()
        .flat_map(|(to, items)| {
            items.iter().map(|m| Ping {
                to: to.clone(),
                kind: m.kind,
                at: m.created_at,
                text: m.text.clone(),
            })
        })
        .filter(|p| p.at >= now - 3_600_000)
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.to.cmp(&b.to)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::coord::ClaimRange;

    fn lr(from: u32, to: u32) -> LineRange {
        LineRange { from, to }
    }

    fn hl(start: u32, end: u32, kind: TokenKind) -> Highlight {
        Highlight {
            line: 1,
            start,
            end,
            kind,
        }
    }

    #[test]
    fn the_request_range_adds_a_margin_and_stays_in_the_file() {
        // Items 0..40 are lines 1..=40.
        assert_eq!(request_range(0, 40, 1000, 80), lr(1, 120));
        assert_eq!(request_range(500, 540, 1000, 80), lr(421, 620));
        assert_eq!(request_range(980, 1000, 1000, 80), lr(901, 1000));
        assert_eq!(request_range(0, 5, 3, 80), lr(1, 3));
        assert_eq!(request_range(0, 0, 0, 80), lr(1, 1));
    }

    #[test]
    fn loaded_ranges_cover_a_visible_range() {
        let loaded = [lr(1, 120)];
        assert!(covers(&loaded, lr(1, 40)));
        assert!(covers(&loaded, lr(80, 120)));
        assert!(!covers(&loaded, lr(100, 130)));
        assert!(!covers(&[], lr(1, 1)));
        // Two ranges that touch cover a range across the seam.
        assert!(covers(&[lr(1, 50), lr(51, 100)], lr(40, 90)));
        assert!(!covers(&[lr(1, 50), lr(52, 100)], lr(40, 90)));
    }

    #[test]
    fn ranges_merge_when_they_touch() {
        let mut loaded = vec![lr(1, 120)];
        add_range(&mut loaded, lr(300, 400));
        assert_eq!(loaded, [lr(1, 120), lr(300, 400)]);
        add_range(&mut loaded, lr(121, 299));
        assert_eq!(loaded, [lr(1, 400)]);
        add_range(&mut loaded, lr(10, 20));
        assert_eq!(loaded, [lr(1, 400)]);
    }

    #[test]
    fn scrolling_past_the_loaded_lines_asks_for_the_visible_lines_and_a_margin() {
        let loaded = [lr(1, 120)];
        // The visible lines 41..=80 are loaded.
        assert_eq!(fetch_for(&loaded, 40, 80, 1000, 80), None);
        // The visible lines 101..=140 are not.
        assert_eq!(fetch_for(&loaded, 100, 140, 1000, 80), Some(lr(21, 220)));
        // Nothing to ask for in an empty file or an empty view.
        assert_eq!(fetch_for(&[], 0, 0, 100, 80), None);
        assert_eq!(fetch_for(&[], 0, 10, 0, 80), None);
        // The view past the end of a short file.
        assert_eq!(fetch_for(&[lr(1, 30)], 0, 40, 30, 80), None);
    }

    #[test]
    fn a_card_opens_above_when_it_does_not_fit_below() {
        assert!(!card_opens_up(100., 800., 150.));
        assert!(card_opens_up(700., 800., 150.));
        // Not enough room above either: stay below.
        assert!(!card_opens_up(60., 120., 150.));
    }

    #[test]
    fn arrow_keys_stop_at_the_ends() {
        assert_eq!(step(0, 5, -1), 0);
        assert_eq!(step(4, 5, 1), 4);
        assert_eq!(step(2, 5, 1), 3);
        assert_eq!(step(0, 0, 1), 0);
    }

    fn turn(session: &str, n: u32, time: i64) -> BlameTurn {
        BlameTurn {
            session: session.into(),
            n,
            prompt: format!("prompt {session} {n}"),
            model: "m".into(),
            time,
            cost: Some(0.1),
        }
    }

    fn hunk(line: u32, count: u32, turn: Option<BlameTurn>, commit: bool) -> BlameHunk {
        BlameHunk {
            line,
            count,
            commit: commit.then(|| CommitRef {
                id: "a41c9e0".into(),
                summary: "s".into(),
                time: 1,
            }),
            turn,
        }
    }

    #[test]
    fn blame_ranks_follow_the_age_of_the_turn() {
        let map = BlameMap::new(
            vec![
                hunk(1, 2, None, true),
                hunk(3, 2, Some(turn("7f3a", 2, 100)), true),
                hunk(5, 1, Some(turn("8a22", 5, 500)), false),
                hunk(6, 2, Some(turn("8a22", 4, 400)), false),
                hunk(8, 1, Some(turn("7f3a", 3, 300)), false),
                hunk(9, 1, Some(turn("7f3a", 1, 50)), false),
                hunk(10, 1, Some(turn("7f3a", 0, 10)), false),
            ],
            12,
        );
        let colors: Vec<u32> = (1..=12).map(|l| map.color_at(l)).collect();
        assert_eq!(colors[0], BLAME_COLORS[4], "commit");
        assert_eq!(colors[2], BLAME_COLORS[3], "turn 7f3a 2: fourth newest");
        assert_eq!(colors[4], BLAME_COLORS[0], "newest turn");
        assert_eq!(colors[5], BLAME_COLORS[1]);
        assert_eq!(colors[7], BLAME_COLORS[2]);
        assert_eq!(colors[8], BLAME_COLORS[4], "rank 4 is the last step");
        assert_eq!(colors[11], BLAME_COLORS[4], "no hunk");
        assert!(matches!(map.kind_at(3), BlameKind::Turn(t) if t.n == 2));
        assert!(matches!(map.kind_at(1), BlameKind::Commit(_)));
        assert!(matches!(map.kind_at(12), BlameKind::Unknown));
        assert!(matches!(map.kind_at(0), BlameKind::Unknown));
    }

    #[test]
    fn the_blame_scale_has_five_steps() {
        assert_eq!(blame_bucket(Some(0)), 0);
        assert_eq!(blame_bucket(Some(3)), 3);
        assert_eq!(blame_bucket(Some(4)), 4);
        assert_eq!(blame_bucket(Some(40)), 4);
        assert_eq!(blame_bucket(None), 4);
        assert_eq!(blame_color(Some(0)), 0x3574F0);
        assert_eq!(blame_color(None), 0xE3E7EF);
    }

    #[test]
    fn token_colors_follow_the_palette() {
        assert_eq!(token_color(TokenKind::Keyword), Some(0x4B5BDB));
        assert_eq!(token_color(TokenKind::Type), Some(0x0D9488));
        assert_eq!(token_color(TokenKind::Function), Some(0x6D28D9));
        assert_eq!(token_color(TokenKind::String), Some(0x15803D));
        assert_eq!(token_color(TokenKind::Number), Some(0xC2410C));
        assert_eq!(token_color(TokenKind::Comment), Some(0x8A93A6));
        assert_eq!(token_color(TokenKind::Macro), Some(0x5A6578));
        assert_eq!(token_color(TokenKind::Attribute), Some(0x5A6578));
        assert_eq!(token_color(TokenKind::Variable), None);
    }

    #[test]
    fn segments_cut_at_tokens_marks_and_the_symbol() {
        let text = "pub fn total(&self)";
        let tokens = [
            hl(0, 3, TokenKind::Keyword),
            hl(4, 6, TokenKind::Keyword),
            hl(7, 12, TokenKind::Function),
        ];
        let marks = [Mark {
            start: 7,
            end: 19,
            sev: Sev::Error,
        }];
        let segs = line_segments(text, &tokens, &marks, Some(7..12));
        assert_eq!(
            segs,
            vec![
                Seg {
                    range: 0..3,
                    color: Some(0x4B5BDB),
                    under: None,
                    bg: false
                },
                Seg {
                    range: 4..6,
                    color: Some(0x4B5BDB),
                    under: None,
                    bg: false
                },
                Seg {
                    range: 7..12,
                    color: Some(0x6D28D9),
                    under: Some(Sev::Error),
                    bg: true
                },
                Seg {
                    range: 12..19,
                    color: None,
                    under: Some(Sev::Error),
                    bg: false
                },
            ]
        );
    }

    #[test]
    fn segments_survive_bytes_past_the_line_and_inside_a_character() {
        let text = "é = 1";
        let tokens = [hl(0, 99, TokenKind::Comment), hl(1, 2, TokenKind::Number)];
        let segs = line_segments(text, &tokens, &[], None);
        assert!(segs.iter().all(|s| text.is_char_boundary(s.range.start)
            && text.is_char_boundary(s.range.end)
            && s.range.end <= text.len()));
        assert!(line_segments("", &[], &[], None).is_empty());
    }

    fn finding(line: u32, col: u32, end_col: Option<u32>, level: FindingLevel) -> Finding {
        Finding {
            rule: "r".into(),
            level,
            message: "m".into(),
            path: "a.rs".into(),
            line,
            col,
            end_line: None,
            end_col,
            tool: String::new(),
            fingerprint: String::new(),
        }
    }

    #[test]
    fn marks_cover_the_range_of_the_finding_or_the_word() {
        let text = "    let x = foo(1);";
        let f = [
            finding(3, 12, Some(15), FindingLevel::Error),
            finding(3, 12, None, FindingLevel::Warning),
            finding(3, 4, Some(4), FindingLevel::None),
            finding(4, 0, Some(1), FindingLevel::Error),
        ];
        let marks = marks_for_line(&f, 3, text);
        assert_eq!(
            marks,
            vec![
                Mark {
                    start: 12,
                    end: 15,
                    sev: Sev::Error
                },
                Mark {
                    start: 12,
                    end: 15,
                    sev: Sev::Warning
                },
            ]
        );
        assert_eq!(gutter_sev(&f, 3), Some(Sev::Error));
        assert_eq!(gutter_sev(&f, 9), None);
    }

    #[test]
    fn a_finding_over_several_lines_marks_each_line() {
        let mut f = finding(2, 4, Some(3), FindingLevel::Note);
        f.end_line = Some(4);
        let fs = [f];
        assert_eq!(marks_for_line(&fs, 2, "abcdefgh")[0].start, 4);
        assert_eq!(marks_for_line(&fs, 3, "abcdefgh")[0].end, 8);
        assert_eq!(marks_for_line(&fs, 4, "abcdefgh")[0].end, 3);
        assert!(marks_for_line(&fs, 5, "abcdefgh").is_empty());
    }

    #[test]
    fn words_come_from_identifier_characters() {
        let text = "  inv.total(on)";
        assert_eq!(word_at(text, 6), Some(6..11));
        assert_eq!(word_at(text, 3), Some(2..5));
        assert_eq!(word_at(text, 5), None);
        assert_eq!(word_at(text, 99), None);
        assert_eq!(word_at("x = 12", 4), None);
        let tokens = [hl(0, 3, TokenKind::Keyword), hl(4, 8, TokenKind::Function)];
        assert!(!is_symbol_at(&tokens, 1));
        assert!(is_symbol_at(&tokens, 5));
        assert!(is_symbol_at(&tokens, 20));
    }

    fn claim(session: &str, state: ClaimState, path: &str, range: Option<(u32, u32)>) -> Claim {
        Claim {
            session: session.into(),
            codebase: "billing".into(),
            key: format!("path:{path}"),
            state,
            path: Some(path.into()),
            range: range.map(|(from, to)| ClaimRange { from, to }),
            lease_s: 420,
        }
    }

    #[test]
    fn heat_skips_the_opening_session_and_landed_claims() {
        let claims = vec![
            claim("8a22", ClaimState::Editing, "a.rs", Some((15, 24))),
            claim("8d07", ClaimState::Intent, "a.rs", Some((20, 39))),
            claim("7f3a", ClaimState::Editing, "a.rs", Some((1, 3))),
            claim("m2e5", ClaimState::Landed, "a.rs", Some((1, 50))),
            claim("k9d4", ClaimState::Editing, "b.rs", Some((1, 9))),
            claim("x", ClaimState::Editing, "a.rs", None),
        ];
        let heat = heat_ranges(&claims, "a.rs", Some("7f3a"));
        assert_eq!(heat.len(), 2);
        assert_eq!(heat_at(&heat, 10), None);
        assert_eq!(heat_at(&heat, 16).map(|h| h.session.as_str()), Some("8a22"));
        // Editing wins over intent where they overlap.
        assert_eq!(heat_at(&heat, 22).map(|h| h.session.as_str()), Some("8a22"));
        assert_eq!(heat_at(&heat, 30).map(|h| h.session.as_str()), Some("8d07"));
        assert_eq!(heat_tag(&heat[0]), "8a22 editing");
        assert_eq!(heat_hover(&heat[0]), "8a22 · editing · lease 7 min");
        assert_eq!(lease_text(30), "lease 30 s");
        assert_eq!(lines_text(15, 24), "L15–24");
        assert_eq!(lines_text(8, 8), "L8");
    }

    fn node(
        name: &str,
        kind: SymbolKind,
        line: u32,
        start: u32,
        end: u32,
        kids: Vec<OutlineNode>,
    ) -> OutlineNode {
        OutlineNode {
            name: name.into(),
            kind,
            line,
            start_line: start,
            end_line: end,
            children: kids,
        }
    }

    fn outline() -> Vec<OutlineRow> {
        flatten_outline(&[
            node("Invoice", SymbolKind::Struct, 7, 6, 12, vec![]),
            node(
                "Invoice",
                SymbolKind::Impl,
                14,
                14,
                40,
                vec![
                    node("total", SymbolKind::Method, 17, 15, 24, vec![]),
                    node("due", SymbolKind::Method, 27, 26, 30, vec![]),
                ],
            ),
        ])
    }

    #[test]
    fn the_outline_flattens_with_depth_and_finds_the_symbol_at_a_line() {
        let rows = outline();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2].depth, 1);
        assert_eq!(symbol_path(&rows, 20), "Invoice › total");
        assert_eq!(symbol_path(&rows, 26), "Invoice › due");
        assert_eq!(symbol_path(&rows, 8), "Invoice");
        assert_eq!(symbol_path(&rows, 1), "");
        assert_eq!(
            focus_symbol(&rows, Some(28)).map(|r| r.name.as_str()),
            Some("due")
        );
        // No cursor: the first function.
        assert_eq!(
            focus_symbol(&rows, None).map(|r| r.name.as_str()),
            Some("total")
        );
        assert_eq!(kind_glyph(SymbolKind::Method), "fn");
        assert_eq!(bare_name("total(&self, on)"), "total");
        assert_eq!(bare_name("impl Invoice"), "Invoice");
    }

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn the_line_diff_matches_the_unified_format() {
        let old = lines("a\nb\nc\nd\ne\nf\ng\nh");
        let new = lines("a\nb\nC\nd\ne\nf\ng\nh\ni");
        let d = diff_lines(&old, &new, 2);
        assert_eq!((d.added, d.removed), (2, 1));
        let shown: Vec<String> = d
            .rows
            .iter()
            .map(|r| match r.kind {
                DiffKind::Hunk => r.text.clone(),
                DiffKind::Context => format!(" {}", r.text),
                DiffKind::Added => format!("+{}", r.text),
                DiffKind::Removed => format!("-{}", r.text),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "@@ -1,5 +1,5 @@",
                " a",
                " b",
                "-c",
                "+C",
                " d",
                " e",
                "@@ -7,2 +7,3 @@",
                " g",
                " h",
                "+i"
            ]
        );
        // Line numbers on both sides.
        let removed = d.rows.iter().find(|r| r.kind == DiffKind::Removed).unwrap();
        assert_eq!((removed.old, removed.new), (Some(3), None));
        let added = d.rows.iter().find(|r| r.text == "C").unwrap();
        assert_eq!((added.old, added.new), (None, Some(3)));
    }

    #[test]
    fn the_lines_of_a_turn_stand_in_for_its_diff() {
        let lines = lines(
            "a
b
c
d
e
f
g
h
i
j",
        );
        let map = BlameMap::new(
            vec![
                hunk(1, 4, None, true),
                hunk(5, 1, Some(turn("a01c", 2, 200)), false),
                hunk(6, 4, Some(turn("a01c", 3, 300)), false),
            ],
            10,
        );
        let d = turn_lines(&lines, &map, "a01c", 2, 1);
        assert_eq!(d.added, 1);
        let shown: Vec<String> = d
            .rows
            .iter()
            .map(|r| format!("{:?}:{}", r.kind, r.text))
            .collect();
        assert_eq!(
            shown,
            ["Hunk:@@ lines 4–6 @@", "Context:d", "Added:e", "Context:f"]
        );
        assert_eq!(d.rows[2].new, Some(5));
        assert!(turn_lines(&lines, &map, "a01c", 9, 1).rows.is_empty());
        // Two runs that touch share one hunk.
        let both = turn_lines(&lines, &map, "a01c", 3, 3);
        assert_eq!(both.added, 4);
        assert_eq!(
            both.rows
                .iter()
                .filter(|r| r.kind == DiffKind::Hunk)
                .count(),
            1
        );
    }

    #[test]
    fn the_line_diff_of_equal_texts_and_new_files() {
        let t = lines("a\nb");
        assert!(diff_lines(&t, &t, 3).rows.is_empty());
        let d = diff_lines(&[], &t, 3);
        assert_eq!((d.added, d.removed), (2, 0));
        assert_eq!(d.rows[0].text, "@@ -0,0 +1,2 @@");
        let gone = diff_lines(&t, &[], 3);
        assert_eq!((gone.added, gone.removed), (0, 2));
    }

    #[test]
    fn text_splits_into_lines() {
        assert_eq!(split_lines("a\r\nb\n"), ["a", "b"]);
        assert_eq!(split_lines("a\n\nb"), ["a", "", "b"]);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn the_timeline_lists_the_newest_first_with_blame_colors() {
        let ev = vec![
            TimelineEvent::Commit {
                id: "3f2a91c0d4b8".into(),
                summary: "Add the skeleton".into(),
                author: "Dana Lee".into(),
                time: 10,
                turns: vec![],
            },
            TimelineEvent::Turn {
                session: "a01c".into(),
                n: 1,
                prompt: "cap fan-out".into(),
                model: "opus-5.5".into(),
                time: 20,
                cost: Some(0.71),
                added: 16,
                removed: 1,
            },
            TimelineEvent::Turn {
                session: "a01c".into(),
                n: 2,
                prompt: "check the limit".into(),
                model: "opus-5.5".into(),
                time: 30,
                cost: None,
                added: 9,
                removed: 0,
            },
        ];
        let points = timeline_points(&ev);
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].who, "a01c · turn 2");
        assert_eq!(points[0].color, Some(BLAME_COLORS[0]));
        assert_eq!(points[1].color, Some(BLAME_COLORS[1]));
        assert_eq!(points[1].detail, "opus-5.5 · $0.71");
        assert_eq!(points[2].who, "3f2a91c");
        assert_eq!(points[2].detail, "Dana Lee · committed");
        assert_eq!(points[2].color, None);
        assert_eq!(
            revs_of(&points[0].kind),
            ("turn:1".to_string(), "turn:2".to_string())
        );
        assert_eq!(
            revs_of(&points[2].kind),
            ("3f2a91c0d4b8^".to_string(), "3f2a91c0d4b8".to_string())
        );
    }

    #[test]
    fn hover_text_drops_the_code_block() {
        let md = "```rust\npub fn check(&self)\n```\n\nmethod in `crates/orch/src/budget.rs:18`";
        assert_eq!(doc_text(md), "method in crates/orch/src/budget.rs:18");
        assert_eq!(doc_text("```\nx\n```"), "");
        let long = "word ".repeat(100);
        assert!(doc_text(&long).chars().count() <= 300);
    }

    #[test]
    fn signatures_get_token_colors() {
        let sig = "pub fn total(&self, on: NaiveDate) -> Money";
        let segs = signature_segments(sig);
        let words: Vec<(&str, u32)> = segs.iter().map(|(r, c)| (&sig[r.clone()], *c)).collect();
        assert_eq!(
            words,
            [
                ("pub", 0x4B5BDB),
                ("fn", 0x4B5BDB),
                ("total", 0x6D28D9),
                ("self", 0x4B5BDB),
                ("NaiveDate", 0x0D9488),
                ("Money", 0x0D9488),
            ]
        );
    }

    #[test]
    fn paths_come_out_of_transcript_lines() {
        assert_eq!(
            find_path("Edit  crates/orch/src/budget.rs").as_deref(),
            Some("crates/orch/src/budget.rs")
        );
        assert_eq!(
            find_path("● Read src/a.rs:12:3 (218 lines)").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            find_path("see \"web/src/findings.ts\".").as_deref(),
            Some("web/src/findings.ts")
        );
        assert_eq!(find_path("Bash  cargo test -p orch budget"), None);
        assert_eq!(find_path("https://example.com/a.rs"), None);
        assert_eq!(find_path("version 1.2.3"), None);
    }

    fn file(path: &str, dir: bool, depth: u8, part: Option<&str>) -> FileNode {
        FileNode {
            path: path.into(),
            name: file_name(path).into(),
            dir,
            depth,
            part: part.map(str::to_string),
            claimed_by: None,
            claim_state: None,
        }
    }

    fn tree() -> Vec<FileNode> {
        vec![
            file("billing/invoices/", true, 0, Some("invoices")),
            file("billing/invoices/mod.rs", false, 1, None),
            file("billing/invoices/invoice.rs", false, 1, None),
            file("billing/db/", true, 0, Some("database")),
            file("billing/db/schema.rs", false, 1, None),
            file("Cargo.toml", false, 0, None),
        ]
    }

    #[test]
    fn collapsed_folders_hide_their_files() {
        let files = tree();
        assert_eq!(
            visible_rows(&files, &HashSet::new(), ""),
            [0, 1, 2, 3, 4, 5]
        );
        let collapsed: HashSet<String> = ["billing/invoices/".to_string()].into();
        assert_eq!(visible_rows(&files, &collapsed, ""), [0, 3, 4, 5]);
    }

    #[test]
    fn the_filter_keeps_matching_files_and_their_folders() {
        let files = tree();
        let none = HashSet::new();
        assert_eq!(visible_rows(&files, &none, "schema"), [3, 4]);
        assert_eq!(visible_rows(&files, &none, "INVOICE.rs"), [0, 2]);
        assert!(visible_rows(&files, &none, "zzz").is_empty());
    }

    #[test]
    fn a_file_takes_the_part_of_its_closest_folder() {
        let files = tree();
        assert_eq!(
            part_of(&files, "billing/invoices/invoice.rs"),
            Some("invoices")
        );
        assert_eq!(part_of(&files, "billing/db/schema.rs"), Some("database"));
        assert_eq!(part_of(&files, "Cargo.toml"), None);
        assert_eq!(indent(0), 14.);
        assert_eq!(indent(1), 32.);
    }

    fn place(path: &str, line: u32) -> Place {
        Place {
            path: path.into(),
            line,
            col: 0,
            len: None,
            preview: String::new(),
            name: None,
            kind: None,
            container: None,
        }
    }

    #[test]
    fn usages_group_by_file_in_first_seen_order() {
        let items = [place("a.rs", 1), place("b.rs", 2), place("a.rs", 9)];
        let groups = group_places(&items);
        assert_eq!(
            groups,
            [
                ("a.rs".to_string(), vec![0, 2]),
                ("b.rs".to_string(), vec![1])
            ]
        );
        assert_eq!(usages_meta(6, 3), "6 in 3 files");
        assert_eq!(usages_meta(1, 1), "1 in 1 file");
        assert_eq!(
            name_range("let owed = self.total(self.issued);", "total"),
            Some(16..21)
        );
        assert_eq!(name_range("totally", "total"), None);
        assert_eq!(name_range("x", ""), None);
    }

    #[test]
    fn the_stage_note_of_the_daemon_becomes_rows() {
        let notes = vec![
            "stages: syntax, rules, linters (clippy); lsp is not running".to_string(),
            "the demo has findings only for session a01c".to_string(),
        ];
        let (stages, rest) = stage_rows(&notes);
        let shown: Vec<(&str, bool)> = stages.iter().map(|s| (s.name.as_str(), s.on)).collect();
        assert_eq!(
            shown,
            [
                ("syntax", true),
                ("rules", true),
                ("linters (clippy)", true),
                ("lsp", false)
            ]
        );
        assert_eq!(rest, ["the demo has findings only for session a01c"]);
    }

    #[test]
    fn radar_hits_filter_to_the_file() {
        let mut sym = claim(
            "8a22",
            ClaimState::Editing,
            "src/invoice.rs",
            Some((40, 52)),
        );
        sym.key = "sym:rust:billing::Invoice::total".into();
        let claims = vec![sym];
        let hit = |keys: &[&str], conflict: &[&str]| RadarHit {
            a: "a".into(),
            b: "b".into(),
            codebase: "billing".into(),
            tier: 1,
            keys: keys.iter().map(|k| k.to_string()).collect(),
            score: 0.4,
            conflict_files: conflict.iter().map(|k| k.to_string()).collect(),
            trial: true,
            trial_error: None,
            at: 0,
            status: None,
        };
        let hits = vec![
            hit(&["path:src/invoice.rs"], &[]),
            hit(&["sym:rust:billing::Invoice::total"], &[]),
            hit(&["mod:src"], &[]),
            hit(&["path:other.rs"], &["src/invoice.rs"]),
            hit(&["path:other.rs"], &[]),
            hit(&["sym:rust:billing::Other::x"], &[]),
        ];
        let found = hits_for_file(&hits, &claims, "src/invoice.rs");
        assert_eq!(found.len(), 4);
        assert_eq!(claims_on(&claims, "src/invoice.rs").len(), 1);
    }

    #[test]
    fn pings_of_the_last_hour_come_newest_first() {
        let item = |id: i64, at: i64, text: &str| MailItem {
            id,
            from: "x".into(),
            kind: PingKind::Fyi,
            key: "k".into(),
            text: text.into(),
            created_at: at,
            delivered_at: None,
            resolved: false,
            count: 1,
            digest: false,
        };
        let now = 10_000_000;
        let mail = vec![
            (
                "7f3a".to_string(),
                vec![
                    item(1, now - 60_000, "new"),
                    item(2, now - 4_000_000, "old"),
                ],
            ),
            ("8a22".to_string(), vec![item(3, now - 30_000, "newest")]),
        ];
        let pings = recent_pings(&mail, now);
        let texts: Vec<&str> = pings.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["newest", "new"]);
        assert_eq!(ping_label(PingKind::Fyi), "FYI");
    }

    #[test]
    fn small_texts_read_like_the_artboard() {
        assert_eq!(cost_text(0.31), "$0.31");
        assert_eq!(lang_label("rust"), "Rust");
        assert_eq!(lang_label("elixir"), "Elixir");
        assert_eq!(lang_label(""), "Text");
        assert_eq!(file_name("billing/invoices/invoice.rs"), "invoice.rs");
        assert_eq!(
            place_text("billing/invoices/invoice.rs", 17),
            "invoice.rs:17"
        );
    }
}
