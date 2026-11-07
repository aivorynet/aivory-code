# ADR 0005 — Link protocol: protobuf frames with state sync

Status: Accepted, 2026-10-21

## Context
Four client kinds (desktop, mobile, Genie, CLI) in three languages (Rust, Dart, Kotlin). Phones
lose the connection often. The lens sends many small binary payloads.

## Decision
Protobuf messages in length-prefixed frames over named pipe, UDS, or QUIC. Views use a snapshot,
numbered deltas, and a resume cursor. A canonical JSON form (JSON-RPC 2.0) serves the CLI,
webhooks, logs, and the first version.

## Rejected alternatives
- **JSON-RPC only (like ACP, MCP, LSP):** simple and readable, but slow for highlight spans and
  index results, and it has no state sync.
- **gRPC:** HTTP/2 overhead on local links and a heavy Dart and Kotlin stack.
- **Cap'n Proto:** fast, but weak Dart support.

## Consequences
- Code generation for Rust (prost), Dart, and Kotlin.
- The first version uses the JSON form only (see Amended). The binary form comes later.

## Amended 2026-10-21 (first version)
- The first version uses JSON-RPC 2.0 only. There is no protobuf. On the named pipe and the Unix
  socket, one message is one line (newline-delimited JSON). On WebSocket (ADR 0021), one
  message is one text frame.
- The contract is the crate `aivory-protocol` (Rust types, camelCase JSON) and the fixtures and
  the JSON Schema in `proto/`. There is no `aivory-link` crate. Dart and Kotlin clients test
  against the fixtures.
- State sync works as decided. `warroom/subscribe` returns a snapshot or deltas. Then
  `warroom/delta` notifications carry a sequence number. A cursor and an epoch (new at each
  daemon start) resume a feed.
- Protobuf and binary payloads stay open. They come with the code lens, when highlight spans
  need them.
