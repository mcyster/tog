# Principles

Use these principles to choose between reasonable approaches. They describe what
matters; [architecture](architecture.md) describes the concepts and boundaries
that put them into practice. The [development standards](development-standards.md)
give concrete coding conventions.

## Building understandable software

Apply these priorities in order: readability and understandability, correctness
through strong types, then simplicity.

### Anchor the design in clear contracts

Define clear interfaces and package responsibilities that let a reader understand
a concept before exploring its implementation. Make the important relationships
and dependency boundaries visible. Reveal detail progressively: the system's
shape, a component's contract, then its mechanics.

Use these contracts to guide implementation and change. Judge packaging by how
well it supports that understanding; extra layers and files must earn the
navigation they require.

### Let the code explain itself

Use clear names, cohesive responsibilities, and direct control flow to express
what the code does. Refactor when understanding depends on reconstructing intent
from scattered details. The code should carry most of the explanation of its
structure and behavior.

Use documentation for the purpose, tradeoffs, and constraints that the code cannot
adequately express. Keep it connected to the code without maintaining a second
description of every implementation detail.

### Do the work of making it clear

Spend the reasoning effort needed to make the result easy for human collaborators
to understand, assess, and extend. Find the underlying principles and revise the
work until the connections are clear. Saving the author effort by transferring
that work to the reader is not simplicity.

Use types to enforce meaningful distinctions and invariants. Prefer conventional
representations and direct interfaces. Build for current requirements and
explicitly accepted direction; remove machinery when its reason disappears.

## Building a composable tool

### Follow the Unix approach

Make tog a command-line tool that works well with other tools. Keep operations
focused and their inputs and outputs useful for composition, so users can combine
simple commands into powerful workflows. Prefer capabilities that remain useful
from scripts and pipelines as well as from an interactive terminal.

### Give implementations room behind a shared model

Use a strong shared abstraction to define what an integration receives, what it
may produce, and which guarantees it must preserve. Leave implementation choices
behind that contract so different providers can work naturally without exposing
their mechanics to every caller.

For model drivers, conversation is that shared foundation: history and event
meaning provide the common language, while each driver constructs provider input
and interprets output in its own way. The [conversation architecture](architecture/conversation.md)
defines the concrete ownership and lifecycle boundaries. Flexibility within the
contract must preserve portable meaning.

## Preserve the owner's intent

Distinguish proposals, implementation choices, and explicitly accepted project
decisions. Agent-authored text and merged code do not by themselves establish
agreement with every embedded choice. See [decision records](decisions/README.md)
for what warrants an enduring record of acceptance.
