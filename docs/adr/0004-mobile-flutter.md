# ADR 0004 — Mobile UI on Flutter with the Rust core

Status: Accepted (user decision), 2026-10-21

## Context
The phone app must replace the Claude Code part of Claude mobile: watch sessions, chat, approve,
review diffs, browse code with hover, and start tasks. It needs iroh, end-to-end crypto, and
syntax highlight.

## Decision
Flutter for the UI. The Rust core (`aivory-ffi`) through flutter_rust_bridge gives tree-sitter
highlight, the Link client, iroh, and MLS crypto. The app reuses the OIDC, push, and voice parts
of `aivory_flutter`.

## Rejected alternatives
- **Pure Dart client:** we must rewrite crypto, iroh, and highlight in Dart.
- **Tauri mobile:** weak mobile support.
- **Native Swift and Kotlin apps:** two more codebases.

## Consequences
- The phone does not run agents. It is a client of one or more daemons.
- The Rust core must build for iOS and Android in CI.

## Amended 2026-10-21 (first version)
- The phone app is pure Dart. There is no Rust core and no `aivory-ffi` crate.
- The app speaks the same JSON-RPC surface as the desktop app. The transport is WebSocket on
  the local network (ADR 0021). Until the link is finished, the app reads the demo fixtures.
- The app reuses the BLoC and go_router scaffolding of `aivory_flutter`. Voice comes with
  P4c (`speech_to_text`). OIDC and push come with P5.
- The Rust core (flutter_rust_bridge, tree-sitter highlight, iroh, MLS) stays a later option.
  It becomes useful with P5 and the code lens. Decide then if Dart is not enough.
