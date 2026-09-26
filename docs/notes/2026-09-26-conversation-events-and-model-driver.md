# Conversation Events And ModelDriver Boundary

Dated note recording the agreed conversation-event vocabulary and driver
boundary, and the concrete decisions that implement it. Implementation is
authoritative for exact field names; this note preserves the intent and the
deliberate choices.

## Agreed direction

`ConversationEvent` is a flat vocabulary owned by the conversation concept:

```text
User
TurnStart
TurnEnd
AssistantResponse
ToolRequest
ToolResponse
ModelRequest
ModelResponse
ModelSpecificEvent
Automation
Context
Data
```

`Command`, `Fact`, `Message`, and `Lifecycle` no longer exist as structural
intermediate branches, and there is no separate `Kind` domain layer. A serialized
`type` discriminator remains for storage but does not introduce a second
hierarchy. A request is a fact to one consumer and a command to another; its
concrete contract describes who acts on it.

## Identity

One `ConversationEventId` identifies events and references. There are no
separate model-request, tool-request, or turn identifier types. References use
descriptive fields: `model_request_id`, `tool_request_id`, `turn_id`.
`ConversationId` remains distinct.

`ConversationEventRecord` is only a persistence envelope: conversation ID, event
ID, position, timestamp, schema version, and event. It introduces no second rich
event hierarchy; serialization and transaction framing stay persistence
concerns.

## Model relationship

Five events are model-related: `AssistantResponse`, `ToolRequest`, `ModelRequest`,
`ModelResponse`, and `ModelSpecificEvent`. A `ModelEvent` trait expresses that
relationship for the four events that reference a model request. It is distinct
from permission to produce events.

The driver receives the full conversation vocabulary through an immutable bounded
view (events through the request's `input_through` position). The engine commits
`ModelRequest` before invoking the driver; the driver cannot produce it. The
driver stream may contain only `AssistantResponse`, `ToolRequest`,
`ModelResponse`, and `ModelSpecificEvent`, and the restricted output enum reuses
those payloads rather than copying them. The engine records the terminal
`ModelResponse` it receives and fills `output_event_ids` from the outputs
committed during the call; drivers cannot know those durable identifiers. A
stream that ends without a terminal response is closed by the engine as a failed
`ModelResponse`, never as success.

## Completion and failure

Assistant output is content, not turn completion. A successful terminal model
call with no outstanding tool work completes a turn without assistant text.

Failure belongs to the operation that failed:

- `ToolResponse` carries the tool outcome
- `ModelResponse` carries the model-attempt outcome
- `TurnEnd` carries the turn outcome

`OperationFailure` is a shared payload: portable `category`, message, retry
guidance, and known-versus-uncertain execution outcome. Storage failures are
never relabeled as provider failures.

## ModelSpecificEvent

`ModelSpecificEvent` carries a model-request reference, provider event type,
payload version, provider-specific payload, and an optional human-readable
message. The message is for display only; its presence does not make the event
assistant content or provider input. Compatible drivers interpret their payloads
for native replay; other drivers ignore them.

## Decisions on the open choices

- **Queued intent.** `User` is the durable record of submitted input; it is both
  the request to process and the accepted content. A turn's queued intent is the
  `User` its `TurnStart` references; `TurnStart.user_id` records that trigger.
  `TurnStart` records actual start and never substitutes for the queued intent.
  Delivery to a model is bounded by each `ModelRequest.input_through`.
- **ToolsAvailable.** It is a `Context` value (`context` event with a `tools_available`
  kind). Each declaration replaces the previous toolset; the latest declaration
  within the bounded view is authoritative.
- **Model data attachments.** The ad-hoc `ModelData` attachment stays only where
  a concrete durable replay need exists: on `ToolRequest` (provider-native call
  correlation) and as `ModelRequest.options`. Model-associated auxiliary data and
  native replay state move to standalone `ModelSpecificEvent` values.
- **Communication importance.** The three-level importance scale was removed.
  Display is governed by event kind: assistant content always renders,
  `ModelSpecificEvent` messages render at medium and high verbosity, and a failed
  `ModelResponse` renders its failure message. No generic severity field was
  added.
- **Migration.** Existing persisted events are explicitly not migrated. The
  serialized shapes changed across the vocabulary, identity, and failure model;
  before the first release the old path is removed outright rather than carried
  forward.

## Module layout

Event types live under `src/conversation/events/`; the reading path is
conversation → events → model → concrete event. `Automation`, `Context`, `Data`,
`User`, `TurnStart`, `TurnEnd`, `OperationFailure`, and `ToolResponse` sit beside
the model tree. Entry files declare relationships; payload implementations live
in the concrete child modules.

## Out of scope

A daemon, concurrent tool execution, automatic retries, and the queued-intent
recovery policy are separate work. `ModelRequest.retry_of` exists for the durable
record; retry scheduling is not implemented.
