# Simplify conversation events and the ModelDriver API

For the goal, boundaries, and remaining choices, start with the
[plan](../plans/conversation-design.md). This document provides the detailed design.

This is the earlier proposal, retained for its rationale and alternatives. The
flat vocabulary and restricted driver output are now implemented. Its `Data`
event, open vocabulary questions, Rust sketches, and persistence envelope are
not the current contract. Use the [current boundary](../architecture/conversation.md)
for established behavior; do not implement this proposal wholesale.

Flatten the conversation vocabulary and make the driver boundary explicit: a
driver accepts the full event vocabulary and produces only a permitted subset.
Keep event meaning, producer permissions, and persistence metadata distinct
without building a separate hierarchy for each.

The proposal below predates the current implementation. It refines the
[conversation model](../conversation.md); the
[durable execution plan](../plans/execute-commands-durably.md) remains the home for
dispatch, dependencies, retries, and recovery. The older
[Phase 1 design](../conversation-design.md) records the still earlier API.

## Event vocabulary

`ConversationEvent` directly represents the following events. The model-related
column describes a domain relationship; the driver-output column describes
permission to produce an event.

| Event | Meaning | Model-related | Driver output |
| --- | --- | --- | --- |
| User | Accepted user content | No | No |
| TurnStart | The engine began a turn | No | No |
| TurnEnd | The engine ended a turn with an outcome | No | No |
| AssistantResponse | Assistant content | Yes | Yes |
| ToolRequest | A model requested tool execution | Yes | Yes |
| ToolResponse | The result of a particular tool request | No | No |
| ModelRequest | One model attempt, its selection and fixed input boundary | Yes | No |
| ModelResponse | The observed outcome and usage of that attempt | Yes | Yes |
| ModelSpecificEvent | Provider-specific state with an optional display message | Yes | Yes |
| Automation | Information contributed by an external or asynchronous actor | No | No |
| Context | Context that may affect later model input | No | No |
| Data | Durable application data, not model input by default | No | No |

Use `ModelRequest` and `ModelResponse` here for the events called
`ModelCallRequest` and `ModelCallResponse` in the existing plan.
`TurnEnd` is the proposed name for `TurnCompleted`. These are naming
refinements, not new layers or additional lifecycle events.

### Remove structural classifications

Remove `Fact` and `Command` as branches of the event hierarchy. A
`ToolRequest` records the fact that the model requested work and asks the tool
executor to act. Whether something is a command depends on the consumer, so
that distinction does not provide a useful exclusive partition of events.
Each concrete event's contract states who produces it and who acts on it.

Remove `Kind` as a separate domain layer. `ConversationEvent` already
distinguishes its variants; it does not need a parallel
`ConversationEventKind` or `StoredConversationEventKind` domain vocabulary.
A serialized discriminator such as `type` is still necessary and is not
another event hierarchy.

Likewise, `Message` and `Lifecycle` are not intermediate branches in the
proposed vocabulary. The concrete events remain directly visible.

### ModelEvent is not the driver's output type

`ModelEvent` describes the model-related subset above. It includes
`ModelRequest`, which the engine must commit before invoking a driver.
Therefore it cannot also mean "everything a driver may return."

Keep that relationship visible without duplicating event payload definitions.
Whether Rust should express it as a trait, an enum, or an accessor remains a
representation choice to settle when shaping the declarations. Do not turn the
table into another mandatory nesting layer merely to label model-related events.

## ModelDriver boundary

The conceptual invocation is:

```rust
invoke(
    request: &ModelRequest,
    conversation: &dyn Conversation,
) -> ModelOutputStream
```

This sketch omits async lifetimes and wrappers; it describes ownership and
inputs, not a final Rust signature. The request passed to the driver must retain
the durable identity of the committed request so outputs can reference it.

The conversation exposes every event type through an immutable view bounded by
the request's input position. Accepting all events does not mean sending every
event to the provider. Input projection determines eligible content and context;
the concrete driver translates it into provider input. A decorator is a coding
style for this separation, not part of the event contract.

The stream contains ordered, nonempty batches of `ModelOutputEvent` values:

```rust
enum ModelOutputEvent {
    AssistantResponse(AssistantResponse),
    ToolRequest(ToolRequest),
    ModelResponse(ModelResponse),
    ModelSpecificEvent(ModelSpecificEvent),
}
```

This is a proposed restricted union. Its variants reuse the exact payload types
used by `ConversationEvent`; they do not introduce driver-owned copies.
Conversion into a conversation event is lossless. The driver cannot emit user
content, model requests, tool responses, or turn boundaries. Generic command or
extension output must not bypass that restriction.

The engine can also produce `ModelResponse`, for example after a timeout.
Permission for a driver to produce it is not exclusive ownership. The engine
validates and commits accepted output, ensures a single terminal response per
request, and owns continuation and turn completion.

The engine commits `ModelRequest` before invoking the driver. Assistant content
does not complete a turn. A successful terminal call with no outstanding work
can end a turn without assistant text; an exhausted stream without a terminal
response is not success. A failed attempt can be retried within a successful turn.

## ModelSpecificEvent

Replace `Communication` with `ModelSpecificEvent` carrying an optional
human-readable message. One provider item can retain both its replay state and
its useful display representation without becoming two events.

Conceptually:

```rust
struct ModelSpecificEvent {
    model_request_id,
    event_type,
    payload_version,
    payload,
    message: Option<String>,
}
```

The referenced request identifies the producing driver and model. The event
type and payload version tell a compatible driver how to interpret the payload.
A message may contain a reasoning summary or progress explanation. Its absence
is normal when the event only carries opaque state.

A display message does not make the event assistant content or automatically
make it model input. A summary is not a substitute for opaque replay state.
Preserve the payload when its driver is unavailable; report unsupported required
state explicitly when native replay is requested. Portable continuation can use
portable content without interpreting optional provider enrichment.

This clarifies tog's existing portability intent. It does not introduce a new
requirement that all providers expose reasoning or that replay produce identical
responses. Whether the old communication importance levels remain useful for
display is unresolved; do not carry them over automatically.

## Persistence envelope

`ConversationEventRecord` is only the persistence envelope:

```rust
struct ConversationEventRecord {
    conversation_id,
    id,
    position,
    timestamp,
    schema_version,
    event: ConversationEvent,
}
```

The metadata supports identity, ordering, and persistence. It adds no semantic
classification and no independent event vocabulary. Serialization may need
internal machinery for extensible payloads, but consumers should not navigate a
second rich hierarchy to reach the event. Preserve the existing atomic batch
contract; transaction framing remains a storage concern outside event payloads.

## Failures and remaining choices

Failure belongs to the operation that failed: `ToolResponse`,
`ModelResponse`, or `TurnEnd`. An explanatory message does not replace that
outcome, and failure of one attempt does not automatically fail its turn.
Storage failure must not be mislabeled as provider failure.

A common failure capability, such as `failure() -> Option<&Failure>`, could
let consumers inspect these outcomes consistently. That is a possible mixin or
trait, not an accepted new event branch. Decide the shared failure fields and
operation-specific details before choosing its type.

Resolve these questions before treating the design as a complete implementation
specification:

- Whether errors outside a particular operation need a standalone `Problem`
  event, and what produces it.
- How to express the model-related grouping in Rust without redundant wrappers.
- Where pending work intent currently carried by `UserMessageRequested` and
  `TurnRequested` belongs. `TurnStart` records actual start and cannot silently
  replace queued intent. Simplifying classification must not lose durable requests.
- How the existing `ToolsAvailable` declaration fits the proposed vocabulary,
  including its schemas and replacement semantics; whether it belongs in
  `Context` is not yet decided.
- Whether event-specific `ModelData` attachments remain alongside standalone
  `ModelSpecificEvent` values, and how they avoid duplicating replay state.
- How existing stored events are migrated or explicitly rejected when the new
  vocabulary is implemented.

## Implementation scope and validation

Start by settling the open declaration choices, then implement the common model
lifecycle through the existing execution loop. Keep concurrent tools, a daemon,
and automatic retries in the durable execution plan rather than requiring them
for this hierarchy change.

When implemented, verify that:

- The driver accepts history containing the full event vocabulary but can only
  return its permitted subset.
- Shared event payloads have one definition and survive persistence round trips.
- A failed request commit prevents invocation, and output references the
  committed request.
- Assistant content, model-call closure, and turn closure remain independent.
- Provider state with and without a message survives storage, while display text
  never implicitly substitutes for replay state.
- Tool, model, and turn failures remain associated with their own operations.
