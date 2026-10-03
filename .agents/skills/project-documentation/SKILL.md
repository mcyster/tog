---
name: project-documentation
description: Consult project direction before choosing an approach to repository work, and maintain concise principles, architecture, plans, designs, notes, and decisions under docs/. Use when starting or revising an effort or changing project documentation; not for end-user manuals or generated API reference.
---

# Project Documentation

Write documentation for people working on the project, including people returning
after the original context has been forgotten. A document is not successful merely
because an LLM can summarize it.

## Consult direction before choosing an approach

Read `docs/principles.md` for priorities and tradeoffs, then `docs/architecture.md`
for the shared model. Inspect the plan and architecture
topic names and read those relevant to the effort, including boundaries it affects.
Follow links to designs, references, decisions, or code when a concrete choice
needs that detail. Do not read every document by default or skip applicable intent
because the code already suggests an implementation.

Distinguish established principles, proposed changes, historical exploration, and
implemented behavior. A merged plan is not evidence of implementation. Check
claims about current behavior against the code. When sources conflict, explain
which difference affects the work; do not silently promote a proposal or replace
intent with accidental behavior. Seek a decision when the conflict changes scope
or an important contract and the user's instructions do not resolve it.

In the work's approach or PR description, briefly identify the direction that
materially shaped it. Avoid a reading log. Update the relevant documents in the
same change when the work changes their meaning.

## Documentation structure

- `docs/principles.md` explains the priorities and tradeoffs guiding choices. Keep
  one short document; architecture shows their consequences and standards give
  concrete conventions.
- `docs/architecture.md` gives the short shared mental model and system boundaries.
- `docs/architecture/` explains established architecture by topic, including known
  gaps between its intent and implementation.
- `docs/plans/` describes intended changes, reasons, constraints, and important
  open choices. Make proposal, acceptance, and implementation state clear in prose.
- `docs/designs/` holds detailed approaches linked from plans. A design can describe
  proposed behavior; identify earlier or superseded material explicitly.
- `docs/notes/` holds dated investigations and developing thoughts. Notes may be
  incomplete, superseded, or wrong; they are not authoritative.
- `docs/decisions/` holds the small current set of significant choices explicitly
  accepted by the project owner and why they were made.

Keep existing topic references directly under `docs/` until moving them improves
navigation. Keep undeveloped possibilities in `docs/ideas.md`. Do not create a
companion design or a new directory merely to fill out this structure.

## File names

Use short, specific, lowercase names separated by hyphens.

- Notes capture thinking at a moment, so name them
  `docs/notes/YYYY-MM-DD-brief-name.md`, dated when the thinking was captured.
- Date decisions by explicit owner acceptance and name them
  `docs/decisions/YYYY-MM-DD-brief-decision.md`. State the decision rather
  than merely its subject: prefer
  `2026-08-29-use-semantic-conversation-events.md` to
  `2026-08-29-conversation-events.md`.
- Plans are living documents, so omit the date and name them by intended outcome,
  such as `docs/plans/add-tool-execution.md`.
- Designs and architecture describe the system, so omit the date and name them by
  concept, such as `docs/architecture/conversation.md`.

Do not date-prefix everything. Dates distinguish historical notes and decisions
from living descriptions. Use the capture date for notes and the acceptance date for decisions;
ordinary revisions do not rename the file. Do not invent an acceptance date.

Do not add status metadata to notes. Their date and location already communicate
that they are historical working material. Do not maintain a README that lists each
note or decision; browse the directories directly.

## Write for human readers

Lead with the point. Preserve the result of the thinking rather than its chronology.
Remove conversational turns, repeated context, false starts, generic background,
and exhaustive inventories that a reader can obtain more accurately from the code.

Keep the few things that change understanding or implementation:

- the problem or goal;
- relevant facts and constraints;
- important boundaries and invariants;
- alternatives that remain plausible or explain a non-obvious choice;
- critical pivot points where a different choice would produce a different design;
- the current conclusion, intended outcome, or unresolved question.

Use only the headings the subject needs. Prefer a short coherent document over a
large standard template. Clearly distinguish observations, conclusions, current
preferences, and open questions when the distinction matters.

For AI-assisted investigations, synthesize a durable note. Do not save a raw chat
transcript unless the exact exchange is itself important evidence. A human reader
should understand the note without access to the original conversation.

## Preserve decision authority

Apply [the decision recording rules](../../../docs/decisions/README.md) before
creating, revising, or relying on a decision record. Record only consequential
choices explicitly accepted by the project owner, with the source of acceptance.
An agent's implementation choice belongs in its PR; a proposal belongs in a note
or plan. A merged PR alone does not make every embedded choice an enduring decision.

Do not ask for acceptance again when the conversation already provides it. If older
records lack evidence, describe the uncertainty rather than asserting "we decided".
Use "the earlier note proposed" or "the implementation does" when that is what
the evidence supports. Escalate only when the unresolved distinction materially
affects the work; do not turn routine implementation into an approval process.

## Keep architecture visible

Prefer code that clearly expresses the implemented architecture through its module
structure, types, names, interfaces, dependencies, and enforced boundaries.
Documentation should not duplicate that structure. Use documentation to explain the
intent and reasoning the code cannot express clearly: why boundaries exist, which
constraints matter, what direction is intended, and where the implementation is
known to fall short.

Do not assume that the current code is the intended architecture. When code and
documentation disagree, identify the difference between current implementation and
intended design. Do not silently rewrite the documentation to match accidental code,
or extend the conflicting implementation as though it were authoritative.

When changing architecture, make the code express the new architecture wherever
practical and update the small amount of documentation that carries intent.

## Reveal detail progressively

Make a plan understandable and assessable without following its links. Explain the
outcome, why it matters, governing principles, scope, important unresolved choices,
and what completion means. Use only the headings the subject needs. Keep any
constraint or tradeoff that could reverse agreement with the direction in the plan.

Keep exact APIs, field inventories, lengthy examples, migration sequences, and
validation cases in a detail section at the end when modest, or in a linked design
when they develop their own complexity. Say what question each link answers.
A link supplies depth; it must not hide a material caveat.

Keep architecture focused on concepts, responsibilities, relationships, and
invariants. Explain why a boundary exists and enough of the code map to locate it;
leave exhaustive declarations and mechanics to references and code.

When a plan is implemented, integrate its lasting principles into architecture.
Remove the completed plan if it no longer serves a purpose, or retain a brief
account that clearly distinguishes completed direction from remaining work. Do
not leave old open questions looking like requirements. Preserve useful rationale
through links without maintaining competing current explanations.

## Maintain the documentation

Read relevant code and nearby documents before writing. Update an existing document
instead of creating a competing account when they cover the same subject. Link to
code, issues, plans, notes, or decisions when the link saves meaningful rediscovery.

Treat Git as the history. Notes may preserve historical investigation, while plans,
decisions, and durable project documentation should reflect current intent. When
implementation changes make durable documentation untrue, update the documentation
in the same change.
