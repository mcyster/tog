# Workstations

`tog-workstation` manages Google Compute Engine workstations through the
`gcloud` CLI. Set up Google Cloud first. Each command tells you exactly what it
is missing, such as the project, the zone, or `gcloud` itself.

The command usage is documented in the [README](../README.md#workstations).

## Install the Google Cloud CLI

The Nix dev environment provides `gcloud` in the `gcloud` shell, not the
default shell, so contributors who never manage workstations do not download
the SDK.

- With direnv: set `TOG_GCLOUD_ENABLE=1` and reload the directory.
- With plain nix: run `nix develop .#gcloud`.

Outside the Nix environment, install the
[Google Cloud SDK](https://cloud.google.com/sdk/docs/install).

## Authenticate

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

## Configure project and zone

`tog-workstation` uses the `--project` and `--zone` flags when you pass them,
and otherwise reads gcloud's configuration. Set project and zone once so the
commands need no flags:

```console
gcloud config set project PROJECT
gcloud config set compute/zone ZONE
```

The workstation is provisioned and stopped in this zone, so choose where you
want the machine to live. Google documents
[regions and zones](https://cloud.google.com/compute/docs/regions-zones).
`list` without `--zone` shows workstations in every zone.

Each command accepts per-call overrides:

```console
tog-workstation create main --machine-type e2-standard-2 \
  --project PROJECT --zone ZONE
```

## Verify access

Confirm the configured values, then list machine types available in the zone:

```console
gcloud config list project compute/zone
gcloud compute machine-types list --zones ZONE
```

`create` validates a requested machine type against the zone before
provisioning. See the [machine types](https://cloud.google.com/compute/docs/machine-resource)
reference for the available families, including `e2-standard-2`, the default.
