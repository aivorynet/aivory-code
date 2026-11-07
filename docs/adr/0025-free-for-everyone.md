# ADR 0025 — Free for everyone, MIT, coffee link

Status: Accepted, 2026-11-07. Replaces the business parts of ADR 0017.

## Context
ADR 0017 planned a one-time Personal license, a Team subscription, and an Enterprise license. The
owner wants everyone to use the app for free.

The app is local-first. The user pays the agent vendors, and the cost per user is near zero. Direct
alternatives are free or MIT, so a one-time price has no anchor. A signed build is a weak reason to pay.
Adoption is the asset now, and a license gate cuts adoption and adds code to maintain.

## Decision
- Desktop app, daemon, and phone app are MIT, with no license check and no paid feature.
- Paid themes do not exist. This closes the paid-theme question of ADR 0024.
- Support comes through an optional coffee: one quiet note per app start, with an off switch in
  Settings > Notifications. The note never shows while a decision waits. Store builds of the phone
  app have no payment link.
- The push gateway stays a private hosted service. The MIT apps do not depend on its code.
- The public repo starts from a clean export (`scripts/export-mirror.sh`), not from the private history.

## Later
Test a paid hosted Team hub only when at least 3 companies ask for one. The local app stays free.
