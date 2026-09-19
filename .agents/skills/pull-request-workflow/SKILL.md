---
name: pull-request-workflow
description: Deliver every repository change through a pull request by committing, pushing, and creating or updating the PR
---

# Pull Request Workflow

Use this skill when completing any repository change.

## Always deliver through a pull request

A completed change is committed, pushed, and represented by a pull request.

Do not leave finished work uncommitted in the working tree.

## Start from the latest default branch

Begin new work by updating the default branch and branching from it. Do not
commit directly to the default branch.

## Create or update the pull request

When a change is complete:

1. Run the required validation.
2. Commit with a focused message.
3. Push the branch.
4. If the branch has a pull request, update it. Verify first that the pull
   request is still open and not merged; a merged pull request cannot receive
   the change.
5. If the branch has no pull request, create one.

When the current pull request is already merged, start a new branch from the
latest default branch and open a new pull request.

## Keep the description current

Update the pull request description when the change it delivers changes: what
the change does, why, and how it was validated.

## Preserve history

Do not force-push. Do not rewrite a pushed branch that may be under review.
