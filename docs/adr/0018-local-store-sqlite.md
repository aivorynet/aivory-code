# ADR 0018 — SQLite (WAL) plus content-addressed segment files

Status: Accepted, 2026-10-21

## Context
The daemon stores sessions, tasks, turns, the inbox, claims, and settings. Many readers (the
UIs) and one writer (the daemon) work at the same time. Index data is large and immutable.

## Decision
- SQLite in WAL mode for all mutable state (`aivory.db`, see Amended).
- Immutable segment files for index data, named by content hash, read with mmap
  (planned: `segments/` next to `aivory.db`).
- No embedded key-value engine in v1.

## Rejected alternatives
- **redb or LMDB for all data:** fast, but no SQL for the inbox, the ledger queries, and migrations.
- **Postgres locally:** too heavy for a desktop daemon.
- **JSON files:** no transactions; slow with thousands of sessions.

## Consequences
- One writer thread in the daemon; readers use separate connections.
- A segment is never changed after it is written, so the cache needs no locks and is safe to share.

## Amended 2026-10-21 (first version)
- The store is `aivory.db`. It lies next to the scan cache `sessions-cache.json` in the per-user
  folder: `%LOCALAPPDATA%ivory\code` on Windows, `$XDG_CACHE_HOME/aivory-code` or
  `~/.cache/aivory-code` on Linux, `~/Library/Caches/aivory-code` on macOS.
  `AIVORY_CACHE_DIR` overrides the folder (tests use it).
- The code is the crate `aivory-engine` (`rusqlite`, bundled; WAL; numbered migrations). Tables:
  `tasks`, `inbox_results`, `settings`, `activity_samples`, `devices`.
- Paired phones go to `devices.json` (ADR 0021), not to the `devices` table. The table is unused
  by the daemon so far.
- Only the live daemon opens the store. The demo daemon keeps its data in memory.
- Segment files do not exist yet. They come with the code lens.
