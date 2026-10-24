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

Today a hosted workstation boots a stock Compute Engine image. Reproducing the
flake environment inside it, so that developing there feels identical to local,
is the intended direction. Until then, the lifecycle commands below manage the
hosted workstation itself.

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

### Configure project and zone

`tog-workstation` uses the `--project` and `--zone` flags when you pass them,
and otherwise reads gcloud's configuration. Set these once so the commands need
no flags. The examples below use the project ID `tog` and the zone
`us-central1-a`:

```console
gcloud config set project tog
gcloud config set compute/zone us-central1-a
```

Project IDs are globally unique, so `tog` may already be in use; choose another
ID if `gcloud config set project tog` is rejected or the project is not yours.

The workstation is provisioned and stopped in this zone, so choose where you
want the machine to live. Google documents
[regions and zones](https://cloud.google.com/compute/docs/regions-zones).
`list` without `--zone` shows workstations in every zone.

Each command accepts per-call overrides:

```console
tog-workstation create main --machine-type e2-standard-2 \
  --project tog --zone us-central1-a
```

### Verify access

Confirm the configured values, then list machine types available in the zone:

```console
gcloud config list project compute/zone
gcloud compute machine-types list --zones us-central1-a
```

`create` validates a requested machine type against the zone before
provisioning. See the [machine types](https://cloud.google.com/compute/docs/machine-resource)
reference for the available families, including `e2-standard-2`, the default.
