# Organize tog into a Cargo Workspace

## Why

Conversations, model drivers, assets, and storage are independently useful.
Enforcing their boundaries with Cargo makes the important concepts easy to find
and prevents the execution engine, CLI, and concrete integrations from leaking
into the reusable library. This supersedes the earlier single-package decision.

## Decision

Use a Cargo workspace of focused crates under `crates/`:

| Crate | Owns |
| --- | --- |
| `tog` | Conversations and events, the model-driver interface, asset definitions, and conversation/asset storage interfaces |
| `tog-context` | Environment and toolset, resolving the capabilities available for work |
| `tog-engine` | Executing conversations: invoking models, dispatching tools, and recording results |
| `tog-driver-openai` | The OpenAI model-driver provider; one driver crate per integration |
| `tog-tools` | Concrete tool implementations |
| `tog-store-local` | Filesystem implementations of storage interfaces |
| `tog-cli` | Command-line interaction and application composition; exposes the `tog` executable |

Dependencies flow toward `tog`: context depends on `tog`, engine depends on
`tog` and `tog-context`, and integrations depend on the contracts they implement.
The CLI selects implementations and wires them together; async runtime ownership
stays at the application boundary. The tool execution interface lives alongside
the toolset in `tog-context`. Each model-driver provider lives in its own crate
so an integration adds or changes a provider without touching the others.

## Consequences

Cargo enforces the intended boundaries, and consumers can use conversations and
a model driver without the engine or CLI. The CLI continues to expose the `tog`
executable. Storage contracts follow their concepts into `tog`, while persistence
mechanics stay in the implementation. Future applications such as a server
become additional composition crates rather than extending the CLI.
