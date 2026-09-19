---
name: rust-domain-oriented-structure
description: Organize Rust code around domain concepts and ownership rather than technical categories
---

# Rust Domain Oriented Structure

Use this skill when adding, moving, or reorganizing Rust types, files, and modules. Use the `rust-abstraction-design` skill as well when deciding whether a trait, input struct, result wrapper, callback, or sink should exist.

## Goal

Organize code around domain concepts and ownership.

The filesystem and module hierarchy should help explain the architecture of the system.

Prefer structures such as:

```
conversation.rs
conversation/
  id.rs
  event.rs
model_driver.rs
openai.rs
```

Avoid structures organized primarily by technical category:

```
identifier.rs
events.rs
models.rs
types.rs
helpers.rs
utils.rs
```

## Core rule

A type should normally live with the domain concept that owns its meaning.

Examples:

* ConversationId belongs to conversation
* ConversationEventId belongs to conversation
* ModelEvent belongs to conversation
* ModelId belongs to the model driver contract
* ToolCall belongs to the domain abstraction that defines what a tool call means

Do not centralize types merely because they share an implementation characteristic.

ConversationId and ConversationEventId may both wrap UUID values, but UUID wrapping is not their architectural relationship.

## Before creating or moving a module

Ask:

1. What domain concept owns this type
2. What other types change for the same reasons
3. What should callers think they depend on
4. Does the proposed module represent a concept in the system or merely a programming mechanism
5. Does this change make the source tree explain the system more clearly

Prefer concept names.

Treat these names as warning signs that require justification:

* identifier
* types
* models
* common
* shared
* helpers
* utils
* events

These names are not forbidden. They often indicate that unrelated concepts are being grouped by implementation detail.

## Grow modules around concepts

A domain concept may begin as one file:

```
conversation.rs
```

When the concept becomes substantial, promote it to a module:

```
conversation.rs
conversation/
  id.rs
  event.rs
```

Do not wait until the original file becomes a dumping ground.

Likewise, do not create one file per type automatically.

Split when meaningful internal concepts emerge.

## Present a conceptual API

Internal file layout should not become the API used throughout the codebase.

For example, conversation may internally contain id.rs and event.rs while callers depend on types through the conversation module.

Prefer:

```
crate::conversation::ConversationId
crate::conversation::ConversationEvent
```

Avoid spreading internal structure through callers:

```
crate::conversation::id::ConversationId
crate::conversation::event::ConversationEvent
```

The domain module should expose the types that form its useful interface.

This allows internal organization to evolve without forcing unrelated code changes.

## Minimize navigation

Organize so a reader can understand a concept and its contract with minimal
navigation. Put the contract and its supporting types where a reader looking
for the concept finds them first, and keep implementation mechanics in child
modules opened only when needed.

Evaluate each module by whether it helps a reader find and understand a
concept. Split cohesive responsibilities rather than grouping unrelated items
by technical form.

## Domain abstractions and integrations

Keep model independent domain concepts separate from provider specific integrations.

For example:

```
conversation/
model_driver.rs
openai.rs
anthropic.rs
```

Conversation defines the model independent language of the application.

Provider integrations translate between provider representations and that common language.

Do not let the first provider implementation define the core domain model by accident.

Follow dependency direction when deciding module ownership:

```text
openai -> model_driver
anthropic -> model_driver
model_driver -/-> openai
```

A concrete type implementing a neutral trait does not make that integration a child of the neutral module. Do not nest or re-export a concrete provider from the contract module merely because it implements the contract. A neutral module must remain usable without knowing which concrete integrations exist.

Provider specific data may be retained when necessary, but it should not distort the common domain abstraction.

## Strong types

Prefer strongly typed domain identifiers over raw UUID values or strings.

Keep each identifier with the concept that owns it.

Do not create a global identifier registry unless there is a genuine cross domain abstraction that requires one.

Technical similarity alone is not sufficient justification.

## Avoid speculative decomposition

Do not create elaborate module trees in anticipation of possible future complexity.

Start with the smallest coherent representation of a domain concept.

Split files when:

* multiple substantial internal concerns exist
* navigation is becoming difficult
* a meaningful internal sub concept has emerged
* the current file is becoming a collection of loosely related definitions

The conceptual boundary matters more than file size.

## Structural review

Before completing architecture or refactoring work, inspect the resulting source tree.

Ask:

Does this tree describe the system

A developer should be able to inspect src and identify the major concepts without first understanding implementation details.

If the tree mostly describes Rust mechanics rather than application concepts, reconsider the structure.

## Default bias

When choosing between grouping types because they are technically similar and grouping types because they belong to the same domain concept, prefer the domain concept.

The filesystem should describe the application, not Rust.
