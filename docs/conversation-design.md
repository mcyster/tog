# Conversation and ModelDriver Architecture

**Status:** Phase 1 asynchronous streaming text milestone implemented
**Purpose:** Define a simple, durable conversation model and a narrow `ModelDriver` boundary that can be implemented against the OpenAI Responses API now and can support switching models/providers within a conversation.

This design is intentionally incomplete.

Phase 1 is not trying to build a perfect event-sourcing framework, a durable provider-protocol log, a distributed runtime, or a universal multi-provider SDK.

The priority is:

> Get the semantic conversation model and ModelDriver boundary right first.

The most important Phase 1 invariant is:

> Every ModelDriver must be able to continue a conversation using the Conversation Log alone, regardless of which ModelDriver produced the earlier events.

People are expected to change models during a conversation far more often than they are expected to replay historical provider protocol streams.

Where uncertain:

> Prefer a simple semantic contract now and preserve room for richer tracing and provider-native optimizations later.

---

## 1. Architectural overview

Phase 1 has one durable ordered log containing commands and semantic facts:

```text
Conversation Log
    requested work
    semantic history
    turn lifecycle
    durable replay source
```

A `ModelDriver` consumes an immutable reference to the reconstructed conversation. One asynchronous invocation establishes a stream that yields completed semantic event kinds:

```text
immutable Conversation
    → asynchronous ModelDriver invocation
    → stream of completed semantic facts
    → append boundary assigns record metadata
    → facts appended and presented incrementally
```

One invocation is one provider/model invocation. For OpenAI, it is one REST request with one SSE response stream; consuming several semantic events from that stream does not make several model requests.

Provider-specific details such as OpenAI Responses events, response IDs, token timing, reasoning protocol state, and HTTP diagnostics are **not part of the Phase 1 semantic replay contract**.

They may be captured through logging/tracing for observability.

Later, concrete benefits may justify making some of that provider-specific information durable, but semantic replay must not depend on it.

---

# Conversation model

## 2. Conversation

A conversation begins with its first accepted semantic event. The durable log is authoritative; `Conversation` is an immutable projection reconstructed from non-command records.

Conceptually:

```rust
struct Conversation {
    id: ConversationId,
    events: Vec<ConversationEvent>,
}
```

There is no independently persisted conversation record and no empty persisted conversation. Construction validates that the sequence contains at least one event, all events carry the same `ConversationId`, and positions are strictly ordered. Commands and lifecycle records may create gaps in projected positions.

The Conversation Log answers:

> What happened in the conversation?

It is the durable source for:

```text
ModelDriver input
CLI projection
model switching
semantic replay
automation
search/indexing
future UIs
```

---

## 3. ConversationEvent and ConversationEventKind

`ConversationEvent` is the complete canonical log record persisted and replayed. Shared `ConversationEventKind` contains command records and portable semantic facts. Driver-defined records use an opaque `DriverEventEnvelope` in the same log and are excluded from the model-facing `Conversation` projection.

Conceptually:

```rust
enum ConversationEventKind {
    UserMessageRequested { ... },
    User {
        content: Vec<UserContent>,
    },
    TurnRequested { ... },
    Assistant {
        model: ModelDetails,
        invocation_id: ModelInvocationId,
        response: AssistantResponse,
    },
    Communication {
        model: ModelDetails,
        invocation_id: ModelInvocationId,
        communication: ModelCommunication,
    },
    Problem {
        model: Option<ModelDetails>,
        problem: ConversationProblem,
    },
    TurnCompleted { ... },
    ToolRequest(...),
    ToolResponse(...),
    Context(...),
    Automation(...),
    Data(...),
}
```

`ModelDetails` keeps the model source and optional model-native data together. Model-produced facts carry a stable `ModelInvocationId`; several facts can refer to one driver-defined invocation record. A model-associated problem has model details, while an unrelated problem may use `None`. User events cannot carry model details. The portable kind contains the complete meaning of the event; model data never does.

`DriverEventEnvelope` stores the driver name, driver version, event type, event
schema version, human-readable description, and opaque JSON payload. A driver
provided decoder may reconstruct its concrete event. If the driver is
unavailable, the envelope remains readable and preserved without decoding.

The vocabulary should grow only when a concrete repeated semantic need justifies another event type.

OpenAI Responses events such as `response.created`, text deltas, and function argument deltas are not themselves conversation events.

---

## 4. Commands and events

The log records both command intent and resulting facts:

```text
UserMessageRequested
TurnRequested
ToolExecutionRequested
```

The distinction is semantic, not physical:

```text
Command record
    a request was received

Fact record
    something was accepted or happened
```

Commands remain in the ordered log for input visibility and future replay. The
model-facing projection excludes them. `TurnCompleted` is an explicit fact and
does not need to be inferred from an assistant response or a problem.

Do not force every command into:

```text
handle(command) -> Vec<Event>
```

The caller requests a turn. The driver owns model invocation identities and
driver-defined invocation records. Model invocation, tools, and external I/O
naturally involve streaming, failures, and incremental output.

---

## 5. Strongly typed identifiers

Durable entities and cross-event references use strongly typed UUIDv7 identifiers.

Conceptually:

```rust
struct ConversationId(Uuid);
struct ConversationEventId(Uuid);
struct ConversationCommandId(Uuid);
struct ConversationTurnId(Uuid);
struct ModelInvocationId(Uuid);
struct ToolCallId(Uuid);
struct ImageId(Uuid);
struct FileId(Uuid);
```

More typed IDs should be introduced when a concrete durable entity requires one.

The compiler should prevent accidental substitution of one identifier type for another.

Serialized forms should include a type prefix where practical:

```text
conversation_019...
conversation_event_019...
tool_call_019...
image_019...
file_019...
```

The verbosity is intentional. Explicit IDs are easier for humans and models to distinguish and reduce accidental or guessed references.

UUIDv7 ordering is useful for locality and diagnostics but is not authoritative replay ordering.

---

## 6. Event positions and replay order

Identity and ordering solve different problems.

Each conversation event has a monotonically increasing position:

```rust
struct ConversationEvent {
    conversation_id: ConversationId,
    position: u64,
    id: ConversationEventId,
    timestamp: OffsetDateTime,
    schema_version: u32,
    #[serde(flatten)]
    kind: ConversationEventKind,
}
```

- `conversation_id` identifies the conversation to which the event belongs
- `id` gives stable event identity
- `position` gives authoritative replay order
- `timestamp` records observed wall-clock time
- `schema_version` permits persisted-format evolution

Phase 1 may assume a single writer and use a simple position allocator.

We do not need locks, distributed sequencing, compare-and-append, or a global event clock yet.

The invariant is simply:

> Replay the Conversation Log in position order.

Future persistence implementations may strengthen atomic allocation without changing the semantic model.

---

## 7. Semantic relationships are not ordering

Semantic relationships use typed IDs rather than stream positions.

For example:

```rust
struct ToolResponse {
    tool_call_id: ToolCallId,
    // ...
}
```

The `ToolCallId` identifies which request the response answers.

The event position identifies when the response entered the conversation.

Phase 1 does not require a generic causal graph or arbitrary predecessor relationships.

---

## 8. User content and external blobs

`User` records user-provided input.

A user event may contain multiple content parts:

```rust
enum UserContent {
    Text(String),
    Image(ImageId),
    File(FileId),
}

struct User {
    content: Vec<UserContent>,
}
```

Large or binary content should not be embedded directly in the Conversation Log.

Instead:

```text
store image/file/blob
    ↓
obtain strongly typed durable ID
    ↓
append User event containing the ID
```

Example:

```text
User
    Text("what is in this image?")
    Image(image_019...)
```

This keeps conversation events small and lets content storage, retention, permissions, deduplication, and provider transport evolve independently.

The ModelDriver resolves referenced content into whatever provider-specific representation is required.

Phase 1 only needs the concrete content types we actually use.

A failed model invocation never removes the already-durable `User` event.

---

## 9. Assistant And Communication

`Assistant` and `Communication` are top-level semantic event kinds. Model provenance and optional model-native data belong to their applicable `ModelDetails`; the event also carries the producing `ModelInvocationId`.

The durable common shape is:

```rust
enum ConversationEventKind {
    Assistant { ... },
    Communication { ... },
}

enum ConversationProblem {
    Issue(ModelIssue),
    Invocation(InvocationError),
}

enum ModelEventImportance {
    Detailed,
    Interesting,
    Important,
}
```

`Assistant` is the actual response used for portable continuation and is always important. `Communication` carries auxiliary information with a subtype and importance.

---

## 10. Reasoning and responses

Exposed chain-of-thought and final responses are distinct typed model events. A driver aggregates provider deltas into coherent messages before yielding them.

Provider transport may involve many low-level events:

```text
text.delta "Hel"
text.delta "lo"
output.done
```

but the semantic conversation may record:

```text
Assistant(model=..., invocation_id=..., message="Hello")
```

A driver emits detailed reasoning as a detailed communication, a reasoning summary as an interesting communication, and final output as an assistant response. Consumers such as the CLI choose which communications to present. Only assistant responses are replayed as assistant history.

---

## 11. Turn Lifecycle

User content and agent work are separate concepts. A user message may be
accepted before any turn is requested, and several messages may be available to
one turn. A turn records requested work and ends with an explicit terminal fact:

```text
UserMessageRequested
User
TurnRequested
driver invocation event
Assistant / Communication / Problem
TurnCompleted
```

An assistant response does not complete a turn by itself. A problem may be
recoverable, so it does not necessarily complete a turn either. `TurnCompleted`
records the terminal outcome after orchestration has finished or given up.

The driver invocation event has a stable `ModelInvocationId`. Every model fact
produced by that invocation references the identifier. A retry may use another
invocation identifier while remaining part of the same turn. A recoverable
problem does not automatically fail the turn; only the driver's explicit
`TurnCompleted` outcome does that. Stream exhaustion without completion means
incomplete execution.

---

## 12. ToolRequest

`ToolRequest` records that a model requested a tool invocation.

Each request has a stable `ToolCallId`:

```rust
struct ToolRequest {
    id: ToolCallId,
    // name
    // arguments
    // ...
}
```

A ToolRequest is a semantic fact.

It does not execute the tool itself.

The caller/runtime owns execution.

---

## 12. ToolResponse

`ToolResponse` records the result of one tool request and references exactly one `ToolCallId`:

```rust
struct ToolResponse {
    tool_call_id: ToolCallId,
    // result / error / metadata
}
```

A response is appended to the Conversation Log as soon as it arrives.

No batch abstraction is required.

---

## 13. Multiple tool requests

A single ModelDriver invocation may produce zero, one, or many `ToolRequest`s:

```text
ModelDriver invocation
    ↓
ToolRequest(A)
ToolRequest(B)
ToolRequest(C)
```

The caller may execute them sequentially or concurrently.

Responses are appended as they arrive:

```text
ToolResponse(B)
ToolResponse(A)
ToolResponse(C)
```

The core model does not prescribe:

```text
tool concurrency
response batching
when reinvocation occurs
whether all tool responses must arrive first
```

The caller owns that policy.

Phase 1 commits only to stable correlation through `ToolCallId`.

---

## 14. Context

`Context` records state that may affect later model invocation.

Examples:

```text
instructions
working directory
selected files
project
permissions
environment information intentionally exposed to the model
```

Context is distinct from user input.

---

## 15. Automation

`Automation` records information contributed by an external or asynchronous actor.

It is distinct from `ToolResponse`, which answers a model-requested tool invocation.

---

## 16. Data

`Data` records durable machine-readable metadata associated with the conversation.

Examples:

```text
external IDs
usage summaries
annotations
tags
diagnostics
UI metadata
```

Data is not model input by default.

---

## 17. Model-specific data

The optional `ModelData` inside `ModelDetails` is the one channel for driver-native enrichment.

It retains model/provider-specific information that is useful enough to preserve but does not justify a universal field. `ModelData` is opaque to the conversation and supports JSON serialization. `ModelDetails` keeps it with the owning `ModelSource`, so the driver can decide whether it knows how to interpret the content. The driver that creates it defines and interprets it, and another driver may ignore it safely.

The portable event kind must contain the complete meaning of the event. `ModelData` may preserve native fidelity or improve continuation, but it must never be required to understand the conversation.

Model data is recorded when the event is created. Later drivers do not mutate old events to attach their own representations.

Cross-driver replay must continue to work from portable fields when model data is not understood. Raw provider protocol events still belong in tracing/diagnostics rather than the semantic Conversation Log.

---

## 18. Model problems

`ConversationProblem::Issue` means the driver understood a meaningful limitation, decision, or unsuccessful model outcome. OpenAI refusals and recognized context-limit responses are model issues. A context-limit issue may arrive through an HTTP error response before an SSE stream exists; the driver represents it as a problem event on its semantic stream rather than a control-flow error. There is no `Other` issue kind: a newly understood semantic problem receives a specific shared kind, while unusable provider output and unclassified invocation failure retain their distinct invocation meanings.

`ConversationProblem::Invocation` means the invocation machinery failed. `ModelDriverError` remains the detailed control-flow error. The turn service sanitizes it, appends the invocation problem, and returns the original error.

A `Problem` is a top-level conversation event. A model-associated problem carries `Some(ModelDetails)`; a future problem unrelated to a model may carry `None`. It is not model output merely because it concerns a model invocation.

An invocation failure before a stream exists therefore leaves:

```text
User(...)
Problem(model=Some(...), problem=Invocation(...))
```

If an established stream fails later, it leaves:

```text
User(...)
Model(...completed semantic event...)
Problem(model=Some(...), problem=Invocation(...))
```

Completed semantic events already yielded remain valid conversation facts, and events already appended are not rolled back. Incomplete provider deltas that never formed a completed `ModelEvent` are discarded.

Events that were already durable before invocation are not rolled back.

Durable problems contain portable sanitized messages, not credentials, raw provider bodies, stack traces, sensitive request data, or provider diagnostics without conversational meaning.

Detailed diagnostics belong in tracing/logging.

---

# ModelDriver

## 19. Why ModelDriver is an explicit abstraction

Phase 1 intentionally introduces a narrow `ModelDriver` abstraction even though only one provider is initially implemented.

This is deliberate.

The abstraction defines:

> What is the rest of the system allowed to know about model invocation?

Its purpose is to keep OpenAI Responses concepts out of the conversation, caller, CLI, and orchestration layers while we learn the new API.

This is an explicit exception to the normal preference against speculative abstraction.

The guardrail is:

> Keep ModelDriver very small and let concrete implementations pressure its shape.

Do not build:

```text
provider capability matrices
generic feature negotiation
large associated-type frameworks
universal provider event enums
provider inheritance hierarchies
```

A second driver should be allowed to reshape the abstraction.

---

## 20. Cross-driver semantic contract

Every ModelDriver must be able to invoke using only the supplied reconstructed `Conversation`.

This is the central portability rule.

For example:

```text
User
OpenAI ModelEvent
User
Anthropic ModelEvent
ToolRequest
ToolResponse
User
Gemini ...
```

must be a valid conversation.

A driver must not require prior turns to have been produced by itself.

Provider-native continuation may later improve fidelity or performance, but it must remain optional.

Correctness and model switching are based on semantic ConversationEvents.

---

## 21. ModelDriver input

The driver receives an immutable reference to a validated `Conversation`:

```rust
&Conversation
```

The projection provides read-only access to its ID and ordered conversation events. The driver cannot mutate ordinary owned data through the shared reference, and `Conversation` provides no mutation methods.

For normal Rust-owned values such as structs, enums, `String`, and `Vec`, that provides the desired deep immutability through the borrowed input.

Interior-mutability types such as:

```text
Mutex
RwLock
RefCell
Atomic*
```

can still mutate behind a shared reference, so conversation events should avoid them unless there is a demonstrated need.

The driver contract is:

> A ModelDriver receives immutable semantic history and returns new facts rather than mutating historical conversation state.

---

## 22. ModelDriver Invocation

The shared contract is exact:

```rust
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;

type ModelOutputStream =
    BoxStream<'static, Result<ModelDriverOutput, ModelDriverError>>;

enum ModelDriverOutput {
    Event(ConversationEventKind),
    Driver(Box<dyn DriverEvent>),
}

trait DriverEvent: Send {
    fn to_envelope(&self) -> Result<DriverEventEnvelope, ModelDriverError>;
}

trait DriverEventDecoder {
    fn decode_event(
        &self,
        envelope: &DriverEventEnvelope,
    ) -> Result<Box<dyn DriverEvent>, DriverEventDecodeError>;
}

trait ModelDriver: DriverEventDecoder {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        conversation: &'invoke Conversation,
        turn_id: ConversationTurnId,
    ) -> BoxFuture<
        'invoke,
        Result<ModelOutputStream, ModelDriverError>,
    >;
}
```

This is conceptually `Future<Stream<ModelDriverOutput>>`, or `Mono<Flux<ModelDriverOutput>>` in Reactor terminology. The caller supplies only the immutable conversation and turn identity. The driver creates invocation identities, driver events, and invocation-specific data. Shared semantic events remain concrete and portable. Driver output is appended through the shared record boundary.

The important Phase 1 properties are:

- one call represents one model invocation
- input is a complete immutable conversation reconstructed from conversation events
- the driver owns and exposes its stable provider/model source
- invocation is asynchronous and stream-first
- the stream yields driver-defined records, portable semantic event kinds, and an explicit `TurnCompleted`
- the consumer controls demand by polling for the next event
- the caller owns the outer model/tool loop and turn request
- the driver owns invocation identities and invocation-specific records
- expected failures are strongly typed
- provider SDK types do not cross the boundary

A caller that wants batch behavior can collect the stream. No separate batch interface is required. Later implementation experience may still pressure the interface.

---

## 23. Returned event persistence

User input and `TurnRequested` are appended before invocation. The driver maps provider-native activity to driver-defined records and semantic event kinds, including `ModelDetails` and driver-created `ModelInvocationId` values where the event concerns a model. The append boundary assigns canonical envelope metadata. The caller persists and may display each returned semantic event immediately while the invocation remains active.

Provider protocol events and raw text deltas remain internal to the driver. They are not `ConversationEvent`s and are not persisted merely because they arrived. The driver aggregates those deltas and yields only completed semantic output such as an `AssistantResponse`, `ModelCommunication`, or `ModelIssue`.

Conceptually:

```text
User already durable
    ↓
await ModelDriver invocation
    ↓
setup failure before stream
    → append sanitized Problem(problem=Invocation(...))
    → return detailed ModelDriverError

or

await ModelDriver invocation
    ↓
poll stream
    ↓
completed ConversationEvent
    → display and append the returned semantic event
    ↓
later stream failure
    → completed events remain durable
    → incomplete provider deltas are discarded
    → append sanitized Problem(problem=Invocation(...))
    → return detailed ModelDriverError
```

This supersedes the previous batch contract, which returned all model events only after the complete invocation succeeded and discarded every model event after a late provider failure. Incremental append does not imply rollback: already appended semantic facts remain durable.

---

## 24. ModelDriver stream

An established invocation yields typed driver records and semantic facts incrementally:

```rust
Result<ModelDriverOutput, ModelDriverError>
```

This supports assistant responses, auxiliary communications, model-reported problems, driver-defined invocation events, and explicit turn completion. The append boundary owns durable envelope construction. Stored driver envelopes remain opaque when their decoder is unavailable. Stream polling supplies demand and natural backpressure at this boundary. Stream exhaustion without `TurnCompleted` is incomplete execution, not success.

---

## 25. ModelDriver errors

Expected model failures are explicit both while establishing the invocation and while consuming it:

```rust
BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>

BoxStream<'static, Result<ConversationEventKind, ModelDriverError>>
```

A small error model might begin with:

```rust
enum ModelDriverError {
    Authentication(...),
    RateLimited(...),
    Transport(...),
    InvalidRequest(...),
    InvalidResponse(...),
    StreamInterrupted(...),
    Provider(...),
}
```

The exact taxonomy should remain small and implementation-driven.

These errors are returned as values. ModelDriver does not decide whether they are written to standard error, sent to a remote logger, retried, or otherwise reported.

Rust does not use Java-style checked exceptions or `throws` declarations.

Expected operational failures are represented through `Result<T, E>`.

Unexpected programming failures may panic, but transport, provider, validation, and similar model failures should normally be represented by `ModelDriverError`. The turn service converts those errors into sanitized durable invocation problems before returning them. Conversation persistence errors belong to the caller.

## Async ecosystem

Choosing async is intentional because `ModelDriver` is expected to become a reusable first-class abstraction used by command-line applications, servers, user interfaces, concurrent tool execution, and multiple conversations.

Rust standard-library `Future` and async/await provide the language foundation. `futures-util` provides conventional `BoxFuture`, `BoxStream`, and stream adapters. Tokio is the async runtime. Reqwest uses its asynchronous client and streaming response support. The explicit `BoxFuture` signature supports dynamic `Box<dyn ModelDriver>` dispatch, so `async-trait` is not currently required.

Async is the architectural choice; Tokio is the conventional runtime choice after choosing asynchronous networking. The application owns and starts the runtime, and reusable library components must not secretly create private runtimes. Tokio features should be enabled narrowly rather than selecting `full` by default. Blocking work must not run directly on async runtime workers when it can materially delay other tasks.

---

# Replay and model switching

## 26. Semantic replay is the Phase 1 correctness contract

Phase 1 reconstructs model input from the Conversation Log.

Conceptually:

```text
ConversationEvents
    ↓
ModelDriver-specific translation
    ↓
provider request
```

OpenAI translates semantic events to OpenAI Responses input.

Anthropic translates the same semantic events to Anthropic content/messages.

Gemini translates the same semantic events to its own representation.

No ModelDriver may require another provider's raw protocol history.

---

## 27. Model switching

Switching models/providers in the middle of a conversation is a first-class expected behavior, not an edge case.

For example:

```text
User
ModelEvent produced by OpenAI
User
ModelEvent produced by another ModelDriver
User
...
```

The new ModelDriver reads the semantic conversation and continues from it.

Some provider-specific fidelity may be lost when switching.

That is acceptable.

The semantic Conversation Log is the portability boundary.

---

## 28. Provider-native continuation

Provider-native continuation is explicitly **not required for Phase 1 correctness**.

For example, OpenAI may offer response IDs or reasoning-state mechanisms that improve same-provider continuation.

These may later be used as optimizations:

```text
lower token usage
higher reasoning fidelity
lower latency
better continuation
```

But the fallback must remain:

```text
semantic Conversation Log
    ↓
fresh provider request
```

A driver should never become unable to continue merely because provider-native history is unavailable.

---

# Caller/runtime

## 29. Outer orchestration loop

ModelDriver represents one model invocation.

It does not own the whole autonomous loop.

The caller owns orchestration:

```text
Conversation
    ↓
ModelDriver.invoke()
    ↓
0..N semantic events
    ↓
perhaps ToolRequests
    ↓
caller executes tools however it chooses
    ↓
ToolResponses appended as they arrive
    ↓
caller decides when to invoke ModelDriver again
```

This keeps concurrency, batching, scheduling, retry, and tool policy outside the ModelDriver abstraction.

---

## 30. Pending tool work

Runtime logic may derive pending tool work from semantic conversation state.

For example:

```text
ToolRequest(A)
ToolResponse(A) absent
    → A may require execution
```

while:

```text
ToolRequest(A)
ToolResponse(A) present
    → no pending response for A
```

Phase 1 needs only enough of this logic to support basic tool round-tripping.

It does not need a general workflow engine.

---

# CLI

## 31. CLI is a projection of the Conversation Log

The CLI consumes semantic conversation events.

It does not consume OpenAI transport events directly.

Conceptually:

```text
ModelDriver
    ↓
ConversationEvents
    ↓
Conversation Log
    ↓
CLI projection
```

This keeps the CLI independent of provider implementation.

A future interactive experience may also consume tracing/progress signals, but those do not redefine the semantic conversation contract.

---

# OpenAI Phase 1

## 32. OpenAI implementation strategy

Phase 1 implements `ModelDriver` against the OpenAI Responses API.

The goal is partly implementation and partly architectural discovery.

We want firsthand experience with the newer Responses model before deciding how much to rely on a provider-neutral Rust library.

OpenAI remains an implementation detail behind ModelDriver.

A later implementation may be:

```text
AnthropicModelDriver
GeminiModelDriver
GenAiModelDriver backed by rust-genai
another direct provider integration
```

and may cause the trait to evolve.

---

## 33. OpenAI ModelDriver responsibilities

The OpenAI implementation owns:

```text
Responses API request construction
semantic ConversationEvent → OpenAI input translation
OpenAI SDK / HTTP interaction
stream parsing
text aggregation
tool-call translation
OpenAI response IDs
reasoning/provider-specific protocol handling
provider errors
```

No OpenAI SDK/API type crosses the ModelDriver boundary.

The implementation returns typed `ConversationEvent`s rather than exposing raw OpenAI protocol events. Provider-native intermediate events remain private to the implementation.

---

## 34. Phase 1 OpenAI scope

Support enough to exercise the semantic architecture:

```text
basic text input/output
Responses API invocation
streaming response consumption
polymorphic ConversationEvents
aggregated exposed reasoning
event importance
function/tool requests
tool responses
multiple tool requests
typed success/error behavior
semantic reconstruction from Conversation Log
```

Out of scope unless nearly free:

```text
durable raw provider-event archival
provider-native replay as a correctness dependency
hosted web search
file search
computer use
image generation
```

Image/file input may be added when needed through typed content references.

---

# Observability and tracing

## 35. Phase 1 tracing

Provider-specific detail is useful even though it is not part of the semantic replay contract.

Phase 1 may trace/log:

```text
provider
model
request/response IDs
latency
time to first token/event
usage/token counts
raw or structured provider events
tool-call protocol activity
error diagnostics
HTTP/provider metadata
```

This information is primarily for:

```text
debugging
performance analysis
cost analysis
understanding provider behavior
development of the ModelDriver abstraction
```

It does not need to be represented as ConversationEvents.

It does not need to be replayable.

It does not need to be durable for correctness.

Existing tracing infrastructure should be preferred before introducing a separate durable event bus.

---

## 36. Raw provider events

Raw provider events may be extremely useful while learning the Responses API.

Capture them through tracing/logging when practical.

For example:

```text
response.created
output_item.added
reasoning events
output_text.delta
function_call_arguments.delta
response.completed
```

The important distinction is:

```text
ConversationEvent
    semantic product behavior

provider trace event
    implementation/diagnostic behavior
```

Phase 1 should not introduce semantic complexity merely to make raw provider protocol history replayable.

---

## 37. Tracing does not constrain ModelDriver implementations

A new ModelDriver should be straightforward to write.

At a high level, an implementation should need to:

```text
1. translate semantic conversation to provider input
2. call the provider
3. interpret provider output
4. yield completed ConversationEvents or a typed error
5. expose its configured provider/model source
6. optionally emit useful traces
```

It should not need to implement:

```text
a timeless deterministic projection algebra
durable provider-log schema migration
cross-version re-projection
provider-log replay compatibility
global projection identities
```

Those requirements should only appear later if concrete value justifies them.

Ease of writing ModelDrivers is a first-class design constraint.

---

# Future direction

## 38. Durable ModelDriver run history

A future phase may introduce a durable ModelDriver run/event log if concrete needs justify it.

Potential motivations include:

```text
crash recovery inside partially completed model invocations
higher-fidelity same-provider replay
reasoning-state preservation
provider-native continuation
forensic debugging
long-running/background model work
distributed execution
```

A possible future shape is:

```text
ModelDriver invocation
    ↓
durable ModelDriverRun
    ↓
durable provider/run events
    ↓
observability / recovery / native replay
```

But that future log must remain supplemental to the semantic contract.

The invariant should remain:

> A ModelDriver can always continue from the Conversation Log alone.

---

## 39. Future event buses

The `ModelDriver` semantic event stream has one polling consumer. Additional publication buses or multiple-subscriber observability streams may be introduced later if required.

For example:

```text
ModelDriver
    → semantic ConversationEvent stream
    → caller persists returned ConversationEvents
    → optional ConversationEvent publication bus

concrete driver
    → optional observability/provider event stream
```

Potential subscribers:

```text
conversation persistence
CLI/UI progress
metrics
tracing
debug logging
provider-native cache
future durable run history
```

Phase 1 does not need messaging infrastructure.

A local function call, sink, callback, or tracing span is sufficient.

The seam matters more than the machinery.

---

## 40. Future provider-native optimizations

When concrete evidence shows value, a ModelDriver may retain provider-native information to improve same-provider continuation.

Examples:

```text
OpenAI response IDs
reasoning state
provider cache references
uploaded file handles
provider-specific conversation IDs
```

These should be treated as caches/optimizations around the semantic conversation, not the only representation of history.

Switching providers must remain possible without them.

---

## 41. Future content storage

Conversation events should continue to reference large/binary content through durable typed IDs.

A future content store may add:

```text
content-addressed storage
deduplication
remote object storage
retention
access control
lazy materialization
provider upload caches
```

The stable contract remains:

```text
ConversationEvent
    references content ID

content store
    owns bytes/lifecycle

ModelDriver
    resolves content for provider invocation
```

---

## 42. Future replay and concurrency

The Phase 1 per-conversation position is sufficient for deterministic semantic replay.

Later requirements may motivate:

```text
global append positions
projection cursors
concurrent writers
optimistic append
transactional sequence allocation
explicit causal relationships
```

Those are persistence/runtime concerns.

They should not change the distinction between:

```text
typed identity
conversation replay order
semantic correlation
```

---

# Security

## 43. Phase 1 security baseline

Conversation context, tool output, content references, and provider traces may contain sensitive information.

Phase 1 does not need a complete redaction/retention system.

It should:

- avoid knowingly persisting obvious credentials
- avoid intentionally capturing environment secrets
- use private filesystem permissions for local durable conversation data
- be cautious when tracing raw provider payloads
- document that diagnostic logs may contain sensitive content

More complete security and retention policy belongs to a later phase.

---

# Phase 1 boundaries

## 44. What Phase 1 commits to

Phase 1 commits to:

```text
Conversation Log as the durable semantic truth

every ModelDriver can work from Conversation alone

model/provider switching within a conversation

a narrow explicit ModelDriver abstraction

immutable Conversation input

strongly typed ModelDriverError

strongly typed UUIDv7 identities

monotonic ConversationEvent positions

ToolCallId correlation

multiple tool requests without batching policy

typed external references for images/files/blobs

incremental persistence of completed yielded model outputs

caller-owned orchestration loop

CLI as a projection of Conversation
```

These are the seams we do not want to need to undo.

---

## 45. What Phase 1 intentionally does not solve

Phase 1 does not require:

```text
durable ModelDriver run logs
durable raw provider-event logs
projection identities between two durable logs
cross-version provider-log reprojection
provider-native replay for correctness
distributed event buses
locks/concurrent append coordination
global event clocks
generic causal DAGs
exactly-once tool side effects
general workflow orchestration
universal provider event taxonomy
production-grade tracing retention
complete OpenAI Responses coverage
```

These may become useful later, but they should not burden the first ModelDriver implementation.

---

## 46. First implementation milestone

The basic semantic text milestone uses the asynchronous streaming boundary. Tool use, content references, and provider tracing remain follow-up work.

The asynchronous streaming implementation proves:

```text
generate ConversationId and append User("hello") as the first event

reconstruct immutable Conversation from conversation events

invoke OpenAiModelDriver

asynchronously establish one OpenAI Responses request and SSE stream

yield completed semantic ConversationEvents as provider deltas are aggregated

construct and append resulting ConversationEvents incrementally

retain appended completed events after a later stream failure

print messages selected by CLI verbosity

reload Conversation

invoke OpenAiModelDriver again using only semantic Conversation history

switch to another ModelDriver later without requiring OpenAI run history
```

Then add:

```text
0..N ToolRequests
tool execution
ToolResponses appended as they arrive
caller-driven reinvocation
content references as concrete use cases require
useful provider tracing
```

The objective is to prove semantic portability and the ModelDriver boundary before adding provider-native sophistication.

---

# Documentation

## 47. Relationship to the Single Binary Decision

The early `ModelDriver` abstraction is a deliberate exception to the normal rule against speculative provider abstractions.

Its purpose is not to predict a universal provider API.

Its purpose is to define and protect the model-invocation boundary while implementing the first provider.

Keep it small.

Let future implementations change it.

The [single binary decision](decisions/2026-08-08-begin-with-single-binary-package.md) or related architecture notes should explicitly record this rationale so the exception is deliberate rather than accidental.

---

## 48. Documentation status

This document is a **Phase 1 architectural direction**, not a finished permanent API.

Implementation experience should feed back into it.

If OpenAI Responses exposes assumptions that conflict with the design, document those pressures rather than hiding them behind increasingly elaborate abstractions.

The shorter `docs/conversation.md` should describe the stable semantic conversation model and should not imply that a durable ModelDriver run log is required for Phase 1.

Major future architectural changes should be captured as concise decisions when useful.
