# Represent Model-Associated Problems as Conversation Events

> Superseded by [Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md) for event taxonomy and turn completion. The distinction between semantic problems and operational errors remains current.

## Why

Model limitations and invocation failures both need a portable, sanitized durable representation. They remain distinct from detailed Rust control-flow errors, but both are facts associated with a selected model invocation and therefore have a meaningful `ModelSource`.

Introducing a broad conversation-problem hierarchy or separate top-level event variants would predict requirements for tool, filesystem, and orchestration failures that do not yet exist. A trait hierarchy would also duplicate the closed enum hierarchy needed for serialization.

## Decision

Use `ConversationEventKind::Problem { model, problem }` as the one durable problem surface. A model-associated problem carries `Some(ModelDetails)`; a future unrelated problem may carry `None`. `ConversationProblem::Issue(ModelIssue)` represents a meaningful limitation or unsuccessful model outcome, and `ConversationProblem::Invocation(InvocationError)` represents a sanitized invocation failure. `ConversationEventKind::Model` remains limited to successful semantic `ModelEvent` output.

`ConversationProblem` is a closed serializable enum. Every concrete variant provides one meaningful sanitized message. The parent delegates common `message` and `retryable` behavior to its categories without introducing a trait. The conversation event does not duplicate the message and does not add generic severity or impact fields.

A concrete driver translates provider-specific information into a complete `ConversationEventKind::Problem` with `ConversationProblem::Issue` and returns it on the conversation-event stream. `ModelDriverError` remains the detailed error returned by the invocation future or stream. The turn service converts that error into a sanitized invocation problem with `Some(ModelDetails)` and `data: None`, persists it, and then returns the original error for Rust control flow.

The decision in [Separate Portable Conversation Meaning from Model Data](2026-08-29-separate-portable-conversation-meaning-from-model-data.md) supersedes the `ModelSource` on the problem kind, the `ModelIssue::Other` extension channel, and the earlier driver-output shape: problems are now stream events named `ConversationProblem`, and model-specific data lives in `ModelDetails`. [Make ModelDrivers Return Conversation Events](2026-08-29-model-drivers-return-conversation-events.md) further makes the public stream return complete `ConversationEvent`s rather than an intermediate driver-event type.

Durable problem messages must not contain credentials, authorization headers, raw provider bodies, stack traces, sensitive request data, or diagnostics without portable conversational meaning. Detailed diagnostics may remain in `ModelDriverError` and application logging.

## Consequences

Later orchestration can observe preceding failures and limitations from immutable conversation history. Provider projection decides whether and how to expose a problem to a later model; retaining it canonically does not require verbatim replay in every provider request.

The current problem hierarchy remains model-specific. Future tool, filesystem, or orchestration problems will be designed from their concrete provenance and behavior rather than being forced into this model-associated event.
