# Drivers Emit Restricted Conversation Messages

## Status

Accepted

## Context

`ModelDriver` output was a `DriverConversationEvent` whose shared branch wrapped
the broad `ConversationFact` vocabulary. A driver could therefore produce
session-owned lifecycle facts such as `TurnCompleted`, and the session relied on
runtime checks to reject wrong turn identities and output after completion. The
type system did not express the ownership boundary described in
[Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md).

## Decision

Conversation owns the shared message vocabulary. `ConversationMessage` defines
the four permitted message payloads and their content, invocation identity,
optional model data, and user-request association:

```rust
enum ConversationMessage {
    User { caused_by, content },
    AssistantResponse { invocation_id, data, response },
    Communication { invocation_id, data, communication },
    Problem { invocation_id, data, problem },
}
```

`ConversationFact` reuses that definition rather than repeating its fields. A
message fact carries an optional session-assigned `turn_id` beside the shared
message payload, and turn completion remains a separate lifecycle fact:

```rust
enum ConversationFact {
    Message { message: ConversationMessage, turn_id: Option<ConversationTurnId> },
    Lifecycle(ConversationLifecycle),
}
```

Conversation neither defines nor imports model-driver types.

The model-driver API owns which outputs a driver may return:

```rust
enum ModelDriverOutput {
    Message(ConversationMessage),
    Command(Box<dyn ConversationEventExtension>),
    Extension(Box<dyn ConversationEventExtension>),
}
```

Drivers cannot emit `UserMessageRequested`, `TurnRequested`, or `TurnCompleted`,
and driver extensions cannot carry a shared lifecycle fact. The session attaches
the current turn association where appropriate, validates user-request
associations, and converts each driver output into the persisted vocabulary.

The session owns turn completion. It records `TurnCompleted` after the driver
stream ends. An assistant response ends a successful turn; any problem fails the
turn even when an assistant response was already persisted. Stream exhaustion
without an assistant response or problem is an incomplete turn. Drivers create
invocation identities and driver-defined invocation events; they do not record
or assign durable envelope metadata.

## Consequences

The ownership boundary is expressed in the type system instead of enforced by
runtime rejection. Conversation, driver output, and persisted facts share one
authoritative definition of each message payload. The session no longer needs
`WrongTurnIdentity`, `MissingTurnIdentity`, or `OutputAfterCompletion` errors.
Persisted lifecycle names and event classes are unchanged.
