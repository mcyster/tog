# Architecture

`tog` is a single Cargo package containing one executable.

## One Ordered Log

The conversation log records both requested work and resulting facts. Every
record carries conversation identity, an append-order position, a timestamp,
an identifier, and a schema version. Commands and semantic facts share the
same ordered stream, but they have different projections and meanings.

```text
Conversation Log
    command records
    semantic conversation facts
    turn lifecycle facts
```

Commands preserve system input and provide a foundation for explicit retry or
re-execution. Facts describe what was accepted or produced. Replaying history
reconstructs state without executing commands again. Explicit re-execution may
use recorded commands to produce new outcomes; model output is not expected to
be deterministic and external side effects require their own idempotency policy.

## Semantic Events

The readable semantic kinds are top-level:

```text
User
Assistant
Communication
Problem
TurnCompleted
```

`Assistant` and `Communication` are not nested beneath a generic `Model` kind.
`Problem` may be model-associated or unrelated. Model provenance is optional
metadata on the applicable fact rather than the fact's primary kind.

Driver-defined invocation events retain the model source and invocation-wide
configuration. A model-produced fact carries only a `ModelInvocationId` and
optional event-specific model data, allowing several assistant, communication,
problem, or future tool facts to refer to one invocation without repeating its
provenance.

## Commands And Turns

A user-content command and its accepted fact are distinct records:

```text
UserMessageRequested
User
```

The user fact is the portable conversation content. It may exist before a turn
is requested, allowing messages to accumulate independently from agent work.

`TurnRequested` starts a turn, and `TurnCompleted` records its terminal outcome.
An assistant response is not a completion marker, and a problem does not always
complete a turn because orchestration may recover or retry.

`ConversationSession` is the caller-facing interface. `add_user_request` records
and queues user input. `invoke` records a turn request, supplies the existing
conversation and pending user requests to the driver, and persists driver
output as it arrives. `Conversation` remains immutable history.

The driver records its own invocation event, including a stable
`ModelInvocationId`, as an opaque driver event. Returned model facts reference
that identifier. The invocation record is execution metadata, not a replacement
for portable assistant or problem meaning.

## Conversation Projection

`Conversation` is an immutable projection reconstructed from the ordered log.
It excludes command records and driver-defined records from model-visible
history and permits positions to have gaps because commands, extension records,
and lifecycle records occupy positions. Provider projections also exclude
commands and turn lifecycle facts unless a provider has a concrete semantic
reason to use them.

Everything made visible to a model must be recorded in semantic facts or
immutably referenced by them. Provider transport events, raw streaming deltas,
credentials, and execution mechanics remain outside the portable semantic
representation.

## Model Driver Boundary

The configured `ModelDriver` receives an immutable `Conversation` and a
`ConversationTurnId`, then yields shared semantic events, driver-defined events,
and an explicit `TurnCompleted` event. It creates invocation identities and
invocation-specific data. It does not allocate durable record positions,
timestamps, or record identifiers. The event store assigns that envelope
metadata at the shared append boundary.

```text
immutable Conversation
    -> ModelDriver invocation
    -> driver events, semantic facts, and TurnCompleted
    -> append boundary assigns record metadata
    -> log and presentation projections
```

Provider-specific protocol events and raw deltas remain private to the driver.
The caller owns command recording, persistence, retry policy, and the outer
orchestration loop. The driver owns invocation identities and reports turn
completion explicitly.

The [Conversation Model](conversation.md) summarizes the stable vocabulary. The
[Conversation and ModelDriver Architecture](conversation-design.md) contains
the detailed implementation boundaries.
