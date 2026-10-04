# Assets

`tog` stores immutable files as assets outside the conversation log. Assets are
addressed by a strongly typed `AssetId`, never by name, and their content is
mutated only by storing a new asset.

## Asset model

An asset has:

- an immutable `AssetId`, distinct from conversation, event, and tool
  identifiers. The identifier is serialized as `ast_<uuid>` and its
  underlying representation is encapsulated.
- a human-readable name (`String`). Names are descriptive metadata, not
  identifiers: duplicate names are allowed, and references always use the
  `AssetId`.
- a `MimeType`.
- immutable content bytes.

The name, MIME type, and content are fixed once an asset is stored. There is no
revision management, extracted text, renaming, or deletion. Metadata exposes
the content size in bytes.

## Store contract

`AssetStore` is the neutral persistence boundary. It supports:

- adding an asset from a reader and returning its `AssetId`
- reading metadata by `AssetId`
- reading content as a reader
- listing metadata without loading content

Content is streamed both in and out, so large assets need not fit in memory.
The contract exposes no filesystem paths or types; a future remote or
object-storage implementation can replace the filesystem implementation.

## Filesystem implementation

Assets live under the resolved data directory alongside `conversations/`:

```text
<data>/assets/<storage-key>/metadata.json
<data>/assets/<storage-key>/content
```

`<data>` is resolved from `$TOG_DATA_DIR`, else `$XDG_DATA_HOME/tog`, else
`$HOME/.local/share/tog`, matching the event store. The storage key is the
`ast_<uuid>` form used for display, serialization, and references. An asset
directory appears only after both files are fully written and synced, so a torn
write stays in a hidden staging directory that listings ignore. Adding an asset
returns its `AssetId` only after the commit succeeds, and an existing asset is
never overwritten or mutated.

## Command line

```console
tog :asset add ~/example/screenshot.jpg
tog :asset add --name custom --mime-type image/png ~/example/image
tog :asset list
```

`:asset add` defaults the name to the source file's basename and infers the
MIME type from the file content using the [infer]
(https://docs.rs/infer) crate, falling back to `application/octet-stream` when
unrecognized. Both can be overridden with `--name` and `--mime-type`. Standard
output carries a single JSON object for the added asset and JSON Lines for
`:asset list`, with `id`, `name`, `mime_type`, and `byte_size` fields:

```json
{"id":"ast_01a0df...","name":"shot.png","mime_type":"image/png","byte_size":8}
```

Stored assets remain available after the source file changes or disappears.