# Organize tog around enforced crate boundaries

For the goal, boundaries, and remaining choices, start with the
[plan](../plans/organize-crate-boundaries.md). This document provides the detailed design.

Make the important concepts easy to find and enforce their dependencies with a
Cargo workspace. Developers should be able to use conversations and model drivers
without adopting tog's execution engine, CLI, or server.

This workspace is now implemented under `crates/`, and the CLI runs through the
extracted engine with explicitly supplied implementations. See the
[crate-workspace decision](../decisions/2026-10-03-organize-into-a-crate-workspace.md)
and the current [architecture overview](../architecture.md).

## Crate responsibilities

| Crate | Owns |
| --- | --- |
| `tog` | Conversations and events, the model-driver interface, asset definitions, and conversation/asset storage interfaces |
| `tog-driver-openai` | The OpenAI model-driver provider; one driver crate per integration |
| `tog-context` | Environment, session, workspace, and toolset; resolving the context and capabilities available for work |
| `tog-engine` | Executing conversations: invoking models, dispatching tools, recording results, and coordinating lifecycle, cancellation, and retries |
| `tog-tools` | Concrete tool implementations |
| `tog-store-local` | Filesystem implementations of storage interfaces |
| `tog-cli` | Command-line interaction, commands, configuration, and application composition |
| `tog-agent-server` | REST interface and server application composition |

Use `tog` for the foundational library rather than a generic `core` crate.
Use `tog-engine` for execution. Keep context definition and resolution separate
from the engine that uses it.

The CLI continues to expose the `tog` executable even though its package is
named `tog-cli`. The server is a future application boundary; this plan does
not require implementing a server as part of the initial split.

Place packages under `crates/`. Within the foundational library, keep
conversation/events, model_driver, and assets visibly separate. Within
`tog-context`, expose environment, session, workspace, and toolset as focused
modules. Start with these four concepts in one crate; split them further only
when an independent use or dependency constraint warrants it.

## Ownership and dependencies

- `tog` does not depend on context, engine, applications, or concrete providers,
  tools, and stores.
- `tog-context` depends on `tog`, not on the engine or concrete integrations.
- `tog-engine` depends on `tog` and `tog-context`. Applications supply the
  implementations it uses through the relevant interfaces.
- Provider drivers depend on the model-driver contract in `tog`.
- Concrete tools depend on the crate owning their execution contract, not on
  the engine.
- Storage implementations depend on the crates owning their storage contracts.
- CLI and server select implementations and wire them together. Async runtime
  ownership stays at these application boundaries.

Put each storage contract with the concept it stores: conversation and asset
contracts belong in `tog`; a session storage contract, when needed, belongs in
`tog-context`. Persistence formats and filesystem mechanics belong to the
implementation. CLI-specific state can remain within the CLI.

The CLI can choose among storage implementations. The engine must not choose a
filesystem store implicitly or require callers to depend on CLI code to use it.

Tool definitions carried by conversation events remain owned by `tog`.
The toolset assembles those definitions and resolves available tools; concrete
implementations live in `tog-tools`. If the toolset holds executable tools,
place their execution interface in `tog-context` alongside it. Settle the
minimal interface when implementing this boundary rather than introducing a
general plugin framework.

## Context and execution

Preserve the model described in the
[environment, session, and workspace note](../notes/2026-09-26-environment-session-conversation.md):

- Environment supplies resources and configuration sources.
- Session represents a particular loading of configuration and capabilities.
- Workspace is a view resolved from session configuration and working location.
- Toolset describes the applicable tools and connects definitions to implementations.

The conversation records session selection and working-location changes.
Workspace does not need an identity or independent persisted snapshot at this
stage. Defining these concepts in `tog-context` does not move execution into
the session or make the foundational library depend on context types.

The engine uses the selected context to carry out work. CLI and server share
that execution behavior. Existing orchestration in `ConversationSession`
should be separated according to these responsibilities rather than moved
wholesale into the new session module.

The [conversation design plan](../plans/conversation-design.md) owns event vocabulary
and the model-driver boundary. The
[durable execution plan](../plans/execute-commands-durably.md) owns execution semantics.
This structural change does not require implementing all planned execution
features or changing event and persistence behavior.

## Implementation direction

Extract the reusable conversation library and provider implementations first,
then separate context, execution, concrete tools, and local storage as their
existing responsibilities are moved. Keep the CLI as the composition point.
Do not create empty crates for future applications or capabilities.

Treat `tog` plus its driver crates as the reusable library offering. Use one
crate per provider integration — `tog-driver-openai` is the first — so an
integration adds, updates, or removes a provider without touching the others and
each provider's dependencies stay contained. Publishing packages is separate
from establishing the workspace.

The split is complete when Cargo dependencies enforce the stated boundaries,
a consumer can use conversations and a model driver without the engine or CLI,
and the CLI runs through the extracted engine with explicitly supplied
implementations. Preserve command behavior, event semantics, and stored data
compatibility. Run the workspace formatting, lint, and test checks after moving
code.
