# Conversation Model

The Conversation Log is `tog`'s ordered append-only record stream. It records requested work and semantic facts without exposing provider transport protocol.

This document describes the intended conversation model. The [durable execution plan](plans/execute-commands-durably.md) develops its model-call lifecycle and recovery rules. The [Conversation and ModelDriver Architecture](conversation-design.md) describes the earlier Phase 1 implementation; where its invocation ownership or completion rules differ, the intent here takes precedence.

The current implementation still has driver-created invocation records and uses assistant output or problems to determine turn completion. The common model-call lifecycle below is accepted direction, not a claim that it is implemented.

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

## Events Are Facts

The log contains both command records and fact records:

```text
Command
    something should happen

Command record
    a request was received

Fact record
    something was accepted or happened
```

For example, `UserMessageRequested` records input received by the system and `User` records the accepted conversation content. `TurnRequested` requests agent work, while the session records `TurnCompleted` with its terminal outcome. The engine commits a `ModelCallRequest` before invoking a driver; its durable event ID identifies the attempt, and produced model facts reference that request.

The shared event vocabulary is organized by command or fact:

```text
Command
    UserMessageRequested
    TurnRequested
    ModelCallRequest

Fact
    User
    Assistant
    Communication
    Problem
    TurnCompleted
    ModelCallResponse
    ToolsAvailable
    ToolRequest
    ToolResponse
Context
Automation
Data
```

The vocabulary should grow only when a repeated semantic or lifecycle need justifies another record type.

## Model Calls And Turns

A model call is one attempt within a turn. The engine selects a committed input
boundary and its history from one snapshot, then commits a `ModelCallRequest`
containing the model selection, input boundary, and turn association before
invoking the driver. If the request cannot be committed, the driver must not run.
Recording intent after starting an HTTP request leaves a crash window in which
external work has no durable record.

The driver receives that request and immutable input. It translates provider
input and output; it does not create the authoritative attempt identity or accept
pending user requests on the engine's behalf. Provider request IDs remain
optional metadata.

The engine commits output associated with the request and one terminal
`ModelCallResponse`. The response records the observed outcome, ordered output
references, and known usage with its completeness. The driver supplies provider
observations; the engine also records failures it observes outside the driver,
such as a timeout. Stream exhaustion without a terminal response is not success,
even if assistant text was already committed.

An assistant message is content, not turn completion. It may precede tool
requests, accompany them, or be absent from a successful call. A turn succeeds
when its terminal model call succeeds and no tool work or continuation remains
outstanding. The engine records that decision as `TurnCompleted`; a final
assistant message is not required. Closing a call that requested tools does not
close its turn. A failed call may be retried within a turn that ultimately
succeeds.

## Layered Semantic Representation

Portable meaning and provider replay state serve different purposes. This
separation is part of tog's original intent: changing drivers should preserve
meaningful conversation history without requiring another provider's protocol.

| Representation | Purpose |
| --- | --- |
| Portable content | User input, assistant content, structured tool requests and results understood by compatible drivers |
| Provider replay state | Opaque, versioned data a compatible driver can use to preserve native continuation |
| Execution records | Requests, outcomes, dependencies, and lifecycle used by the engine; not prompt text by default |

Driver-specific data enriches portable semantics; it must not replace them. A
tool request retains its portable identity, tool name, and arguments even when
it also preserves a provider-specific call ID. Common completion, failure, and
dependency meaning must remain readable without the originating driver.

Preserve provider state through serialization and reconstruction even when its
driver is unavailable. Reading the log does not require interpreting that state.
Using it for native replay does: a driver must recognize its format and version.
A reasoning summary is portable communication, not a substitute for opaque
reasoning state, and opaque state must never be converted into prompt text.

Portable continuation may omit optional enrichment. If the requested native
continuation requires state the selected driver cannot interpret, report that
limitation explicitly; do not silently claim equivalent replay. Preserving data
alone does not implement replay, and replay does not promise identical model
responses. Raw transport deltas remain outside the durable semantic vocabulary.

The engine determines the input boundary and eligible history. The driver
converts that history to provider input and interprets supported replay state.
Decorators are an implementation style for this separation, not a requirement
of the conversation model.

## Event Meanings

### User

`User` records user-provided input. One event may contain multiple content parts, such as text with an image or file.

Large or binary content belongs in a content store and is referenced by a strongly typed durable ID. Conversation events should not embed large payloads directly.

### Assistant And Communication

`Assistant` records the model's actual response to the conversation. It participates in portable continuation and is always `Important`. Its model-call request reference identifies the producing attempt; provenance belongs to that request.

`Communication` records auxiliary model-produced information such as detailed reasoning, reasoning summaries, status, or emerging concepts. Communications are persisted but are not automatically replayed as assistant responses.

Communication importance has three ordered levels: `Detailed`, `Interesting`, and `Important`. Consumers decide which messages to present, and the CLI maps low, medium, and high verbosity to progressively broader levels. Repeated cross-driver communication concepts may later receive more specific top-level event kinds.

Both event kinds retain meaningful portable messages. Optional `ModelData` may preserve native fidelity or improve continuation; understanding their portable meaning never requires it, while native replay may. Exposed reasoning is aggregated into coherent communications rather than persisting every transport delta.

### Problem

`Problem` records a `ConversationProblem` as a top-level conversation event. It is not model output merely because it concerns a model invocation. Applicable problems may carry an invocation ID and event-specific model data, but do not repeat invocation provenance. `ConversationProblem::Issue` records a semantic model limitation or unsuccessful outcome, such as refusal or context exhaustion. `ConversationProblem::Invocation` records a sanitized operational invocation failure. Every concrete problem provides one meaningful message, and the shared parent exposes that message and whether retrying the unchanged invocation may reasonably succeed. The enclosing conversation event does not duplicate the message and does not add generic severity.

There is no `Other` problem kind. A newly understood semantic problem receives a specific shared kind, while unusable provider output and unclassified invocation failure retain their distinct existing meanings. Problems are not automatically projected into every provider request; the engine's input projection selects eligible history, and the driver translates it.

Failure belongs to the operation that failed:

| Operation | Durable outcome |
| --- | --- |
| Tool execution | The correlated `ToolResponse`, including failure details |
| Model attempt | Its `ModelCallResponse`, including failure details |
| Turn | `TurnCompleted` with the engine's terminal outcome |

A problem message may explain a failure, but its presence does not replace the
operation's outcome or automatically fail the entire turn. A tool failure can be
input to a subsequent model call, and a failed model attempt can be retried.
Problems outside a particular operation remain conversation-level information.

`ModelDriverError` carries Rust control-flow information, not durable state. The
engine must record an appropriate call outcome when such a failure escapes, if
storage remains available. Storage failure is not provider failure: an append
failure may leave a request without a response for recovery to reconcile.
Committed partial output remains in history. Raw provider bodies, credentials,
stack traces, and sensitive request data are not copied into durable failures.

For example, several provider events may project to one response:

```text
text.delta "Hel"
text.delta "lo"
output.done
    -> Assistant(model_call_request=..., message="Hello")
```

### ToolsAvailable

`ToolsAvailable` records the complete toolset offered by the caller before a
model invocation. The latest declaration in the conversation is authoritative:
each declaration replaces the previous toolset, and an empty list removes all
tools. Earlier declarations remain in the ordered history. The event is
conversation context, not a user or assistant message.

### ToolRequest And ToolResponse

`ToolRequest` records that a model requested a tool invocation. Each request has
a stable portable `ToolCallId`, the tool name, JSON arguments, the producing
model-call request reference, and optional `ModelData` that may preserve a provider-native
call identifier while the portable contract remains provider-neutral.

`ToolResponse` records the result of one request and references exactly one
`ToolCallId`. It contains either a successful result or a typed execution
problem. A response is appended when it arrives, so response order does not need
to match request order; the caller records responses before invoking the model
again.

These events record semantic facts. They do not prescribe whether tools run
sequentially or concurrently, or when the model is invoked again. The caller
owns that orchestration policy; the current session executes sequentially and
bounds continuation rounds.

### Context

`Context` records state that may affect later model invocation, such as instructions, working directory, selected files, project, or permissions. Context is distinct from user input.

### Automation

`Automation` records information contributed by an external or asynchronous actor. It is distinct from `ToolResponse`, which answers a model-requested tool invocation.

### Data

`Data` records durable machine-readable metadata such as external IDs, usage summaries, annotations, tags, or diagnostics. It is not model input by default.

## Identity, Order, And Relationships

Durable entities and references use strongly typed UUIDv7 identifiers. Distinct types prevent accidental substitution, for example:

```text
ConversationId
ConversationEventId
ConversationCommandId
ConversationTurnId
ToolCallId
ImageId
FileId
```

Each conversation event also has a monotonically increasing stream position. Identity, order, and semantic relationships serve different purposes:

- the conversation ID identifies the conversation to which the event belongs
- the event ID provides stable identity
- the position provides authoritative replay order within the conversation
- the timestamp records observed wall-clock time but does not determine order
- typed references such as `ToolCallId` express semantic relationships

Event positions must not be used as semantic identifiers.

## Durability And Projection

User input and the model-call request are committed before driver invocation.
The driver returns ordered, nonempty batches of completed semantic output
associated with that request. A batch groups events that must become visible
together; single-event batches are the normal case. The session commits each
batch atomically before publishing its events or dispatching requested work.
The append boundary assigns durable envelope metadata.

The consumer controls demand by polling for the next batch. Receiving several
events or batches does not represent several model calls. Completed output stays
durable when a later batch or the call fails; it is not rolled back. A caller
that needs the complete output can collect the stream.

The engine validates output ownership and records terminal call and turn
outcomes independently of content. Unknown usage stays unknown rather than
being counted as zero. Recovery reconciles requests without outcomes; replaying
history alone must not execute them.

The Conversation Log remains the source for local reconstruction, portable
continuation, and provider replay where supported.

## System Boundary

The conversation model deliberately excludes:

- provider request construction and transport
- raw provider events
- provider transport diagnostics
- tool execution policy
- retry and scheduling policy
- CLI rendering and interactive progress

Those concerns consume, produce, or project conversation events. Model-call and turn lifecycle records do belong in the log, even though they are not ordinary prompt content. Their intended execution boundaries are defined in the [durable execution plan](plans/execute-commands-durably.md).
