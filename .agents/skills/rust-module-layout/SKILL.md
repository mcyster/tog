---
name: rust-module-layout
description: Prefer clear modern Rust module layout and avoid unnecessary mod.rs files
---

# Rust Module Layout

Use this skill when creating, moving, or restructuring Rust modules.

## Preferred module layout

For a module with child modules, prefer the modern Rust layout:

```
src/
  conversation.rs
  conversation/
    event.rs
    id.rs
```

Prefer this over:

```
src/
  conversation/
    mod.rs
    event.rs
    id.rs
```

Both layouts are valid Rust.

Use the first form by default because the module root has a meaningful filename and remains easy to identify in editors, search results, tabs, stack traces, and filesystem navigation.

## Module root

The module root defines the module and its external surface.

For example:

```
conversation.rs
```

may declare child modules:

```
mod event;
mod id;
```

and expose selected types from them:

```
pub(crate) use event::ConversationEvent;
pub(crate) use id::{ConversationEventId, ConversationId};
```

Callers should normally depend on the conceptual module surface:

```
crate::conversation::ConversationEvent
crate::conversation::ConversationId
```

rather than internal file structure:

```
crate::conversation::event::ConversationEvent
crate::conversation::id::ConversationId
```

A module root may present a contract rather than an implementation: trait
signatures and error variants, with concrete implementations and mechanics in
child modules. Keep the root understandable without opening its children.

## Naming

Prefer meaningful domain names over generic module filenames.

A file named conversation.rs communicates more information than a file named mod.rs.

Do not introduce mod.rs merely because a module has child files.

Use mod.rs only when there is a concrete reason to prefer that layout or when matching an established local convention that should remain consistent.

## Refactoring rule

When converting an existing module from:

```
conversation/
  mod.rs
  event.rs
  id.rs
```

to:

```
conversation.rs
conversation/
  event.rs
  id.rs
```

move the module root contents without changing behavior.

Preserve:

* module visibility
* re exports
* public type paths
* tests
* child module relationships

Treat this as a filesystem and module organization change, not an opportunity for unrelated redesign.

## Default bias

Prefer module filenames that describe the domain concept.

Prefer:

```
conversation.rs
conversation/
```

over:

```
conversation/
  mod.rs
```

unless the surrounding codebase has a strong reason to do otherwise.
