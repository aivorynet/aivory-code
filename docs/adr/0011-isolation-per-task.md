# ADR 0011 — Isolation is chosen per task

Status: Accepted, 2026-10-21

## Context
Some tasks only read code. Some change many files. Some must not touch the host. A worktree per
session costs disk and set-up time in large repos. Plain NTFS has no copy-on-write.

## Decision
Four isolation levels, chosen per task or by the playbook:
1. **None:** the session uses the main checkout (read-only tasks, questions).
2. **Worktree (default):** created on the first write, not at session start. Copy-on-write
   where the file system allows it (ReFS Dev Drive, APFS, Btrfs, XFS). Sparse checkout for
   large repos. Build caches (cargo, gradle, npm, pip) are shared.
3. **Container:** Docker or Podman with the repo mounted from a worktree.
4. **Runner:** a remote pod of the team (`09-backend-infra.md`).

## Rejected alternatives
- **Always a worktree (Orca, Conductor):** slow start in large repos; wasted disk for questions.
- **Always a container (Air with Docker):** needs Docker Desktop; slow on Windows and macOS.

## Consequences
- The UI shows the isolation level of a session (the place is open; see Amended).
- The ledger works the same on all levels, because it records Git snapshots.

## Amended 2026-10-21 (first version)
- The first version has no isolation levels. The daemon creates no worktree and no container.
  The `worktrees` counter of a codebase is 0 in live mode.
- The accepted pane design has no isolation mark. The mark may go to the session deep dive
  when the levels exist.
