# Organize tog around enforced crate boundaries

Make the important concepts easy to find and enforce their dependencies with a
Cargo workspace. A developer should be able to use conversations and model drivers
without adopting tog's execution engine, CLI, or server.

This is intended direction. Tog currently remains one binary package.

## Direction

Keep conversations, events, model-driver and storage contracts, and asset
concepts in a foundational `tog` library. Put provider implementations in
`tog-drivers`. Separate context definition and resolution (`tog-context`) from
execution (`tog-engine`), with concrete tools and local storage outside both.
Applications choose implementations and compose them.

Dependencies point toward contracts. The foundational library must not depend
on the engine, CLI, or concrete integrations. Storage contracts stay with the
concepts they store; filesystem formats belong to implementations. Context
supplies configuration and capabilities without taking ownership of execution.

The CLI continues to expose the `tog` executable. A REST server is a future
application boundary, not required work for this split. Do not create empty
crates in anticipation of future capabilities.

## Boundaries and completion

Extract the reusable library and providers first, then move existing responsibilities
into context, execution, tools, and storage. Keep current command behavior, event
semantics, and stored-data compatibility. Update the initial single-package decision
and repository guidance when the workspace is actually introduced.

The split is complete when Cargo enforces these dependencies, a consumer can use
conversations and a model driver without the engine or CLI, and the CLI uses the
engine with explicitly supplied implementations.

Settle the minimal executable-tool interface while establishing its ownership.
Keep provider implementations together until dependency isolation or distribution
justifies separate crates. Publishing packages is a separate choice.

For the crate responsibility table, dependency rules, and context relationships,
read the [detailed boundary design](../designs/organize-crate-boundaries.md).
For dispatch and recovery semantics, read the
[durable-execution plan](execute-commands-durably.md).
