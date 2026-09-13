# Use Schemars For Tool Contracts

## Status

Accepted

## Context

A portable `ToolDefinition` needs JSON Schema for its `parameters` and
`result`. The schemas should stay aligned with the Rust types a tool actually
deserializes and produces, and the project prefers deriving descriptions from
the type instead of maintaining duplicated schema constants. Argument
validation is not part of this slice, but a conventional schema generator is
still the right source for the recorded contract.

## Decision

Use `schemars` with the `derive` feature and store schemas as
`schemars::Schema`. Tool implementations build definitions from their Rust
parameter and result types with `schema_for!`.

Schemars generates schemas from Serde-derived type shapes. It does not validate
argument values at runtime; tools validate by deserializing arguments into
their parameter type and returning an `invalid_arguments` execution problem on
failure.

The standard library has no JSON Schema derivation. Schemars is the
conventional Rust crate for this capability, is widely used, and keeps the
schema next to the type it describes. Hand-maintained schema constants were the
main alternative and were rejected because they drift from the types.
`jsonschema` was not needed because this slice does not validate values.

## Consequences

`schemars::Schema` wraps an arbitrary JSON value and intentionally does not
implement `Eq`. Conversation facts that carry tool definitions therefore
implement `PartialEq` rather than `Eq`. Tests and comparisons use equality, not
hashing, so no behavior depends on total equality.

Schemars includes `$schema` metadata in generated schemas. The recorded
definition keeps that metadata for portability. OpenAI request construction
projects a provider view that omits `$schema` without mutating the recorded
definition.
