---
name: find-the-essence
description: Find the intent and underlying principles that make a subject understandable. Apply as the primary skill for reasoning, design, and reader-facing writing in tog, including interpreting conversations and revising work after feedback.
---

# Find the essence

Do the work needed to understand the subject well enough to explain what matters
and why. Find the underlying principles from which the important choices follow.
Let that understanding shape both the work and its explanation. Aim for a reader
who can reason about the next case, not merely repeat the current answer.

## Understand the intent

Read the conversation as a developing understanding of the problem. Identify the
outcome the user wants, the constraints they care about, and the question still
being resolved. Give corrections particular attention: ask what they reveal about
the model of the problem, beyond the local edit they request.

Distinguish accepted decisions from suggestions, examples, rejected approaches,
and your own inferences. Use earlier context to understand the latest request;
let explicit corrections replace superseded assumptions. Do not turn an inferred
principle into a new requirement. Ask a focused question when competing
interpretations would materially change the work and the available evidence
cannot resolve them.

## Find the principles

Study the thing being described: its behavior, responsibilities, relationships,
constraints, and reasons for existing. Read relevant code, contracts, and examples
when a claim depends on them. Distinguish how the system currently works from how
it is intended to work.

Work toward a short account of what makes the problem take its shape. Ask why the
important rules exist, what must remain true, and which choices follow from those
facts. Make each principle concrete enough to guide a decision; statements such
as "keep it simple" need the distinction that tells us what simplicity means here.

Test that account against a concrete case and a plausible exception. If it cannot
explain an important constraint, investigate whether the account is incomplete,
the constraint is independent, or the evidence conflicts. Preserve the difference.
Do not invent one unifying story when the subject has several independent concerns.

## Let understanding change the work

When feedback reveals a different boundary or responsibility, reconsider the whole
proposal affected by it. Remove conclusions and abstractions that depended on the
old understanding. Carry the revised principle through the design, implementation,
and explanation within the agreed scope.

For example, "keep an identifier beside its concept" points to organizing by
responsibility. Moving every identifier into a new shared directory misses the
principle even if it makes the original file shorter.

Spend effort where uncertainty could change the answer. Follow contradictions and
missing causal links until the conclusion is supported, or make the unresolved
point explicit. Match the depth to the task; a straightforward correction can
remain straightforward. Stop when further investigation would not change the work.

## Write from that understanding

Lead with the central idea, then develop the consequences in the order a reader
needs them. Give each section and paragraph a clear purpose. Keep the facts,
reasoning, examples, and evidence needed to understand, assess, or apply the claim.
Move investigation history elsewhere only when it distracts from that purpose.

Preserve conditions, exceptions, scope, and uncertainty even when they complicate
the explanation. Keep quotations, logs, code, identifiers, and data faithful to
their source; apply this method to the explanation around them.

Revise for understanding rather than word count. A shorter text can still hide the
idea; a longer explanation can earn its space. Before finishing, check whether the
reader can see the intent, the principles that matter, and what follows from them
without reconstructing the conversation. Use the appropriate specialist skill for
the artifact's format and conventions.
