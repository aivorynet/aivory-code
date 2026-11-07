# ADR 0001 — A daemon owns the sessions

Status: Accepted, 2026-10-21

## Context
Sessions must continue when the desktop UI closes. The phone, Genie, and the CLI must see the
same sessions. Orca and Conductor keep sessions inside the UI process.

## Decision
`aivoryd` (Rust) owns all sessions, agent processes, the index, and the ledger. A client starts
it on demand. It stops after 30 minutes without active sessions or clients. All UIs, also the
desktop UI, are clients of the Link protocol.

## Rejected alternatives
- **Sessions in the UI process:** simple, but sessions stop with the UI, and the phone cannot reach them.
- **Desktop embeds the core and serves the other clients:** the desktop becomes a server, and a
  UI crash stops all agents.

## Consequences
- One more process. Budget: start ≤ 50 ms, idle ≤ 60 MB.
- The protocol must be fast enough for the code lens (binary payloads, visible range only).

## Amended 2026-10-21 (first version)
- The user starts the daemon by hand with `aivoryd serve`. No client starts it on demand yet.
- The desktop app starts only an in-process demo server (`aivory --demo`).
- The daemon runs until a client calls `daemon/shutdown`. There is no idle stop yet.
- The target stays: start on demand, and stop after 30 minutes without sessions and clients.
