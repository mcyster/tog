---
name: rust-abstraction-design
description: Design or revise Rust traits, service boundaries, input structs, result types, callbacks, sinks, and provider abstractions from ownership and current behavior
---

# Rust Abstraction Design

Use this skill when introducing or changing a Rust trait, service boundary, provider boundary, input or configuration struct, result wrapper, callback, sink, or event-producing API.

## Goal

Produce the smallest contract that accurately describes current domain behavior.

Do not begin from a conceptual architecture sketch and translate every box into a Rust type. Begin from ownership, lifetime, input, output, and failure behavior.

## State the contract first

Before editing code, describe the abstraction in one sentence and write its smallest plausible signature.

For example:

```text
A configured ModelDriver receives conversation history and returns new conversation events.
```

```rust
trait ModelDriver {
    fn model(&self) -> &ModelId;

    fn invoke(
        &self,
        conversation: &[ConversationEvent],
    ) -> Result<Vec<ConversationEvent>, ModelDriverError>;
}
```

If the sentence is vague or the signature needs several speculative types to make sense, the boundary is not ready to implement.

## Separate exploration from commitment

Conversation about a possibility does not make it a requirement.

Before implementation, classify relevant statements as:

* Current requirement: behavior the current change must provide
* Accepted invariant: behavior explicitly chosen as a durable constraint
* Exploration: an idea being considered or compared
* Future possibility: something that may matter later
* Superseded: a direction replaced by later feedback

Only current requirements and accepted invariants justify implementation complexity.

Words such as `might`, `could`, `later`, `perhaps`, `preserve room`, and `future` are evidence that a statement is not a current requirement.

Do not promote an explored concern such as crash recovery, streaming, compatibility, extensibility, observability, or multiple providers into architecture without an explicit current scenario and desired behavior.

When the user is exploring alternatives, summarize the tradeoff and wait for a direction before writing code. Once a direction is chosen, restate the accepted contract without carrying every explored alternative into it.

## Require traceability

Every nontrivial abstraction should trace to a current requirement or accepted invariant.

Use this test:

```text
Requirement -> behavior -> smallest mechanism
```

Do not reason in the opposite direction:

```text
interesting mechanism -> possible future benefit -> assumed requirement
```

If a type, callback, sink, log, identifier, configuration field, or recovery path has no current requirement, remove it.

## Classify values by lifetime

For every proposed field or parameter, ask when it changes.

* Stable for the object's lifetime: constructor argument and private field
* Different for each operation: method parameter
* Derived from another value already supplied: do not pass it separately
* Produced by the operation: return it
* Needed only by presentation, persistence, logging, or process control: keep it outside the domain abstraction

This classification takes priority over making a method have fewer parameters.

For example, a selected model belongs to a configured model driver if it does not change between invocations. It does not belong in every invocation request merely because the provider API accepts a model field.

## Avoid parameter bags

An input struct should represent a coherent domain concept, not conceal an unsettled method signature.

Treat names such as these as warning signs:

* `SomethingInput`
* `SomethingConfig`
* `SomethingOptions`
* `SomethingContext`
* `SomethingResult`

These names are not forbidden. Require each one to have an independent meaning and lifecycle.

Do not combine identity, history, object configuration, and per-call data into one request merely because they are all available at the call site.

If removing object-owned and derived values leaves one direct input, pass that input directly.

## Do not duplicate relationships

Avoid passing both an entity identifier and data already known to belong to that entity unless the callee independently needs both.

For example:

```rust
struct InvocationInput {
    conversation_id: ConversationId,
    conversation_events: Vec<ConversationEvent>,
}
```

does not enforce that the events belong to the identifier. Prefer only the value the callee uses, or introduce a validated domain snapshot if the relationship itself matters.

## Prefer return values

Return produced domain values directly by default.

Introduce a callback, sink, channel, or stream only when a current requirement needs at least one of:

* output before the operation completes
* preservation of partial output on failure
* backpressure
* unbounded output
* concurrent consumption
* multiple subscribers

State which requirement justifies the mechanism and test its failure semantics.

Do not add a sink because the provider protocol streams internally. An integration may consume a stream and still return aggregated domain values.

Do not add a sink to preserve partial output or crash recovery unless preserving that output is an accepted current requirement. If the caller explicitly accepts losing partial output on failure, remove the sink and the recovery machinery.

Do not make a subtype-specific sink when the operation produces the enclosing domain type. If an invocation produces `ConversationEvent`s, return or emit `ConversationEvent`s even when some variants contain model events.

## Keep output hierarchies intact

Distinguish the operation's output type from variants nested inside it.

For example:

```text
ConversationEvent
    User
    Model(ModelEvent)
    ToolRequest
    ToolResponse
```

A model event is part of a conversation event. It is not automatically a separate output channel.

Avoid parallel result objects that duplicate facts already represented by returned events. Add a result wrapper only when control-flow information cannot be derived clearly from the domain output.

## Separate mechanism from policy

Domain abstractions return errors and values. They do not choose whether callers print to standard error, write to standard output, send to a remote logger, retry, or ignore information.

Rust's `std::error::Error` and `Display` traits provide interoperability and formatting. They do not log or select an output destination.

Configuration belongs to the layer whose behavior it controls. For example, CLI verbosity that filters event messages belongs to the CLI, not to a model driver or provider request.

## Preserve dependency direction

Concrete integrations depend on neutral contracts.

```text
openai -> model_driver
model_driver -/-> openai
```

Do not nest or re-export a concrete implementation from the neutral contract merely because it implements the trait.

Do not allow concrete provider request types, response types, or configuration to define the neutral abstraction.

## Justify a trait by its contract

Introduce a trait when it lets readers understand the available operations
independently of an implementation.

Acceptable evidence includes:

* a readable contract that exposes operations and their semantics
* two current implementations
* tests that substitute behavior at an intentional boundary
* an explicit accepted architectural requirement, such as provider switching

A second implementation is not required. Keep the trait smaller than its first
implementation. A trait describes what callers may rely on, not everything the
concrete type can do.

## Keep the interface file to the contract

An interface file presents operations and their documented semantics. The
contract is the trait signatures and the error variants they can produce.
Formatting, conversions, and other implementations belong with the concrete
type or a dedicated implementation module.

An interface file holds no fields, implementation bodies, private helpers, or
storage details.

Place the concrete implementation with its state, implementation helpers, and
construction. Construction that selects configuration, such as an explicit
location or an environment-derived default, belongs to the concrete type, not
the trait.

Prefer no comments. Treat a comment as a last resort for meaning that no name
or type can carry.

Before writing one, try:

* a more specific method or type name
* a stronger parameter or return type that makes the condition impossible
* an error type whose variants name each failing condition

Do not restate the method name, parameters, or return type. Do not describe
implementation details or concepts the interface does not own.

## Describe failure with types

Give each operation an error type whose variants name that operation's failing
conditions. Keep a shared low-level error small and opaque; do not leak
positions, markers, checksums, or other implementation mechanics through it.

Represent absence with `Option` rather than an error.

## Keep abstractions proportional to their value

Evaluate every wrapper, conversion, and helper type by whether it makes the
code easier to understand and use. Keep those that earn their place.

A type that only makes an invalid value unrepresentable while the consumer
must still report the same failure adds a concept without removing work.
Prefer the direct parameter and the error variant.

## Keep concepts together

Identifiers and supporting types belong alongside the concept they describe.
A consumer reading one concept should not need to open an unrelated module to
find its types.

Extract by responsibility. Split cohesive responsibilities, rather than
grouping unrelated items by technical form.

## Reset after structural feedback

When feedback changes ownership, lifetime, output hierarchy, or failure semantics, stop patching the current design.

Restate:

1. what the abstraction owns
2. what one operation receives
3. what one operation returns
4. who persists the result
5. who presents or logs it
6. what happens on failure

Then remove types that no longer follow from that model before introducing replacements.

Do not preserve a recently introduced abstraction merely because code has already been written around it.

When a simpler direction intentionally gives up a previous property:

1. state the lost property plainly
2. confirm the tradeoff if it is consequential
3. mark the previous direction superseded
4. remove its code, tests, and authoritative documentation

Do not continue defending or accommodating a superseded property after the tradeoff is accepted.

Delete superseded formats and mechanisms outright. Compatibility or migration
code needs a released representation and a current consumer; before the first
release, remove the old path instead of carrying it forward.

Architectural documents may contain future sections. Future sections do not constrain the current implementation unless the current milestone explicitly adopts them.

## Red flags

Pause and reconsider when:

* an interface file contains fields, implementation bodies, or private helpers
* a comment restates the method name, parameters, or return type
* a comment describes implementation details or concepts the interface does not own
* a failure condition exists only in a comment instead of the return type
* a trait exposes construction or configuration that only one implementation can choose
* a method is on the contract only because its implementation needs it
* an invocation input contains object identity, history, model selection, and configuration
* the callee receives values it never reads
* a trait method repeats values already stored by the implementor
* a sink exists only because future streaming might be useful
* recovery machinery exists without an accepted recovery guarantee
* a result wrapper duplicates returned events
* a neutral module imports or re-exports a concrete integration
* presentation policy appears in a provider or domain contract
* each correction adds another type instead of deleting a mistaken one
* an idea survives only because it appeared earlier in the conversation or documentation
* the abstraction cannot be explained without describing its first provider

## Review checklist

Before completing an abstraction change, verify:

1. The abstraction has a one-sentence responsibility.
2. Every field is owned at the correct lifetime.
3. Every parameter varies per operation and is used directly.
4. Derived relationships are not passed twice.
5. Outputs use the enclosing domain type.
6. A direct return was considered before a sink or callback.
7. Failure behavior is explicit and tested.
8. Persistence, logging, CLI, and provider policy remain outside unless they are the abstraction's stated responsibility.
9. Dependency arrows point from integrations to contracts.
10. Removing any type would make a current requirement impossible.
11. Every explored but unaccepted idea has been excluded from implementation.
12. Superseded guarantees have been removed from code, tests, and authoritative documentation.
