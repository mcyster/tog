# Conversation and ModelDriver

The later [Record Commands And Turn Lifecycle](2026-09-05-record-commands-and-turn-lifecycle.md) note supersedes the driver-owned envelope and command-exclusion portions of this note.

The central conclusion from the design discussions is that `tog` should preserve
a portable semantic conversation before it preserves provider execution history.

People are more likely to change models during a conversation than to replay a
partially completed provider stream. Phase 1 therefore optimizes for clear
cross-model continuation and accepts that a failed invocation may need to be
rerun.

The authoritative design is in
[Conversation and ModelDriver Architecture](../conversation-design.md). This
note preserves the critical choices that led there without duplicating the full
specification.

## Important boundaries

The append-only Conversation Log is the durable semantic truth. It contains or
immutably references everything made visible to a model. A `Conversation` is
an immutable projection reconstructed from that log.

A `ModelDriver` represents one provider/model invocation. It receives the
portable conversation and yields completed semantic events, optionally
accompanied by opaque `ModelData` it defines. Provider request types, SSE
events, raw deltas, response identifiers, and transport diagnostics remain
inside the concrete driver.

The driver owns returned canonical `ConversationEvent` envelopes and their
provenance. The caller owns persistence, tool execution, retries, cancellation,
and the outer orchestration loop. A future
`Agent` may coordinate multiple invocations, delegation, workflows, and
approvals above this boundary; it is not the provider integration abstraction.

## Critical choices

### Semantic portability over provider replay

Earlier designs treated a durable provider/run log and projection identities as
Phase 1 requirements. They could improve crash recovery and same-provider
fidelity, but they would make the first implementation substantially more
complex without improving cross-model continuation.

Only the Conversation Log is required for correctness. Provider events may be
traced for development and observability. A durable run log remains a possible
future supplement, never a replacement for semantic history.

See [Conversation Is the Portable Model Input](../decisions/2026-08-22-use-conversation-as-portable-model-input.md).

### Completed semantic output over raw streaming detail

A provider invocation may produce many protocol events, but the rest of `tog`
consumes completed semantic outputs. The driver aggregates raw deltas privately
and exposes an asynchronous stream of `ConversationEvent` values, where a
model-reported problem is an event on the same stream as model output.

This replaced the earlier batch interface. Completed outputs can be persisted
immediately and remain valid if the provider stream later fails. Incomplete
deltas are discarded.

See [Use Asynchronous Streaming ModelDriver Invocations](../decisions/2026-08-22-use-asynchronous-streaming-model-driver-invocations.md).

### Durable failed-turn semantics without rollback

User input is durable before model invocation. A meaningful model limitation or
a sanitized invocation failure becomes a top-level
`ConversationEventKind::Problem`. The problem is not model output merely
because it concerns a model invocation. Model-associated problems carry
`Some(ModelDetails)`; a future unrelated problem may carry `None`.

Already appended facts are never rolled back. Detailed transport errors remain
control-flow errors and diagnostics; sensitive provider bodies and credentials
do not enter the conversation.

See [Represent Model-Associated Problems as Conversation Events](../decisions/2026-08-23-represent-model-associated-problems-as-conversation-events.md).

## Open pressure points

The current boundary should be tested by concrete implementation rather than
expanded speculatively. The most useful next pressures are:

- complete tool request, execution, response, and reinvocation behavior;
- cancellation, timeout, and retry ownership;
- useful provider tracing without turning traces into semantic history;
- concrete file and image references;
- a second production ModelDriver;
- provider-native continuation when it demonstrates measurable value.

These are open implementation and design questions, not accepted Phase 1
requirements.
