# Shell tools

Provide independent command execution, bounded
inline output, and readable output streams. A short command such as `ls` should
complete in one tool call. Longer commands should continue while the model waits
or inspects their output.

Mark accepted independent execution for phase 1, optional shell context selection
for phase 2, and memory-first output that spills to temporary disk storage.
Phase 1 is implemented: it replaces the former `shell` tool with the
`shell_execute`, `shell_wait`, `shell_cancel`, and `stream_read` tools in
`crates/tog-tools/src/shell.rs`. The [tool documentation](../tools.md)
describes the implemented contracts and lifecycle. Phase 1 contracts below
reflect the implemented behavior, including decisions recorded during
implementation. Phase 2 (shell contexts) and phase 3 (stdin) remain proposals
for review.

The former [shell implementation](../../crates/tog-tools/src/shell.rs) started
`/bin/sh -c`, drained both output pipes, retained at most 64 KiB per stream, and
returned when execution finished or timed out. The new tools replace that tool
and its registration rather than offering a second competing shell tool.

## Independent execution first

Each call starts a fresh shell with tog's inherited environment. The shell
interprets pipes, redirects, variable expansion, and command sequences.

- For phase 1, the execution context uses tog's current working directory and
  inherited environment. A successful command can change its own directory
  with `cd ... && ...`. A directory override belongs to the phase 2 shell
  context, not to an execution.
- Setup commands, `cd`, and `export` affect only that execution.
- Filesystem changes and other external effects remain shared.
- Omitted `shell_context_id` always means independent execution, including later.
- Reserve the optional context parameter. Reject supplied IDs in phase 1.
- Keep stdin closed in phase 1. Stdin support is deferred to phase 3.
- PTYs remain outside this plan.

Retaining `/bin/sh` is the smallest initial interpreter choice. The owner
confirmed `/bin/sh` for phase 1; Bash-specific setup requires Bash explicitly.

## Execution and output have separate lifetimes

`shell_execute` waits briefly and returns bounded output. It returns completed
state if the command finishes, or a running execution ID otherwise.
`shell_wait` returns subsequent output and an updated state.

The wait duration limits a tool call. The execution timeout limits the command's
lifetime. Returning from a wait does not stop the command. Cancellation requests
termination of the command and its child processes; waiting retrieves the final
result. Repeated cancellation should be safe.

Execution completion does not mean the reader consumed all captured output.
Keep completion and output EOF separate. Preserve nonzero exit codes as valid
command outcomes. Represent timeout, cancellation, signal termination, launch
failure, and capture failure explicitly without inventing exit codes.

Each execution exposes stdout, stderr, and a combined output stream. The combined
stream contains bytes in the order tog observes them. It cannot promise exact
write order across separate pipes. Inline results read from this combined stream.

Each reader supplies its own offset. Stream IDs and offsets remain valid across
memory-to-disk spill. Readers must be able to follow output while the command is
running and reread retained output after completion.

## Stream in memory, then spill to the temporary directory

Spill-to-disk is required in phase 1. Do not wait for command completion before
writing large output to disk.

1. Drain stdout and stderr concurrently as bytes arrive.
2. Retain small output in memory.
3. When a buffer exceeds its memory threshold, move its retained bytes into a
   temporary file. Append subsequent bytes to that file.
4. Serve reads through the same stream API before and after spill. Preserve byte
   order and offsets without gaps or duplication.
5. Keep disk-backed reads bounded. Do not reload the full file to return an excerpt.

Use the OS temporary directory, respecting the platform's temporary-directory
configuration, rather than the repository or asset directory. Create private,
uniquely named temporary storage through an established library. Do not build a
custom spooling implementation if a maintained crate meets the requirement.
The proposed library is [`tempfile::SpooledTempFile`](https://docs.rs/tempfile/latest/tempfile/struct.SpooledTempFile.html).
It supplies memory-to-file rollover and synchronous `Read`, `Write`, and `Seek`.
The standard library has no equivalent spooling type. Using this crate avoids
maintaining custom rollover and temporary-file creation code, at the cost of
another dependency to review and update. No dependency is added by this plan.

The crate provides storage, not asynchronous following or independent cursors.
Tog must coordinate append/read access, signal new data and final EOF, and run
blocking disk operations outside Tokio's async worker threads. Readers' offsets
must not disturb the append position. These coordination responsibilities remain
small even though the underlying spool has one seek position.

The stream owner owns its temporary files. Cleanup must not delete output still
available to readers. Normal release removes owned files; abnormal termination
and stale-file cleanup need an explicit policy. Never remove another live tog
process's temporary storage.

Separate the memory threshold, total capture limit, and tool-response limit.
Spilling is not truncation. If a capture limit or disk failure prevents retention,
report that fact explicitly and define whether execution stops or continues to
drain output. Avoid an unbounded in-memory combined-stream index after spill.
Sharing storage between stream views is desirable where it stays simple.

Capture raw bytes. Implemented offset units are bytes. Returned content decodes
the requested raw byte range as UTF-8 lossily, so a read may split a multibyte
character; the model continues from `next_offset` to recover.

## Temporary output and durable history

A spill file is temporary execution storage. Crossing the memory threshold does
not automatically make it a [tog asset](../assets.md). Assets are immutable;
running output is still growing.

Decide how long completed streams remain readable and whether completed output
is promoted to assets. Do not present temporary paths or stream IDs as durable
content references. Model-visible output must be recorded in tool responses or
immutably referenced, as required by [project principles](../principles.md).

Implemented phase-1 decisions: executions and streams remain identifiable and
readable for the life of the tog process; completed output is not promoted to
assets; stream identifiers are transient references, not durable content
references. A resumed conversation in a new process sees an unknown identifier
and fails explicitly; tog never reruns a command to reconstruct a missing
execution. Phase 1 does not promise process recovery or automatic command
replay. The [durable-execution plan](execute-commands-durably.md) covers that
broader work.

Execution and stream identifiers follow the conversation's identifier
conventions: version-7 UUIDs under the `exec_` and `stream_` prefixes. They are
not reused across service instances or process restarts, so a persisted
identifier cannot resolve to unrelated newer work.

Use the Conversation API for tool results. Live UI progress through conversation
events remains a separate choice. Do not introduce a parallel event bus or label
shell output as model-specific data.

## Service boundary and lifecycle ownership

As implemented, a `ShellService` exposes the four operations—execute, wait,
cancel, and stream read—behind a focused contract. It owns the execution and
stream registries and their synchronization privately. Tool adapters only
decode arguments, call the service, and encode results. No new crate or trait
is involved.

Each `Execution` has a private implementation and a focused caller contract:
`standard_output()`, `standard_error()`, and `combined_output()` return the
read-only captured streams; `state()` returns the process state (status, exit
status, error); `cancel()` requests cancellation; and `wait_for_completion`
observes state with waiting. Construction happens through `Execution::try_new`,
which starts the work; launching the OS process is a private step of that
constructor. Callers never
touch process identifiers, capture tasks, signal flags, locks, or `Arc`
sharing. Output excerpts and reader offsets are not part of the execution; they
belong to stream reads, and the service adapter combines a read with the
process state into each tool response.

Lifecycle ownership is deliberate:

- A supervisor task owns the live child and is the only entity that signals the
  process group. Cancellation is a request to the supervisor; it terminates the
  group only while the child is unreaped, when the saved process id cannot yet
  be reused.
- Completion is recorded by the same supervisor, so a late cancellation can
  neither signal a reused group nor overwrite a completed outcome. Cancellation
  after completion is a no-op.
- Releasing the service requests cancellation of tracked executions through
  this owner. `Drop` is a synchronous fallback that only issues the request; it
  does not await process cleanup.

## Phase 2: shell contexts

Keep `environment` for tog. A `shell_context` would hold a live, prepared shell.
A separate operation could change its directory, variables, functions, and
options. Ordinary commands would run in real subshells of that context so their
shell-state changes do not alter it.

Context mutations and command launches need serialization. Running commands keep
their inherited state. Failed setup may leave partial changes. Replaying setup
cannot guarantee restoration of arbitrary shell state after the context dies.
These are phase 2 concerns, not phase 1 implementation requirements.

## Phase 3: stdin support

Add stdin support after shell contexts. This is outside phase 1 and phase 2.
The following API remains a proposal for review.

Make stdin opt-in when starting execution, for example `stdin: true`.
Default to closed stdin so commands that consume input can finish without a
second tool call. When requested, return an optional `stdin_stream_id`.

Proposed additions:

```text
shell_execute(..., stdin?)
  -> existing fields
     stdin_stream_id?

stream_write(stream_id, content, close?)
  -> write result (delivery and failure semantics to be settled)
```

Use `stream_write(stream_id, content, close?)` to send input. With `close: true`,
close the pipe after writing the supplied content so the command observes EOF.
Allow empty content with close to finish input. A newline is data; EOF is closure.

This is a writable pipe, not a seekable output spool. Writes need bounded
backpressure and must report partial delivery on failure. Do not imply that a
successful write means the command has processed the input. Serialize writes;
reject writes after closure. Retrying a write can duplicate input, so it must
not be retried automatically after an uncertain outcome.

An open stdin pipe does not provide terminal behavior. Programs requiring a TTY
may still need separate support. The opt-in flag and write result/timeout contract
remain proposals for review.

## Phase 1 tool contracts

Use explicit time and size units in the final schemas. Select defaults so ordinary
commands require only the command string. The implemented schemas use the field
names below with explicit units: `wait_time_seconds`, `timeout_seconds`,
`output_limit_bytes`, `offset`, and `limit_bytes`.

```text
shell_execute(commands, shell_context_id?,
              wait_time_seconds?, timeout_seconds?, output_limit_bytes?)
  -> execution_id
     stdout_stream_id
     stderr_stream_id
     output_stream_id
     state: ExecutionSnapshot

shell_wait(execution_id, offset, wait_time_seconds?, output_limit_bytes?)
  -> ExecutionSnapshot

shell_cancel(execution_id)
  -> cancellation acknowledgement

ExecutionSnapshot
  output
  next_offset
  output_eof
  status          # running | complete
  exit_status?    # exited {code} | signaled {signal}, only for natural end
  error?          # timed_out | cancelled | capture_failed

stream_read(stream_id, offset?, limit_bytes?, timeout_seconds?)
  -> content
     next_offset
     eof
```

`commands` is one required, nonblank shell command string. `output` and
`next_offset` refer to the combined stream. Bound every inline response.

`stream_read` defaults to offset zero. It returns available data immediately.
At the current end of a live stream, it may wait up to its timeout for more data.
EOF is true only when the stream is closed and the returned read reaches its end.
The same rule applies to `ExecutionSnapshot.output_eof`.

Failure handling is implemented as follows. Tool-call failures remain the
existing `OperationFailure` problems: invalid arguments (including unknown or
malformed execution and stream identifiers), launch failure as
`execution_failed`, and unknown tool names handled by the registry. A failed
command outcome is a successfully retrieved execution state: `exit_status` for
a natural end, or an `error` of kind `timed_out`, `cancelled`, or
`capture_failed`. Timeout and cancellation therefore appear in the state, not
as tool-call problems. `exit_status` is omitted when tog ended the execution
for timeout or cancellation.

## Completion criteria

- The new tools replace the existing `shell` registration and schemas.
- A short command completes with useful output in one call.
- A long command returns control and can be waited on or cancelled.
- Large stdout and stderr spill during execution into temporary storage.
- Reads across spill preserve output and offsets; memory stays bounded.
- Inline limits do not silently discard retained output.
- Completion, unread output, and capture failures remain distinguishable.
- Retention, shutdown, and cleanup policies are documented and tested.

The criteria are met by phase 1. The behavioral tests live in
`crates/tog-tools/src/shell/`, and the policies are documented in the
[tool documentation](../tools.md).

Keep process control, stream storage, and tool adapters focused. Reuse existing
contracts and libraries where they fit. This plan does not require a new crate,
persistent shell, daemon, or automatic retry mechanism.
