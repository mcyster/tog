# Describe drivers independently of named configurations

Give each model invocation a resolved driver description, so history explains what
ran and a reading driver can decide whether it understands saved model-specific
events. Keep configuration names independent of driver identity: several named
configurations can select the same implementation with different defaults.

Mark accepted this direction on 2026-10-03. Implementation remains planned.
Today `ModelDriver::source()` exposes a `ModelSource` containing provider and
model, and `ModelRequest` records that source. Driver identity, driver-defined
description attributes, and named configurations are not yet represented by
that contract.

## Description and configuration

The conversation owns the portable description shape. The driver supplies its
values and defines the meaning of its attributes.

| Field | Meaning |
| --- | --- |
| `driver` | Implementation or protocol identity, such as `openrouter-chat` |
| `provider` | Service handling the invocation, such as `openrouter` |
| `model` | Selected model |
| `attributes` | Resolved driver-defined settings or characteristics, such as thinking level, context length, or tool-search mode |

A description explains the configured invocation and supports interpretation of
its output. It is not a serialization of all runtime configuration. Credentials
and other secrets remain outside recorded history. Attributes may include
characteristics reported by the implementation; their presence does not make them
all user-configurable.

A configuration name selects a configuration, not a driver identity. For example,
`openrouter` and `openrouter-with-toolsearch` can both select
`driver: openrouter-chat` and `provider: openrouter`, with different defaults.
A caller can select a configuration, specify a model and thinking level, and
leave other settings at their defaults.

For configurable settings, resolve in this order:

1. Driver implementation defaults.
2. Named configuration defaults.
3. Explicit invocation settings.

The implementation interprets its settings and supplies the resulting description.
The engine does not impose a universal vocabulary for thinking or other
driver-specific attributes. The exact configuration file and CLI syntax remain
implementation choices.

The description recorded for an invocation contains resolved values. Changing a
named configuration later cannot change those historical values. A configuration
name may also be recorded for provenance, but it is not a compatibility key and
must not be needed to interpret historical events.

## Recorded events and interpretation

Model events must expose the description of the invocation that produced them.
The proposed placement is on `ModelRequest`, reached through the existing
`model_request_id` on output events. `ModelSpecificEvent` keeps its opaque
payload, event type, and payload version beside that ownership link.

Whether to copy the description directly onto model-specific events for
independent reading remains an open representation choice. The accepted
requirement is that a reader can obtain the actual writer description from
recorded history, without consulting mutable configuration. Do not introduce two
independently authored descriptions of one invocation.

The reading driver owns compatibility. It will commonly start with
`driver/provider/model`, then inspect attributes, payload type, and payload
version where needed. The conversation layer preserves these values and does not
require exact equality or prescribe which differences matter.

Consequently, two configuration names may produce compatible state, and one name
may produce incompatible state after its configuration changes. A driver may
understand state across thinking levels or older payload versions; another may
need stricter matching. Neither outcome follows from the configuration name.
State the concrete driver's behavior for unsupported state when implementing it;
do not silently feed an unrecognized payload into a provider.

This also supplies the writer context for the
[tool-loading plan](load-tools-through-drivers.md). A driver decides whether a
recorded `ToolsLoaded` snapshot and accompanying model-specific search state
apply to its current invocation. The engine still owns tool execution and
availability enforcement.

## Implementation and completion

Extend the shared description contract and record it for each invocation. Update
the existing driver to describe itself and to use writer descriptions when
interpreting model-specific history. Add named configuration selection without
using configuration names as implementation identities.

Verify resolved defaults and overrides, preservation through serialization,
historical descriptions after a configuration changes, multiple configurations
of one driver, and the driver's chosen compatibility behavior. Keep descriptions
free of secrets. No provider-specific matching policy belongs in the shared
conversation model.

Completion means invocation history explains the resolved driver, provider,
model, and attributes; configuration names remain convenient selection; and
drivers can interpret compatible historical state using the recorded writer
description. Exact attribute value representation, event placement, and concrete
configuration syntax must be settled during implementation.
