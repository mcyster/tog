# Workstation

A workstation is the environment you use to build and develop tog. It can run
locally on your machine or hosted on Google Cloud. The two should feel
identical: both come from the same [Nix flake](../flake.nix), so tools,
versions, and commands match.

## The environment comes from a Nix flake

The repository's flake defines the development environment: toolchain, linters,
formatters, and helper tools. Nix delivers the same environment wherever it
runs, which is what lets a local and a hosted workstation match.

Enter it with `nix develop`, or automatically through
[direnv](../.envrc). The flake provides two shells:

- `.#default` — the day-to-day build environment.
- `.#gcloud` — `.#default` plus the Google Cloud CLI, for managing hosted
  workstations.

## Local workstation

Your machine, with the flake environment active and the repository checked out.
This is where you build and develop tog day to day.

## Hosted workstation

A hosted workstation runs on Google Compute Engine as an instance backed by a
persistent boot disk. [`tog-workstation`](../README.md#workstations) manages its
lifecycle:

- `create` provisions it with a machine type, defaulting to `e2-standard-2`.
- `stop` halts compute charges while keeping the boot disk.
- `start` resumes the same instance.
- `destroy` deletes the instance and its boot disk.
- `list` shows hosted workstations and their machine types.

### SSH and file transfer

`tog-workstation shell` opens an interactive SSH session on a running
workstation:

```console
tog-workstation shell
```

gcloud manages the SSH key, so no password is needed. The underlying commands
work directly too: `gcloud compute scp` transfers files (with `--recurse` for
directories) using the same key.

`tog-workstation config-ssh` adds an SSH config alias for the workstation, after
which plain `ssh`, `scp`, and `rsync` connect without gcloud flags:

```console
tog-workstation config-ssh
ssh workstation-tog.us-central1-a.tog-dev
rsync -avz ./tog workstation-tog.us-central1-a.tog-dev:~/
```

### Web terminal

`create --web-terminal` adds a web terminal so the workstation is usable from any
device with a browser, with no local tooling installed. It installs
[code-server](https://github.com/coder/code-server) — VS Code and a shell in the
browser — serves it on `https://EXTERNAL_IP:8443` behind a password, and starts it
as a service that survives `stop` and `start`. Accept the self-signed certificate
warning the browser shows.

```console
tog-workstation create --web-terminal
```

Every web terminal password is random and never printed or stored on the
workstation. The machine keeps only an argon2 hash of it, and the plaintext is
stored in Google Cloud Secret Manager under the secret `tog-web-terminal-password`.
Anyone with view access to the secret can read the current password from the
Google Cloud console (no terminal needed); the workflow's service account can
only add versions, never read them, so the value never appears in GitHub
output.

```console
tog-workstation web-terminal
```

`web-terminal` enables the browser terminal if it is not installed, rotates the
password on a running workstation, and stores the fresh value in Secret Manager;
the previous password stops working. `start` also issues fresh credentials when
the workstation boots. Because rotation rewrites the hash, already-open browser
sessions lose access on their next request and get redirected to the login page.
A `start` on a workstation that is already running leaves credentials untouched.

Without `--web-terminal`, `create` stays ssh-only and opens no extra firewall
port.

### Host setup

`create` injects first-boot setup (Google's cloud-init) so the host is ready for
development as soon as it boots. It creates the `tog` user and installs nix,
direnv, nix-direnv, and git, then checks out the tog repository at
`/home/tog/tog/main` with direnv allowed. The web terminal and SSH both run as
the `tog` user.

The setup lives in `host/setup-host.sh` and is embedded into the instance's
`user-data` metadata at create time, so the versioned script is deployed with
the tool rather than fetched from the network later. It is guarded by a marker
file and safe to rerun.

```console
tog-workstation create --web-terminal
```

### Reusing the host setup pattern

`host/setup-host.sh` is deliberately small and generic. To point it at another
project, change the two variables at the top:

- `TOG_REPO_URL` — the repository to check out.
- `TOG_CHECKOUT_DIR` — where to clone it (default `/home/tog/tog/main`).

The rest — creating the `tog` user, installing nix + direnv, hooking the shell,
allowing the checkout's `.envrc` — transfers as-is. Keep the script idempotent:
any new step must be safe when the marker already exists or a previous run
partially completed. `create` embeds whatever `host/setup-host.sh` contains at
invocation time, so committing a change to that file changes what the next fresh
workstation boots into.

## Google Cloud setup

Managing hosted workstations needs a Google Cloud project, the `gcloud` CLI,
and credentials.

### Install the Google Cloud CLI

The Nix dev environment provides `gcloud` in the `.#gcloud` shell, not the
default shell, so contributors who never manage workstations do not download
the SDK.

- With direnv: set `TOG_GCLOUD_ENABLE=1` and reload the directory.
- With plain nix: run `nix develop .#gcloud`.

Outside the Nix environment, install the
[Google Cloud SDK](https://cloud.google.com/sdk/docs/install).

### Authenticate

For interactive use, log in with your Google account:

```console
gcloud auth login
```

For automation, provide application default credentials read by gcloud, for
example a service-account key:

```console
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json
```

The GitHub Actions workflow authenticates with
[Workload Identity Federation](https://github.com/google-github-actions/auth)
instead.

### Configure the workflow from the repository

The workflow's manual run form asks only for the action and workstation name.
The Google Cloud details come from repository settings, so you configure them
once instead of typing them per run. The web terminal turn-on defaults to on,
so a manual `create` from the action is ready to use in a browser; turn it off
for an ssh-only workstation.

Set the cloud configuration as repository variables and secrets, under
**Settings -> Secrets and variables -> Actions** in `mcyster/tog`:

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `TOG_GCLOUD_PROJECT` | Your project ID, for example `tog-dev` |
| Variable | `TOG_GCLOUD_ZONE` | Default `us-central1-a`; set it to change |
| Secret | `TOG_GCLOUD_SERVICE_ACCOUNT` | The email of the service account to run as, for example `github-actions@tog-dev.iam.gserviceaccount.com` |
| Secret | `TOG_GCLOUD_WORKLOAD_IDENTITY_PROVIDER` | The workload identity provider used to mint credentials for that service account |

**What the workload identity provider is.** It is the `gcloud` login for a
machine that has no browser: it lets GitHub exchange an OIDC token for short-lived
Google credentials scoped to a service account. The value looks like
`projects/123456789012/locations/global/workloadIdentityPools/tog-actions/providers/github`.

**How to create the values.** Run the setup script. It creates the service
account, pool, provider, and the Secret Manager secret `tog-web-terminal-password`,
grants the roles, and prints the values:

```console
scripts/tog-workstation-gcloud-identity --project tog-dev \
  --repo mcyster/tog
```

The script is idempotent: re-running it creates nothing new and reprints the
values. See the script's `--help` for the details of what it provisions. It
requires the Google Cloud CLI and a principal with project IAM-admin
permissions in the project.

The workflow passes these on to `gcloud`; the script itself never reads them.

### Create a project

Create a project and note its ID:

```console
gcloud projects create tog-dev
```

Project IDs are globally unique, so if `tog-dev` is unavailable, choose another
ID. Creation can fail transiently with HTTP `429 RESOURCE_EXHAUSTED` ("Quota
exceeded for ... 'Write requests per minute'"); wait a minute and retry, or
create the project from the [Cloud Console](https://console.cloud.google.com).
Enable billing before provisioning any instances.

### Configure project and zone

`tog-workstation` uses the `--project` and `--zone` flags when you pass them,
and otherwise reads gcloud's configuration. Set these once so the commands need
no flags. The examples below use the project ID `tog-dev` and the zone
`us-central1-a`:

```console
gcloud config set project tog-dev
gcloud config set compute/zone us-central1-a
```

The workstation is provisioned and stopped in this zone, so choose where you
want the machine to live. Google documents
[regions and zones](https://cloud.google.com/compute/docs/regions-zones).
`list` without `--zone` shows workstations in every zone.

Each command accepts per-call overrides:

```console
tog-workstation create main --machine-type e2-standard-2 \
  --project tog-dev --zone us-central1-a
```

### Verify access

Confirm the configured values, confirm billing is enabled, then list machine
types available in the zone:

```console
gcloud config list
gcloud billing projects describe tog-dev
gcloud compute machine-types list --zones us-central1-a
```

`gcloud billing projects describe tog-dev` reports `billingEnabled: true` when
billing is on. Provisioning an instance requires it, so enable billing on the
project if the check is false.

`create` validates a requested machine type against the zone before
provisioning. See the [machine types](https://cloud.google.com/compute/docs/machine-resource)
reference for the available families, including `e2-standard-2`, the default.
