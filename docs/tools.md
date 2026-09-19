# Tools

This document describes the first end-to-end tool-calling slice: a portable
tool contract owned by the conversation, a caller-owned execution loop, and one
concrete shell tool.

## Ownership

The conversation owns portable tool meaning:

- `ConversationFact::ToolsAvailable { tools }` records the complete toolset the
  caller is offering before a model invocation. The latest declaration in the
  conversation is authoritative; each declaration replaces the previous
  toolset, and an empty list removes all tools. Earlier declarations remain in
  the ordered history.
- `ToolDefinition` contains a name, a description, a `parameters` JSON Schema,
  and a `result` JSON Schema.
- `ToolRequest` contains a portable `ToolCallId`, the tool name, JSON
  arguments, the producing `ModelInvocationId`, and optional `ModelData`.
- `ToolResponse` references the same `ToolCallId` and contains a `ToolOutcome`:
  either a successful `result` value or a typed `problem`.

The available toolset varies by conversation, not by model driver. The driver
only presents recorded definitions through its provider mechanism and
translates completed provider tool calls into portable requests. The caller
executes tools and persists responses. Executable implementations and closures
are registered outside the conversation and are never persisted.

`ToolDefinition` schemas are derived from Rust parameter and result types with
[Schemars](https://docs.rs/schemars). Schemars generates JSON Schema; it does
not validate argument values. A tool validates arguments by deserializing them
into its parameter type and reports mismatches as an execution problem.

## Execution loop

`ConversationSession::invoke` owns the caller loop:

1. Record `ToolsAvailable` from the registered toolset before every driver
   invocation.
2. Persist completed driver output as it arrives, including every `ToolRequest`.
3. Execute requested tools sequentially after their requests are persisted.
4. Persist each correlated `ToolResponse`, including execution problems, before
   continuation.
5. Invoke the model again with the updated conversation.
6. Continue until an invocation produces no tool work.

Assistant text accompanied by tool requests does not end the turn. A tool
execution problem is returned to the model as the tool response so the model
can interpret or recover from it; it does not fail the turn. A
`ConversationProblem` still fails the turn. Only `ConversationSession` records
`TurnCompleted`.

The loop is bounded to
`MAXIMUM_TOOL_CONTINUATION_ROUNDS = 8` completed tool rounds. Reaching the limit
records a failed turn and returns an explicit continuation-limit error. There
is no daemon, dispatcher, parallel execution, command group, join, automatic
recovery, or automatic retry. Requests are persisted before execution and each
driver event batch is committed atomically; tool execution and result appends
remain sequential and outside that transaction, so re-executing shell commands
after a crash is not crash-safe.

## Shell tool

The `shell` tool runs `/bin/sh -c COMMAND`. Parameters:

| Parameter | Required | Default |
| --- | --- | --- |
| `command` | yes | none; blank commands are invalid arguments |
| `working_directory` | no | the directory of the `tog` process |
| `timeout_seconds` | no | `30` |

A successful shell result contains `stdout`, `stderr`, `exit_status`,
`stdout_truncated`, and `stderr_truncated`. `exit_status` is either
`{ "type": "exited", "code": N }` or `{ "type": "signaled", "signal": N }`. A
normal nonzero exit status is a valid result; signal termination is represented
explicitly rather than as an invented exit code.

Output is captured incrementally so the child cannot deadlock on full pipes,
and each stream retains at most 64 KiB. Truncation is reported through the
explicit boolean flags; output is never silently truncated.

A timeout is an execution problem, not a shell result. On timeout the tool
terminates the child's process group, reaps the child, and returns a problem of
kind `timed_out` whose details include the timeout duration, the partial
`stdout` and `stderr` captured so far, and their truncation flags. The result
schema does not contain a timeout field.

An execution problem records a portable `kind`, a `message`, and optional
tool-specific `details`:

- kind `invalid_arguments` for missing, mistyped, or blank parameters
- kind `unknown_tool` when no registered implementation matches the requested
  name
- kind `timed_out` when the tool exceeded its allowed duration
- kind `execution_failed` when the tool process could not be started or
  completed

The kind and message carry the portable meaning; understanding the failure never
requires interpreting details. Details are optional diagnostics, so another
tool can report a timeout with different details or none at all. The shell tool
defines its own timeout details locally rather than requiring them from the
shared `timed_out` category.

Strings are an explicitly accepted limitation of this first implementation.
There are no file handles, artifact storage, or output-reference abstractions.
There are no approval prompts, sandbox policy, allowlists, or automatic
retries.

## Dependencies

- `schemars` derives JSON Schema from Rust types, as recorded in
  [the Schemars decision](decisions/2026-09-12-use-schemars-for-tool-contracts.md).
- `nix` provides safe process-group signalling. The standard library has no
  safe process-group kill, and `tog` forbids unsafe code.
- `tokio` gains the `process`, `time`, and `io-util` features for asynchronous
  child execution, bounded draining, and timeouts.
