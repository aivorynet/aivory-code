//! A block in the mailbox of a session: "Stop changing X until Y lands". The daemon denies the
//! edits of the receiver on the keys until a human overrides the block. A pane and the deep dive
//! show the open block as a banner with an Override button.

use aivory_protocol::coord::{MailItem, PingKind};
use gpui::prelude::*;
use gpui::{Div, FontWeight, SharedString, div, px, rgba};

use crate::ui::{self, OnClick};

/// The blocks of a mailbox that a human did not override yet.
pub fn open_blocks(items: &[MailItem]) -> Vec<MailItem> {
    items
        .iter()
        .filter(|m| m.kind == PingKind::Block && !m.resolved)
        .cloned()
        .collect()
}

/// `Blocked by checkout: stop changing the pay API until the merge lands`.
pub fn banner_text(item: &MailItem) -> String {
    let who = item.from.trim();
    let why = item.text.trim();
    match (who.is_empty(), why.is_empty()) {
        (true, true) => "Blocked".to_string(),
        (true, false) => format!("Blocked: {why}"),
        (false, true) => format!("Blocked by {who}"),
        (false, false) => format!("Blocked by {who}: {why}"),
    }
}

/// The colors of a banner (`0xRRGGBBAA`).
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub bg: u32,
    pub border: u32,
    pub text: u32,
    pub hover: u32,
}

/// The banner of one open block. `compact` clips the text to one line (micro panes).
pub fn banner(
    item: &MailItem,
    id: &str,
    colors: Colors,
    compact: bool,
    on_override: OnClick,
) -> Div {
    let text = banner_text(item);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(5.))
        .min_w(px(0.))
        .border_b_1()
        .border_color(rgba(colors.border))
        .bg(rgba(colors.bg))
        .text_color(rgba(colors.text))
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .when(compact, |d| {
                    d.overflow_hidden().whitespace_nowrap().text_ellipsis()
                })
                .font_weight(FontWeight::MEDIUM)
                .child(text),
        )
        .child(
            div()
                .id(SharedString::from(format!("override-{id}-{}", item.id)))
                .h(px(22.))
                .px(px(8.))
                .flex()
                .flex_none()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(rgba(colors.border))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |s| s.bg(rgba(colors.hover)))
                .tooltip(ui::tip("Let the session edit again"))
                .on_click(on_override)
                .child("Override"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: PingKind, resolved: bool, from: &str, text: &str) -> MailItem {
        MailItem {
            id: 1,
            from: from.into(),
            kind,
            key: "crates/pay/**".into(),
            text: text.into(),
            created_at: 0,
            delivered_at: None,
            resolved,
            count: 1,
            digest: false,
        }
    }

    #[test]
    fn only_open_blocks_count() {
        let items = vec![
            item(PingKind::Fyi, false, "a", "x"),
            item(PingKind::Block, true, "b", "done"),
            item(PingKind::Block, false, "c", "stop"),
        ];
        let open = open_blocks(&items);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].from, "c");
    }

    #[test]
    fn the_banner_names_the_sender_and_the_reason() {
        let b = item(
            PingKind::Block,
            false,
            "checkout",
            "stop changing the pay API",
        );
        assert_eq!(
            banner_text(&b),
            "Blocked by checkout: stop changing the pay API"
        );
        assert_eq!(
            banner_text(&item(PingKind::Block, false, "", "wait")),
            "Blocked: wait"
        );
        assert_eq!(
            banner_text(&item(PingKind::Block, false, "ops", "")),
            "Blocked by ops"
        );
        assert_eq!(
            banner_text(&item(PingKind::Block, false, "", "")),
            "Blocked"
        );
    }
}
