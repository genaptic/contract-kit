# conkit-sketch Crate Agent Guide

This file layers on top of the repository root `AGENTS.md`. Use it for work in
the `conkit-sketch` crate. Read [ARCHITECTURE.md](ARCHITECTURE.md) for the structural
map, public surface, formats, and data flows; keep operational rules and exact
validation commands here.

## Crate Role

- Treat `conkit-sketch` as a storage-agnostic, byte-in/byte-out domain library for
  language-neutral sketch contracts.
- Keep `SketchContractKit` as the public behavior owner and
  `SketchContractKitBuilder` as its public builder.
- Keep `check`, `generate`, and `diff` async and runtime-neutral.
- Treat `FileCatalog` as every source, contract, report, and generated-contract
  byte boundary.
- Treat `CatalogPath` as a validated logical UTF-8 path using `/`. It is not an
  operating-system path.

## Skill Routing

- Use `$use-rust-best-practices-core` for any Rust planning, editing, review,
  debugging, or validation task in this crate.
- Add `$use-rust-best-practices-architecture` when changing crate boundaries,
  modules, public APIs, private dispatch, or test placement.
- Add `$use-rust-best-practices-testing` when changing unit tests, integration
  tests, boundary scanners, doctests, or validation strategy.
- Add `$use-rust-best-practices-async` before changing public async methods,
  work-pool behavior, CPU scheduling, or the sync/async boundary.
- Add `$use-rust-best-practices-abstractions` before changing the private
  backend trait, error shape, dispatcher, or other polymorphic boundaries.
- Add `$rust-code-structuring-best-practices` before changing structs, enums,
  builders, repeated data groups, receiver-method ownership, or standalone
  helper placement.
- Add `$use-rust-best-practices-dependencies-platforms` before changing Cargo
  dependencies, feature policy, or cross-platform catalog behavior.

## External Domain Boundaries

- Preserve the caller/crate ownership described in
  [ARCHITECTURE.md](ARCHITECTURE.md): callers own storage and presentation;
  `conkit-sketch` owns sketch-domain behavior over logical catalogs and bytes.
- The word "file" in this crate means a `FileCatalog` entry containing a
  `CatalogPath` and `Vec<u8>`. It never implies local filesystem IO.
- Do not add dependencies on `conkit` or `conkit-signature`, and do not move sketch
  behavior into either caller or the signature domain.
- Keep signature interpretation minimal and YAML-only: index top-level labels,
  files, signature types, and signature-to-sketch links without invoking Rust
  parsing or depending on the `conkit-signature` crate. Callers provide exact source
  spans as neutral `SketchSeed` values.
- Do not add language-specific AST parsing to matching. The sketch contract is
  the normalized contiguous line-sequence behavior documented in the
  architecture guide.

## Public Boundary Rules

- Keep `lib.rs` limited to module declarations, crate policy, and intentional
  re-exports of public API and boundary types.
- Public requests and responses must use catalog bytes and logical paths,
  never `Path`, `PathBuf`, filesystem roots, written-file lists, or storage
  provider details.
- Keep public request/response DTOs and enum fields serializable,
  deserializable, and comparable when they cross the API boundary.
- Preserve deterministic `FileCatalog` ordering, duplicate-insert rejection,
  and construction-time plus serde-time `CatalogPath` validation.
- Keep `SketchContractKit` opaque over its private inner enum. The public
  handle and every concrete backend payload must implement the same private
  `SketchContractKitBackend` trait.
- Keep backend dispatch explicit and exhaustive. Call receiver methods in
  variant arms, do not use wildcard arms or macro-generated dispatch, and
  reserve `<Self as Trait>::method(self, ...)` for public-handle bridges.
- Keep `SketchContractKitError` as the builder/check/generate/diff error
  boundary.
  Keep `FileCatalogError` as the separately exported error for public catalog
  and path operations.
- Keep public types documented and preserve the crate-level missing-docs and
  broken-link lints.

## Contract And Check Guardrails

- Treat the parser, normalization, matching, counting, check-mode, and report
  semantics in [ARCHITECTURE.md](ARCHITECTURE.md) as the current behavior
  contract. Update that guide and focused tests whenever those semantics
  change.
- Keep `SketchContractDocuments` as the single parsed combined-document owner
  for validation and generation. Do not add a parallel YAML representation or
  a second parse.
- Keep contract validity failures as operation errors. Do not silently
  downgrade malformed YAML, unknown fields, duplicate IDs, invalid or missing
  fields, invalid paths, orphan or ambiguous links, kind mismatches, or empty
  required values to match diagnostics.
- Keep ordinary missing-source and non-matching-snippet outcomes as response
  diagnostics. Preserve at most one diagnostic per parsed sketch and keep
  response counts consistent with the diagnostic collection.
- Keep sketch IDs as diff identity. For the same ID, semantic comparison uses
  the linked source file, linked signature label, `signature_type`, and
  normalized code. Contract-document location, YAML formatting, YAML comments,
  and mapping order are nonsemantic; comments and other tokens inside `code`
  remain part of the normalized snippet and are semantic.
- Keep report rendering byte-only. Report modules return catalog entries;
  callers choose persistence paths and storage mechanisms.
- Do not change `Warning`, `Default`, or `Strict` behavior without changing
  their tests and the architecture description in the same work.

## Generation Guardrails

- Refresh only sketches already linked from signatures in existing combined
  documents. Require one neutral seed for every explicit link and validate its
  document, ID, signature type, source file, and nonempty code.
- Keep generation byte-only. It returns contract catalogs and never creates
  directories, writes local files, prints output, or performs CLI merging.
- Preserve `root`, `files`, `signatures`, signature-owned links, sketch IDs,
  and sketch signature types while replacing only linked sketch code.
- Render only targeted root documents. Preserve untargeted root-document bytes
  and nested or non-YAML passthrough entries byte-for-byte.
- Treat changes to combined-document fields, link direction, IDs, or entry
  ordering as contract-format changes that require parser round trips, public
  API tests, fixtures, and documentation updates.

## Async, Structure, And Determinism

- Keep the public future runtime-neutral. Submit finite CPU work to the private
  Rayon pool and return results through futures oneshot channels.
- Do not add Tokio, `async_trait`, `spawn_blocking`, `block_in_place`, or an
  internal `block_on`. Runtime entry belongs to the caller.
- Put production behavior on cohesive structs, data-carrying enums, builders,
  or explicit trait contracts. Do not introduce Rust item type aliases,
  same-root lifecycle/state carrier families, loose standalone helpers, or
  macro-generated abstractions.
- Preserve ordered catalog/set storage and explicit sorting after parallel
  work. Worker scheduling must not change diagnostics, counts, generated file
  order, generated entry order, or rendered bytes.
- Normalize each contract `SketchSnippet` at construction and reuse its cached
  normalized value for matching and semantic comparison.
- Keep production modules identical in test and non-test builds. Test-only
  support belongs inside local `#[cfg(test)] mod tests` blocks or under
  `conkit-sketch/tests`, never in test-gated production impls or imports.

## Internal Ownership Rules

- `api.rs` owns public DTOs, builder and handle wiring, private backend
  dispatch, check/generate/diff composition, and top-level async work
  submission.
- `files.rs` and `work.rs` own the catalog/path boundary and runtime-neutral
  CPU bridge respectively.
- `contract.rs` owns parsed documents, original-byte passthrough, contract
  validation, targeted refresh/rendering, and ID-based semantic diffing;
  `normalize.rs`, `matcher.rs`, and `inventory.rs` own normalization, matching,
  diagnostics, counts, and check modes.
- `generate.rs` owns seed validation and generation orchestration, while
  `report.rs` renders returned report bytes. Neither may gain filesystem or
  presentation responsibilities.
- Keep the complete descriptive module map in
  [ARCHITECTURE.md](ARCHITECTURE.md) rather than duplicating it here.

## Testing Expectations

- Keep unit tests beside the private behavior they exercise.
- Keep public API, async behavior, serialization, and source-boundary scanners
  under `conkit-sketch/tests`.
- Preserve boundary coverage rejecting `conkit_signature::`, `clap`, Tokio,
  `async_trait`, filesystem IO, process exits, OS-path DTOs, and production
  `#[cfg(test)]` shims.
- Cover contract validity separately from match diagnostics. Cover linked
  refresh, deterministic output, all check modes, malformed source bytes, and
  generated-contract round trips at the narrowest useful test level.
- Cover semantic diff additions, removals, and changes as well as no-op
  comparisons for YAML formatting, YAML comments, mapping order, and
  contract-file relocation. Cover comments or other token changes inside
  sketch code as semantic changes.

## Validation Defaults

Run the smallest relevant checks while iterating. Before finishing a sketch
crate change, prefer:

```bash
cargo fmt --all -- --check
cargo check --locked -p conkit-sketch --all-targets --all-features
cargo clippy --locked -p conkit-sketch --all-targets --all-features -- -D warnings
cargo test --locked -p conkit-sketch --all-targets
cargo test --locked -p conkit-sketch --doc
cargo test --locked --workspace --all-targets
```

When a change also affects CLI adaptation, run the `conkit` package tests and the
checked-in scenarios before the final workspace checks.
