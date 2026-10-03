# Decisions

A decision record documents a significant choice explicitly accepted by the project
owner. Writing the record does not make the decision. Keep this directory small:
record a choice when future collaborators need to understand an accepted tradeoff
that constrains their work.

Routine implementation choices belong in the PR. Suggestions and consequential
choices still being explored belong in notes or plans, clearly marked as proposals.
A PR merge accepts the change; it does not automatically endorse every embedded
choice as an enduring project constraint. Existing explicit acceptance is enough;
do not ask for it again just to write the record.

Name a record `YYYY-MM-DD-brief-decision.md`. Use the date of explicit acceptance,
not the date an agent first proposed or documented the idea. State the accepted
choice in the filename. Ordinary edits do not change its date. Do not invent a
date if it cannot be established.

Keep the record focused on the choice, essential reasoning, and important
consequences. Reference the owner's acceptance in a discussion or review. Use a
link when available; otherwise identify the conversation and the actual statement
of acceptance accurately. Never fabricate a source or infer acceptance from the
record itself. Link to a note for deeper investigation.

When citing older material, distinguish "the note proposed", "the implementation
does", and "the owner accepted". Existing decision files without evidence of
acceptance must not be treated as proof that the owner agreed. If the distinction
matters to current work, find the acceptance or surface the uncertainty; do not
silently ratify the record or change the implementation because evidence is missing.

When the owner explicitly supersedes a decision, replace or remove its record in
the same change; Git retains history. A replacement uses the new acceptance date.
Browse the directory directly. No numbered series, formal ADR template, or
file-by-file index is required.
