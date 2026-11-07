# ADR 0017 — Open core, one-time Personal license, team subscription

Status: Superseded by ADR 0025 (2026-11-07). The Personal, Team, and Enterprise plans below are not active.

Status: Accepted (user decision on open core), 2026-10-21

## Context
The user wants a cheap product: a one-time payment, and a subscription only for team features.
The user chose open core like Orca (MIT), with closed team services. AIVory Code is an original
codebase; it contains no Orca code.

## Decision
- **MIT:** the daemon, the desktop and mobile clients, the protocol, the packs, the Kit engine,
  the CLI. Anyone can build and use them for free.
- **Personal (one-time payment):** signed and auto-updated builds, the store mobile app unlock,
  our hosted relay and push, settings sync between the user's devices.
- **Team (subscription per user and month):** hub, claims, conflict radar across members, shared
  index cache, merge queue, team inbox, SSO, audit export, outbound webhooks.
- **Runners:** metered by runner minutes.
- **Enterprise:** self-hosted hub, relay, and runners (Helm chart), yearly license.
- Repository: one private monorepo. CI exports the MIT paths to a public mirror by an explicit
  allowlist. `services/` has a proprietary license file and never leaves the private repo.

## Rejected alternatives
- **Fully closed product with a one-time price:** competes badly with free open tools (Orca has
  about 100k stars).
- **Feature limits in the MIT client (for example, 3 sessions):** anyone can remove them.
- **Two repositories (open and closed):** cross-repo changes are harder; the user asked for a monorepo.

## Consequences
- Revenue comes mainly from the team subscription.
- The export job must fail when a path outside the allowlist appears in the public mirror.

## Clarified 2026-10-21
- Two folders are not MIT: `services/` and `infra/`. Both hold closed team code (hub, push
  gateway, runner operator, deployment files). `services/` has the file `LICENSE-PROPRIETARY.md`.
  `infra/` follows it. Neither folder goes to the public mirror.
