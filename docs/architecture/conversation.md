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
references, its observed outcome, and the driver's continuation decision.

Every terminal response records a continuation:

- `complete`: the turn should end.
- `continue`: execute the requested tools, then invoke the model again.

The driver derives the decision from provider semantics. A failed attempt may
also request continuation. Assistant text can accompany tool requests. It can
also be absent from a successful call. Neither text nor the absence of tool
requests decides completion.

The session follows the recorded continuation:

- `continue`: the session executes the requested tools sequentially. It records
  their results. It invokes the model again. It bounds continuation rounds.
- `complete`: the session ends the turn. A successful response ends it as
  `succeeded`. A failed response ends it as `failed`. Assistant text is not
  required.

A `complete` response that also requested tools is a driver contract error. The
session preserves the response. It answers each unexecuted tool request with a
durable failure. It fails the turn.

The session records normal stream exhaustion without a terminal response as
failure.

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
