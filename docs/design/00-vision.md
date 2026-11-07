# 00 — Vision

> **Status 2026-10-21.** The accepted design is in [screens/README.md](screens/README.md). It wins
> where this document differs. Replaced text carries a short note with a link. The steps and the
> first version are in [../roadmap.md](../roadmap.md).

## The product in one sentence

AIVory Code is a local-first mission control in which a developer directs many coding agents
of any provider, and the agents coordinate with each other on one shared, IDE-grade model of
the code.

## The shift

The old loop: "I write code, a tool helps me." The new loop has two levels:

1. **The human orchestrates agents.** The human sets goals, approves risk, and judges results.
   The human reads more code than the human writes.
2. **The agents orchestrate each other.** An agent splits a task, asks another session a
   question, claims the code that it changes, hands work to a different model, and reviews the
   work of another agent.

An IDE puts the editor in the center. AIVory Code puts the **war room** and the **attention of
the human** in the center. The war room shows all codebases and all sessions at once; the
sessions that need the human are on top (see [screens/README.md](screens/README.md), section 1).
The code view stays important, but it is a lens for understanding and review, not the main place
where code is typed.

## Design principles

1. **Local first.** Everything works on one machine without our servers, also offline with
   local models. Our servers add reach (phone, team), not basic function.
2. **Bring your own agent.** Claude, Codex, Gemini, and each ACP agent work out of the box, with
   the user's own installed CLI and subscription. We never resell tokens.
3. **One model of the code for everyone.** The human and all agents query the same index. An
   agent asks "who calls `foo`" and gets an exact answer, not a grep.
4. **Coordination over isolation.** Worktrees isolate files. Isolation alone lets agents
   collide later. AIVory Code tells each session early what the other sessions change.
5. **Attention is the scarce resource.** The system ranks every event. It interrupts the human
   only when the human must act.
6. **Every change has a cause.** Each line links to a turn, a prompt, a model, and a cost.
7. **Fast or absent.** A feature that makes start-up or typing slow does not ship. Budgets are
   in `01-architecture.md`.
8. **Open core.** The clients and the protocol are MIT. Team services are a paid subscription.

## Questioned assumptions

| Old assumption | AIVory Code answer |
|---|---|
| The editor is the center of the tool. | The war room is the center: session panes grouped by codebase, with "Needs you" on top. Inbox and Tasks are separate screens ([screens/README.md](screens/README.md)). The code lens opens from a diff or a ping, after the first version. |
| Chat is the main UI of an agent tool. | A session pane is a terminal-style transcript. The main unit is the turn: a changeset with prompt, diff, tests, and cost. The session deep dive shows turns as spans ([screens/README.md](screens/README.md), section 2). |
| One agent, one provider. | A provider is chosen per session. Work moves between providers with a handoff packet. |
| Agents must be isolated from each other. | Agents are isolated in files but connected in knowledge: claims, change feed, mailbox. |
| The human approves each step. | Policies approve routine steps. The human approves risk. Approvals reach the phone. |
| Files are the unit of change. | Symbols are the unit of conflict. Turns are the unit of history. |
| An IDE indexes code for the human. | AIVory Code indexes code for the human and for the agents (through MCP), which also saves tokens. |
| A team tool must store the code on its servers. | The hub never stores code. Code moves through the team's own Git remote. Events are end-to-end encrypted. |
| Many sessions need many processes. | Most sessions are parked (no process) and resume on demand. The autopilot limits the running sessions (default 14, see `02`). |
| A native app needs a different codebase per platform. | One Rust core. Two thin UIs: GPUI on desktop, Flutter on mobile. |
| Grammars are written once per tool. | A language pack uses Grammar-Kit BNF and JFlex files without changes, so the flexible-* grammars port directly. |

## Pillars

1. **Orchestrate** — tasks, sessions, playbooks, handoff, agent-to-agent tools (`02`).
2. **Understand** — the code lens: structure highlight, hover docs, navigation, refactors,
   diagnostics, quality rules (`03`, `14`).
3. **Coordinate** — claims, conflict radar, mailbox, merge queue, team sync (`04`).
4. **Trace** — the turn ledger, blame by prompt, file timeline over Git, Mercurial, and jj (`05`).
5. **Focus** — the attention layer (`06`).
6. **Anywhere** — desktop, mobile, JetBrains, CLI; local, SSH, team runners (`07`, `08`, `09`, `13`).

## Glossary (one word, one meaning)

| Word | Meaning |
|---|---|
| Daemon | The `aivoryd` process. It owns sessions, the index, the ledger, and the links to the hub and relay. |
| Client | A UI or tool that talks to the daemon: desktop, mobile, Genie, the `aivoryd` command line. |
| Agent | A coding agent program, for example Claude Code, Codex CLI, Gemini CLI. |
| Provider | The company or model family behind an agent (Anthropic, OpenAI, Google, ...). |
| Session | One conversation of one agent, with its native transcript and its workspace. |
| Native session | A session in the agent's own store (`~/.claude`, `~/.codex`, `~/.gemini`), also one started outside AIVory Code. |
| Turn | One prompt and the agent work that follows, until the agent waits again. |
| Changeset | The recorded result of a turn: file changes plus prompt, model, cost, and test result. |
| Task | A goal with acceptance criteria. One or more sessions work on a task. |
| Playbook | A reusable graph of tasks and gates, for example "implement, review by another model, fix, test". |
| Gate | A check that must pass before a task continues: tests, review, Guard scan, human approval. |
| Workspace | The files that a session can change: the main checkout, a worktree, a container, or a runner. |
| Claim | A lease of a session on a path, a symbol, or a test. |
| Ping | A message to a session from another session or from the system. |
| Handoff packet | The provider-neutral summary that moves a task from one session to another. |
| Focus score | The rank of an event for one human. The first version uses a simple rule: "Needs you" first, the oldest wait first. |
| War room | The main desktop screen: the codebases band, the session grid, and the quiet section. |
| Pane | One session in the war room, with a terminal look and square corners. |
| Part | A named area of a codebase, for example `payment` or `ui`. The file `.aivory/map.toml` lists the parts. |
| Lens | The code view: highlight, hover, navigation, diagnostics, refactors. It comes after the first version. |
| Pack | A language pack: grammar, lexer, rules, and logic for one language. |
| Hub | The team service that routes encrypted events, claims, and index segments. |
| Relay | The service that connects a phone to a daemon when no direct path exists. |
| Runner | A remote machine that runs sessions for a team. |

## Traceability (requirements → design)

| Req | Requirement | Where |
|---|---|---|
| R1 | IDE-grade code view, hover, navigation, rename, simplify | `03` §Lens features; `07` §Code lens |
| R2 | Git history and prompt-to-change log | `05` §Turn ledger, §File timeline |
| R3 | Multi-window, many sessions visible | `screens/README.md` §1 War room; `07` §Windows (later) |
| R4 | Grammar, lexer, parser; errors, warnings, quality | `03` §Diagnostics pipeline, §Quality rules; `14` |
| R5 | All providers, per session, handoff | `02` §Drivers, §Handoff |
| R6 | Orchestration; agents orchestrate each other | `02` §Tasks and playbooks, §Agent tools (MCP), §Lead sessions |
| R7 | Project creation per language | `10` |
| R8 | Desktop and full mobile app | `07`, `08` |
| R9 | Local backend, remote sessions, own infra, webhooks | `01`, `09` |
| R10 | Minimal start-up and overhead | `01` §Performance budgets |
| R11 | JetBrains-grade tree index, stubs, caching | `03` §L1 Stubs, §L2 Segments and session deltas |
| R12 | Hundreds of sessions, sync, pings | `04` |
| R13 | Layer that organizes what is important | `06` |
| R14 | Git, Mercurial, Anthropic Scion | `05` §Backends |
| R15 | Genie upgrade | `13` |
| R16 | Reuse aivory-guard and infrastructure | `09`, `12` |
| R17 | Cheap: one-time payment, team subscription | `12` |
| R18 | Replace Claude Code and Claude mobile with native sessions | `02` §Native sessions; `08` |
| R19 | Grammar-Kit support, port of flexible-* grammars and logic | `14` |
| R20 | Open core, original codebase | `12` §License; ADR 0017 |

## Non-goals for v1

- A general text editor for long manual coding sessions. The lens edits, but it does not try to
  replace a full IDE for typing all day. Genie covers that inside JetBrains IDEs.
- Our own agent loop with direct model APIs. Open ACP agents (OpenCode, Goose) cover API-key
  models in v1.
- Hosting code. The team's Git remote stays the source of truth.
- Chat access to claude.ai conversations. Only Claude Code sessions are in scope.
