# Principles

Use these principles to choose between reasonable approaches. They describe what
matters across the project; [architecture](architecture.md) describes the concepts
and boundaries that put them into practice. The
[development standards](development-standards.md) give concrete coding conventions.

Apply these priorities in order:

1. Readability and understandability
2. Correctness through strong types
3. Simplicity
4. Performance supported by evidence

## Make understanding part of the work

Spend the reasoning effort needed to make the result easy for human collaborators
to understand, assess, and extend. Find the intent and underlying principles before
settling on an explanation. A short answer that leaves the reader to reconstruct
the connections has not saved work.

Make important contracts independently understandable. Organize around concepts
and responsibilities, keeping related declarations together and implementation
mechanics out of the way. Judge structure by whether it helps a reader understand
the whole, including the navigation it requires.

## Express meaningful constraints

Use types and boundaries to make important distinctions explicit and invalid states
hard to construct. Keep contracts independent of concrete integrations. A type or
abstraction earns its place by enforcing an invariant or clarifying a responsibility;
strong typing does not mean wrapping every value.

## Keep the solution proportionate

Build for current requirements and explicitly accepted direction. Prefer conventional
representations and direct interfaces. Add machinery when a concrete need justifies
it, and remove it when the reason disappears. Require evidence before trading
clarity for performance.

## Preserve the owner's intent

Distinguish a proposal, an implementation choice, and an explicitly accepted
project decision. Agent-authored text and merged code do not by themselves establish
agreement with every embedded choice. Keep uncertainty visible and describe the
source accurately. See [decision records](decisions/README.md) for what warrants
an enduring record of acceptance.
