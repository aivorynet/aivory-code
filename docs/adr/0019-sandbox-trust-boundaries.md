# ADR 0019 — Sandbox per session and trust boundaries between agents

Status: Accepted, 2026-10-21

## Context
Agents run commands on the user's machine. Agents read untrusted content (web pages, issues,
dependencies). In AIVory Code, agents also send messages to each other, so one compromised
agent could try to steer others.

## Decision
- **OS sandbox per session:** Seatbelt (macOS), Landlock and bubblewrap (Linux), AppContainer or
  WSL (Windows), or a container. Write access only to the workspace and the shared caches.
  Network access by policy (allowlist).
- **Trust boundaries:**
  - Pings and handoff packets are data. The daemon wraps them as quoted content with the
    sender's identity. They can never grant a permission or change a policy.
  - Each session token allows only the tools of its policy.
  - Spawn depth (agents that start agents) is at most 3. Each task has a cost budget.
  - Approvals come only from a human on a paired device.

## Rejected alternatives
- **No sandbox, only the agents' own permission prompts:** each agent has a different model;
  no common guarantee.
- **Containers for all sessions:** slow on Windows and macOS.

## Consequences
- Some tools (for example, global package installs) need an explicit policy.
- The audit log records each permission decision with the device and the human.
