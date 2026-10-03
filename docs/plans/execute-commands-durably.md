# Execute work durably from the conversation log

Build toward a single-user daemon with CLI clients and independent asynchronous
execution. Use each conversation's committed log to derive pending work and
observed outcomes, so restarting a process can recover what remains to be done.
The queue and model-visible history are views of that log, not separate authorities.

The common model-request and response lifecycle already exists. The current
session runs a bounded, sequential tool loop; durable dispatch and recovery,
automatic retries, cancellation, and interleaved execution remain direction.

## Principles

Commit intent before starting external work. Publish outcomes and release dependent
work only after committing the relevant events. Select each model attempt's input
from a fixed committed boundary so concurrent arrivals cannot change its meaning.

Keep call completion, tool completion, and turn completion separate. A continuation
requires the preceding call to close and its required tool results to be recorded.
A failed attempt can be retried without rerunning successful tools, and unknown
usage remains unknown.

The log cannot make external side effects transactional. A request without an
outcome may already have executed. Recovery must use idempotency or reconciliation
where available and require explicit handling when a retry would be unsafe.
Ordinary history replay never dispatches work.

## Boundaries and open choices

Use one daemon authority for each conversation's append and scheduling decisions.
Bound concurrency. Keep distributed workers, external brokers, snapshots, and
general graph scheduling outside the initial scope.

Before enabling recovery and retries, settle the storage crash guarantees, handling
of late results, and interrupted calls that already requested tool side effects.
Before enabling cancellation, settle its scope, resumption, and how failed or
cancelled predecessors affect queued turns. These choices change observable behavior
and must not be left to incidental implementation.

Build in slices: verify storage recovery guarantees, then dispatch and dependencies,
then retries, deadlines, and cancellation. Completion means restart and duplicate
notifications preserve eligible work without duplicating scheduling decisions;
partial output and uncertain side effects remain accounted for. This does not
promise exactly-once external effects or deterministic model responses.

For lifecycle fields, an interleaved example, recovery policies, and validation
scenarios, read the [durable-execution design](../designs/execute-commands-durably.md).
For today's executor boundary, read
[conversation architecture](../architecture/conversation.md).
