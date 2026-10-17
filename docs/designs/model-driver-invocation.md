# Model driver invocation

For the current boundary and ownership, start with
[`architecture/conversation.md`](../architecture/conversation.md). This document
records the design alternatives and consequences that shaped the invocation
contract, so future work does not re-propose rejected shapes.

## The contract

One `ModelDriver::invoke` call represents one provider/model invocation. It
returns an asynchronous stream of ordered, nonempty batches of permitted
conversation outputs. The session commits each batch atomically before reporting
its events, so readers never observe a partially committed batch. The driver
aggregates provider-native detail privately and returns only completed semantic
output; raw provider deltas are not conversation events.

## Alternatives considered

- **Batch delivery.** The original contract returned a batch of semantic events
  only after the whole invocation succeeded. That withheld completed output and
  discarded everything on a late provider failure. Streaming lets completed
  output be presented and persisted incrementally, and a late failure no longer
  invalidates earlier completed facts.
- **Per-event persistence.** Persisting each stream item through a separate
  write cannot commit a group atomically: a crash could expose part of the group.
  Batching the events that must become visible together and committing the batch
  as one transaction gives readers a committed boundary.
- **Runtime rejection.** Letting a driver produce any conversation fact and
  rejecting wrong shapes at runtime left the ownership boundary implicit. The
  type system now expresses what a driver may produce, so the restriction is
  enforced by construction.
- **Separate model input.** Passing instructions, tool definitions, or context
  to the driver as a separate bag would create unrecorded state. The conversation
  is the portable input: everything visible to a model is recorded in it or
  immutably referenced by it.

## Consequences

- Completed semantic output can be displayed and appended while the invocation
  remains active; already appended events are never rolled back.
- The conversation is the only durable history required for correctness.
  Provider-native continuation may optimize fidelity, latency, or cost, but it
  must never be required.
- A model-associated problem is a conversation event on the same stream as model
  output, while `ModelDriverError` remains detailed control flow for the caller.
- The stream item is an atomic batch, not a single event; a batch does not
  necessarily end a message, model call, or turn.
