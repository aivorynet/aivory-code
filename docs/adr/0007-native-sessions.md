# ADR 0007 — Native sessions are first-class

Status: Accepted, 2026-10-21

## Context
The user wants to replace Claude Code and keep its sessions, also the sessions started in a
terminal. Claude keys sessions by working directory.

## Decision
- Read-only, tolerant parsers for `~/.claude/projects`, `~/.codex/sessions`, `~/.gemini/tmp`.
- Each native session appears in the war room with its provider, cwd, title, branch, and time.
- Resume runs the native CLI in the original cwd. A fork into a new worktree uses
  `--fork-session` (Claude) or `thread/fork` (Codex).
- The daemon records the CLI version per session. A format change never deletes or rewrites a store.

## Rejected alternatives
- **Own sessions only:** loses the user's history and breaks the "replace" goal.
- **Import into our own format:** two copies drift apart.

## Consequences
- The first version lists and resumes the sessions of all three agents. A live tail of
  transcripts that a terminal started works through the scanner (every 2 s). Resume of Codex
  and Gemini needs a working login. Fork works for Claude only (ADR 0006).
