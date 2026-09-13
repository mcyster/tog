# tog

`tog` provides durable command-line conversations through OpenAI's Responses API.

## Prerequisites

The supported development environment uses [Nix](https://nixos.org/) with flakes enabled. You also need an OpenAI API key.

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
export OPENAI_API_KEY=...
```

With direnv, you can instead put the export in `.env` or `$HOME/.env.tog`; both files are loaded by the repository's `.envrc`.

## Run From Source

Start a conversation without installing the binary:

```console
cargo run -- "Explain ownership in Rust"
```

The command writes progress and the conversation ID to standard error. After OpenAI completes, it writes model-event messages admitted by the selected verbosity to standard output. Continue the conversation with that ID:

```console
cargo run -- --conversation conversation_019... "Show an example"
```

The default model is `gpt-5.6`. Select another OpenAI model with `--model`:

```console
cargo run -- --model MODEL_NAME "Explain ownership in Rust"
```

CLI verbosity defaults to `low`. Model events classify their messages as detailed, interesting, or important. Select `low`, `medium`, or `high` to print progressively more of those messages:

```console
cargo run -- --verbosity medium "Explain ownership in Rust"
```

`:turn` is the default command when no command is given. Commands are prefixed with a
colon, so the explicit form is `cargo run -- :turn "Explain ownership in Rust"`.
Run `cargo run -- --help` or `cargo run -- :turn --help` for the complete command-line
help.

## Tools

A turn may request tools. The current build registers one `shell` tool that
runs a command through `/bin/sh -c` without an approval prompt, in the current
directory unless `working_directory` is given, with a 30-second timeout unless
`timeout_seconds` is given. Standard output and standard error are captured up
to 64 KiB each with explicit truncation flags. Nonzero exits are results;
timeouts, unknown tools, invalid arguments, and launch failures are returned to
the model as typed execution problems. Tool definitions, requests, and
responses are recorded in the conversation log. See [Tools](docs/tools.md).

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

Semantic conversation events are stored in the first available location:

1. `$TOG_DATA_DIR`
2. `$XDG_DATA_HOME/tog`
3. `$HOME/.local/share/tog`

The current semantic event schema is a clean break from the earlier agent-run-based prototype. Existing prototype conversations are not migrated.

## Development

Run the project checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Detailed project documentation is in [`docs/`](docs/README.md).
