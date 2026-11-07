# ADR 0002 — Rust for the core and the team services

Status: Accepted, 2026-10-21

## Context
The core parses, indexes, runs VCS operations, and holds hundreds of sessions. The phone needs
the same parsing, crypto, and network code. Accounts and billing already exist in aivory-guard
(Python).

## Decision
Rust for the daemon, index, Kit engine, VCS layer, protocol, hub, and relay. Accounts, teams,
licenses, and billing stay in aivory-guard (Python) behind a small HTTP API.

## Rejected alternatives
- **Kotlin/JVM (reuse Genie and PSI):** JVM start-up and memory break the budgets; no good path to iOS.
- **Go:** good for services, but its tree-sitter, iroh, jj-lib, and GPUI support is weaker.
- **Rewrite billing in Rust:** duplicates working code.

## Consequences
- One language from the daemon to the phone (through flutter_rust_bridge).
- Two service stacks in production: Rust (hub, relay) and Python (aivory-guard).
