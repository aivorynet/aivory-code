//! A transcript as a virtual list of blocks. Item 0 is the top padding (or the marker of the
//! older lines), the last item is the bottom padding, and the blocks sit between them. A
//! change of the lines splices only the blocks that differ, so the scroll position stays.

use std::collections::HashSet;

use aivory_protocol::Line;
use gpui::{AnyElement, FollowMode, ListAlignment, ListState, px};

use super::blocks::{self, Block, Pending};
use super::draw::{self, Frame};

pub struct BlockList {
    pub list: ListState,
    lines: Vec<Line>,
    start: u32,
    blocks: Vec<Block>,
    expanded: HashSet<u32>,
    pending: Pending,
    marker: bool,
}

impl std::fmt::Debug for BlockList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockList")
            .field("lines", &self.lines.len())
            .field("start", &self.start)
            .finish_non_exhaustive()
    }
}

impl BlockList {
    pub fn new(alignment: ListAlignment) -> Self {
        let list = ListState::new(2, alignment, px(300.));
        // A top-aligned transcript starts at the top and follows its tail while the reader
        // stays at the end.
        if alignment == ListAlignment::Top {
            list.set_follow_mode(FollowMode::Tail);
        }
        Self {
            list,
            lines: Vec::new(),
            start: 0,
            blocks: Vec::new(),
            expanded: HashSet::new(),
            pending: Pending::No,
            marker: false,
        }
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The index of the first line in the full transcript.
    pub fn start(&self) -> u32 {
        self.start
    }

    pub fn pending(&self) -> Pending {
        self.pending
    }

    /// Show `lines`; the first one has the index `start` in the full transcript.
    pub fn set(&mut self, lines: Vec<Line>, start: u32, pending: Pending) {
        if lines == self.lines && start == self.start && pending == self.pending {
            return;
        }
        self.lines = lines;
        self.start = start;
        self.pending = pending;
        self.regroup();
    }

    /// Fold or unfold the result rows of the block that starts at line `abs`.
    pub fn toggle(&mut self, abs: u32) {
        if !self.expanded.remove(&abs) {
            self.expanded.insert(abs);
        }
        self.regroup();
    }

    fn regroup(&mut self) {
        let new = blocks::group(&self.lines, self.start, self.pending, &self.expanded);
        let (old_n, new_n) = (self.blocks.len(), new.len());
        let prefix = self
            .blocks
            .iter()
            .zip(&new)
            .take_while(|(a, b)| a == b)
            .count();
        let room = old_n.min(new_n) - prefix;
        let suffix = self
            .blocks
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        if prefix + suffix < old_n || prefix + suffix < new_n {
            self.list
                .splice(1 + prefix..1 + old_n - suffix, new_n - prefix - suffix);
        }
        self.blocks = new;
        let marker = self.start > 0;
        if marker != self.marker {
            self.marker = marker;
            self.list.splice(0..1, 1);
        }
    }

    /// The element of list item `ix`.
    pub fn item(&self, ix: usize, f: &Frame) -> AnyElement {
        if ix == 0 {
            return draw::header_item(self.start, f);
        }
        match self.blocks.get(ix - 1) {
            Some(block) => draw::block_item(block, f),
            None => draw::footer_item(f),
        }
    }

    /// The text of all lines, for the clipboard.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| match (&l.label, l.kind) {
                (Some(label), aivory_protocol::LineKind::Tool) => {
                    format!("{label}({})", l.text)
                }
                _ => l.text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<Line> {
        (0..n).map(|i| Line::agent(format!("row {i}"))).collect()
    }

    #[test]
    fn the_list_has_a_header_and_a_footer_item() {
        let mut b = BlockList::new(ListAlignment::Bottom);
        assert_eq!(b.list.item_count(), 2);
        let mut l = lines(1);
        l.push(Line::tool("Bash", "ls"));
        b.set(l, 0, Pending::No);
        assert_eq!(b.list.item_count(), 4);
    }

    #[test]
    fn an_unchanged_transcript_does_nothing() {
        let mut b = BlockList::new(ListAlignment::Bottom);
        let mut l = lines(1);
        l.push(Line::tool("Bash", "ls"));
        b.set(l.clone(), 5, Pending::No);
        let count = b.list.item_count();
        b.set(l, 5, Pending::No);
        assert_eq!(b.list.item_count(), count);
        assert_eq!(b.start(), 5);
    }

    #[test]
    fn a_new_tool_call_adds_one_block() {
        let mut b = BlockList::new(ListAlignment::Bottom);
        let mut l = lines(1);
        l.push(Line::tool("Bash", "ls"));
        b.set(l.clone(), 0, Pending::No);
        l.push(Line::output("x"));
        l.push(Line::tool("Read", "a.rs"));
        b.set(l, 0, Pending::No);
        assert_eq!(b.list.item_count(), 5);
    }

    #[test]
    fn a_fold_toggle_keeps_the_item_count() {
        let mut b = BlockList::new(ListAlignment::Bottom);
        let mut l = vec![Line::tool("Bash", "ls")];
        l.extend((0..9).map(|i| Line::output(format!("o{i}"))));
        b.set(l, 0, Pending::No);
        let count = b.list.item_count();
        b.toggle(0);
        assert_eq!(b.list.item_count(), count);
        assert!(b.blocks[0].expanded);
        b.toggle(0);
        assert!(!b.blocks[0].expanded);
    }

    #[test]
    fn the_text_joins_the_lines() {
        let mut b = BlockList::new(ListAlignment::Top);
        b.set(
            vec![Line::agent("a"), Line::tool("Bash", "ls")],
            0,
            Pending::No,
        );
        assert_eq!(b.text(), "a\nBash(ls)");
    }
}
