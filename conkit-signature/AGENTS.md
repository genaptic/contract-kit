# conkit-signature Crate Agent Guide

This file layers on top of the repository root `AGENTS.md`. Use it for work in
the `conkit-signature` crate. Read [ARCHITECTURE.md](ARCHITECTURE.md) for the
structural boundary, public surface, formats, data flows, and module map; keep
operational rules and exact validation commands here.

## Crate Role

- Treat `conkit-signature` as a byte-in, byte-out domain library for contract
  signatures.
- Keep `SignatureContractKit` as the public behavior owner and
  `SignatureContractKitBuilder` as the public builder.
- Keep public behavior async-only through `check`, `generate`,
  `resolve_sketches`, and `diff`.
- Keep public futures runtime-neutral. Do not add Tokio, `spawn_blocking`,
  runtime-specific task APIs, or runtime-specific public types.
- Treat `FileCatalog` as every byte input and byte output boundary.
- Treat `CatalogPath` as a validated logical UTF-8 catalog path. It is not an
  operating-system path.

## Skill Routing

- Use `$use-rust-best-practices-core` for any Rust planning, editing, review,
  debugging, or validation task in this crate.
- Add `$use-rust-best-practices-architecture` when changing crate, module,
  public API, parser-dispatch, or test placement.
- Add `$use-rust-best-practices-testing` when changing unit tests,
  integration tests, boundary scanners, doctests, or validation strategy.
- Add `$use-rust-best-practices-async` before changing public async methods,
  work-pool behavior, cancellation, or CPU scheduling.
- Add `$use-rust-best-practices-abstractions` before changing dispatcher
  traits, private backend traits, error shapes, or receiver-method ownership.
- Add `$rust-code-structuring-best-practices` before changing structs, enums,
  builders, repeated parameter groups, Rust item type handling, or standalone
  helper placement.
- Add `$use-rust-best-practices-dependencies-platforms` before changing Cargo
  dependencies, feature policy, or cross-platform catalog behavior.

## Domain Boundaries

- The caller owns filesystem discovery, platform path handling, byte reads,
  byte writes, storage providers, output merging, stale-output cleanup, CLI
  behavior, and persistence of returned catalog entries.
- `conkit-signature` owns catalog validation, parsing bytes into neutral signature
  inventories, comparing inventories, generating report bytes, and Rust
  signature parsing plus Rust YAML handling. Diff receives current and previous
  contract catalogs that the caller has already obtained or decoded.
- Archive transport belongs to the CLI because one archive contains the mixed
  signature-and-sketch catalog. Do not restore an archive codec, format DTO, or
  archive/decode method in this crate.
- The word "file" in this crate means a `FileCatalog` entry made of a
  `CatalogPath` and `Vec<u8>`. It does not mean local filesystem IO.
- Shared modules communicate through `FileCatalog`, `CatalogPath`,
  `SignatureInventory`, and `SignatureContractKitError`.
- Rust-specific production behavior belongs under `languages/rust`. Shared
  modules must not import Rust parser internals or expose Rust YAML DTOs.
- Sketch behavior belongs outside this crate. Do not use `ContractScope` or
  response counters as a reason to add sketch generation or sketch comparison
  logic here.

## Combined Rust YAML Boundary

Treat [the Rust YAML format architecture](ARCHITECTURE.md#rust-yaml-contract-format)
as the semantic source of truth. Operationally:

- Preserve the single combined dialect; add no compatibility path, split
  dialect, or standalone implementation representation.
- Keep one parsed document-set owner and immediate raw-YAML-to-domain
  conversion private under `languages/rust/parser/yaml/`. Do not add a second
  catalog parse or a post-validation YAML semantic mirror.
- Preserve module traversal, global local-owner normalization, strict
  kind-specific fields, ABI-presence semantics, declaration order, linked-item
  extraction, and lossless generated round trips exactly as documented.
- Keep scenario contracts on the current combined format, and add focused
  parser, generation, round-trip, or linked-source tests under
  `languages/rust/parser/yaml/tests/` before changing the format behavior.

## Public Boundary Rules

- `lib.rs` should re-export only the public API and boundary types needed by
  callers.
- Do not publicly export inventory internals, parser internals, transport
  payloads, Rust parser domain structs, Rust canonical forms, or Rust YAML DTOs.
- Public request and response DTOs must use catalog bytes and logical catalog
  names, not `Path`, `PathBuf`, filesystem roots, storage locations, or written
  file lists.
- Do not restore sync public APIs, path-based compatibility shims, shared
  contract repositories, or typed contract-file wrappers.
- Keep public DTOs serializable and comparable when they cross the API
  boundary.

## Internal Ownership Rules

- Keep `api.rs` responsible for public DTOs, the builder, public behavior
  wiring, private backend dispatch, and report byte rendering.
- Keep `error.rs` responsible for `SignatureContractKitError`, contextual
  operation errors, catalog conversion, and language-neutral inventory-error
  conversion.
- Keep `files.rs` responsible for `FileCatalog`, `CatalogPath`, deterministic
  catalog ordering, duplicate rejection, and catalog errors.
- Keep `work.rs` responsible for the runtime-neutral Rayon-backed CPU work
  bridge.
- Keep `inventory.rs` language-neutral. It stores opaque IDs and digests only.
- Keep `languages/mod.rs` as a thin parser surface.
- Keep Rust parsing, Rust identity, Rust type conversion, Rust canonical bytes,
  and Rust YAML read/render behavior under `languages/rust/parser`.
- Keep combined-document parse/render and linked-sketch resolution under
  `languages/rust/parser/yaml/`. Shared modules should only see the neutral
  parser boundary and generated `FileCatalog` bytes.
- Keep Rust parser domain structs under `languages/rust/types`.
- Keep production modules free of test-only API shape changes. Do not add
  `#[cfg(test)]` impls, methods, imports, fields, trait impls, or constructors
  to shared or Rust parser types. Put test conveniences inside the owning
  `#[cfg(test)] mod tests` block or in `conkit-signature/tests`.

## Determinism

- Preserve `BTreeMap`-backed catalog ordering and sorted logical catalog paths.
- Preserve deterministic inventory merge, compare, diff, report, and generated
  YAML behavior.
- Parse and render multi-file Rust inputs in parallel only when results are
  sorted before merging or returning.
- Use explicit dispatcher match arms with receiver-method calls. Do not use
  wildcard arms or macro-generated dispatch for implementation families.

## Validation Defaults

Run the smallest relevant subset while iterating. Before finishing a signature
crate change, prefer:

```bash
cargo fmt --all -- --check
cargo check --locked -p conkit-signature --all-targets --all-features
cargo clippy --locked -p conkit-signature --all-targets --all-features -- -D warnings
cargo test --locked -p conkit-signature --all-targets --all-features
cargo test --locked -p conkit-signature --doc
cargo test --locked --workspace --all-targets
```

For documentation-only changes, still run the planned package checks when the
docs describe current behavior or validation boundaries.

## Boundary Test Expectations

- Keep crate-level integration tests in `conkit-signature/tests`.
- Keep unit tests next to the implementation they exercise.
- Keep structural boundary scanners in `conkit-signature/tests/public_api.rs` when
  they guard public API or production-source invariants.
- Boundary tests should continue rejecting production filesystem IO markers,
  path-based public API markers, stale phase markers, exported Rust YAML DTOs,
  shared-module imports of Rust parser internals, and production-scope
  `#[cfg(test)]` shims. They also require archive transport APIs, modules, and
  dependencies to remain outside `conkit-signature`.
