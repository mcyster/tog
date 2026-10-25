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
## tool_request shell {"command":"echo \"test 123\""}
## tool_response {"type":"exited","code":0,"stdout":"test 123\n","stdout_truncated":false}
## assistant_response test 123
```

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

## Workstations

Manage Google Compute Engine workstations backed by a persistent boot disk with
[`scripts/tog-workstation`](scripts/tog-workstation). It wraps the `gcloud`
CLI; follow the [Google Cloud setup](docs/workstation.md) for the SDK,
authentication, and project and zone, then run:

```console
scripts/tog-workstation create
scripts/tog-workstation create main --machine-type e2-standard-2
scripts/tog-workstation list
scripts/tog-workstation stop
scripts/tog-workstation start
scripts/tog-workstation destroy
```

- The machine type defaults to `e2-standard-2` (2 vCPU, 8 GB RAM). Any machine
  type available in the zone is accepted; a requested type is validated against
  the zone before provisioning.
- The workstation name defaults to `tog`; pass a name to target another
  workstation.
- `stop` halts compute charges while keeping the boot disk, which continues to
  incur storage charges. `start` resumes the same workstation.
- `shell` opens an interactive SSH session on a running workstation; gcloud
  manages the key, so no password is needed.
- `config-ssh` adds an SSH config alias for the workstation so plain `ssh`,
  `scp`, and `rsync` connect without gcloud flags.
- `destroy` deletes the workstation and its boot disk after a typed
  confirmation, or unconditionally with `--yes`.
- `create` on an existing workstation leaves it unchanged, so its machine type
  is preserved. There is no resize operation.
- `create --web-terminal` installs code-server on the workstation: VS Code and a
  terminal in the browser at `https://EXTERNAL_IP:8443`, behind a password and a
  firewall rule for that port. Run create without `--web-terminal` to keep the
  workstation ssh-only.
- Web terminal passwords are random and never printed or stored on the
  workstation; the machine keeps only an argon2 hash, and the plaintext lives in
  Google Cloud Secret Manager under `tog-web-terminal-password`, where the owner
  reads it from the Google Cloud console. `web-terminal` enables the browser
  terminal if missing and rotates the password in Secret Manager; `create
  --web-terminal` and `start` (after a boot) do the same. Password forgotten?
  Run `web-terminal` again.
- `create` also injects first-boot host setup (via cloud-init): it creates the
  `tog` user, installs nix, direnv, nix-direnv, and git, and checks out the tog
  repository at `/home/tog/tog/main` with direnv allowed. The hosting runs in
  `host/setup-host.sh`, which is embedded into the instance at create time; see
  `docs/workstation.md` for the reusable pattern.
- No snapshots or backups are created automatically.

The reusable workflow `.github/workflows/tog-workstation.yml` wraps the same
commands for GitHub Actions. The manual run form asks only for the action and
workstation name, and installs the web terminal by default; `project`, `zone`,
and the workload identity credentials come from repository variables and
secrets. Provision those once with `scripts/tog-workstation-gcloud-identity`
(see `docs/workstation.md`). For `create --web-terminal`, `start`, and
`web-terminal` it writes the web terminal URL to the job summary plus a link to
the secret in the Google Cloud console; the password itself never appears in
GitHub output.
Retired passwords are deliberately recoverable only by running the workflow
again and reading the new value from Secret Manager, which is an accepted
tradeoff for a personal workstation on a public repository.

## Development

Run the project checks:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Detailed project documentation is in [`docs/`](docs/README.md).
