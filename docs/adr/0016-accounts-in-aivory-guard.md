# ADR 0016 — Accounts, teams, licenses, and billing in aivory-guard

Status: Accepted, 2026-10-21

## Context
aivory-guard already has Keycloak login, organizations, seats, Stripe, Apple in-app purchase,
Ed25519 offline licenses, and webhooks. It has no teams, only the role `Admin`, no device
registry, and no public-key directory.

## Decision
Extend aivory-guard:
- `product_type='code'` for licenses and seats.
- New tables: `team`, `team_members(team_id, username, role)`, `devices`, `public_keys`,
  `outbound_webhooks`.
- Roles: Owner, Admin, Member, Viewer, Bot.
- The Personal license is an Ed25519 offline token (the `slaide_pro.py` flow), so the desktop
  checks it without a network.
- Stripe for desktop purchases and team subscriptions; Apple in-app purchase and Google Play
  billing for the mobile unlock.

## Rejected alternatives
- **A separate account service for AIVory Code:** duplicates login, billing, and tax handling.
- **Online-only license checks:** breaks the local-first principle.

## Consequences
- The hub and the relay validate Keycloak tokens or device keys against aivory-guard.
- Database migrations in aivory-guard come after the first version (`docs/roadmap.md`, P5 and the team services).
