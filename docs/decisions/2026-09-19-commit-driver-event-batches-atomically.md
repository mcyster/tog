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

Phase 1 storage keeps one append-only log file per conversation. A committed
batch is a sequence of length-framed, checksummed event records followed by a
commit marker. Readers ignore a trailing batch without a valid commit marker and
treat corruption inside committed history as an error. The append boundary
assigns event identities, timestamps, and positions, flushes, and syncs each
batch before acknowledging it. Appending rewrites nothing but an uncommitted
tail. Existing per-event files remain readable and migrate into the log on the
next append to that conversation.

## Consequences

The commit marker keeps replay a single sequential read per conversation and
makes an incomplete trailing batch trivially recoverable. Checksums distinguish a
torn write from corruption: an incomplete trailing frame is discarded, while a
present frame that fails its checksum is an error. A batch is committed before
any of its events are presented, so readers never observe partial output.

This refines [Use Asynchronous Streaming ModelDriver Invocations](2026-08-22-use-asynchronous-streaming-model-driver-invocations.md):
the stream and completed-event semantics remain, but the stream item is now an
atomic batch rather than a single event.
