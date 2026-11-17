# ADR 0026 — Machine mesh: peer daemons, task placement, file sync over VCS and iroh

Status: Accepted, 2026-11-11

## Context
One user runs AIVory Code on more than one machine, often on different networks. Tasks must run
on any machine, the files that agents change must reach the other machines while they work, and
each repo must stay in step with its own VCS: Git, Mercurial, Subversion, Jujutsu or Perforce.
The OS of a machine must not matter. Until now a daemon only accepted phones (ADR 0015, 0021) and
never dialed out.

## Decision
- **Peer link.** Daemons connect to each other over iroh QUIC with the ALPN `aivory/mesh/1`.
  The frames are the same as `aivory/rpc/1` (length + one JSON-RPC message). Hole punch first,
  relay as the backup (`remote.relay`). A peer has its own caller kind with a method allowlist;
  it does not get the phone surface.
- **Pairing.** Machine A creates an invite link `aivory://peer?node=&relay=&direct=&code=`
  (one-time code, 10 minutes). Machine B joins with the link. Both store the key that the QUIC
  handshake proves, in `peers.json` next to `aivory.db`. Each connect swaps the current direct
  addresses and relay URL, and both sides store them (no third-party lookup, ADR 0015).
- **Ids.** Session and task ids get a node prefix when they leave their machine.
- **Placement.** A task has a placement: `auto` or one node. The local daemon picks a node with
  the provider installed and logged in, free slots, low load, and the repo present. Each machine
  runs only its own logged-in CLIs. Login tokens never move.
- **VCS layer.** The `Vcs` trait becomes backend-neutral (WIP refs and repo identity, not
  refspecs). Backends run the CLI of each tool: git, hg, jj, svn, p4.
  WIP store: git hidden refs `refs/aivory/wip/<node>/<session>`, hg and jj bookmarks,
  p4 shelved changelists, svn a shadow store that only peers fetch.
- **File sync.** A workspace has one writer: the node that runs its agent. Other nodes keep
  read-only mirrors. Live changes go as content-addressed blobs on a separate QUIC stream
  (`aivory/blob/1`). A WIP commit at each turn end, at most one push per minute, gives the
  durable state. The owner moves only on handoff.
- **Auto-sync.** Each daemon updates its checkouts from the VCS on an interval, fast-forward
  only, and agent worktrees only between turns. A conflict becomes an inbox item.

## Rejected alternatives
- **Multi-writer live sync (CRDT on files):** agents write whole files fast; merges of live
  writes give broken states. One writer per workspace is simple and safe.
- **Shared network folder (SMB, NFS):** OS-specific, slow across networks, and no history.
- **A hub that carries code:** breaks ADR 0013.
- **Copy native session files between machines:** the native stores are read-only and carry
  account data. A handoff starts a new session with a handoff packet.

## Consequences
- ADR 0011: isolation level "runner" can be a paired machine of the same user.
- ADR 0012: Mercurial, Jujutsu, Subversion and Perforce backends join git; all use the CLI.
- ADR 0013: the WIP ref name gets the node (`refs/aivory/wip/<node>/<session>`); peers fetch
  WIP from each other before they fall back to the remote.
- The team hub (ADR 0013) stays separate. A mesh is one user's machines.
