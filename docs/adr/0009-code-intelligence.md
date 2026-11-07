# ADR 0009 — Layered code intelligence

Status: Accepted, 2026-10-21

## Context
The lens needs JetBrains-grade highlight, hover, navigation, rename, and diagnostics for many
languages. Hundreds of sessions change the code at the same time. One rust-analyzer per session
costs gigabytes.

## Decision
- **L0 syntax:** tree-sitter or the Kit engine (ADR 0010), into one tree model.
- **L1 stubs:** one per file, keyed by (blob hash, grammar version, extractor version). Made by
  tree-sitter tag queries or Kit `stubClass` rules.
- **L2 segments:** immutable segment files with an FST name index. Each session has a small
  delta segment for its changed files. A query merges the base and the delta.
- **L3 team cache:** encrypted segments shared through the hub.
- **SCIP import** from official indexers gives exact cross-file references.
- **LSP pool:** one server per repo lineage, started when a session gets focus. Each query
  swaps in the changed files of the session as overlays.
- **Rules:** ast-grep rules and fixes. All diagnostics in SARIF.

## Rejected alternatives
- **LSP only (Zed):** one server per worktree costs too much memory; no team cache.
- **Own semantic engine per language:** years of work.
- **stack-graphs:** archived.

## Consequences
- Identical files share index data across sessions, branches, and team members.
- Exact refactors depend on the LSP of the language. The index gives navigation for all.
