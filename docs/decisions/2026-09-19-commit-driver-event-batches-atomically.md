# Commit Driver Event Batches Atomically

## Status

Accepted

## Context

The model driver stream yielded one `ModelDriverOutput` per stream item, and the
session persisted each output through a separate per-event file write. Per-event
files cannot commit a group atomically: a crash could expose part of the group,
and a reader had no boundary separating committed history from an incomplete
write. The driver knows which completed events must become visible together; the
caller owns durable identities, positions, and storage.

## Decision

The driver stream yields ordered, nonempty `ModelDriverOutputBatch` values. A
batch contains the events that must become visible together; single-event batches
are the normal case, and a batch does not necessarily end a message, model call,
or turn. The session commits each batch as one storage transaction and only then
reports its events. Yielding a batch does not acknowledge its commit, and no
driver acknowledgment protocol exists.

The storage contract commits an ordered vector of conversation events as one
atomic, durable transaction and returns the committed records in input order.
An empty vector is rejected with `EmptyBatch`. The contract returns stored
records; conversation reconstruction and domain validation belong to consumers.

Phase 1 storage keeps one append-only JSON Lines log per conversation. A
transaction opens with `{"transaction":"begin"}`, carries one event record per
line, and closes with a commit marker:

```json
{"transaction":"commit","event_count":2,"first_position":3,"last_position":4,"crc":"7d1ba4bd"}
```

`crc` is CRC32 over the transaction's event-line bytes. Readers ignore a
transaction without a valid commit marker and treat corruption inside committed
history as an error. The append boundary assigns event identities, timestamps,
and positions, flushes, and syncs each transaction before acknowledging it.
Appending rewrites nothing but an uncommitted tail.

## Consequences

The log is readable and queryable with standard command-line tools such as `jq`,
and `:log` prints the committed event lines unchanged. The commit marker keeps
replay a single sequential read per conversation and makes an incomplete trailing
transaction trivially recoverable. Since appends are prefixes, every
newline-terminated line was fully written: an incomplete trailing line is a torn
write and is discarded, while a complete line that fails validation or its
checksum is corruption. A batch is committed before any of its events are
presented, so readers never observe partial output.

This refines [Use Asynchronous Streaming ModelDriver Invocations](2026-08-22-use-asynchronous-streaming-model-driver-invocations.md):
the stream and completed-event semantics remain, but the stream item is now an
atomic batch rather than a single event.
