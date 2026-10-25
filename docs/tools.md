# Tools

This document describes the first end-to-end tool-calling slice: a portable
tool contract owned by the conversation, a caller-owned execution loop, and the
concrete shell tools.

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

Schemars is the conventional crate for deriving JSON Schema from Serde types; the
standard library has no JSON Schema derivation, and hand-maintained schema
constants drift from the types. Because `schemars::Schema` does not implement
`Eq`, conversation facts that carry tool definitions implement `PartialEq`
rather than `Eq`. Generated schemas keep their `$schema` metadata for
portability; provider request construction projects a view that omits it without
mutating the recorded definition.

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

## Shell tools

Phase 1 of the [shell-tools plan](plans/shell-tools.md) replaces the former
`shell` tool with four tools: `shell_execute`, `shell_wait`, `shell_cancel`,
and `stream_read`. They provide independent command execution, bounded inline
output, and readable output streams.

Each execution starts a fresh `/bin/sh -c COMMAND` with tog's inherited
environment and current working directory. Stdin is closed. Setup commands,
`cd`, and exported variables affect only that execution; a command can change
its own directory with `cd ... && ...`. A working-directory override belongs to
the future shell context, not to an execution. Supplied `shell_context_id`
values are rejected; shell contexts are planned, not implemented.

### Execution state

`shell_execute(commands, shell_context_id?, wait_time_seconds?,
timeout_seconds?, output_limit_bytes?)` starts the command and waits up to
`wait_time_seconds` (default 30 seconds) for it to finish. The result contains:

- `execution_id`
- `stdout_stream_id`, `stderr_stream_id`, and `output_stream_id`
- `state`, the combined-stream execution state

The execution state distinguishes a completed process from its captured
output:

- `status`: `running` or `complete`
- `output` and `next_offset`: the returned bytes of the combined stream, in
  byte offsets
- `output_eof`: true only when the combined stream is closed and the returned
  read reaches its end
- `exit_status`: `{ "type": "exited", "code": N }` or
  `{ "type": "signaled", "signal": N }` for a naturally ended process
- `error`: `{ "kind": "timed_out" | "cancelled" | "capture_failed",
  "message": "..." }`

A nonzero exit code is a normal result. Signal termination is explicit. When
tog ends an execution for timeout or cancellation, `exit_status` is omitted and
`error` carries the reason. A capture failure reports `capture_failed`.

`shell_wait(execution_id, offset, wait_time_seconds?, output_limit_bytes?)`
returns the execution state from the given byte offset. It returns when output
beyond the offset is available, when the combined stream reaches its end, or
when the wait time elapses. The returned state always reports the execution's
current completion status and error.

`shell_cancel(execution_id)` requests termination of the execution and its
process group, and returns `{ "cancelled": true }`. The request is routed
through the execution's supervisor, which terminates the group only while the
command is still live. Cancellation is safe to repeat. Cancelling an execution
that has already completed is a no-op and leaves its result unchanged. A later
`shell_wait` reports the cancelled final state.

Tool-call failures remain `OperationFailure` problems. Launch failure is kind
`execution_failed`; a timeout or cancellation applies to the execution and is
reported in the state, not as a tool-call problem.

### Streams

`stream_read(stream_id, offset?, limit_bytes?, timeout_seconds?)` reads a
stream by byte offset and returns `content`, `next_offset`, and `eof`.
`content` is the UTF-8 lossy decoding of the requested byte range; a read may
split a multibyte character. A read returns available data immediately and may
wait up to `timeout_seconds` (default 30 seconds) at the live end of an open
stream.

Each execution exposes stdout, stderr, and a combined stream. The combined
stream holds bytes in the order tog observes them. It cannot promise exact
write order across the child's separate pipes.

Captured bytes are retained in memory and spill to a temporary file when a
stream passes its 64 KiB in-memory threshold, through
[`tempfile::SpooledTempFile`](https://docs.rs/tempfile). Stream identifiers and
byte offsets remain valid across the spill. Inline responses are bounded by
`output_limit_bytes` but never discard retained output; the model reads the
remainder with `stream_read`. Phase 1 has no total capture limit.

### Lifecycle

Executions and streams live for the life of the tog process. A completed
execution stays identifiable and its retained output stays readable. A new
process cannot reference an execution or stream from an earlier process;
unknown identifiers are invalid-argument errors.

Execution and stream identifiers are unique UUIDs with `exec_` and `stream_`
prefixes. They are never reused within or across services or process restarts,
so a persisted identifier cannot resolve to unrelated newer work. An
unavailable identifier fails explicitly; tog never reruns a command to
reconstruct a missing execution.

Cancellation is a request to the execution's supervisor. Only the supervisor,
while it still owns the live command, signals the process group. Completion is
recorded by the same owner, so a late cancellation cannot signal a reused
process group or overwrite a completed outcome. Releasing the shell service
requests cancellation of tracked executions; the supervisors perform the
termination. The release path does not wait for the processes to finish.

Spill files are anonymous temporary files removed when their stream is
released. No temporary path or stream identifier is a durable content
reference. Phase 1 does not promote completed output to assets. There are no
approval prompts, sandbox policies, allowlists, or automatic retries.

## Dependencies

- `schemars` derives JSON Schema from Rust types, as described above.
- `nix` provides safe process-group signalling. The standard library has no
  safe process-group kill, and `tog` forbids unsafe code.
- `tempfile` provides `SpooledTempFile` memory-to-disk rollover. The standard
  library has no spooling type, and the crate handles private temporary-file
  creation and removal.
- `tokio` gains the `process`, `time`, `sync`, and `io-util` features for
  asynchronous child execution, stream capture, and output waiting.
- `uuid` provides collision-resistant version-7 identifiers for executions and
  streams, following the conversation's identifier conventions.
