# Record Commands And Turn Lifecycle

This note captures the direction following the review of PR #5. It records
design guidance and unresolved questions; the implementation remains the
authoritative source for exact field names and serialization.

## Direction

The durable history explains both what was requested and what happened. Command
records and semantic facts share one ordered append-only log. This keeps intent
visible for future replay without making commands part of model-visible history.

Semantic event kinds remain readable. `User`, `Assistant`, `Communication`, and
`Problem` describe different meanings. A generic `Model` wrapper should not hide
those distinctions. Applicable model facts carry `ModelDetails` and a stable
`ModelInvocationId`.

## Simple Greeting

Adding user content and requesting agent work are separate concepts:

```text
UserMessageRequested
User
TurnRequested
DriverInvocationEvent
Assistant
TurnCompleted
```

`UserMessageRequested` records input received by the system. `User` records the
accepted contribution to portable conversation history. User content can exist
before a turn is requested, so it does not need to belong to a turn.

`TurnRequested` starts agent work. The driver creates a driver-defined invocation
event and its `ModelInvocationId`. Assistant, communication, problem, and future
model-produced tool facts reference that identifier.

`TurnCompleted` is an explicit terminal fact. An assistant response and turn
completion are independent: one invocation may produce multiple events, and a
turn may finish with a problem without producing an assistant response. A
problem does not necessarily complete a turn because orchestration may recover
or retry.

## Replay And Ordering

Replaying history reconstructs state without executing commands again. Explicit
re-execution can use recorded commands to produce new outcomes; model output
may differ and tools may have side effects.

Turn IDs associate work and outcomes. Invocation IDs distinguish multiple
attempts or invocations within one turn. Tool execution will need its own
correlation when introduced; adjacency is not sufficient once work runs
concurrently.

Driver-defined records use a shared envelope containing the driver name and
version, event type and schema version, human-readable description, and opaque
payload. A driver decoder may reconstruct the concrete event; unavailable
drivers do not prevent preserving the envelope.

The append boundary assigns durable positions, record IDs, and timestamps.
Drivers do not allocate positions from an input snapshot. Log order records
append order; typed references explain causal relationships.

## Open Questions

- What is the final name for the user-content command?
- Should command and fact records use one enum or an outer record envelope?
- What exactly defines a turn when tools are outstanding?
- What metadata identifies the input actually used by a model invocation?
- How should retries relate to the original turn and invocation?

Implement the smallest coherent lifecycle before building a replay engine or a
general scheduling framework.
