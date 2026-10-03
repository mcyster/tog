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

## Turn management and execution

Mark accepted this separation on 2026-10-03. Keep two focused responsibilities
inside `tog-engine`; a separate worker process or crate is not required.

| Responsibility | Owns |
| --- | --- |
| Turn management | Stable turn identity, lifecycle, suspension/resumption, scheduling, and retry policy |
| Turn execution (worker) | Model invocations, tool execution, result recording, and determining what work remains |
| Driver | Translation to and from a provider for one model invocation |

Execution reports done, waiting (with an optional wake time), or failed. Turn
management decides when execution runs again and records turn lifecycle events;
it does not interpret tool calls to make scheduling decisions. The driver emits
tool requests, while execution assigns durable deadlines and runs tools.

A turn retains its ID across retries and suspension. Model attempts have distinct
request IDs and usage. Model-call retry, tool re-execution, and turn resumption
are separate operations; retrying a turn must not blindly rerun its tools.

## Tool deadlines and uncertain outcomes

Record a response deadline on a tool request before dispatch. It lets later
execution recognize an unanswered request after a process interruption. The
execution timeout limits how long the live executor runs or waits; the recorded
deadline limits how long the request may remain unanswered. These are different
responsibilities, even if their configured durations coincide.

When a recorded deadline passes with no answer, record a correlated tool response
with outcome `unknown`. The operation may already have had its effect. A timeout
or cancellation likewise does not prove an external side effect never happened.
Use recorded results, idempotency, or reconciliation before considering a retry.

Keep competing results and deadline responses from releasing a continuation
twice. Settle the acceptance rule for racing or late responses before enabling
recovery. Persist enough deadline and waiting state that correctness does not
depend on a surviving in-memory timer.

## Boundaries and open choices

Use one daemon authority for each conversation's append and scheduling decisions.
Bound concurrency. Keep distributed workers, external brokers, snapshots, and
general graph scheduling outside the initial scope.

Before enabling recovery and retries, settle the storage crash guarantees, handling
of late results, and interrupted calls that already requested tool side effects.
Before enabling suspension, settle its event representation under the same turn ID.
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
