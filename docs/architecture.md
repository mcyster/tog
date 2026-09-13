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
ToolsAvailable
ToolRequest
ToolResponse
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

`TurnRequested` starts a turn, and the session records `TurnCompleted` with its
terminal outcome. An assistant response ends a successful turn; a problem fails
the turn even when earlier output was already produced.

`ConversationSession` is the caller-facing interface. `add_user_request` records
and queues user input. `invoke` records a turn request, supplies the existing
conversation and pending user requests to the driver, persists permitted driver
output as it arrives, and records the turn outcome after the driver stream ends.
`Conversation` remains immutable history.

The driver records its own invocation event, including a stable
`ModelInvocationId`, as an opaque driver event. Returned model facts reference
that identifier. The invocation record is execution metadata, not a replacement
for portable assistant or problem meaning.

## Tools

The caller records `ToolsAvailable` before each driver invocation. The latest
declaration determines the toolset the driver presents, and each declaration
replaces the previous one. `ToolRequest` records a model-requested tool call
with a portable `ToolCallId`, tool name, and JSON arguments. `ToolResponse`
correlates the outcome of that call, either a successful result or a typed
execution problem.

The driver synthesizes portable requests from completed provider tool calls and
translates recorded definitions into its provider's tool schema; it never
executes tools. The session executes registered tools sequentially, persists
each response, and invokes the model again until no tool work remains. Assistant
text does not end a turn while tool requests are outstanding, and an execution
problem is returned to the model rather than failing the turn. The loop is
bounded and reports reaching its limit explicitly. See [Tools](tools.md).

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

The configured `ModelDriver` receives a typed `TurnInput` containing an immutable
`Conversation`, pending user-message requests derived from that snapshot, and a
`ConversationTurnId`. Conversation defines `ConversationMessage` for accepted
user content, assistant responses, communications, and problems. The
model-driver API selects the outputs a driver may return through
`ModelDriverOutput`: a shared conversation message or a driver-defined invocation
or extension event. It creates invocation identities and invocation-specific
data. It cannot emit session-owned user requests or turn lifecycle facts, and it
does not allocate durable record positions, timestamps, or record identifiers.
The event store assigns that envelope metadata at the shared append boundary.

```text
immutable Conversation
    -> ModelDriver invocation
    -> permitted conversation messages and driver events
    -> session appends facts and records the turn outcome
    -> append boundary assigns record metadata
    -> log and presentation projections
```

Provider-specific protocol events and raw deltas remain private to the driver.
The caller owns command recording, persistence, retry policy, and the outer
orchestration loop. The session owns the turn lifecycle and records
`TurnCompleted` from its own completion policy. The driver owns invocation
identities and reports problems on its stream.

The [Conversation Model](conversation.md) summarizes the stable vocabulary. The
[Conversation and ModelDriver Architecture](conversation-design.md) contains
the detailed implementation boundaries.
