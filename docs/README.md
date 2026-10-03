# Project documentation

Start with [principles](principles.md) for what guides choices, then
[architecture](architecture.md) for tog's concepts and boundaries. Consult the relevant plans before choosing an approach to an effort.
Read linked detail when the decision or implementation depends on it.

| Location | What it explains |
| --- | --- |
| `principles.md` | Priorities and tradeoffs that guide choices across the project |
| `architecture.md` | The shared mental model and important system boundaries |
| `architecture/` | Established architecture of a particular area and known implementation gaps |
| `plans/` | Intended changes, their reasons, constraints, and important open choices |
| `designs/` | Detailed approaches, examples, and validation scenarios linked from plans |
| `notes/` | Dated investigation and developing thought; historical, not authoritative |
| `decisions/` | Significant choices explicitly accepted by the project owner and their reasons |

Plans and designs state whether their direction is proposed, accepted, or already
implemented in ordinary prose where it matters. A merged document does not imply
implemented behavior. Browse directories by topic; there is no required reading
of every document before each change.

Each overview should be understandable without following its links. Keep reasons
and constraints that could change agreement with the direction in that overview.
Put modest detail at the end; split it into a linked document when it develops
its own complexity. Explain what each link helps the reader decide or implement.

Existing topic references can stay directly under `docs/` until moving them serves
a concrete reading need. Keep one authoritative explanation of each concern and
update it when the work changes its meaning. Git preserves superseded text.

Name notes `YYYY-MM-DD-brief-name.md`, using the date the thinking was captured.
Name decision records `YYYY-MM-DD-brief-decision.md`, using the date the owner
explicitly accepted the choice. Ordinary edits do not change either date. A
record's name states the decision; no numbering or formal ADR template is needed.
Follow the [decision recording rules](decisions/README.md) before creating or
relying on a record. Notes and merged PRs do not automatically establish acceptance.
