# Make ModelDrivers Return Conversation Events

> Superseded by [Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md). Drivers now return semantic event kinds; the append boundary creates durable envelopes.

## Status

Accepted

## Context

The preceding model-data decision made the distinction between portable
conversation meaning and provider-specific model data explicit, but its proposed
intermediate driver event
remained the public output type. That left the caller responsible for
assembling the durable `ConversationEvent` envelope, provenance, and model
data, even though those values are part of the driver's translated result.

The public boundary should expose the durable semantic fact. Provider-native
intermediate activity should not become a second model-facing contract.

## Decision

`ModelDriver::invoke` returns a stream of `ConversationEvent`s:

```rust
type ModelOutputStream =
    BoxStream<'static, Result<ConversationEvent, ModelDriverError>>;
```

Each concrete driver translates provider-native activity into a complete event,
including its `ModelSource` provenance, optional `ModelData`, event identity,
position, timestamp, and schema version. A `ModelDriverEvent` intermediate type
may exist inside a concrete driver, but it is not part of the neutral driver
contract.

The caller persists and presents returned conversation events. It still owns
the user event, sanitized invocation-problem events, persistence policy, and
the outer orchestration loop. `ModelDriverError` remains operational control
flow; a stream error may be recorded by the caller as a sanitized problem.

## Consequences

The driver boundary now has one semantic output hierarchy: a stream of durable
conversation facts. A new driver must produce the same canonical event shape,
but the caller no longer needs provider-specific knowledge to assemble it.
Provider-native parsing and intermediate event types remain private to each
integration.

The event store validates the returned event's conversation membership and
next position before persisting it unchanged. This preserves the driver's
provenance and model data while retaining append-only ordering guarantees.
