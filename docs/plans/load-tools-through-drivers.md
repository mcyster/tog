# Let drivers manage tool loading

Record available tools in the conversation and let each model driver decide how
to present or discover them. `Tools` defines availability; `ToolsLoaded` records
a driver's loading decision. The engine executes requested tools and persists
output, while the driver manages its provider's context.

Mark accepted this direction on 2026-10-03. This plan extends existing tool support;
it does not describe completed loading support. Today the conversation defines
`Tools`, full tool definitions, and `Immediate` / `Discoverable` availability.
The engine builds declarations from registered tools, and bounded `TurnInput`
already exposes `tools()`. There is no `ToolsLoaded` event yet. Environment
currently supplies the data directory; its role in configuring toolsets remains
intended direction.

## Responsibilities

| Component | Responsibility |
| --- | --- |
| Conversation | Defines Tools, ToolsLoaded, definitions, availability, and portable event meaning |
| Environment and toolset | Provide configured available definitions and executable implementations |
| Engine | Records Tools from the toolset, commits driver output, executes tool requests, and records tool responses |
| Driver | Reads recorded definitions and state, chooses presentation or discovery, and emits ToolsLoaded when applicable |

The environment composes configuration; the toolset describes availability and
the executable registry supplies implementations. Neither executable objects nor
credentials belong in conversation events.

The driver never executes an application tool. When discovery is an ordinary
tool invocation, the driver emits a tool request and the engine executes it and
records its response. On the next invocation the driver interprets that response
and decides what to load. The engine does not automatically write `ToolsLoaded`
because a search succeeded.

Provider-hosted discovery can happen within one model invocation. The driver
records its portable loading decision through `ToolsLoaded`, with opaque
provider continuation state in `ModelSpecificEvent` when needed. The engine
commits both through the existing driver-output path.

## Availability and loading

`Tools` is the complete available set, including definitions. Each declaration
replaces the previous one; an empty set removes all available tools.

- `Immediate`: present without discovery.
- `Discoverable`: may be deferred by a driver that supports discovery.

Loading never grants access beyond current availability. A driver without loading
support may present all available tools on every call and emit no `ToolsLoaded`.
Absence of that event does not mean that no tools are usable.

For a driver using deferred loading, the initial loaded set is the immediate
tools. Each `ToolsLoaded` records a complete snapshot, including immediate tools,
rather than a delta. A compatible reader can reconstruct that state from the
latest applicable snapshot.

A newer `Tools` declaration always wins: removed tools cease to be available,
new immediate tools are included, and previously loaded discoverable tools remain
loaded only while still available. Definitions come from the current declaration.
The driver applies these rules to its context and records subsequent loading
decisions; the engine enforces availability when executing requests.

A name outside availability, or one with no executable implementation, receives a
tool response describing the problem. The driver records the model's request;
it does not fail the whole model invocation merely because the name is unknown.
Being absent from a loaded snapshot is not itself an execution permission failure.

## Conversation input and event detail

Keep `invoke`'s existing conversation-based input. No separate tools parameter or
definition resolver is needed: the bounded history already contains definitions
and loading decisions. A helper can expose recorded snapshots, but must not imply
that the latest snapshot applies universally to every driver.

`ToolsLoaded` is conversation-owned and a permitted driver output. It participates
in the normal model-event ownership and persistence rules.

| Attribute | Meaning |
| --- | --- |
| `tool_names` | Complete set loaded by the driver, including immediate tools |
| `model_request_id` | The invocation that emits this loading decision |
| `cause_event_id` | The tool request or model request that caused loading |

The emitting invocation and discovery cause can differ: a search tool result is
interpreted on a later invocation. Keep that distinction when finalizing the event
API.

The current driver decides whether another driver's recorded loading state is
applicable. Before implementation, settle how the record identifies its writer
and how compatibility is determined, alongside the driver-description design.
Do not assume that a snapshot or opaque provider state can be reused after every
driver change. A driver must also define how it recognizes the discovery results
it supports; no universal search-result format is prescribed here.

## Implementation and completion

Add the conversation event and permit it in driver output. Implement loading in
one driver with the existing engine execution path, preserving the option for
other drivers to declare all tools. Keep the engine independent of provider
discovery protocols.

Verify ordinary and hosted discovery paths as applicable to that driver, complete
snapshots across multiple discoveries, resuming recorded state, changes to
availability, switching drivers, unknown tool names, and a driver that emits no
loading events. Historical input remains bounded to the model request.

Completion means a supporting driver can discover and load tools, continue from
recorded state, and request their execution without a second source of tool
definitions or engine-owned loading decisions. New providers and search algorithms
are separate work.
