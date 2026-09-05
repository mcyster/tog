# Separate Portable Conversation Meaning from Model Data

> Superseded by [Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md) and the current conversation record implementation. This file remains historical context for the model-data separation.

## Why

Model-specific data lived inside the portable event vocabulary: `AssistantResponse` and `ModelCommunication` carried `extensions`, and `ModelIssue::Other` accepted arbitrary extension objects. The `Problem` kind also repeated the invoked `ModelSource`, which made problems look like model output merely because they concerned a model invocation. At the driver boundary, the earlier output wrapper ran as a second channel beside model events, so a model-reported problem was not visibly a conversation event on the driver's stream.

These choices obscured the portability contract: another driver could not tell which parts of a durable event it must understand to continue the conversation.

## Decision

Every `ConversationEvent` carries a flattened portable `ConversationEventKind`. Model-associated kinds carry `ModelDetails`, which keeps the source and optional data together:

```rust
struct ConversationEvent {
    // identity, position, timestamp, and schema
    kind: ConversationEventKind,
}

enum ConversationEventKind {
    User { content: Vec<UserContent> },
    Model { model: ModelDetails, event: ModelEvent },
    Problem { model: Option<ModelDetails>, problem: ConversationProblem },
}

struct ModelDetails {
    source: ModelSource,
    data: Option<ModelData>,
}
```

`ModelData` is opaque to the conversation and serializes as JSON. `ModelDetails` keeps it with its `ModelSource`, so the driver can decide whether it knows how to interpret the content. The driver that creates it defines and interprets it; any other driver may ignore it safely. Model data is recorded when the event is created; later drivers never mutate old events to attach their own representations.

The portable kind contains the complete meaning of the event. `AssistantResponse` and `ModelCommunication` no longer carry extensions. A model-associated `Problem` carries `Some(ModelDetails)`; a future unrelated problem may carry `None`. `ModelProblem` is renamed `ConversationProblem`.

There is no `Other` problem kind. A newly understood semantic problem receives a specific shared `ModelIssue` kind, while unusable provider output (`InvocationError::InvalidProviderResponse`) and unclassified invocation failure (`InvocationError::ProviderFailure`) retain their distinct existing meanings.

A `ModelDriver` receives the portable `Conversation` and produces one stream of complete `ConversationEvent` values. Provider-specific intermediate events are internal to the concrete driver. `ModelDriverError` remains operational control flow; the caller may also record an appropriate sanitized `Problem`. The caller persists returned conversation events.

## Consequences

Portable conversation meaning and optional model data are visibly separate, and switching drivers never requires the previous driver to read the conversation. Reconstructing a conversation never requires the original driver or its historical version.

Events persist at schema version 10. Earlier problem events load without their source, and earlier events load without their extensions, because the conversation no longer models those fields. Events persisted with `ModelIssue::Other` no longer deserialize; no production driver ever emitted them.

This decision supersedes the extension mechanism described in the earlier problem decision and restates its problem surface without `ModelSource`. The linked driver-boundary decision amends the stream boundary so `ModelDriverEvent` is internal to each driver and the public stream returns `ConversationEvent`s.
