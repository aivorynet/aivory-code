# ADR 0012 — VCS abstraction on changesets and an operation log

Status: Accepted, 2026-10-21

## Context
The user wants Git, Mercurial, and the new Anthropic system. The Anthropic system is Scion: a
closed, server-side VCS with changesets in a write-ahead log and no public client. Agents make
many small changes, and each turn needs an exact record.

## Decision
- A `Vcs` trait with changesets (one per agent turn) and an operation log (each VCS action of
  the daemon can be undone). jj-lib is the design reference.
- Backends: **Git** (gix for reads, the `git` CLI for writes, push, worktrees), **Mercurial**
  (command server), **Jujutsu** (jj-lib, embedded), **Scion** (adapter slot; enabled when a
  client or API becomes public).
- Turn snapshots are commits under `refs/aivory/turns/<session>/<n>` (Git) or hidden
  changesets (Mercurial, jj). They never appear on user branches.

## Rejected alternatives
- **Git only:** breaks the Mercurial requirement and gives no model for Scion-style changesets.
- **jj as the only model:** jj lacks hooks, LFS, submodules, and partial clone support today.
- **Own VCS:** the team remote stays Git or Mercurial; an own VCS adds a migration.

## Consequences
- We cannot support Scion directly until Anthropic publishes a client. We say this in the docs
  and in the product.
- The ledger and the time machine work the same on all backends.
