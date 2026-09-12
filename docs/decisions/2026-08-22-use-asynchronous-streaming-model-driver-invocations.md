# Use Asynchronous Streaming ModelDriver Invocations

> Superseded by [Record Commands And Turn Lifecycle](../notes/2026-09-05-record-commands-and-turn-lifecycle.md) for the durable record shape and append boundary. The asynchronous streaming choice remains current.

## Why

The original `ModelDriver` contract returned a batch of semantic model events only after an entire provider invocation succeeded. That contract could not expose completed semantic output incrementally or preserve it when a provider stream failed later.

`ModelDriver` is expected to become a reusable first-class abstraction used by command-line applications, servers, user interfaces, concurrent tool execution, and multiple conversations. Potentially long-running network operations at this boundary therefore need an asynchronous contract. Provider protocols may also produce many transport events during one request, but those protocol events are not the semantic output consumed by the rest of `tog`.

## Decision

One `ModelDriver::invoke` call represents one provider/model invocation and asynchronously establishes a stream of completed semantic `ConversationEvent`s. The intended interface is approximately:

```rust
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

pub(crate) type ModelOutputStream =
    BoxStream<'static, Result<ConversationEvent, ModelDriverError>>;

pub(crate) trait ModelDriver {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        conversation: &'invoke Conversation,
    ) -> BoxFuture<
        'invoke,
        Result<ModelOutputStream, ModelDriverError>,
    >;
}
```

This has the conceptual shape `Future<Stream<ConversationEvent>>`, or `Mono<Flux<ConversationEvent>>` in Reactor terminology. The outer future constructs the request and establishes the provider invocation. It may fail before a stream exists because of request construction, authentication, connection, or HTTP errors. Once established, the stream yields `Result<ConversationEvent, ModelDriverError>` because the invocation may fail after streaming begins. The consumer controls demand by polling for the next event. A caller that needs batch behavior may collect the stream; no separate batch interface is required. The driver-boundary decision clarifies that provider-native intermediate events remain private to each concrete driver.

For OpenAI, one invocation uses one REST request and one SSE response stream. Consuming several events from that stream does not make several model requests. Provider protocol events, raw text deltas, and intermediate driver events remain private to the concrete driver. The driver aggregates provider deltas and returns only completed `ConversationEvent`s. `ModelEvent::Assistant` and `ModelEvent::Communication` are successful model events; semantic model issues are top-level problem events. Raw SSE deltas are not `ConversationEvent`s and are not persisted merely because they arrived.

Completed semantic `ConversationEvent`s may be displayed and appended incrementally while the provider invocation remains active. If the stream later fails, already yielded completed events remain valid conversation facts and already appended events are not rolled back. Incomplete provider deltas that never formed a completed `ModelEvent` are discarded. [Represent Model-Associated Problems as Conversation Events](2026-08-23-represent-model-associated-problems-as-conversation-events.md) defines how the caller records the stream error as a sanitized model-associated problem. This supersedes the previous contract under which every model event was withheld until the complete invocation succeeded and all model output was discarded after a late provider failure.

The driver-boundary decision supersedes this decision's intermediate output shape: concrete drivers now keep provider-native intermediate events private and return complete `ConversationEvent`s.

Asynchronous operation is the architectural decision. Standard-library `Future` and async/await provide the language foundation. `futures-util` provides the conventional `BoxFuture`, `BoxStream`, and stream adapters. Tokio is the async runtime, and Reqwest uses its asynchronous client and streaming response support. Explicit `BoxFuture` return values support dynamic `Box<dyn ModelDriver>` dispatch, so `async-trait` is not currently required.

The application owns and starts the runtime; reusable components must not secretly create private runtimes. Tokio is the conventional runtime choice after choosing asynchronous networking, and its features should be enabled narrowly rather than using `full` by default. Blocking work must not run directly on async runtime workers when it can materially delay other tasks.

## Consequences

Callers can present and persist completed semantic output with stream backpressure before an invocation finishes, and late failures no longer invalidate earlier completed facts. Drivers must distinguish setup failures from failures after stream establishment and must retain incomplete protocol aggregation privately until it forms a complete semantic event.

The model boundary uses `futures-util`, Tokio, and asynchronous Reqwest support. Dependency versions, exact Tokio feature flags, persistence asyncness, orchestration, cancellation, timeouts, retries, raw token streaming, background requests, and provider-native response continuation remain implementation-pressure decisions.
