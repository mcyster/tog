# Execute commands durably from the conversation log

Build toward a single-user daemon with CLI clients, one append-only file per
conversation, and independent asynchronous execution. Requests and observed
outcomes share that log. The command queue and model-visible history are derived
views; neither needs a second authoritative file or an external broker.

This plan describes intended work, not the implemented API. Replace the earlier
operation/command-group model for model attempts with `ModelCallRequest` and
`ModelCallResponse`. A turn spans model attempts, tool execution, retries, and an
explicit terminal outcome. A model call can finish while tools it requested are
still running; no fixed group membership is required before streaming begins.

The current [driver](../../src/model_driver.rs) still uses driver-owned invocation
details and a turn-oriented input. Introduce the common call lifecycle below
without making the conversation vocabulary depend on driver types.

## Commit intent before execution

The storage contract must commit a set of events as one transaction within a
conversation. Readers and executors see all of a committed batch or none of it.
Assign ordered positions at the append boundary. An input position must refer to
a committed boundary, never a partly visible transaction.

For files, define batch framing, integrity checks, a commit marker, flushing, and
recovery from an incomplete trailing batch before dispatch is enabled. A successful
commit must survive the documented crash model. Do not silently discard corruption
inside committed history. A crash after commit but before acknowledgment is
ambiguous to the caller: stable batch/request identities must make retry safe.

Commit together when the application intends transitions together, such as a user
request and its immediate turn request, or the final prerequisite outcome and an
eligible continuation request. When prerequisites finish separately, recovery must
still derive the missing continuation without creating it twice.

Only committed requests may execute. The engine creates and commits each
`ModelCallRequest` before invoking the driver, which receives that request and
its fixed input. A failed request commit must prevent invocation; a driver-emitted
record after external work starts cannot establish durable intent. The engine
also owns acceptance of pending user input.

Do not hold a transaction open during a model
call or while tools run. External side effects are outside the file transaction;
the log cannot prove that an interrupted external operation never happened.

## Common model-call lifecycle

Use `ModelCallRequest` / `ModelCallResponse`, consistently with
`ToolRequest` / `ToolResponse`. The model-call request's durable event ID
identifies one attempt; a retry is another request in the same turn.

| Event | Tog-owned meaning | Optional driver data |
| --- | --- | --- |
| ModelCallRequest | Turn association, model/driver identity, fixed `inputThrough` position, dependencies, retry relationship | Provider options, continuation state, provider-specific input metadata |
| ModelCallResponse | Request reference, observed outcome, ordered `outputEventIds`, known usage and its completeness | Provider request ID, raw finish reason, additional usage details |

Tog owns these event types and their lifecycle. Drivers interpret providers and
supply output and metadata; the engine records requests and terminal responses,
including when a driver fails or times out. A driver cannot redefine completion,
failure, retry eligibility, or dependency satisfaction through an opaque payload.
Scheduling-relevant meaning must have a common typed representation.

Use optional `ModelData` for provider-specific payloads. Preserve that data through
serialization and replay even when its driver is unavailable; interpretation may
require the driver. Provider invocation IDs remain metadata, not substitutes for
the common request reference. These are extensible payloads within a stable
lifecycle, not driver-defined replacements for the lifecycle events.

Keep portable continuation distinct from native replay. A compatible driver may
omit optional enrichment when using portable content; native replay requires a
recognized payload format and version. Unsupported required replay state must
produce an explicit limitation, never silently become prompt text. See
[Layered Semantic Representation](../conversation.md#layered-semantic-representation).

Streamed model outputs reference their `ModelCallRequest`. Each tool response
references its particular tool request. The model-call response lists only outputs
produced by that call, in order, in `outputEventIds`; it excludes tool responses
even when they arrive during the call. Validate that output ownership and the
ordered list agree. `outputEventIds` contains durable event references, not
embedded event payloads.

The engine records one terminal response per model-call request. A timeout closes
the engine's attempt; it does not assert that the provider stopped, that no side
effects occurred, or that token usage was zero. Late output cannot silently extend
a closed attempt or release a continuation again. Specify how late provider
information is retained before implementing that path.

## Turn membership and completion

Each `ModelCallRequest` explicitly references its turn. Other call-related events
inherit turn membership through their references:

| Event | Path to its turn |
| --- | --- |
| Thinking, tool request, assistant response | Model-call request → turn |
| Tool response | Tool request → model-call request → turn |
| Model-call response | Model-call request → turn |

Do not repeat turn references on these dependent events. Arrival order does not
establish membership: events from concurrent turns can interleave. Resolve and
validate the reference chain when reconstructing the conversation.

Retain explicit `TurnStart` and `TurnCompleted` lifecycle events.
`TurnCompleted` references its turn. A successful turn requires a successful
terminal model call with no tool work or continuation outstanding; it does not
require a final assistant message. Any assistant reference is optional and must
belong to that turn. Assistant text may accompany tools, and a successful call
may produce no text. Stream exhaustion without a terminal call response is not
success. The engine records the terminal turn outcome; a failed model attempt
can be followed by a successful retry in the same successful turn.

## Fixed input and tool dependencies

`inputThrough` bounds the committed conversation used to construct a call's
input. It does not mean every preceding event is sent verbatim to the provider.
Capture this position after prerequisites are recorded and before invoking the
driver. Select the boundary and history from one snapshot; concurrent appends
must not change that call's selected input. The engine determines eligibility;
the driver translates eligible history into provider input. This separation does
not require a decorator or another particular code structure.

`dependsOn` uses tool-request IDs to identify operations. Each dependency is
satisfied when its correlated `ToolResponse` is recorded, including a failure
response. Satisfaction means the result is available, not that the tool succeeded;
the continuation/retry policy decides how to handle it.

Wait for the preceding model call's terminal response and all required tool
responses before recording the next model-call request. Dependencies do not
extend input beyond `inputThrough`. Scheduling intent for work whose prerequisites
are still pending is separate from the fixed-input model-call request.

Tools may start as their complete requests become durable. More tool requests may
arrive during the same model call, including after earlier tools finish. Progress
and results become visible immediately after commit. Closing the call seals its
ordered outputs; it does not imply that all requested tools have finished.

Whether a driver can make a tool request durable before the call ends is driver
capability, not an event-model requirement. A provider stream that announces calls
incrementally may emit each one mid-call; a driver that only observes them at
completion emits them with the call's output. The engine schedules continuations
from committed facts and must remain correct for either emission order.

## Interleaved example

The user asks: "List my home directory files, show the working directory, and read
the first line of ~/README.md." IDs below show arrival order; `call` references a
model-call request and `tool` references a tool request. Everything from
`TurnStart` onward is associated with turn 2. These labels illustrate intended
semantics rather than mandate renaming existing serialized turn/message events.

| ID | Event | References | Content |
| --- | --- | --- | --- |
| 1 | User | — | The request above |
| 2 | TurnStart | user:1 | Starts turn 2 |
| 3 | ModelCallRequest | turn:2; inputThrough:2 | retryCount:0 |
| 4 | Thinking | call:3 | I'll check the directory and working location. |
| 5 | ToolRequest | call:3 | ls ~ |
| 6 | ToolRequest | call:3 | pwd |
| 7 | ToolResponse | tool:6 | /home/mcyster |
| 8 | Thinking | call:3 | I'll also read the README. |
| 9 | ToolRequest | call:3 | Read first line of ~/README.md |
| 10 | ToolResponse | tool:9 | My personal notes |
| 11 | ModelCallResponse | call:3; outputEventIds:[4,5,6,8,9] | Successful; input:1,000, output:200 |
| 12 | ToolResponse | tool:5 | README.md, notes.txt |
| 13 | ModelCallRequest | turn:2; inputThrough:12; dependsOn:[5,6,9] | retryCount:0 |
| 14 | Thinking | call:13 | I'll summarize the results. |
| 15 | ModelCallResponse | call:13; outputEventIds:[14] | Timeout; usage unknown |
| 16 | ModelCallRequest | turn:2; inputThrough:15; dependsOn:[5,6,9] | retryOf:13; retryCount:1 |
| 17 | Thinking | call:16 | I have the directory results. |
| 18 | AssistantResponse | call:16 | Files: README.md, notes.txt. Working directory: /home/mcyster. README begins: "My personal notes". |
| 19 | ModelCallResponse | call:16; outputEventIds:[17,18] | Successful; input:1,400, output:300 |
| 20 | TurnCompleted | turn:2; assistantResponse:18 | Successful |

Call 3 finishes at 11, while its directory listing finishes at 12. Request 13 uses
`inputThrough:12` so its fixed input includes all three tool responses.
Call 16 is another potentially billable attempt and reuses those recorded results;
it does not rerun the tools. `retryOf` makes that relationship explicit.

The derived view of turn 2 is successful, with three model attempts, 2,900 known
tokens, and incomplete total usage: call 3 reports 1,200, call 13 is unknown, and
call 16 reports 1,700. The view combines the explicit turn outcome with usage
across its referenced calls. Unknown usage must not be counted as zero or presented
as a complete total.

When preparing the next model input, reconstruct call 3's output in order
4,5,6,8,9, then supply tool responses in request order 5,6,9 (response events
12,7,10). The UI can retain actual arrival order.

Preserve partial output from unsuccessful attempts in the log and UI. Exclude
incomplete-attempt thinking such as event 14 from subsequent model input by
default. Tool requests already acted upon and their results must remain accounted
for. A timeout after emitting a tool request is a required design/validation case:
settle its model-input projection and recovery policy before enabling automatic
retries for that case; do not discard evidence of side effects.

## Recovery, retries, and notifications

Recovery must cover user-request handling, turns, model calls, tools, and
continuations. It cannot begin only after a model call has started. Reconstruct
committed requests, accepted outcomes, dependencies, cancellations, and continuation
eligibility before dispatching after restart.

Keep request identity, attempt identity, and logical completion distinct where
retries require them. A completed model attempt need not complete its turn.
Correlate tool failures to `ToolResponse`; separate conversation problems describe
failures outside a tool call. Failure details need a portable category, useful
message, retry guidance, and known versus uncertain execution outcome.

Record failure on the operation's outcome: `ToolResponse`, `ModelCallResponse`,
or `TurnCompleted`. An explanatory problem message neither substitutes for that
outcome nor automatically fails the turn. Keep storage failures separate from
provider failures; an unsuccessful append may leave an unfinished request for
recovery rather than permit a terminal response to be recorded.

The runtime applies bounded retries, backoff, deadlines, and cancellation policy.
A recorded request without an outcome does not prove that external execution never
began. Reuse recorded successful tool results; do not rerun successful side effects
as part of model-call retry. Use tool-side idempotency or reconciliation where
available; otherwise require explicit recovery for unsafe retries. Specify
tool-attempt retry correlation before implementing it, so an earlier response
cannot accidentally satisfy a later attempt's dependency.

Provide a small runtime-facing notification/completion interface. Accept wakeups
from committed log changes, scheduler deadlines, and running-operation signals.
Keep wakeups, progress, and terminal outcomes distinct. Notifications only prompt
another read; the committed log is the source of truth. The runtime validates and
commits outcomes before publishing them or releasing dependents.

The dispatcher reads committed events from a position. Avoid the gap between
replaying history and subscribing. Recover deadlines and eligibility from durable
state, not only in-memory timers. Duplicate notifications or stale/duplicate
responses must not duplicate execution decisions or continuation requests.

Use one daemon authority to serialize each conversation's append and scheduling
decisions; prevent competing daemon writers. This does not require a thread per
conversation. REST tools await nonblocking I/O; subprocess tools await process
completion. Bound concurrency and isolate blocking or CPU-heavy implementations.
Tools return results through the runtime interface rather than owning log writes.

## Turn context and cancellation

Separate turn eligibility from input selection:

| Requested behavior | Eligible when | Context |
| --- | --- | --- |
| Run alongside an active turn using its original input | Now | Explicit position used by that turn |
| Run now using available history | Now | Position captured when requested |
| Run after an active turn finishes | Dependency is terminal | Position captured when eligible |

Turn start records its selected input position; each model-call request records
its own `inputThrough`. The current user request and explicit arguments remain
inputs even when the historical context predates them. A continuation selects a
new boundary including its required results.

Support turn cancellation and conversation-wide cancellation requests outside
individual tools. Cancellation is durable intent, not proof that remote work has
stopped. Prevent new dispatch and automatic retries within the cancelled scope;
attempt cancellation of running work and record acknowledgments or uncertainty.
Late results must not restart a cancelled continuation.

Before implementation, specify whether conversation cancellation affects only
currently outstanding work or also prevents future requests, and how it is
explicitly resumed. Also specify how queued-turn eligibility treats a failed or
cancelled predecessor. Do not choose these policies implicitly.

## Implementation and acceptance

Implement in reviewable slices: transactional file storage and recovery first;
common model-call lifecycle, references, input projection, and usage next; then
dispatch and dependency coordination; then retries, deadlines, and cancellation.
Keep snapshots, distributed workers, Kafka, and generalized graph scheduling
outside the initial scope.

Verify these observable boundaries when implementing:

- A failed model-call request commit prevents driver invocation.
- A torn batch never dispatches tools or exposes partial output.
- Assistant text with tools does not complete a turn; a successful terminal call
  with no outstanding work can complete it without assistant text.
- Stream exhaustion without a terminal response is not mistaken for success.
- Storage failure is not recorded as provider failure.
- Crash recovery finds requests and eligible continuations that never started.
- Streaming tools and results interleave as in the example, while call completion
  and tool completion remain independent.
- Fast, duplicate, and reordered results release a continuation only once.
- A continuation requires both call closure and its correlated tool responses.
- Input positions remain stable during concurrent appends and include every
  required result; model projection and UI arrival order remain distinct.
- Output references agree with ownership and ordering; a closed call cannot gain
  additional output or a second terminal response.
- Turn membership follows references even when multiple turns interleave; a final
  assistant response referenced by `TurnCompleted` belongs to that turn.
- A model retry preserves recorded tool results and separately accounts for usage;
  a successful turn can include failed attempts and incomplete total usage.
- Timeout after tool dispatch preserves side-effect evidence and follows the
  explicitly chosen recovery/projection policy.
- Optional driver data round-trips without loading its driver; common lifecycle
  handling does not require interpreting that data. Supported native replay uses
  preserved state, and unsupported required state is reported explicitly.
- Cancellation, restart during backoff, and late results obey the selected policy.

Ordinary replay reconstructs state without executing requests. Recovery then
explicitly schedules eligible work. This plan does not promise deterministic
model responses or exactly-once external side effects.
