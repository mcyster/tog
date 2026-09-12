# Introduce the ModelDriver Boundary

> Superseded by [Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md) for driver output envelope ownership. The provider-neutral driver boundary remains current.

## Why

The first provider integration established concrete OpenAI Responses transport and replay concerns. Allowing those concepts to remain in turn orchestration would make provider-native history a requirement for conversation continuation and would prevent model or provider switching from semantic history alone.

This is the concrete requirement anticipated by [Begin with a Single Binary Package](2026-08-08-begin-with-single-binary-package.md), which rejected speculative provider abstractions.

## Decision

Introduce a narrow `ModelDriver` boundary. A configured driver exposes its typed provider/model source, receives an immutable view of the semantic Conversation Log, and produces zero or more complete `ConversationEvent`s or a typed model error. The driver records its source while constructing each returned event; the caller persists the returned event. [Use Asynchronous Streaming ModelDriver Invocations](2026-08-22-use-asynchronous-streaming-model-driver-invocations.md) supersedes this decision's original batch delivery and failure semantics, and [Make ModelDrivers Return Conversation Events](2026-08-29-model-drivers-return-conversation-events.md) clarifies that provider-native intermediate events remain private to each driver.

OpenAI request types, streaming events, response IDs, and transport errors remain inside the OpenAI implementation. The Conversation Log is the only durable history required for correctness. Provider-native continuation may be added later only as an optional optimization.

## Consequences

Turn orchestration and the CLI do not depend on OpenAI protocol concepts. Every invocation currently reconstructs provider input from semantic history, which costs more input tokens than provider-native continuation but proves cross-driver portability. The abstraction remains intentionally small and may change when a second production driver provides more evidence. [Use Asynchronous Streaming ModelDriver Invocations](2026-08-22-use-asynchronous-streaming-model-driver-invocations.md) defines incremental persistence and late stream failure behavior.
