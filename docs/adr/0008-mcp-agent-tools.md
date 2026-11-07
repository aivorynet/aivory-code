# ADR 0008 — One MCP endpoint gives agents IDE and coordination tools

Status: Accepted, 2026-10-21

## Context
Agents use grep and full-file reads to understand code. This costs tokens and misses facts.
Agents also need tools to coordinate. Each tool definition costs context in every session.

## Decision
The daemon serves one local HTTP MCP endpoint. Each session gets its own token, which limits
what the agent may see and do. Maximum 10 tools: `symbols`, `references`, `rename`,
`diagnostics`, `claim`, `inbox`, `task`, `handoff`, `ask_session`, `status`. A matching `aiv`
CLI gives the same functions to agents that prefer the shell.

## Rejected alternatives
- **Many small tools:** too much context per session.
- **CLI only:** no structured results; agents parse text.

## Consequences
- Tool results are compact and cite file and line, so agents read less.
- Messages from other agents arrive as data, never as instructions (ADR 0019, `11-security.md`).

## Amended 2026-10-21 (first version)
- The endpoint serves one tool, `permission`. Each live run gets its own HTTP MCP server on
  `127.0.0.1` with a random port and a random bearer token. Claude calls the tool through
  `--permission-prompt-tool mcp__aivory__permission`. The call stays open until the user answers.
  Plan approval and questions use the same tool.
- The other nine tools (`symbols`, `references`, `rename`, `diagnostics`, `claim`, `inbox`,
  `task`, `handoff`, `ask_session`, `status`) come later, with the code lens and the
  coordination layer. The limit of 10 tools stays.
- There is no `aiv` CLI. The command line is `aivoryd` (`list`, `show`, `resume`, `serve`,
  `call`, `map`). An `aiv` alias is open.
