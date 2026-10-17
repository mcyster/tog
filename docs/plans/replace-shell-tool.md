# Replace the shell tool with streamed execution

Replace the current `shell` tool with independent command execution, bounded
inline output, and readable output streams. A short command such as `ls` should
complete in one tool call. Longer commands should continue while the model waits
or inspects their output.

Mark accepted independent execution for phase 1, optional shell context selection
for phase 2, and memory-first output that spills to temporary disk storage.
This plan records the intended replacement; it does not implement it. The API
details below remain a proposal for review.

The current [shell implementation](../../crates/tog-tools/src/shell.rs) starts
`/bin/sh -c`, drains both output pipes, and retains at most 64 KiB per stream.
It returns when execution finishes or times out. Replace that tool and its
registration rather than offering two competing shell tools. Update
[tool documentation](../tools.md) when the replacement is implemented.

## Independent execution first

Each call starts a fresh shell with tog's inherited environment. The shell
interprets pipes, redirects, variable expansion, and command sequences.

- Default the directory to tog's working directory. An override affects one call.
- Setup commands, `cd`, and `export` affect only that execution.
- Filesystem changes and other external effects remain shared.
- Omitted `shell_context_id` always means independent execution, including later.
- Reserve the optional context parameter. Reject supplied IDs in phase 1.
- Keep stdin closed. Interactive input and PTYs are outside phase 1.

Retaining `/bin/sh` is the smallest initial interpreter choice. Confirm that
choice before implementation; Bash-specific setup requires Bash explicitly.

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
Choose the crate during implementation and explain its fit and tradeoffs.

The stream owner owns its temporary files. Cleanup must not delete output still
available to readers. Normal release removes owned files; abnormal termination
and stale-file cleanup need an explicit policy. Never remove another live tog
process's temporary storage.

Separate the memory threshold, total capture limit, and tool-response limit.
Spilling is not truncation. If a capture limit or disk failure prevents retention,
report that fact explicitly and define whether execution stops or continues to
drain output. Avoid an unbounded in-memory combined-stream index after spill.
Sharing storage between stream views is desirable where it stays simple.

Capture raw bytes. Before implementation, define offset units, text decoding,
invalid UTF-8 handling, and reads that split a multibyte character.

## Temporary output and durable history

A spill file is temporary execution storage. Crossing the memory threshold does
not automatically make it a [tog asset](../assets.md). Assets are immutable;
running output is still growing.

Decide how long completed streams remain readable and whether completed output
is promoted to assets. Do not present temporary paths or stream IDs as durable
content references. Model-visible output must be recorded in tool responses or
immutably referenced, as required by [project principles](../principles.md).

Also settle who owns executions after a turn ends, what happens at tog shutdown,
and what a resumed conversation sees when an execution or stream no longer
exists. Phase 1 does not promise process recovery or automatic command replay.
The [durable-execution plan](execute-commands-durably.md) covers that broader work.

Use the Conversation API for tool results. Live UI progress through conversation
events remains a separate choice. Do not introduce a parallel event bus or label
shell output as model-specific data.

## Later: shell contexts

Keep `environment` for tog. A `shell_context` would hold a live, prepared shell.
A separate operation could change its directory, variables, functions, and
options. Ordinary commands would run in real subshells of that context so their
shell-state changes do not alter it.

Context mutations and command launches need serialization. Running commands keep
their inherited state. Failed setup may leave partial changes. Replaying setup
cannot guarantee restoration of arbitrary shell state after the context dies.
These are phase 2 concerns, not phase 1 implementation requirements.

## Proposed tool contracts

Use explicit time and size units in the final schemas. Select defaults so ordinary
commands require only the command string.

```text
shell_execute(commands, shell_context_id?, directory?,
              wait_time?, timeout?, output_limit?)
  -> execution_id
     stdout_stream_id
     stderr_stream_id
     output_stream_id
     state: ExecutionState

shell_wait(execution_id, offset, wait_time?, output_limit?)
  -> ExecutionState

shell_cancel(execution_id)
  -> cancellation acknowledgement

ExecutionState
  output
  next_offset
  output_eof
  status          # running | complete
  exit_code?
  error?

stream_read(stream_id, offset?, limit?, timeout?)
  -> content
     next_offset
     eof
```

`commands` is one required, nonblank shell command string. `output` and
`next_offset` refer to the combined stream. Bound every inline response.

`stream_read` defaults to offset zero. It returns available data immediately.
At the current end of a live stream, it may wait up to its timeout for more data.
EOF is true only when the stream is closed and the returned read reaches its end.
The same rule applies to `ExecutionState.output_eof`.

Before implementation, settle the failure schema and its relationship to the
existing `OperationFailure` contract. Keep tool-call failures distinct from a
successfully retrieved execution state that reports a failed command.

## Completion criteria

- The new tools replace the existing `shell` registration and schemas.
- A short command completes with useful output in one call.
- A long command returns control and can be waited on or cancelled.
- Large stdout and stderr spill during execution into temporary storage.
- Reads across spill preserve output and offsets; memory stays bounded.
- Inline limits do not silently discard retained output.
- Completion, unread output, and capture failures remain distinguishable.
- Retention, shutdown, and cleanup policies are documented and tested.

Keep process control, stream storage, and tool adapters focused. Reuse existing
contracts and libraries where they fit. This plan does not require a new crate,
persistent shell, daemon, or automatic retry mechanism.
