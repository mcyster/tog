# Use Short Type Prefixes In Identifier Storage

## Status

Accepted

## Context

Identifier types each had a long display prefix (`conversation_`,
`conversation_event_`, `asset_`), but their serialized and stored forms
(JSON records and filesystem storage keys) dropped the prefix, leaving bare
uuids on disk. Persistent ids were then not self-describing: a reader had to
infer from context whether an id named a conversation, an event, or an asset.

## Decision

Every identifier carries a short type prefix of 1-4 characters followed by an
underscore, then the underlying id:

- conversation: `cnv_`
- conversation event: `evt_`
- asset: `ast_`

The owner accepted this during development after reviewing the on-disk storage:
prefixes should be included in the stored ids and storage keys, shortened to
`cnv_`, `evt_`, and `ast_`.

The prefixed form is used uniformly for display, JSON serialization, and
filesystem storage keys, so an id is self-describing wherever it appears.
Deserialization also accepts the previous long prefixes and unprefixed uuids so
data written before this change remains readable. Storage lookups fall back to
the legacy bare-key directory names when the prefixed name is absent, keeping
existing conversations and assets accessible without migration.

The prefixes are short by design so the redundancy (the storage location often
already implies the type) stays unobtrusive while ids remain unambiguous.

## Consequences

New serialized records and storage keys use `cnv_`, `evt_`, and `ast_`. Existing
data without prefixes or with the earlier long prefixes continues to load. The
event record schema version was bumped to 14 to reflect the serialized id format
change. The choice of a specific prefix for a future identifier type should
follow the same 1-4 character underline convention.