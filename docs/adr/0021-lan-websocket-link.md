# ADR 0021 — LAN WebSocket link and QR pairing for the phone (first version)

Status: Accepted, 2026-10-21. Refines [ADR 0015](0015-personal-remote-iroh.md) for the first
version.

## Context
The phone must reach the daemon. ADR 0015 plans iroh (QUIC), our relay, and push. That work is
P5 and comes after the first version. The first version needs a phone link on the local network
with no server of ours and little code. The phone app is pure Dart (ADR 0004), and Dart has
WebSocket in its standard library.

## Decision
The first version links the phone to the daemon with JSON-RPC over WebSocket on the local network.

- **Start:** `aivoryd serve --remote <addr>`, for example `--remote 0.0.0.0:7420`. The daemon
  serves the same JSON-RPC methods as on the pipe. The code is `crates/aivoryd/src/remote.rs`
  (plain TCP, own RFC 6455 framing).
- **Framing:** one text frame is one JSON-RPC message, as one line is on the pipe (ADR 0005).
  The limit of a frame is 8 MB.
- **Pairing:**
  1. The desktop calls `devices/pair`. The daemon returns a code `AIV-xxxxxx` that is valid for
     10 minutes, and the URL `aivory://pair?host=<lan ip>:<port>&code=<code>` for a QR code.
  2. The phone connects once to `ws://<host>:<port>/rpc?code=<code>&device=<name>&os=<os>`.
  3. The daemon sends the notification `device/paired` with a device token (64 hex characters).
  4. Later connections use `?token=<token>`. A wrong token gets HTTP 401.
- **Token storage:** the daemon keeps only the SHA-256 of each token, in `devices.json` next to
  `aivory.db`. The phone keeps the token.
- **No TLS** on the local network. The link is for a trusted network only.

## Rejected alternatives
- **iroh from the start:** it needs the relay, the push service, and the device registry in
  aivory-guard. This is too much work for the first version (P5).
- **A WebSocket relay on our servers:** ADR 0015 rejects it, because the relay sees all
  metadata. This ADR uses no server of ours.
- **TLS with a self-signed certificate:** the phone needs a trust step for each daemon. The
  first version skips it and states the limit.
- **A fixed password instead of pairing:** a password cannot be revoked for one phone.

## Consequences
- The phone works only in the local network until P5. Settings and the docs say so.
- P5 (iroh) replaces this link outside the local network. The JSON-RPC methods stay the same.
  ADR 0015 still holds for the relay.
- Anyone in the local network can try to connect. The code is valid for 10 minutes and works
  once. A paired phone can be removed with `devices/remove`.
- The Flutter side (`WebSocketRepository`, QR scan) is open. The daemon side is done and checked.
