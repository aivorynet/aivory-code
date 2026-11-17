//! Settings › Daemons and devices › Machines: the machine mesh (ADR 0026, step M7).
//!
//! The card lists the paired machines with their path (direct or relay), the round-trip time,
//! and the free session slots. "Invite a machine" copies a one-time link; "Join from the
//! clipboard" joins the mesh of another machine with its link.

use aivory_protocol::mesh::{Peer, PeerInvite, PeerJoinParams, PeerList, PeerPathKind, PeerRef};
use aivory_protocol::rpc::{Empty, method};
use gpui::prelude::*;
use gpui::{ClipboardItem, Context, Div, FontWeight, div, px, rgb};

use super::{SettingsScreen, Tone, badge, button};
use crate::theme::*;

/// The state of the Machines card.
#[derive(Default)]
pub(super) struct Machines {
    pub list: Option<PeerList>,
    pub invite: Option<PeerInvite>,
    /// The last error of a mesh call (the daemon runs without `--iroh`, a bad link).
    pub error: Option<String>,
    pub busy: bool,
}

/// The badge of a peer: its path when online.
pub(super) fn peer_badge(p: &Peer) -> (&'static str, Tone) {
    match (p.online, p.path) {
        (true, PeerPathKind::Direct) => ("direct", Tone::Ok),
        (true, PeerPathKind::Relay) => ("relay", Tone::Warn),
        (true, PeerPathKind::None) => ("online", Tone::Ok),
        (false, _) if p.last_error.is_some() => ("offline", Tone::Error),
        (false, _) => ("offline", Tone::Off),
    }
}

/// The detail line of a peer row.
pub(super) fn peer_detail(p: &Peer) -> String {
    let mut parts = vec![if p.os.is_empty() {
        "unknown OS".to_string()
    } else {
        p.os.clone()
    }];
    if let Some(rtt) = p.rtt_ms.filter(|_| p.online) {
        parts.push(format!("{rtt} ms"));
    }
    if let Some(st) = &p.state {
        parts.push(format!("{} of {} slots free", st.free(), st.max_running));
        if !st.providers.is_empty() {
            parts.push(st.providers.join(", "));
        }
    }
    if let (false, Some(e)) = (p.online, &p.last_error) {
        parts.push(e.clone());
    }
    parts.join(" · ")
}

impl SettingsScreen {
    /// Read the peer list.
    pub(super) fn load_peers(&mut self, cx: &mut Context<Self>) {
        let call = self.model.read(cx).call::<method::PeersList>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.machines.list = Some(list);
                        this.machines.error = None;
                    }
                    Err(e) => this.machines.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn invite_machine(&mut self, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::PeersInvite>(Empty {}, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(invite) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(invite.url.clone()));
                        this.machines.invite = Some(invite);
                        this.machines.error = None;
                    }
                    Err(e) => this.machines.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn join_machine(&mut self, cx: &mut Context<Self>) {
        let link = cx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .unwrap_or_default();
        if !link.trim().starts_with("aivory://peer?") {
            self.machines.error =
                Some("Copy the invite link of the other machine first.".to_string());
            cx.notify();
            return;
        }
        self.machines.busy = true;
        let call = self.model.read(cx).call::<method::PeersJoin>(
            PeerJoinParams {
                link: link.trim().to_string(),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.machines.busy = false;
                match result {
                    Ok(_) => {
                        this.machines.error = None;
                        this.load_peers(cx);
                    }
                    Err(e) => this.machines.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_machine(&mut self, id: String, cx: &mut Context<Self>) {
        let call = self
            .model
            .read(cx)
            .call::<method::PeersRemove>(PeerRef { id }, cx);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.machines.error = Some(format!("{e:#}"));
                }
                this.load_peers(cx);
            });
        })
        .detach();
    }

    pub(super) fn machines_card(&self, cx: &mut Context<Self>) -> Div {
        let m = &self.machines;
        let mut card = super::card();
        let peers = m.list.as_ref().map(|l| l.peers.as_slice()).unwrap_or(&[]);
        if peers.is_empty() {
            card = card.child(
                div()
                    .px(px(20.))
                    .py(px(14.))
                    .text_color(rgb(tk().ink_3))
                    .child("No other machine is paired."),
            );
        }
        for (i, p) in peers.iter().enumerate() {
            let id = p.id.clone();
            let (text, tone) = peer_badge(p);
            let name = if p.name.is_empty() {
                p.id[..8.min(p.id.len())].to_string()
            } else {
                p.name.clone()
            };
            card = card.child(Self::device_row(
                i == 0,
                "icons/device-daemon.svg",
                name,
                peer_detail(p),
                badge(text, tone),
                button(format!("peer-remove-{}", p.id), "Remove", false).on_click(
                    cx.listener(move |this, _, _, cx| this.remove_machine(id.clone(), cx)),
                ),
            ));
        }
        let mut foot = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(20.))
            .py(px(14.))
            .border_t_1()
            .border_color(gpui::rgba(super::ROW_LINE));
        if let Some(invite) = &m.invite {
            foot = foot.child(
                div()
                    .font_family(tk().mono)
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().ink_3))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(format!("link copied, 10 minutes · {}", invite.url)),
            );
        }
        if let Some(error) = &m.error {
            foot = foot.child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_CAPTION))
                    .text_color(rgb(tk().e_text))
                    .child(error.clone()),
            );
        }
        foot = foot.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(
                    button("peer-invite", "Invite a machine", true)
                        .on_click(cx.listener(|this, _, _, cx| this.invite_machine(cx))),
                )
                .child(
                    button(
                        "peer-join",
                        if m.busy {
                            "Joining…"
                        } else {
                            "Join from the clipboard"
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.join_machine(cx))),
                )
                .child(
                    button("peer-refresh", "Check now", false)
                        .on_click(cx.listener(|this, _, _, cx| this.load_peers(cx))),
                ),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(crate::ui::ts(crate::theme::T_BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Machines"),
            )
            .child(card.child(foot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivory_protocol::mesh::NodeState;

    fn peer() -> Peer {
        serde_json::from_value(serde_json::json!({
            "id": "ab".repeat(32), "name": "box", "os": "linux", "pairedAt": 1,
        }))
        .unwrap()
    }

    #[test]
    fn a_peer_row_names_path_time_and_slots() {
        let mut p = peer();
        assert_eq!(peer_badge(&p).0, "offline");
        p.online = true;
        p.path = PeerPathKind::Relay;
        p.rtt_ms = Some(42);
        p.state = Some(NodeState {
            running: 1,
            max_running: 4,
            providers: vec!["claude".into()],
            ..NodeState::default()
        });
        assert_eq!(peer_badge(&p), ("relay", Tone::Warn));
        assert_eq!(
            peer_detail(&p),
            "linux · 42 ms · 3 of 4 slots free · claude"
        );
        p.online = false;
        p.last_error = Some("timeout".into());
        assert_eq!(peer_badge(&p), ("offline", Tone::Error));
        assert!(peer_detail(&p).ends_with("timeout"));
    }
}
