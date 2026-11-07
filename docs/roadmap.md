# Roadmap

Status on 2026-11-07.
Legend: done, in progress, open.

## Steps

| Step | What | Status | Open part |
|---|---|---|---|
| P1 | Contract and demo data: `aivory-protocol` types, JSON-RPC surface, layout helpers, demo data, fixtures, JSON Schema | done | none |
| P2 | Daemon war-room engine: part map, live runs, session state, transcripts, activity, autopilot, SQLite store, spans and diff | done | none |
| P3 | Desktop app (GPUI). P3a shell, P3b war room, P3c session deep dive, P3d inbox, P3e Tasks, P3f Repos, P3g Settings are done | done | small open items in `nextsession.txt` |
| P4 | Phone app (Flutter). P4a shell, P4b Focus, Sessions, Stream, P4c Approval, Diff, Voice, Me, and P4d LAN link (WebSocket client, QR or link pairing, token in the OS keystore, live feed, 5 s undo window) are done (ADR 0021) | done | test on a real phone (camera scan, keystore, biometrics, speech) |
| P5 | Remote access and push: iroh QUIC, own relay, encrypted APNs and FCM push, offline answer queue, device keys (ADR 0015) | done (not deployed) | deploy the relay and the push gateway; Firebase config; APNs service extension (iOS) |
| P6 | Repo documents: README, AGENTS, LICENSE, this roadmap, skeleton folders, design documents aligned to the accepted design, ADR 0020 and 0021, consistency review and fixes | done | the LICENSE holder name (placeholder) |
| P8 | Open live work: Codex and Gemini live runs, price table and cost estimate, usage windows, task priority, fork at a turn | done | a test with real Codex and Gemini logins |
| P9 | Desktop open items: composer attach, screen capture, voice dictation, rotated Done strip | done | voice needs Windows online speech recognition |
| P10 | Code intelligence, local scope (design 03, 04, 05): syntax, index, diagnostics and LSP pool, turn ledger and snapshots, claims, conflict radar, mailbox, lens UI | done | the lens and radar design is proposed, not accepted; team scope (hub) |
| P7 | Release and publication flow: CI, installers, version and change log, public mirror of the MIT paths (docs/design/15-release.md, .gitlab-ci.yml) | files done; never ran | the dry run on tag v0.0.0-dry needs the CI runners and variables; mirror host; macOS runner |

## First version

The first version is done when P1 to P4, P6, and P7 pass their gates. P5 is not part of it.

The first version contains:

- The daemon `aivoryd`. It lists the native sessions of Claude Code, Codex CLI, and Gemini CLI,
  derives their state, and serves the war room over JSON-RPC.
- Live runs of Claude Code under the daemon: start, send, answer a permission, plan, or
  question, pause, stop, park, and fork (forks at the end of the session).
- Autopilot: park idle sessions and start queued tasks. It is off by default.
- The part map `.aivory/map.toml`.
- The desktop app with the six screens of the accepted design: War room, Session, Inbox,
  Tasks, Repos, Settings.
- The phone app with the seven screens of the accepted design: Focus, Sessions, Stream,
  Approval, Diff, Voice, Me. It connects to the daemon in the local network (ADR 0021).
- Installers and the release flow of P7.

The first version does not contain:

- Remote access outside the local network, and push (P5).
- The code lens, the turn ledger, and the conflict radar (`docs/design/03-code-intelligence.md`,
  `05-vcs-ledger.md`, `04-coordination-sync.md`).
- Team services: a hub, shared claims, and a team inbox, if companies ask for them.
- Language packs, project templates, and the Genie upgrade (`packs/`, `templates/`,
  `plugins/genie/`).

## Open live work

- An answer for an ask of a session that runs in a terminal. This is not possible, because the
  ask belongs to that process.
- Codex and Gemini runs are tested only with fake CLIs. The two CLIs need a new login first.
- Fork for Gemini (`NOT_AVAILABLE`).
- A Linux and macOS test of the Unix socket code.

## Open decisions

- The host and the organization of the public mirror.
- A macOS runner for the signed desktop build and the iOS build.
- The `Needs you` group spans the full row (accepted design). The hard rule "no empty areas"
  asks for packing. Keep the design or pack the other groups next to it.
- A grace time for the rule that every open tool call is a permission ask.
- The copyright holder name in `LICENSE`.
- Accept or change the proposed lens and radar design (`docs/design/screens/README.md`
  section 8).
- The license exceptions in `deny.toml`: webpki-roots (CDLA-Permissive-2.0), cbindgen
  (MPL-2.0, macOS build tool).
- Turn snapshots (`ledger.snapshots`, default on) write hidden refs `refs/aivory/turns/*`
  into each repo that the live daemon scans.
