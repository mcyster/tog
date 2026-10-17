# Conversation and model-driver boundary

The conversation records meaning shared by the application and model providers.
A driver consumes a fixed view of that history and returns a restricted set of
outputs. Execution owns the work around that invocation. This keeps provider
translation separate from decisions about when work runs and when it is complete.

## Ownership

The current vocabulary includes user content, turn boundaries, model requests
and responses, assistant responses, tool requests and responses, provider-specific
events, context, tool declarations, and automation. `ConversationEvent` owns its
identity, conversation identity, timestamp, and payload. The store adds position
and schema version; event meaning does not depend on transaction framing.

`ModelDriver` returns only assistant responses, tool requests, model responses,
and model-specific events. The session commits the model request first and gives
the driver an immutable input view bounded by its recorded position. The driver
cannot create the authoritative request, accept user input, or close the turn.

`Tools` declares the available definitions; executable implementations live
outside the conversation. `Context` supplies named, typed values that consumers
interpret when applicable. Neither a context value nor provider-specific display
text automatically becomes model input.

## Calls and turns

A model response closes one attempt. The session records its ordered output
references, observed outcome, and the driver's continuation decision. Every
terminal response states whether the model needs another invocation: `complete`
means the turn should end, while `continue` asks the session to execute any
requested tools and invoke the model again. The driver derives that decision
from provider semantics and may also request continuation after a failed
attempt. Assistant text can accompany tool requests or be absent from a
successful call; neither text nor the absence of tool requests decides
completion.

The session follows the recorded continuation: `continue` executes the requested
tools sequentially, records their results, and invokes the model again, bounding
continuation rounds; `complete` ends the turn with `succeeded` for a successful
response and `failed` for a failed one, with or without assistant text. A
response that records `complete` while this call also requested tools is a
driver contract error: the session records that attempt as failed rather than
completing a successful turn with work left unanswered. Normal stream
exhaustion without a terminal response is recorded as failure.

This does not promise complete recovery from every interruption. Invocation or
stream errors can escape before a terminal event is written. Automatic retries,
recovery, and tools running while model output is still arriving belong to the
[durable-execution plan](../plans/execute-commands-durably.md).

## Follow the boundary into detail

For the exact declarations and permitted output variants, start with
[`conversation/events.rs`](../../src/conversation/events.rs) and
[`model_driver.rs`](../../src/model_driver.rs). For orchestration behavior, read
[`conversation_session.rs`](../../src/conversation_session.rs).

For the broader semantic intent, provider replay, and event relationships, read
[the conversation model](../conversation.md). It includes intended guarantees
beyond the current executor. For the earlier alternatives and their disposition,
read the [conversation plan](../plans/conversation-design.md).
