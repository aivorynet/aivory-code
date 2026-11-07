# ADR 0013 — The hub never stores code; events are end-to-end encrypted

Status: Accepted, 2026-10-21

## Context
Teams run hundreds of sessions and must know what the others change. Customers do not want
their code on a third-party server. The service must be cheap to run.

## Decision
- Code moves through the team's own Git or Mercurial remote: hidden WIP refs
  (`refs/aivory/wip/<user>/<session>`), maximum one push per minute per session. Teammates fetch
  objects peer to peer over iroh when they are online.
- The hub routes events, claims, and index segments. All payloads are encrypted with a team
  key. MLS (OpenMLS) manages the team keys; a member who leaves causes a new key.
- Routing keys are HMACs of paths, symbols, and test names under the team key. The hub matches
  claims and subscriptions by routing key without seeing names.

## Rejected alternatives
- **Hub stores code (cloud IDE model):** cost, trust, and compliance problems.
- **Plain TLS without end-to-end encryption:** the hub sees code structure and names.
- **Pure peer-to-peer without a hub:** no durable mailbox, no claims when members are offline.

## Consequences
- Conflict detection that needs code (trial merges) runs on the members' daemons or the team
  runners, not on the hub.
- Hub cost scales with events, not with code size.
