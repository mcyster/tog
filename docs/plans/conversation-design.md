# Keep conversation meaning and driver permissions separate

Give conversation events one clear vocabulary. Keep an event's meaning, permission
to produce it, and storage metadata distinct, so callers can understand the model
without navigating parallel classifications.

The central direction is now implemented: the flat event vocabulary, restricted
model-driver outputs, common model-request and response lifecycle, `Context`, and
`Tools`. Use the [current conversation boundary](../architecture/conversation.md)
when changing these areas. This plan is not a request to reimplement the earlier
proposal.

## Preserve the principles

Conversation owns shared event definitions. Drivers translate providers and produce
only permitted outputs; execution owns requests, tool execution, and turn completion.
Provider-specific state can accompany portable meaning without replacing it.

Event identity and references belong to the conversation model. Storage adds order
and versioning while keeping transaction mechanics outside event payloads. Failures
belong to the operation that failed; assistant text is not a lifecycle signal.

## Earlier design and remaining work

The [earlier detailed proposal](../designs/conversation-design.md) preserves the
alternatives and rationale. Several questions there were subsequently resolved:
`Tools` is its own event, `Data` is absent, and the event itself owns identity and
timestamp. Its signatures and envelope sketches must not be treated as current API.

Further dispatch, retry, and recovery work belongs to the
[durable-execution plan](execute-commands-durably.md). Resolve any remaining contract
or migration question against the current code and semantic intent before making
that change; the old proposal's open-question list is not an implementation backlog.
