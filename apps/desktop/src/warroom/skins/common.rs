//! What the three full-screen skins share: session data, the actions on it, the live values
//! (`Pulse`), text formats, motion and color helpers, and the canvas painter. A skin keeps only
//! its own look (projection, rain, Bridge frame, layouts).

mod paint;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use aivory_protocol::{AskKind, LineKind, Pane, SessionState, WarRoom};
use gpui::Context;

pub(super) use paint::{Align, Painter, TextCache, ink};

use crate::actions::Action;
use crate::warroom::decisions::decisions;
use crate::warroom::{WarRoomEvent, WarRoomScreen};

// ---- session data -------------------------------------------------------------------------

/// The live sessions in the order of the war room.
pub(super) fn live(war: &WarRoom) -> Vec<&Pane> {
    war.sessions.iter().filter(|s| s.state.is_live()).collect()
}

/// The ask queue: live sessions with an open ask, the longest wait first.
pub(super) fn asks(war: &WarRoom) -> Vec<&Pane> {
    decisions(war)
}

/// The id of the ask after `current` in the queue; the first one when `current` is not in it.
pub(super) fn next_ask(queue: &[&Pane], current: Option<&str>) -> Option<String> {
    let at = current.and_then(|id| queue.iter().position(|p| p.id == id));
    queue
        .get(at.map_or(0, |i| (i + 1) % queue.len().max(1)))
        .map(|p| p.id.clone())
}

/// The part of a session, or `main` for a session without one.
pub(super) fn part_of(pane: &Pane) -> String {
    if pane.part.is_empty() {
        "main".to_string()
    } else {
        pane.part.clone()
    }
}

/// The latest tool line of a session as short text.
pub(super) fn last_tool(pane: &Pane) -> Option<String> {
    pane.tail
        .iter()
        .rev()
        .find(|l| l.kind == LineKind::Tool)
        .map(|l| short_tool(l.label.as_deref().unwrap_or(""), &l.text))
}

/// `Edit  C:\src\focus.rs +2` -> `Edit focus.rs +2`: the label and the file name only.
pub(super) fn short_tool(label: &str, text: &str) -> String {
    let mut out = label.trim().to_string();
    for w in text.split_whitespace() {
        let name = match w.rfind(['/', '\\']) {
            Some(at) if at + 1 < w.len() => &w[at + 1..],
            _ => w,
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name);
    }
    out
}

/// The word for a kind of ask.
pub(super) fn kind_word(kind: AskKind) -> &'static str {
    match kind {
        AskKind::Permission => "Permission",
        AskKind::Plan => "Plan",
        AskKind::Question => "Question",
        AskKind::Error => "Error",
    }
}

/// The label of an answer key: short and upper case.
pub(super) fn key_label(option: &str) -> String {
    let up = option.to_uppercase();
    match up.as_str() {
        "ALWAYS ALLOW" => "ALWAYS".to_string(),
        _ => clip(up.split_whitespace().next().unwrap_or(""), 8),
    }
}

// ---- format -------------------------------------------------------------------------------

/// `2380` as `2,380`.
pub(super) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Cut `text` to `max` characters with a trailing `…`.
pub(super) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Wrap `text` at word borders into at most `lines` lines of `width` characters.
#[allow(dead_code)]
pub(super) fn wrap(text: &str, width: usize, lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut cut = false;
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        let line_len = line.chars().count();
        if line_len > 0 && line_len + 1 + word_len > width {
            out.push(std::mem::take(&mut line));
            if out.len() == lines {
                cut = true;
                break;
            }
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !cut && !line.is_empty() {
        out.push(line);
    }
    out.truncate(lines);
    for l in out.iter_mut() {
        *l = clip(l, width);
    }
    if cut && let Some(last) = out.last_mut() {
        *last = clip(&format!("{last}…"), width);
    }
    out
}

#[cfg(test)]
/// The rate as a short number: `840`, `2.4k`.
pub(super) fn rate_text(rate: f32) -> String {
    if rate < 0.5 {
        "—".to_string()
    } else if rate >= 10_000. {
        format!("{:.0}k", rate / 1000.)
    } else if rate >= 1000. {
        format!("{:.1}k", rate / 1000.)
    } else {
        format!("{:.0}", rate)
    }
}

// ---- motion and color ---------------------------------------------------------------------

/// How long a new ask blinks: two flashes of 200 ms with a 200 ms gap.
pub(super) const BLINK: Duration = Duration::from_millis(800);

/// `Some(lit)` while a new ask blinks (lit in the first and third 200 ms), `None` once calm.
pub(super) fn blink(elapsed: Duration) -> Option<bool> {
    if elapsed >= BLINK {
        return None;
    }
    Some((elapsed.as_millis() / 200).is_multiple_of(2))
}

/// A color `0xRRGGBB` mixed toward another one, `t` from 0 to 1.
pub(super) fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 255) as f32, ((b >> shift) & 255) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// A color `0xRRGGBBAA` with its alpha scaled.
#[allow(dead_code)]
pub(super) fn fade(color: u32, a: f32) -> u32 {
    let alpha = ((color & 0xFF) as f32 * a.clamp(0., 1.)) as u32;
    (color & !0xFF) | alpha
}

// ---- charts -------------------------------------------------------------------------------

// ---- live values --------------------------------------------------------------------------

/// The most lines the event feed keeps.
const FEED: usize = 20;

/// The live values of a skin: the tokens per second, eased numbers, new asks and the event
/// feed. One per skin state; call `observe` once per frame.
#[derive(Default)]
pub(super) struct Pulse {
    /// Tokens per second, smoothed.
    pub rate: f32,
    sample: Option<(Instant, u64)>,
    open: HashSet<String>,
    primed: bool,
    fresh: HashMap<String, Instant>,
    seen: HashMap<String, (SessionState, String)>,
    /// The newest event lines last.
    pub feed: VecDeque<String>,
    now: Option<Instant>,
    /// True while an eased number or a blink still moves.
    pub moving: bool,
}

impl Pulse {
    /// Take the values of a frame. Returns the new event lines of this frame.
    pub fn observe(&mut self, war: &WarRoom, now: Instant) -> Vec<String> {
        self.now = Some(now);
        self.moving = false;
        let live = live(war);
        // Tokens per second from the growth of the total, sampled once a second.
        let total: u64 = live.iter().map(|s| s.tokens).sum();
        match self.sample {
            None => self.sample = Some((now, total)),
            Some((at, was)) if now - at >= Duration::from_secs(1) => {
                let now_rate = total.saturating_sub(was) as f32 / (now - at).as_secs_f32();
                self.rate = self.rate * 0.5 + now_rate * 0.5;
                self.sample = Some((now, total));
            }
            Some(_) => {}
        }
        // A new ask blinks; the first frame only learns the asks that are already open.
        let open: HashSet<String> = decisions(war)
            .iter()
            .filter_map(|p| p.ask.as_ref().map(|a| a.id.clone()))
            .collect();
        for id in &open {
            if !self.open.contains(id) && self.primed {
                self.fresh.insert(id.clone(), now);
            }
        }
        self.open = open;
        self.primed = true;
        self.fresh
            .retain(|id, at| self.open.contains(id) && blink(now - *at).is_some());
        self.moving = !self.fresh.is_empty();
        // The feed: what changed since the last frame.
        let first = self.seen.is_empty();
        let mut lines = Vec::new();
        for s in &live {
            let who = format!("{}@{}", s.provider, s.codebase);
            let part = part_of(s);
            match self.seen.get(&s.id) {
                None if first => lines.push(format!("{who} / {part}")),
                None => lines.push(format!("{who} joined {part}")),
                Some(before) => lines.extend(feed_event(&who, before, s.state, &part)),
            }
            self.seen.insert(s.id.clone(), (s.state, part));
        }
        self.seen.retain(|id, _| live.iter().any(|s| s.id == *id));
        for l in &lines {
            self.feed.push_back(l.clone());
        }
        while self.feed.len() > FEED {
            self.feed.pop_front();
        }
        lines
    }
}

/// The feed line for a change of a session, if the change is worth a line.
pub(super) fn feed_event(
    who: &str,
    before: &(SessionState, String),
    state: SessionState,
    part: &str,
) -> Option<String> {
    if before.0 != state {
        return Some(match state {
            SessionState::NeedsYou => format!("{who} raised a beacon"),
            SessionState::Error => format!("{who} hit an error"),
            SessionState::Working => format!("{who} works on {part}"),
            SessionState::Idle => format!("{who} is idle"),
            SessionState::Parked | SessionState::Done => return None,
        });
    }
    (before.1 != part).then(|| format!("{who} -> {part}"))
}

// ---- actions ------------------------------------------------------------------------------

impl WarRoomScreen {
    /// Send answer `option` of the ask `ask` of a session and mark the session as answered.
    pub(super) fn skin_answer(
        &mut self,
        session: &str,
        ask: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) {
        self.act(
            Action::Answer {
                id: session.to_string(),
                ask: ask.to_string(),
                option: Some(option.to_string()),
                text: None,
            },
            cx,
        );
        self.answered(session, cx);
    }

    /// Open one session in the Focus view.
    pub(super) fn skin_focus(&mut self, id: &str, cx: &mut Context<Self>) {
        cx.emit(WarRoomEvent::Focus(id.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_flashes_twice() {
        let at = |ms| blink(Duration::from_millis(ms));
        assert_eq!(at(0), Some(true));
        assert_eq!(at(250), Some(false));
        assert_eq!(at(450), Some(true));
        assert_eq!(at(650), Some(false));
        assert_eq!(at(800), None);
    }

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2380), "2,380");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn mix_and_fade() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.5), 0x808080);
        assert_eq!(fade(0x00FF66FF, 0.5), 0x00FF667F);
        assert_eq!(fade(0x00FF66FF, 2.), 0x00FF66FF);
    }

    #[test]
    fn tool_lines_keep_the_label_and_the_file_name() {
        assert_eq!(
            short_tool("Edit", "C:\\src\\focus.rs +2"),
            "Edit focus.rs +2"
        );
        assert_eq!(short_tool("", "a/b/c.rs"), "c.rs");
    }

    #[test]
    fn text_clips_and_wraps() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
        let w = wrap("one two three four five", 9, 2);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|l| l.chars().count() <= 9));
    }

    #[test]
    fn answer_keys_have_short_labels() {
        assert_eq!(key_label("Always allow"), "ALWAYS");
        assert_eq!(key_label("Deny this thing"), "DENY");
    }

    #[test]
    fn rates_are_short() {
        assert_eq!(rate_text(840.), "840");
        assert_eq!(rate_text(2400.), "2.4k");
        assert_eq!(rate_text(12000.), "12k");
    }

    #[test]
    fn the_feed_names_a_beacon_and_a_move() {
        let before = (SessionState::Working, "ui".to_string());
        assert_eq!(
            feed_event("claude@x", &before, SessionState::NeedsYou, "ui").as_deref(),
            Some("claude@x raised a beacon")
        );
        assert_eq!(
            feed_event("claude@x", &before, SessionState::Working, "daemon").as_deref(),
            Some("claude@x -> daemon")
        );
        assert_eq!(
            feed_event("claude@x", &before, SessionState::Working, "ui"),
            None
        );
    }
}
