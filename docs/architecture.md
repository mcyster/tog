# Architecture

Tog builds agentic work around a durable conversation. The conversation preserves
what was requested and what happened; model drivers interpret it for providers,
and execution coordinates the work. Keeping these responsibilities separate lets
the same conversation outlive a particular model, interface, or process.

## The important boundaries

| Concept | Responsibility |
| --- | --- |
| Conversation | Immutable, ordered events and their meaning; shared references, context, and tool definitions |
| Model driver | Translate bounded conversation input and provider output; preserve portable meaning alongside provider-specific state |
| Execution | Commit requests, invoke models, execute tools, and record call and turn outcomes |
| Storage | Persist events and immutable assets behind contracts; keep filesystem mechanics outside event meaning |
| Application | Choose implementations and provide the command-line or other user interface |

A model driver requests tool work through events; it does not execute tools or
own the turn. Assistant text is content, not a completion signal. A model attempt
and the turn containing it have separate outcomes.

Recorded history is the source for reconstruction. Replaying it must not execute
external work. Model-visible semantic state belongs in events or immutable
references; credentials and execution machinery remain outside that history.
Portable conversation meaning must not depend on another provider's protocol.

## Where the implementation stands

Tog currently has one binary Cargo package. `conversation` defines the event model,
`model_driver` its provider boundary, and `openai` the concrete integration.
`ConversationSession` coordinates a bounded loop with sequential tool execution.
The CLI selects local storage and concrete tools. Assets are immutable files
managed through `AssetStore`.

These are responsibilities within the current package, not yet separate crates.
The future context/session model must not be inferred from the existing
`ConversationSession` name. Daemon scheduling, automatic recovery and retries,
and concurrent tool dispatch remain planned work.

## Read further when the work needs it

- For event ownership, model input, and completion rules, read the
  [conversation boundary](architecture/conversation.md).
- For asset identity and persistence, read [assets](assets.md).
- For executable tools and shell behavior, read [tools](tools.md).
- For the intended library, context, engine, and application split, read the
  [crate-boundary plan](plans/organize-crate-boundaries.md).
- For future scheduling, recovery, and side-effect constraints, read the
  [durable-execution plan](plans/execute-commands-durably.md).

Plans describe intended changes. Their presence does not make a proposed API or
capability part of the current architecture.
