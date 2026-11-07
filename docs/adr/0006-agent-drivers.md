# ADR 0006 — Agent drivers: native first, ACP generic, TUI mode

Status: Accepted, 2026-10-21

## Context
The product must support Claude, Codex, Gemini, and all other agents out of the box, with the
user's own subscriptions. ACP covers about 43 agents, but the Claude and Codex ACP adapters wrap
vendor SDKs (policy risk), and ACP has no session list or fork.

## Decision
1. Native drivers for the big three: Claude (`claude -p`, stream-json in and out), Codex
   (`codex app-server`, fallback `codex exec --json`), Gemini (`--acp`, fallback stream-json).
2. A generic ACP driver for every registry agent.
3. A TUI mode: the real CLI in a PTY pane, plus hooks and a transcript tail for structure. This
   keeps slash commands, plan mode, and other interactive features.
4. No own agent loop in v1. API-key models come through open ACP agents (OpenCode, Goose).

## Rejected alternatives
- **ACP only:** loses Claude-specific features and carries the SDK policy risk.
- **PTY only:** no structure for the ledger, approvals, or the phone.
- **Own agent loop for all models:** large scope; competes with the agents that we drive.

## Consequences
- A driver test suite runs nightly against the latest CLI versions.
- Each driver declares its capabilities (resume, fork, permissions, cost data).

## Amended 2026-10-21 (first version)
- Only Claude runs live. The driver starts `claude -p` with stream-json in and out, `--mcp-config`,
  and `--permission-prompt-tool mcp__aivory__permission` (ADR 0008).
- Codex and Gemini only resume a native session. Codex runs `codex exec --json ... resume <id> -`.
  Gemini runs `gemini --session-file <file> -o stream-json --prompt=<text>`. The daemon does not
  use `codex app-server` or `--acp`. Live runs for both stay open work (both CLIs need a new login).
- `session/fork` works for Claude only. It forks at the end of the session. Codex and Gemini
  answer `NOT_AVAILABLE`.
- There is no generic ACP driver and no TUI mode yet. They stay as later work.
- `aivory-agents::detect` finds the installed CLIs (claude, codex, gemini, and others) on the PATH.
