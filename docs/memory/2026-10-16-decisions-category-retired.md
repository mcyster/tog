# Memory: decisions category retired

2026-10-16: `docs/decisions/` was removed as a separate documentation category.

Decision records were folded back into the documents that describe their subject:

- Enduring tradeoffs, including conversation-as-portable-input and semantic
  portability, live in [`../principles.md`](../principles.md).
- Current environment and conversation choices live in
  [`../architecture/conversation.md`](../architecture/conversation.md) and
  [`../conversation.md`](../conversation.md).
- The Schemars tool-contract choice and its `PartialEq`-not-`Eq` consequence
  live in [`../tools.md`](../tools.md).
- The `cnv_` / `evt_` / `ast_` identifier-prefix convention lives in
  [`../conversation.md`](../conversation.md) and [`../assets.md`](../assets.md).
- Crate-workspace structure lives in [`../architecture.md`](../architecture.md)
  and [`../designs/organize-crate-boundaries.md`](../designs/organize-crate-boundaries.md).
- Asset storage lives in [`../assets.md`](../assets.md).

The decision `docs/decisions/README.md` rules no longer apply. Acceptance is now
stated in ordinary prose where it matters, and Git keeps the history of superseded
choices. Do not recreate a decisions directory.