# Store Assets Immutably Beside The Data Directory

## Status

Accepted

## Context

`tog` needs to reference durable binary content, such as images and files, from
conversation events without embedding large payloads in the ordered log. The
conversation remains the authoritative portable record; assets are external
content referenced by a strongly typed identifier.

## Decision

Assets are immutable files stored under `<data>/assets/<asset-id>/` in the same
resolved data directory that holds `conversations/`. Each asset directory
contains a `metadata.json` record and a `content` file. The contract is a
neutral `AssetStore` trait with add, metadata, read, and list
operations that stream content and expose no filesystem types, so a future
remote or object-storage implementation can replace the filesystem
implementation.

An asset is committed by writing both files into a hidden staging directory and
atomically renaming it into place, then syncing the parent directory. The
`AssetId` is returned only after the commit succeeds, existing assets are never
overwritten, and incomplete writes never appear in listings or as readable
content. Names are descriptive metadata, not identifiers: duplicate names are
allowed and references use `AssetId`.

## MIME type inference

MIME inference at the `:asset add` command uses the [infer]
(https://docs.rs/infer) crate, which detects well-known formats from content
signatures. The standard library has no MIME inference. Scanning a small
content prefix with `infer` is more robust than mapping file extensions and
falls back to `application/octet-stream` when unrecognized. `infer` is a small,
widely-used, MIT-licensed crate with no unsafe code and no mandatory
dependencies beyond the standard library.

## Consequences

Assets are durable outside the conversation log, addressable by stable typed
identifiers, and streaming-safe for large content. Conversation integration,
model-driver support, and agent tool access to assets are future work and will
reference assets through `AssetId`.