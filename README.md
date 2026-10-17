# tog

`tog` provides durable command-line conversations through OpenAI's Responses API,
OpenRouter's OpenAI-compatible chat completions API, or OpenRouter's Responses API.

## Prerequisites

The supported development environment uses [Nix](https://nixos.org/) with flakes
enabled. You also need an API key for the driver you select: an OpenAI API key for
the `openai` driver, or an OpenRouter API key for the `openrouter-completions`
and `openrouter-responses` drivers.

Enter the environment directly:

```console
nix develop
```

If you use [direnv](https://direnv.net/), enter it automatically when opening the repository:

```console
direnv allow
```

Set your API key in the current shell:

```console
export OPENAI_API_KEY=...        # for the openai driver
export OPENROUTER_API_KEY=...    # for the openrouter-completions driver
```

With direnv, you can instead put the exports in `.env` or `$HOME/.env.tog`; both files are loaded by the repository's `.envrc`.

## Run From Source

Start a conversation without installing the binary:

```console
cargo run -- "Explain ownership in Rust"
```

The command writes progress, the conversation ID, and a description of each
committed event to standard error. Model-event messages (like reasoning steps)
are admitted by the selected verbosity. The assistant response is written to
standard output, so the final result can be piped or redirected on its own.
Continue the conversation with that ID:

```console
cargo run -- --conversation cnv_019... "Show an example"
```

The default driver is `openai` and the default model is `gpt-5.6`. Select another
model with `--model`, and select a driver with `--driver`:

```console
cargo run -- --model MODEL_NAME "Explain ownership in Rust"
cargo run -- --driver openrouter-completions --model openrouter/auto "Explain ownership in Rust"
cargo run -- --driver openrouter-responses "Explain ownership in Rust"
```

Selecting the `openrouter-completions` driver uses the chat completions API and reads
`OPENROUTER_API_KEY` (and an optional `TOG_OPENROUTER_BASE_URL`). The OpenRouter
driver's default model is `openrouter/auto`. Selecting the `openrouter-responses`
driver uses the same credentials against OpenRouter's Responses API and defaults to
`deepseek/deepseek-v4.1-flash`. It streams progress and provider-reported usage and
cost on each model response.

CLI verbosity defaults to `low`. It controls the model events described on
standard error. Select `low`, `medium`, or `high` to print progressively more
of those messages:

```console
cargo run -- --verbosity medium "Explain ownership in Rust"
```

`:turn` is the default command when no command is given. Commands are prefixed with a
colon, so the explicit form is `cargo run -- :turn "Explain ownership in Rust"`.
Run `cargo run -- --help` or `cargo run -- :turn --help` for the complete command-line
help.

## Conversation Log

Dump a conversation log to standard output as JSON Lines, one stored record per
line, including command records and extension events. Without an identifier, the
most recently active conversation is dumped:

```console
cargo run -- :log
cargo run -- :log cnv_019...
```

Standard output contains only the JSON Lines, so the log can be piped directly
into another consumer:

```console
tog :log | jq 'select(.class == "fact")'
```

Replace the raw dump with a concise human-readable summary of the same
conversation using `--readable`. Each event prints its type and a one-line
description under the conversation header:

```console
cargo run -- :log --readable
# conversation cnv_aaaa
## user test 123
## tool_request shell_execute {"commands":"echo \"test 123\""}
## tool_response {"execution_id":"exec_1","stdout_stream_id":"stream_1","stderr_stream_id":"stream_2","output_stream_id":"stream_3","state":{"output":"test 123\n","next_offset":9,"output_eof":true,"status":"complete","exit_status":{"type":"exited","code":0}}}
## assistant_response test 123
```

## Tools

A turn may request tools. The current build registers `shell_execute`,
`shell_wait`, `shell_cancel`, and `stream_read`. A command runs through
`/bin/sh -c` without an approval prompt, in the current directory unless a
`directory` is given. `shell_execute` waits up to 30 seconds and returns a
bounded slice of the combined output; a longer command returns a running
execution that a model can wait on and cancel. Capture stays in memory until a
stream passes 64 KiB, then spills to a temporary file, and `stream_read` reads
any stream by byte offset. Nonzero exits, signals, timeouts, and cancellation
are reported in the execution state; invalid arguments, unknown identifiers,
and launch failures are returned as typed tool-call problems. Tool definitions,
requests, and responses are recorded in the conversation log. See
[Tools](docs/tools.md).

## Build

Build an optimized binary from inside the Nix development environment:

```console
cargo build --release
```

The binary is written to `target/release/tog`. Inside the development environment,
`target/release` is on `PATH`, so run it as:

```console
tog "Explain ownership in Rust"
```

Outside the development environment, place the binary in a directory on your `PATH`.

## Data

Conversation events and assets are stored in the first available location:

1. `$TOG_DATA_DIR`
2. `$XDG_DATA_HOME/tog`
3. `$HOME/.local/share/tog`

Conversation logs live under `conversations/` and immutable assets under
`assets/`.

The current semantic event schema is a clean break from the earlier agent-run-based prototype. Existing prototype conversations are not migrated.

## Assets

Store a file as an immutable asset and list stored assets:

```console
tog :asset add ~/example/screenshot.jpg
tog :asset list
```

`:asset add` defaults the name to the source file's basename and infers the
MIME type from content, falling back to `application/octet-stream`; override
either with `--name` and `--mime-type`. Standard output carries the added
asset or a JSON Lines listing. See [Assets](docs/assets.md).

## Development

Run the project checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Detailed project documentation is in [`docs/`](docs/README.md).
