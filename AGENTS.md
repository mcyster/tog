# Repository Guidelines

## Priorities

Apply these priorities in order:

1. Readability and understandability
2. Correctness through strong types
3. Simplicity
4. Performance supported by evidence

The complete development standards are in [`docs/development-standards.md`](docs/development-standards.md). Follow them for every change.

## Rust Source

- Use descriptive, full-word names for project-defined types, functions, modules, variables, parameters, and generic parameters.
- Do not use project-defined abbreviations or single-letter names.
- Standard Rust and domain terminology such as `std`, `str`, `Ok`, `Err`, `Self`, and `tog` is allowed.
- Do not add comments to Rust source files. Extract complexity into well-named entities instead.
- Make domain values valid by construction: keep fields private, enforce invariants in constructors, and return typed errors rather than panicking or asserting for invalid external input.
- Treat deserialized values as untrusted. If derived deserialization bypasses constructors, validate them when reconstructing the containing domain object.
- Preserve valid external values exactly unless normalization is part of the domain contract; validation may inspect a normalized form without changing the value.
- Use `From` and `from_*` only for infallible conversions; use `TryFrom`, `try_*`, or another clearly fallible constructor when conversion can fail.
- Put conversions on the containing or destination type rather than making inner domain types depend on outer systems.
- Expose contractually readable state through immutable typed accessors, including extension data intended for compatible consumers; do not expose mutation for convenience.
- Use `Option` for absence and `Result` for recoverable failure.
- Keep public interfaces minimal.
- Keep provider-neutral contracts independent of concrete integrations; integrations depend on contracts, never the reverse.
- Classify values by lifetime before designing an API: object-stable values belong to the object, per-operation values are parameters, derived values are not passed twice, and produced values are returned.
- Prefer direct parameters and return values. Add input structs, result wrappers, callbacks, sinks, or streams only for a concrete current requirement.
- Prefer concrete implementations. Introduce traits and generic abstractions only when a demonstrated need exists.
- Do not use unsafe Rust.
- Prefer conventional, idiomatic Rust and standard-library facilities when they adequately represent the problem.
- Do not create custom types, abstractions, utilities, or implementations merely to avoid a normal standard-library facility. Domain types must enforce a genuine distinction or invariant.
- Do not recreate established library functionality solely to avoid a dependency.
- Before adding a dependency, explain what the standard library lacks, identify the conventional crate, compare its maintenance and security cost with reasonable standard-library or local implementations, and discuss the choice.
- Use conventional representations unless a requirement forces a specialized one. Document persistent or external unit, encoding, precision, or compatibility requirements before introducing a custom type or dependency for them.

## Structure

- Keep `src/main.rs` limited to application composition and process input or output.
- Organize modules by feature or responsibility, not by generic technical layers.
- Keep the project as one binary package until a concrete requirement justifies a library target or workspace.
- Keep durable documentation in `docs/`.
- Record consequential decisions in `docs/decisions/`.
- Keep unaccepted proposals in `docs/ideas.md`.

## Change Discipline

- Make repository changes on a branch and submit them through a pull request. Do not commit directly to the default branch.
- Make the smallest complete change that satisfies the requirement.
- Treat exploratory ideas and future possibilities as non-requirements until explicitly accepted.
- Do not add speculative compatibility, configuration, abstractions, or extension points.
- When feedback changes ownership, lifetime, output hierarchy, or failure semantics, restate the complete boundary and remove invalid abstractions instead of layering another type onto them.
- When a simpler accepted direction gives up an earlier property, remove the superseded code, tests, and documentation rather than preserving it implicitly.
- Preserve existing behavior unless the task explicitly changes it.
- Add or update tests for observable behavior and validation rules.
- Deliver every change through a pull request: commit, push, and create one, or update the current open pull request. Check that an existing pull request is not already merged before updating it.

## Required Validation

Run all checks inside the Nix development environment:

```console
nix develop --command cargo fmt --check
nix develop --command cargo clippy --all-targets --all-features -- -D warnings
nix develop --command cargo test --all-targets --all-features
```
