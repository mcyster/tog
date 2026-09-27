# Conversation Model

The Conversation Log is `tog`'s ordered append-only record stream. It records requested work and semantic facts without exposing provider transport protocol.

The [Conversation and ModelDriver Architecture](conversation-design.md) is authoritative for model invocation, provider events, replay strategies, and Phase 1 implementation boundaries. This document summarizes the conversation concepts that should remain stable across those details.

## Conversation

A conversation begins with its first accepted semantic event. `Conversation` is an immutable projection reconstructed from non-command records carrying one `ConversationId`:

```text
Conversation
    id
    events

Conversation Log
    position 0: ConversationEvent
    position 1: ConversationEvent
    ...
```

There is no independently persisted conversation record and no empty persisted conversation. Construction rejects an empty event sequence, mixed conversation IDs, and invalid event order. Commands and lifecycle records may create gaps in projected positions.

The semantic Conversation projection answers:

> What happened in the conversation?

It is the stable source for semantic replay and consumers such as the CLI, automation, search, and future user interfaces.

## Completeness

The Conversation Log must be self-contained with respect to model-visible semantic state. Everything visible to a model, including content, instructions, context, model-visible tool descriptions and schemas, tool requests and responses, and referenced files or images, must be present in the log or immutably referenced by it.

Executable tool implementations, provider credentials, transport configuration, retry policy, and orchestration remain external. Their model-visible descriptions and schemas do not: those must be recorded or immutably referenced so a compatible `ModelDriver` can construct its request from the conversation alone.

## Event-Sourced State

The Conversation Log event-sources requested work, semantic conversation state, and turn lifecycle. Its records preserve command intent and durable facts. The current conversation view is derived by replaying semantic records and resolving immutable content references; it must not depend on a separately mutable representation.

New semantic state is introduced by appending events, never by modifying earlier events. Derived projections, indexes, summaries, and provider requests may be rebuilt from the log and its immutable references.

This does not mean that all of `tog` is part of the model-visible conversation. Provider transport events, raw streaming deltas, credentials, diagnostics, and execution mechanics remain outside the semantic projection. Commands are retained in the ordered log because requested work is durable system input.

## The Event Vocabulary

`ConversationEvent` is a flat vocabulary. There is no separate `Command` or
`Fact` division, no `Message` wrapper, and no `Lifecycle` branch: a request can
be a fact to one consumer and a command to another, and the concrete contract of
each event describes who acts on it. The serialized `type` discriminator is a
storage concern, not a second domain hierarchy.

```text
User
TurnStart
TurnEnd
AssistantResponse
ToolRequest
ToolResponse
ModelRequest
ModelResponse
ModelSpecificEvent
Automation
Context
Tools
```

`User` is the durable record of submitted input: it is both the request to
process and the accepted content. `TurnStart` records actual start and references
the `User` that prompted it; it never substitutes for queued intent. `TurnEnd`
records the terminal turn outcome. `ModelRequest` records one driver/model
attempt (its turn, source, bounded `input_through`, dependencies, and retry
relationship) and is committed by the engine before the driver runs. The engine
records the terminal `ModelResponse` it receives and completes its
`output_event_ids` from the outputs committed during that call.

The vocabulary should grow only when a repeated semantic or lifecycle need justifies another event type.

## Layered Semantic Representation

Conversation events define a universal semantic minimum and permit lossless enrichment beyond that minimum. Every compatible `ModelDriver` must understand the minimum, may interpret recognized enrichment for richer or more efficient continuation, and must safely ignore enrichment it does not understand. The conversation is therefore portable without being restricted to the lowest common denominator.

Driver-specific data enriches portable semantics; it must not replace them. The portable representation must contain enough information for another compatible driver to continue meaningfully, and semantically important structured concepts remain structured. For example, a tool request retains its portable tool name, arguments, and request reference even if it also contains a provider-specific call identifier.

A `ModelRequest` owns the model source and configuration for the attempt it
records. Provider-specific model data that is not part of the portable contract
travels in `ModelSpecificEvent` values or as the opaque attachment on a tool
request. Raw provider transport events do not become semantic conversation
events merely because they are provider-specific.

## Event Meanings

### User

`User` records user-provided input. One event may contain multiple content parts, such as text with an image or file.

Large or binary content belongs in a content store and is referenced by a strongly typed durable ID. Conversation events should not embed large payloads directly.

### Turns

`TurnStart` records that a turn began. It references the `User` event that
prompted it, when one did, and records the input position selected at start.
`TurnEnd` records the terminal outcome of the turn: `succeeded`, or `failed`
with the portable failure details. Assistant output is content, not turn
completion; a successful terminal model call with no outstanding tool work can
complete a turn without assistant text.

### AssistantResponse

`AssistantResponse` records the model's actual response to the conversation. It
references its model request, participates in portable continuation, and is the
content consumers render as the answer.

### ModelRequest, ModelResponse, And ModelSpecificEvent

`ModelRequest` records one attempt: its turn, the model source, the fixed
`input_through` position that bounds the conversation used to construct input,
and any `depends_on` tool requests it waits for. Retried attempts reference
their predecessor through `retry_of`; retry scheduling is runtime policy, not a
requirement of this slice.

`ModelResponse` is the terminal response to one model request. It references the
request, lists the ordered durable output ids it produced, and carries the
attempt outcome (`succeeded` or `failed` with portable failure details) and
known usage. Only one terminal response exists per model request. A stream that
ends without a terminal response is closed by the engine as a failed
`ModelResponse`; it is never treated as success.

`ModelSpecificEvent` carries model-request reference, provider event type,
payload version, provider-specific payload, and an optional human-readable
message. The message is for display only; its presence does not make the event
assistant content or provider input. Compatible drivers interpret their payloads
for native replay; other drivers ignore them.

Operation failures reuse one portable shape: a `category`, a message, retry
guidance, and known-versus-uncertain execution outcome. Tool failure belongs to
the `ToolResponse`, model-attempt failure to the `ModelResponse`, and turn
failure to the `TurnEnd`. Storage failures are never relabeled as provider
failures.

`ModelDriverError` is not durable conversation state. It carries detailed Rust
control-flow information from invocation setup or stream consumption. Provider
failure is recorded as a failed `ModelResponse` on the driver stream or by the
engine on timeout or stream end. Raw provider bodies, credentials, stack traces,
and sensitive request data are not copied into durable events.

For example, several provider events may project to one response:

```text
text.delta "Hel"
text.delta "lo"
output.done
    -> AssistantResponse(model_request_id=..., content="Hello")
```

### Context

`Context` records named, typed conversation-associated values, such as
instructions, working directory, selected files, project, or permissions.
Each context carries a resolved type identifier, a name (defaulting to the
resolved type), and a JSON value; consumers that understand a type interpret and
validate its payload. The latest context event under a name replaces the
effective value for that name regardless of its previous type; earlier events
remain unchanged. Context is distinct from user input, is not automatically sent
to the model, and is not passed to every tool.

A workspace can be recorded as `Context(name: "workspace", type:
"tog.workspace", value: ...)`; workspace shape and its session relationship are
separate work. If workspace and toolset change as one logical operation, their
events are appended atomically.

### Tools

Tool availability is a shared concept understood by model drivers. A `Tools`
event supplies the complete replacement list with its full tool definitions:
each tool pairs a definition with an availability policy (`immediate` for direct
model exposure, `discoverable` for discovery). An empty list clears available
tools, and a new event is emitted when the effective list changes. The effective
tools are derived from the latest declaration within an invocation's history
boundary, preserving definition order and provider-visible content. Model
drivers translate the effective tools into their provider representation;
executable implementations remain the application's responsibility. The
toolset — however the application assembles tools — supplies this event's data
but is not part of the conversation model. The discovery mechanism is unsettled
and is not part of this model.

### ToolRequest And ToolResponse

`ToolRequest` records that a model requested a tool invocation. It references
its model request and carries the tool name, JSON arguments, and optional opaque
data that may preserve a provider-native call identifier while the portable
contract remains provider-neutral.

`ToolResponse` records the result of one request and references exactly one
`ToolRequest` by its event ID. It contains either a successful result or a typed
failure. A response is appended when it arrives, so response order does not need
to match request order; the caller records responses before invoking the model
again.

These events record semantic facts. They do not prescribe whether tools run
sequentially or concurrently, or when the model is invoked again. The caller
owns that orchestration policy; the current session executes sequentially and
bounds continuation rounds.

### Automation

`Automation` records information contributed by an external or asynchronous actor. It is distinct from `ToolResponse`, which answers a model-requested tool invocation.

## Identity, Order, And Relationships

Durable entities and references use strongly typed UUIDv7 identifiers, for
example `ConversationId` and `ConversationEventId`. One `ConversationEventId`
type covers event identity and every cross-event reference; descriptive fields
such as `model_request_id`, `tool_request_id`, and `turn_id` state what a
reference points to. A `ConversationId` remains distinct because it identifies
the conversation.

A complete `ConversationEvent` owns its ID, conversation ID, timestamp, and
content. Event construction assigns the ID and timestamp and receives the
conversation ID; the store preserves them, assigns the record position, and
restores the original identity and timestamp on load. The storage record holds
only a position, a schema version, and the complete event, and lives with the
event-store contract.

Each conversation event also has a monotonically increasing stream position. Identity, order, and semantic relationships serve different purposes:

- the conversation ID identifies the conversation to which the event belongs
- the event ID provides stable identity and is the reference type
- the position provides authoritative replay order within the conversation
- the timestamp records observed wall-clock time but does not determine order
- typed references express semantic relationships

Event positions must not be used as semantic identifiers. Accepted events
validate that referenced events exist in the same conversation and have the
expected type; reconstruction performs the same validation on the whole history.

## Durability And Projection

User input and `TurnStart` are appended before a model attempt. The engine
commits a `ModelRequest` and then invokes the driver with an immutable view of
the conversation bounded by that request's `input_through`. The driver returns a
stream of ordered, nonempty batches limited to `AssistantResponse`,
`ToolRequest`, `ModelResponse`, and `ModelSpecificEvent`. A batch groups events
that must become visible together; single-event batches are the normal case. The
session commits each batch atomically before reporting any of its events, so
readers never observe a partially committed batch. The engine completes the
terminal `ModelResponse` it receives and records its `output_event_ids`; when a
stream ends without a terminal response, the engine records a failed
`ModelResponse` instead.

The append boundary assigns record identity, timestamp, and position. The
session records the terminal turn outcome as `TurnEnd`: a successful terminal
model response with no outstanding tool work succeeds the turn, a failed
response fails it, and a stream ending without a terminal response is failed by
the engine. Completed semantic events already yielded remain valid conversation
facts and appended events are not rolled back.

This supersedes the earlier contract in which a driver emitted session-owned
commands and lifecycle facts and the driver-owned invocation record carried
`ModelInvocationId` provenance. A caller that needs the complete model output
can collect the stream.

Provider-native state may later improve same-provider continuation, but the Conversation Log remains the durable representation used for local reconstruction and cross-provider replay.

## System Boundary

The conversation model deliberately excludes:

- provider request construction and transport
- raw provider events
- model invocation lifecycle and diagnostics
- tool execution policy
- retry and scheduling policy
- CLI rendering and interactive progress

Those concerns consume, produce, or project conversation events without becoming part of the semantic model. Their detailed boundaries are defined in [Conversation and ModelDriver Architecture](conversation-design.md).
