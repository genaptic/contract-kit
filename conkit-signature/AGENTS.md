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
- Treat direct `.await` as the normal executor-neutral integration. A spawned
  task must own its request and the kit or an `Arc` clone; its owning future and
  output must remain `Send + 'static`.
- Keep `WorkerPool`, active-root admission, and pending-root admission as three
  independent budgets. `RuntimeDefault` and `Dedicated` own a local Rayon pool;
  `Shared` reuses a caller-owned `Arc<rayon::ThreadPool>` without selecting an
  async runtime.
- Reject work immediately when active plus pending admission is full. Dropping
  a queued future releases admission without submitting work. Dropping a
  running future sets its cooperative cancellation probe; synchronous domain
  owners check that probe between files, modules, diagnostic batches, and
  rendered documents.
- Document cancellation as cooperative, not thread preemption. Keep pending
  ownership bounded by `max_pending_operations`, promise no FIFO order, and
  return a typed queue-full error when saturated.
- Keep worker completion on the runtime-neutral one-shot bridge, release
  admission and active permits before waking completion, and forward worker
  panics to the polling thread. Because operations only transform owned
  in-memory catalogs, discarded results must not leave partial external side
  effects.
- Use exactly one `AsyncWorkPool::execute` call around each complete public
  operation. Keep the parser synchronous and `Send + Sync`; it must not
  accept the work pool or submit work itself.
- Treat `FileCatalog` as every byte input and byte output boundary.
- Treat `CatalogPath` as a validated logical UTF-8 catalog path. It is not an
  operating-system path.

## Skill Routing

- Use `$use-rust-best-practices-core` for any Rust planning, editing, review,
  debugging, or validation task in this crate.
- Add `$use-rust-best-practices-architecture` when changing crate, module,
  public API, parser ownership, or test placement.
- Add `$use-rust-best-practices-testing` when changing unit tests,
  integration tests, boundary scanners, doctests, or validation strategy.
- Add `$use-rust-best-practices-async` before changing public async methods,
  work-pool behavior, cancellation, or CPU scheduling.
- Add `$use-rust-best-practices-abstractions` before changing concrete parser
  ownership, error shapes, or receiver-method boundaries.
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

- Preserve the single mandatory-v2 combined dialect and nested sketch body;
  add no versionless/v1 compatibility path, split dialect, flattened-sketch
  branch, or standalone implementation representation.
- Keep one typed parsed document-set owner and immediate maintained-YAML-to-
  domain conversion private under `languages/rust/parser/yaml/`. Physical
  document path/index locators disambiguate processing and diagnostics but are
  not semantic identity. Do not add a second catalog parse or a
  post-validation YAML semantic mirror.
- Load the lossless YAML tree only after full document-render inequality is
  known. Preserve original bytes on exact no-ops, edit only affected nodes,
  and semantically reparse every changed result before returning it.
- Preserve explicit crate roots, allowlist-bounded module traversal, lexical
  local-owner resolution, strict kind-specific fields, ABI-presence semantics,
  declaration order, linked-item extraction, and lossless generated round
  trips exactly as documented. Never restore a global bare-name fallback.
- Treat structurally repeatable items that share a crate, logical module,
  item kind, and semantic name as distinct by one-based declaration
  occurrence. A physical source path is locator evidence, not semantic
  identity. Preserve numeric occurrence ordering beyond `#9`, label retention
  by occurrence during regeneration, and occurrence-aware linked-source
  resolution.
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
- Keep `CatalogPath` Serde scalar-only and validate deserialized values through
  `CatalogPath::new`; reject legacy object forms. Serialized `FileCatalog` map
  keys must remain in deterministic logical-path order.
- Do not restore sync public APIs, path-based compatibility shims, shared
  contract repositories, or typed contract-file wrappers.
- Keep public DTOs serializable and comparable when they cross the API
  boundary.

## Internal Ownership Rules

- Keep `api.rs` responsible for public DTOs, the builder, public behavior
  wiring, direct top-level async work submission, the
  complete check and diff workflow owners, report byte rendering, and borrowed
  standalone/embedded report views. Do not expose or copy a second report DTO
  for CLI composition.
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
  synchronous parser execution, and Rust YAML read/render behavior under
  `languages/rust/parser`.
- Keep the one private syntax/compiler backend contract and exhaustive enum
  dispatcher under `languages/rust/parser/backend`; retain concrete adapters,
  receiver-style forwarding, and no trait objects or macro dispatch.
- Keep operation-scoped declaration/implementation collection and capability
  diagnostics in `inventory_collector.rs`; projections and source graphs must
  not snapshot or reinsert those diagnostics.
- Keep combined-document parse/render and linked-sketch resolution under
  `languages/rust/parser/yaml/`. The `input` facade delegates document,
  declaration, member, and metadata wire concerns to its children; the
  `render` facade delegates proposal, lossless-edit, and typed-output concerns
  to its children. Shared modules should only see the neutral parser boundary
  and generated `FileCatalog` bytes.
- Keep compiler-artifact validation, immutable indexing, module traversal,
  declaration/type lowering, and provenance resolution in the focused
  `languages/rust/rustdoc` children. Full rustdoc semantics are decoded once
  here and lowering borrows artifact items while mutable output state remains
  separate.
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
- Keep closed domain-enum match arms explicit and receiver-owned. Do not use
  wildcard arms or macro-generated forwarding for implementation families.

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
- Keep `conkit-signature/tests/public_api.rs` as the one integration-crate root
  over focused support, async-contract, boundary, and workflow children.
- Keep compile-time coverage for `Send` directly borrowed operation futures and
  `Send + 'static` owning task futures and outputs in
  `conkit-signature/tests/public_api/async_contract.rs`.
- Keep deterministic admission, pre-start and post-start cancellation,
  maximum active-root, panic-forwarding, and worker-loss tests in the local
  `work.rs` test module. Coordinate them with channels, manual polling, and
  bounded completion guards rather than sleeps or production test seams.
- Boundary tests should continue rejecting production filesystem IO markers,
  path-based public API markers, stale phase markers, exported Rust YAML DTOs,
  shared-module imports of Rust parser internals, and production-scope
  `#[cfg(test)]` shims. They also require archive transport APIs, modules, and
  dependencies to remain outside `conkit-signature`.
