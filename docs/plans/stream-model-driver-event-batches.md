# Stream model driver events in atomic batches

Follow [PR #5](https://github.com/mcyster/tog/pull/5) with a small change to the
model driver API: stream ordered, nonempty batches of completed events instead
of individual events. A model API call may produce many messages and batches;
message, call, and transaction boundaries need not coincide.

The driver groups events that must become visible together. The caller persists
each batch atomically, preserving event and batch order, before publishing or
presenting its events. Single-event batches are the normal case. Earlier batches
remain durable if a later batch or the model call fails.

The driver owns provider interpretation, provider invocation identities, and semantic
grouping. Tog owns the common model-call request identity and lifecycle described
in [Execute commands durably](execute-commands-durably.md). The driver does not
persist events or assign durable record IDs, positions, or timestamps. The caller and event store own those responsibilities. The caller may
include related orchestration records in the same transaction, but must not split
a driver batch across transactions.

## Intended change

- Introduce a nonempty batch type for the existing driver event vocabulary and
  make it the output stream item. Keep shared facts and driver-defined events.
- Adapt the session to commit a complete batch before reporting any of its events.
  Coordinate this with atomic batch support in storage; sequential per-event writes
  do not satisfy the contract.
- Keep explicit model-call and turn completion distinct from batch completion.
  A batch does not necessarily end a message, model call, or turn. The engine
  records `ModelCallRequest` with fixed `inputThrough` before invocation, attaches
  its reference to streamed outputs, and records `ModelCallResponse` with ordered
  `outputEvents`, outcome, and usage when the attempt closes. Optional driver data
  extends these common events without redefining their lifecycle.
- Preserve immediate progress: commit completed tool requests and dispatch eligible
  tools while the model call continues. Tool responses reference their own requests
  and are not part of the call's `outputEvents`. Call completion does not wait for
  tool completion; the next call waits for both and captures an input boundary
  including the required results.
- Update the supporting API documentation to state this boundary consistently.

Yielding a batch does not acknowledge that it has been committed. Add no driver
acknowledgment protocol unless a concrete operation must wait for durable acceptance
before proceeding. Scheduling, retries, and broader turn orchestration remain
separate work.

Completion means single-event and multi-event batches preserve order, readers and
presentation never observe a partially committed batch, and failure of a later
batch preserves earlier committed batches. Verify empty-batch rejection and these
observable persistence boundaries when implementing the change.
