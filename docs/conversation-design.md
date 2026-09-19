# Conversation and ModelDriver Architecture

**Status:** Phase 1 asynchronous streaming text and bounded tool-calling slice implemented
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

A `ModelDriver` consumes an immutable reference to the reconstructed conversation. One asynchronous invocation establishes a stream of ordered, nonempty batches of completed semantic events:

```text
immutable Conversation
    → asynchronous ModelDriver invocation
    → stream of atomic batches of completed semantic facts
    → append boundary commits each batch
    → committed facts presented incrementally
```

One invocation is one provider/model invocation. For OpenAI, it is one REST request with one SSE response stream; consuming several semantic events or batches from that stream does not make several model requests.

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
    events: Vec<ConversationEventRecord>,
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

`ConversationEvent` is the append vocabulary for a conversation. It contains a
session request, a shared `ConversationFact`, or a namespaced extension event.
The persisted canonical record is `ConversationEventRecord`; it adds identity,
position, timestamp, and schema metadata. Extension records use an opaque
`ConversationEventEnvelope` in the same log and are excluded from the
model-facing `Conversation` projection.

Conceptually:

```rust
enum ConversationEventKind {
    Command(ConversationCommand),
    Fact(ConversationFact),
}

enum ConversationCommand {
    UserMessageRequested { ... },
    TurnRequested { ... },
}

enum ConversationMessage {
    User { ... },
    AssistantResponse { ... },
    Communication { ... },
    Problem { ... },
}

enum ConversationFact {
    Message { message: ConversationMessage, turn_id: Option<ConversationTurnId> },
    Lifecycle(ConversationLifecycle),
    ToolRequest(...),
    ToolResponse(...),
    Context(...),
    Automation(...),
    Data(...),
}

enum ConversationLifecycle {
    TurnCompleted { ... },
}
```

Model provenance belongs to the driver-defined invocation event, not to shared
assistant, communication, or problem facts. Those facts carry only an
`invocation_id` where applicable and optional event-specific `ModelData`. A
driver-independent consumer can continue from their portable content without
interpreting the invocation event.

`ConversationEventEnvelope` stores the namespace, namespace version, event type,
event schema version, human-readable description, and opaque JSON payload. A
decoder for that namespace may reconstruct its concrete event. If no decoder is
available, the envelope remains readable and preserved without decoding.

Every event also has a typed `ConversationEventClass`: shared variants derive
`Command` or `Fact` from their kind, while extension events declare the class
through the extension contract and persist it in the envelope. The class is
independent from the `Shared`/`Extension` schema owner dimension.

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
model-facing projection excludes them. `TurnCompleted` is an explicit fact
recorded by the session after the driver stream ends, using the session's
completion policy rather than a driver lifecycle event.

Do not force every command into:

```text
handle(command) -> Vec<Event>
```

The caller requests a turn. The driver owns model invocation identities and
driver-defined invocation records. Model invocation, tools, and external I/O
naturally involve streaming, failures, and incremental output.

The caller-facing API is `ConversationSession`:

```rust
enum ConversationCommand { /* UserMessageRequested or TurnRequested */ }

impl ConversationSession {
    fn add_user_request(...) -> Result<ConversationCommandId, _>;

    async fn invoke(
        &self,
        report_progress: impl FnMut(ConversationSessionProgress) -> Result<(), _>,
    ) -> Result<TurnOutcome, _>;
}
```

`ModelDriver` receives a `TurnInput` constructed from the immutable conversation
and turn identity. `TurnInput` derives the pending user requests from that same
snapshot. Its output stream returns nonempty ordered batches of `ModelDriverOutput`
values: shared `ConversationMessage` content or driver-defined invocation and
extension events. The driver groups events that must become visible together, and
single-event batches are the normal case. It cannot return session-owned commands
or turn lifecycle facts. The session commits each batch atomically before reporting
any of its events, converts those outputs into the persisted event vocabulary, and
records `TurnCompleted` from its completion policy.

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

Phase 1 stores one append-only JSON Lines log file per conversation. A transaction opens with a `begin` marker line, carries one event record per line, and closes with a `commit` marker line containing the event count, position range, and a CRC32 of the transaction's event-line bytes. The log is directly readable and queryable with tools such as `jq`. A reader ignores a trailing transaction without a valid commit marker and rejects corruption inside committed history. The append boundary flushes and syncs each transaction before it is acknowledged.

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

`Assistant` and `Communication` are top-level semantic event kinds. They carry
portable content, optional event-specific `ModelData`, and the producing
`ModelInvocationId`, but do not repeat driver or model provenance.

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

An assistant response and a problem are meaningful output, but the driver does
not decide the turn's terminal state. `TurnCompleted` records that outcome once
the driver stream ends and the session applies its completion policy: an
assistant response ends a successful turn, while any problem fails the turn.
Output already persisted remains valid when a later problem fails the turn.

The driver invocation event has a stable `ModelInvocationId`. Every model fact
produced by that invocation references the identifier. A retry may use another
invocation identifier while remaining part of the same turn. Stream exhaustion
without an assistant response or problem is incomplete execution.

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

`ModelData` is optional event-specific driver data carried directly by the
shared fact it describes.

It retains model/provider-specific information that is useful enough to preserve
but does not justify a universal shared field. `ModelData` is opaque to the
conversation's meaning but remains serialized and inspectable. The driver that
creates it defines and interprets it, and another driver may ignore it safely.

The portable event kind must contain the complete meaning of the event. `ModelData` may preserve native fidelity or improve continuation, but it must never be required to understand the conversation.

Model data is recorded when the event is created. Later drivers do not mutate old events to attach their own representations.

Cross-driver replay must continue to work from portable fields when model data is not understood. Raw provider protocol events still belong in tracing/diagnostics rather than the semantic Conversation Log.

---

## 18. Model problems

`ConversationProblem::Issue` means the driver understood a meaningful limitation, decision, or unsuccessful model outcome. OpenAI refusals and recognized context-limit responses are model issues. A context-limit issue may arrive through an HTTP error response before an SSE stream exists; the driver represents it as a problem event on its semantic stream rather than a control-flow error. There is no `Other` issue kind: a newly understood semantic problem receives a specific shared kind, while unusable provider output and unclassified invocation failure retain their distinct invocation meanings.

`ConversationProblem::Invocation` means the invocation machinery failed. The driver sanitizes provider failures into that problem on its message stream. `ModelDriverError` remains the detailed control-flow error for shared-contract violations.

A `Problem` is a top-level conversation event. It may reference an invocation
and carry event-specific `ModelData`, but does not repeat invocation provenance.
It is not model output merely because it concerns a model invocation.

An invocation failure before a stream exists therefore leaves:

```text
User(...)
Problem(invocation_id=Some(...), problem=Invocation(...))
```

If an established stream fails later, it leaves:

```text
User(...)
Assistant(...completed semantic event...)
Problem(invocation_id=Some(...), problem=Invocation(...))
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
    BoxStream<'static, Result<ModelDriverOutputBatch, ModelDriverError>>;

struct TurnInput<'conversation> {
    conversation: &'conversation Conversation,
    turn_id: ConversationTurnId,
}

impl<'conversation> TurnInput<'conversation> {
    fn new(conversation: &'conversation Conversation, turn_id: ConversationTurnId) -> Self;
    fn conversation(&self) -> &'conversation Conversation;
    fn turn_id(&self) -> ConversationTurnId;
    fn pending_user_requests(&self) -> &[UserMessageRequest];
}

enum ConversationMessage {
    User { ... },
    AssistantResponse { ... },
    Communication { ... },
    Problem { ... },
}

enum ModelDriverOutput {
    Message(ConversationMessage),
    ToolRequest(ToolRequest),
    Command(Box<dyn ConversationEventExtension>),
    Extension(Box<dyn ConversationEventExtension>),
}

struct ModelDriverOutputBatch {
    outputs: Vec<ModelDriverOutput>,
}

trait ConversationEventExtension: Send {
    fn namespace(&self) -> &str;
    fn namespace_version(&self) -> &str;
    fn event_type(&self) -> &str;
    fn event_schema_version(&self) -> u32;
    fn description(&self) -> &str;
    fn serialize_payload(&self) -> Result<Value, ConversationEventError>;
}

trait ConversationEventReader {
    fn read_event(
        &self,
        envelope: &ConversationEventEnvelope,
    ) -> Result<Box<dyn ConversationEventExtension>, ConversationEventReadError>;
}

trait ModelDriver: ConversationEventReader {
    fn source(&self) -> &ModelSource;

    fn invoke<'invoke>(
        &'invoke self,
        input: TurnInput<'invoke>,
    ) -> BoxFuture<
        'invoke,
        Result<ModelOutputStream, ModelDriverError>,
    >;
}

enum ModelDriverError {
    UnassociatedUserMessage,
    UnexpectedUserRequest { command_id: ConversationCommandId },
    IncompleteTurn,
}
```

This is conceptually `Future<Stream<ModelDriverOutputBatch>>`, or `Mono<Flux<ModelDriverOutputBatch>>` in Reactor terminology. The caller supplies only the immutable conversation and turn identity. The driver creates invocation identities, driver events, and invocation-specific data. Shared messages remain concrete and portable and are defined by the conversation vocabulary. The driver groups outputs into ordered nonempty batches: a batch contains events that must become visible together, and single-event batches are the normal case. The session commits each batch atomically before converting and persisting its events through the shared record boundary.

`ModelDriverError` describes only failures of this shared contract. Provider
failures that the driver can describe are emitted as portable `Problem` messages.
The session records the turn outcome after the stream ends; drivers do not emit
turn lifecycle facts.

The important Phase 1 properties are:

- one call represents one model invocation
- input is a complete immutable conversation reconstructed from conversation events
- the driver owns and exposes its stable provider/model source
- invocation is asynchronous and stream-first
- the stream yields ordered nonempty batches of permitted conversation messages: accepted user content, assistant responses, communications, problems, and driver-defined events
- the consumer controls demand by polling for the next batch
- the session commits a batch atomically before reporting any of its events
- the caller owns the outer model/tool loop, the turn request, and the turn lifecycle
- the driver owns invocation identities and invocation-specific records
- expected failures are strongly typed
- provider SDK types do not cross the boundary

A batch does not necessarily end a message, model call, or turn. Later implementation experience may still pressure the interface.

---

## 23. Returned event persistence

User input and `TurnRequested` are appended before invocation. The driver maps provider-native activity to driver-defined records and permitted conversation messages, including driver-created `ModelInvocationId` values and event-specific `ModelData` where applicable. The session converts each batch of messages into the persisted event vocabulary and commits the whole batch as one transaction. The append boundary assigns canonical envelope metadata. The session may display a batch's events immediately after the commit, while the invocation remains active.

Provider protocol events and raw text deltas remain internal to the driver. They
are not conversation events and are not persisted merely because they arrived.
The driver aggregates those deltas and yields permitted conversation messages
such as an `AssistantResponse`, `ModelCommunication`, or `Problem`, alongside any
driver-defined conversation events.

Conceptually:

```text
User already durable
    ↓
await ModelDriver invocation
    ↓
setup failure before stream
    → driver yields sanitized Problem(problem=Invocation(...)) on its stream

or

await ModelDriver invocation
    ↓
poll stream
    ↓
permitted conversation message in a batch
    → commit the whole batch, then display and persist its converted events
    ↓
later stream failure
    → completed and committed events remain durable
    → incomplete provider deltas are discarded
    → driver yields sanitized Problem(problem=Invocation(...)) on its stream
    ↓
session records TurnCompleted with the resulting outcome
```

This supersedes the earlier contract, which returned all model events only after the complete invocation succeeded and discarded every model event after a late provider failure. Incremental commit does not imply rollback: already committed semantic facts remain durable.

---

## 24. ModelDriver stream

An established invocation yields ordered nonempty batches of permitted conversation messages incrementally:

```rust
Result<ModelDriverOutputBatch, ModelDriverError>
```

This supports assistant responses, auxiliary communications, model-reported problems, and driver-defined invocation events. The session commits each batch atomically, converts those messages into durable events, and the append boundary owns durable envelope construction. Stored driver envelopes remain opaque when their decoder is unavailable. Stream polling supplies demand and natural backpressure at this boundary. The session records `TurnCompleted` after the stream ends; stream exhaustion without an assistant response or problem is incomplete execution, not success.

---

## 25. ModelDriver errors

Contract failures are explicit both while establishing the invocation and while consuming it:

```rust
BoxFuture<'invoke, Result<ModelOutputStream, ModelDriverError>>

BoxStream<'static, Result<ModelDriverOutputBatch, ModelDriverError>>
```

The shared contract error model is:

```rust
enum ModelDriverError {
    UnassociatedUserMessage,
    UnexpectedUserRequest { command_id: ConversationCommandId },
    IncompleteTurn,
}
```

The exact taxonomy should remain small and implementation-driven. Provider authentication, rate limiting, transport, invalid-request, invalid-response, stream-interruption, and provider failures are represented as sanitized `ConversationProblem::Invocation` messages on the driver stream rather than contract errors.

These errors are returned as values. ModelDriver does not decide whether they are written to standard error, sent to a remote logger, retried, or otherwise reported.

Rust does not use Java-style checked exceptions or `throws` declarations.

Expected operational failures are represented through `Result<T, E>`.

Unexpected programming failures may panic, but contract and validation failures should normally be represented by `ModelDriverError`; provider failures that the driver can describe stay on the message stream. The session records the turn outcome and returns it. Conversation persistence errors belong to the caller.

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

The basic semantic milestone uses the asynchronous streaming boundary with a
bounded sequential tool-calling loop and a shell tool. Content references and
provider tracing remain follow-up work.

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
