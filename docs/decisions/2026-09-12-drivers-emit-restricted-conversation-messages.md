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

`ModelDriver::invoke` returns a stream of `DriverConversationMessage` values
containing only permitted variants:

```rust
enum DriverConversationMessage {
    User { command_id, content },
    AssistantResponse { invocation_id, data, response },
    Communication { invocation_id, data, communication },
    Problem { invocation_id, data, problem },
    Command(Box<dyn ConversationEventExtension>),
    Extension(Box<dyn ConversationEventExtension>),
}
```

The session converts those variants into the broader persisted event vocabulary.
Drivers cannot emit `UserMessageRequested`, `TurnRequested`, or `TurnCompleted`,
and driver extensions cannot carry a shared lifecycle fact. `User` requires the
accepted `command_id`, so an unknown or repeated request is the only remaining
user-acceptance check.

The session owns turn completion. It records `TurnCompleted` after the driver
stream ends. An assistant response ends a successful turn; any problem fails the
turn even when an assistant response was already persisted. Stream exhaustion
without an assistant response or problem is an incomplete turn. Drivers create
invocation identities and driver-defined invocation events; they do not record
or assign durable envelope metadata.

## Consequences

The ownership boundary is expressed in the type system instead of enforced by
runtime rejection. The session no longer needs `WrongTurnIdentity`,
`MissingTurnIdentity`, or `OutputAfterCompletion` errors. Persisted lifecycle
names and event classes are unchanged; drivers simply no longer produce
lifecycle facts.
