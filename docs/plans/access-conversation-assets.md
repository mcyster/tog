# Give drivers access to conversation assets

Associate stored assets with the conversation through event references, and give
drivers access to those assets through their conversation input. Each driver
decides which referenced assets to include in a provider request and how to
represent them.

Mark accepted this direction on 2026-10-03. Implementation remains planned.
Tog already stores immutable assets with an ID, name, MIME type, and byte size.
The store provides metadata and streaming content reads. Conversation events and
the driver's bounded input do not yet connect those assets to model invocations.

## Association and ownership

An asset is stored once and referenced by ID from the event that introduces or
uses it. User content, assistant content, and tool responses can associate assets
with the conversation. This preserves the context in which an asset appeared
without maintaining a second authoritative asset list on the conversation.

Keep asset bytes in the existing asset store. An attachment is an asset's use on
an event, not another storage system or a required new domain abstraction. Reuse
the existing asset concepts where they express the association clearly.

The association carries the asset ID and its immutable metadata: name,
`mime_type`, and `byte_size`. A reader can describe the reference without opening
the content. The producer obtains this metadata from the stored asset, rather
than inventing a second description. Different events can reference the same
asset; names remain descriptive and need not be unique.

The [asset contract](../assets.md) continues to own identity, immutability, and
storage. The [conversation boundary](../architecture/conversation.md) owns event
meaning and the driver's input.

## Access through conversation input

Expose referenced assets through the driver's existing conversation input
abstraction. Metadata can be read without loading bytes; content is accessed only
when the driver needs it, through the store's read contract. The application or
engine supplies the asset access implementation when composing the input.

The driver does not need filesystem paths or a separate independently maintained
list of attachments. Event references remain the source of association, and asset
storage remains behind its interface. A convenience view over associated assets
is derived from those references.

Respect the model request's fixed input boundary: references introduced by later
events do not silently enter an earlier invocation's input. Reading asset content
does not append events or execute tools.

## Provider presentation

Association does not require inclusion on every model call. The driver chooses
which assets from its input are relevant and which representations its provider
supports. It may use native image content, file content, or text, while preserving
the relationship to the event that supplied the asset.

Do not require every driver to support every media type. Define how a concrete
driver reports unsupported content and missing or unreadable assets when
implementing it; unavailable content must not be silently represented as an empty
successful read.

Text extraction is not part of the current asset store. It may become useful for
particular formats, but this plan does not add a general extraction service or
promise an extracted-text accessor. Likewise, it does not require eager base64
encoding, an in-memory copy of every asset, or resending all assets on every call.

## Implementation and completion

Add asset associations to the relevant conversation content, provide lazy access
through the bounded driver input, and implement one supported provider path.
Settle the exact association shape and accessor signatures against those concrete
uses. Keep bytes out of event JSON and executable access mechanisms out of
persisted history.

Verify metadata and references through serialization, asset-only user input,
repeated references to one asset, associations on tool results, fixed input
boundaries, lazy reads, and missing or unsupported content. Provider fixtures
should establish how the selected driver represents an asset without requiring
live paid calls.

Completion means an event can reference a stored asset and a driver can read and
present it through the conversation abstraction, while deciding what to include
per invocation. New media formats, extraction, retention policies, and automatic
asset selection strategies remain separate work.
