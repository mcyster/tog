---
name: pull-request-description
description: Write concise pull request descriptions and comments that focus the review on what matters. Use when creating or updating a pull request description or writing review comments.
---

# Pull Request Description

Keep PR descriptions short. Use only the structure the change needs.

## Start with a short description

- State what changes and why it matters.
- Describe the result. Do not repeat the request or narrate the work.

## Add zero to five callouts when useful

- Highlight decisions, tradeoffs, surprising behavior, or compatibility changes.
- Include usage examples only when they help the reviewer try the change.
- Each callout should direct attention to something worth reviewing.

## Use simple, precise language

Apply the [human-readable](human-readable/SKILL.md) skill for sentence-level style.

## Leave implementation detail in the code

- Do not summarize every file, function, or error path.
- Omit routine housekeeping.
- Put longer explanations in `docs/architecture` or `docs/design`.
- Link to those documents when they help the review.

## Mention validation only when it adds useful information

- Include unusual checks, known gaps, or results that affect confidence.
- Omit boilerplate test and build summaries already visible in CI.

Do not require fixed headings or empty sections. A short leading description may be enough.
