# Execute commands durably from the conversation log

Build toward a single-user daemon with CLI clients, one append-only file per
conversation, and independent asynchronous command execution. Commands and facts
share that log. The command queue and model-visible history are derived views;
neither needs a second authoritative file or an external broker.

This is intended work following the driver/session boundary in
[PR #5](https://github.com/mcyster/tog/pull/5). At that baseline,
[EventStore](../../src/persistence.rs) writes one JSON file per event and
[ConversationSession](../../src/conversation_session.rs) consumes a driver stream
directly. Transactional batches, a dispatcher, and command groups are not yet
implemented. Preserve portable conversation meaning and driver-owned invocation
details while introducing execution responsibilities.

## Commit intent before execution

The storage contract must commit a set of events as one transaction within a
conversation. Readers and executors see all of a committed batch or none of it.
Assign ordered positions at the append boundary. A context position must refer to
a committed boundary, never a partly visible transaction.

For files, define batch framing, integrity checks, a commit marker, flushing, and
recovery from an incomplete trailing batch before dispatch is enabled. A successful
commit must survive the documented crash model. Do not silently discard corruption
inside committed history. A crash after commit but before acknowledgment is
ambiguous to the caller: stable batch/command identities must make retry safe.

Commit together when the application intends these transitions together:

- A user request and its immediate turn request. Queued input alone is valid.
- Group membership and the commands to dispatch.
- Group completion and the next operation's request.

Only committed commands may execute. Do not hold a transaction open while tools
run. External side effects are outside the file transaction; the log cannot prove
that an interrupted external operation never happened.

## Turns, groups, and commands

A turn is requested work through an explicit terminal outcome. It can contain
several model calls and command groups. A group is a join over a fixed set of
logical command IDs, associated with a turn. It may be only a subset of the work
outstanding in the conversation.

Use the names `command_group_start` and `command_group_end`. Start establishes
membership before members are dispatched; end establishes that all members have
terminal outcomes. Failure and cancellation can be terminal outcomes. An attempt
that will be retried is not a terminal logical-command outcome.

An illustrative successful flow, with IDs shortened:

| Batch | Events |
| --- | --- |
| 1 | user request; turn request |
| 2 | accepted user fact; turn_start with input position |
| 3 | command_group_start(G, [A, B]); tool requests A and B |
| 4 | tool A start with attempt identity and input |
| 5 | tool B start with attempt identity and input |
| 6 | tool A result |
| 7 | tool B result |
| 8 | command_group_end(G); continuation request |
| 9+ | continuation start; assistant response; explicit turn completion |

The lifecycle names here describe the intended vocabulary, not a mandate to
rename existing serialized turn events. Results arrive independently and become
durable immediately. Group completion does not itself merge context or complete
the turn. Record enough continuation information to schedule the next operation
after recovery; an in-memory callback is insufficient.

## One lifecycle for all requested work

Recovery must cover user-request handling, turn execution, groups, tools, and
continuations. It cannot begin only after a group has started.

Specific events should expose shared typed lifecycle capabilities:

| Role | Required meaning |
| --- | --- |
| Requested | Command ID, requested operation, parent association where applicable, start conditions, input-selection policy |
| Started | Command ID, attempt ID, actual selected input boundary or explicit arguments |
| Done | Command ID, attempt ID, structured outcome and failure details |
| Logical completion | Whether the command is terminal or another attempt remains eligible |

Keep Command/Fact classification separate: a request is a command; started and
done records are facts. Shared and driver-defined execution events must expose
enough common lifecycle information for scheduling without interpreting arbitrary
provider payloads. Final Rust trait names and placement should be settled in the
first implementation slice; the table defines their responsibilities.

A done attempt does not automatically complete its logical command. Preserve the
logical ID across retries and distinguish attempts so late or duplicate results
cannot complete the command twice or release a join twice. Normal group retry
resumes coordination and retries eligible unfinished members; it does not rerun
successful side effects. Explicitly rerunning completed work is a new request.

Failure results need a portable category, useful message, retry guidance, and
known versus uncertain execution outcome. The runtime applies bounded retries,
backoff, deadlines, and cancellation policy. A missing start is recoverable queued
work; a start without a result is uncertain work, not proof of nonexecution.
Use tool-side idempotency or reconciliation where available. Otherwise require
explicit recovery for unsafe retries.

## Dispatch, notifications, and completion

Provide a small runtime-facing notification/completion interface. Accept wakeups
from committed log changes, scheduler deadlines, and running-command signals, but
keep wakeups, progress, and terminal outcomes distinct.

The dispatcher reads committed events from a position. Notifications only prompt
another read; they are not the source of truth. Completion reports carry command
and attempt identity. The runtime validates and commits outcomes before publishing
them or releasing dependents. Recover deadlines and eligibility from durable
state, not only in-memory timers.

Rebuild requests, attempts, group membership, accepted results, cancellations, and
continuations before dispatching after restart. A log-position reader must avoid
the gap between replaying history and subscribing. Duplicate notifications and
results must not duplicate execution decisions.

Use one daemon authority to serialize each conversation's append and scheduling
decisions; prevent competing daemon writers. This does not require a thread per
conversation. REST tools await nonblocking I/O; subprocess tools await process
completion. Bound concurrency and isolate blocking or CPU-heavy implementations.
Tools return results through the runtime interface rather than owning log writes.

## Context selection and cancellation

Separate eligibility from input selection:

| Requested behavior | Eligible when | Context |
| --- | --- | --- |
| Run alongside an active turn using its original input | Now | Explicit position used by that turn |
| Run now using available history | Now | Position captured when requested |
| Run after an active turn finishes | Dependency is terminal | Position captured when eligible |

`turn_start` and `command_group_start` record their selected input positions.
Individual commands also identify their actual inputs when these differ from
group context. The current user request and command arguments remain explicit
inputs even when the historical context position predates them. A continuation
selects a new boundary that includes the relevant results; it does not silently
reuse the group's original boundary. Log position identifies available history,
not necessarily every item selected by a model projection.

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
typed lifecycle and input contracts next; then dispatcher/group coordination;
then retries, deadlines, and cancellation. Resolve the file-format migration from
existing per-event files before switching storage. Keep snapshots, distributed
workers, Kafka, and generalized graph scheduling outside the initial scope.

Verify the observable boundaries:

- A torn batch never dispatches tools or exposes partial group membership.
- A crash after commit but before notification still recovers eligible commands.
- Fast, duplicate, and reordered results release a group only once.
- Group completion cannot lose or duplicate its continuation across restart.
- Recovery covers a requested turn or continuation that never started.
- Retrying an attempt preserves successful siblings and rejects stale outcomes.
- Context positions remain stable while concurrent turns append new facts.
- Cancellation, restart during backoff, and late results obey the selected policy.

Ordinary replay reconstructs state without executing commands. Recovery then
explicitly schedules eligible work. This plan does not promise deterministic
model responses or exactly-once external side effects.
