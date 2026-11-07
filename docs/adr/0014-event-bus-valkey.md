# ADR 0014 — Valkey Streams as the event bus of the hub

Status: Accepted, 2026-10-21

## Context
The hub needs a durable, ordered event log per team with cursor reads. The AIVory
infrastructure runs Valkey and no message broker. 500 sessions send about 17 events per second,
with peaks near 200.

## Decision
One Valkey stream per team. Retention 14 days. Clients read from their last cursor. Claims use
Valkey keys with a TTL. JetStream (NATS) is the scale-up path when one team needs more than
about 5,000 events per second or multi-region replication.

## Rejected alternatives
- **NATS JetStream now:** better fan-out, but one more system to run and watch.
- **Postgres LISTEN/NOTIFY:** no durable replay.
- **Kafka:** too heavy for the volume.

## Consequences
- No new infrastructure for the first team customers.
- The hub hides the bus behind a trait, so the change to JetStream needs no client change.
