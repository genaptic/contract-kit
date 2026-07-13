# Conkit Crate Agent Guide

This file layers on top of the repository root `AGENTS.md`. Use it for work in
the `conkit` crate. Read [ARCHITECTURE.md](ARCHITECTURE.md) for the structural
flow and full module map; keep operational rules and validation commands here.

## Crate Role

- Treat `conkit` as the command-line adapter for Contract Kit.
- Keep `conkit` responsible for `clap`, argv parsing, command grammar,
  operating-system paths, directory walking, byte reads, byte writes,
  stdout/stderr, process exit behavior, platform display naming, and local
  archive/report persistence.
- Keep `main.rs` thin. It should map the root app result to `ExitCode` and use
  `futures_executor::block_on` only at the process boundary.
- Preserve the `App` / `CommandContext` / `AppCommand` execution shape.
  `clap` produces typed parser values, `App` initializes runtime context, and
  command implementations adapt parsed values into domain requests.
- Keep command execution routed through the crate-private `AppCommand` trait
  with explicit exhaustive match arms.
- Keep stdout for command results and human summaries. Put diagnostics and
  errors on stderr through the application boundary.

## Skill Routing

- Use `$use-rust-best-practices-core` for any Rust planning, editing, review,
  debugging, or validation task in this crate.
- Add `$use-rust-shell-cli-best-practices` when changing command grammar,
  `clap` parsing, command execution, output behavior, filesystem handling, or
  process boundaries.
- Add `$add-new-cli-command` when adding or changing a `conkit` CLI command.
- Add `$use-rust-best-practices-architecture` when changing crate boundaries,
  module ownership, binary naming, target layout, or test placement.
- Add `$use-rust-best-practices-testing` when changing unit tests,
  integration tests, scenario coverage, or validation strategy.
- Add `$use-rust-best-practices-async` before changing `AppCommand`,
  command dispatch, the single `block_on` boundary, or domain future
  orchestration.
- Add `$use-rust-best-practices-abstractions` before changing the private
  command trait, closed dispatch families, or shared adapter boundaries.
- Add `$rust-code-structuring-best-practices` before changing command structs,
  enums, receiver-method ownership, repeated data groups, or standalone
  helper placement.
- Add `$use-rust-best-practices-dependencies-platforms` before changing CLI
  dependencies, cross-platform path behavior, binary target policy, or release
  naming.

## Workspace Domain Boundaries

Preserve the dependency and ownership boundaries in
[ARCHITECTURE.md](ARCHITECTURE.md). The CLI owns OS/process/persistence/archive
behavior; `conkit-signature` and `conkit-sketch` remain independent semantic
domains over catalog bytes. `CommandContext` owns `SignatureContractKit`
directly. Keep signature-to-sketch catalog conversion and linked-seed
adaptation localized to `contracts/sketch.rs`, with no dependency between the
domains or back into `conkit`.

## Command Boundary Rules

- Keep the public command surface verb-oriented:
  `check`, `generate`, `archive`, and `diff`.
- Keep `check` and `generate` target-oriented:
  `all`, `signatures`, and `sketches`.
- Match `ContractTarget` exhaustively once per check or generate workflow, then
  delegate to concrete signatures, sketches, or all-family receiver methods.
  Do not use optional response matrices, an `includes` helper, or a generic
  family runner.
- Preserve CLI-level `ContractTarget::All` as orchestration across signature
  and sketch families. `generate all` intentionally passes
  `conkit_signature::ContractScope::All` so stale signatures and their linked sketches
  can be removed together; check and signature-only generation use the
  signature scope appropriate to their branch.
- Preserve singular aliases `signature` and `sketch` while keeping plural
  `signatures` and `sketches` visible in help.
- Preserve the optional, mutually exclusive check modes. Omitting a mode and
  passing `--default` both select the domain `Default` mode; `--strict` and
  `--warning` select their corresponding modes.
- Preserve the `generate all` pre-write guarantee: finish signature generation,
  linked-item resolution, and sketch refresh before calling
  `ContractsStore::write_generated`. The baseline-bound `GeneratedContracts`
  value pairs the completed document catalog with the exact non-metadata
  catalog used to compute it. Use
  `ContractsStore` to recover an interrupted updating journal before parsing
  combined documents, without creating metadata for missing or committed
  ownership. After acquiring the generation lock and recovering ownership,
  reconciliation requires the current catalog to match that baseline before
  preflight or mutation. An updating journal stores only its generation and
  complete `before`/`after` ownership catalogs, with no family provenance.
  Preflight reads and hashes each existing requested path once, then only stale
  owned paths absent from `after` once; retain separate commit-time
  revalidation, same-file alias checks, digest-bound reservations, and direct
  root `.yml`/`.yaml` ownership rules.
- Preserve `check all` as CLI-owned orchestration: run signature and sketch
  checks with domain reports disabled, then render one combined CLI report.
- Preserve `archive --gzip` as an optional format selector. Gzip is currently
  the only archive format, so both the omitted and explicit forms select it.
- Keep the versioned mixed-catalog archive codec in `archive.rs`. Decode once
  in the CLI, then pass the same previous catalog to both domain diff APIs.
- Preserve `diff` semantics: a successful comparison exits successfully even
  when the compared contracts changed. A future opt-in flag can add diff-tool
  exit-code behavior.

## Filesystem And Platform Rules

- Use `Path`, `PathBuf`, `OsStr`, and `OsString` for OS-facing paths. Convert
  to UTF-8 strings only at logical catalog boundaries or display boundaries.
- Keep path resolution and containment in `catalog/path.rs`, source traversal
  in `catalog/source.rs`, and contracts-root traversal and atomic file
  primitives in `catalog/store.rs`.
- Keep symlink following explicit. A selected source or contracts root may
  itself be a symlink to a directory, but descendants may not traverse
  symlinks. Selected source files are containment-checked, opened once,
  verified against the current path identity, and read through that opened
  handle.
- Keep portable Windows device-name, reserved-character, C0-control, and
  trailing-space/trailing-period validation in `platform/windows_names.rs` and
  apply it through the single `PortablePathRules::validate_component` entry
  point in `platform.rs` on every host.
- Reject distinct ASCII-case-equivalent logical paths, including case-only
  prior-to-current transitions, before filesystem mutation.
- Preserve binary naming policy:
  - Cargo target name: `conkit`
  - runtime/help display name on every supported host: `conkit`
- Do not use Cargo features to decide OS behavior. Use `cfg(windows)` or
  target configuration only for real platform differences.
- Keep archive file names Windows-safe. The current archive names use Unix
  nanoseconds plus `-archive.gzip` and collision suffixes.
- Open an existing archive with atomic final-component no-follow behavior so
  symlink or reparse-point traversal is refused. Verify any opened handle is a
  regular file and read through that same handle. Treat metadata length only as
  an early rejection, enforce the compressed-byte limit while reading, and
  never reopen the archive path for decoding.
- Do not describe multi-file catalog persistence as one atomic transaction.
  Report and generated-file writes are individually atomic, generation uses a
  locked, digest-backed journal persisted by the ownership model and
  orchestrated by reconciliation, and each logical catalog path is validated
  immediately before its sequential file write. Archive publication fully
  syncs a sibling temporary and then uses a no-clobber hard link. A failed
  publication explicitly removes the temporary and reports a cleanup failure
  rather than relying only on `Drop`.
- Clean only recognized atomic manifest temporaries inside the reserved
  metadata namespace. Never infer or delete an output-directory temporary as
  generated ownership after an abrupt process termination.

## Internal Layering Rules

Preserve the execution direction mapped in
[ARCHITECTURE.md](ARCHITECTURE.md):

- Keep `args.rs` grammar-only and keep `App` limited to parse, initialize, and
  delegate.
- Keep `command.rs` as the exhaustive `AppCommand` facade. Keep check,
  generation, archive, and diff sequencing in `command/check.rs`,
  `command/generate.rs`, `command/archive.rs`, and `command/diff.rs`; these
  modules must not own contract semantics or archive codec details.
- Keep cross-family routing in `contracts.rs`, direct-root document validation
  in `contracts/document.rs`, combined layout/source binding in
  `contracts/layout.rs`, and substantive sketch adaptation in
  `contracts/sketch.rs`. Do not add a signature wrapper around the kit owned by
  `CommandContext`.
- Keep `catalog.rs` as a facade over `catalog/path.rs`, `catalog/source.rs`,
  `catalog/store.rs`, `catalog/ownership.rs`, and
  `catalog/reconciliation.rs`. Persisted version-3 values and intrinsic
  validation belong in ownership; locking, recovery, preflight, reservations,
  mutation, rollback, and final verification belong in reconciliation.
- Keep the mixed-catalog codec in `archive.rs`, publication mechanics in
  `archive/publication.rs`, and verified existing-archive reads in
  `archive/source.rs`.
- Keep fallible human-summary writes in `output.rs`; keep report format
  inference, rendering, and individually atomic replacement in `report.rs`.
- Keep domain adapters limited to request/response and catalog adaptation, with
  no filesystem roots, terminal presentation, or process policy.
- Keep layout, catalog, ownership, report, archive, platform, output, and error
  mechanics in their owning CLI modules; they must not learn signature or
  sketch parsing semantics.

## Validation Defaults

Run the smallest relevant subset while iterating. Before finishing a CLI
behavior change, prefer:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked -p conkit --all-targets
cargo test --locked --workspace --all-targets
```

For documentation-only changes, still run markdown/reference checks and the
smallest relevant Cargo check when the docs describe live behavior.

## Boundary Test Expectations

- Keep unit tests next to the implementation they exercise.
- Keep CLI integration tests under `conkit/tests`.
- Use `assert_cmd` to invoke the compiled `conkit` binary in integration
  tests.
- Keep filesystem tests portable with `assert_fs`; include paths with spaces
  and non-ASCII characters when command behavior touches the filesystem.
- Treat [the scenario authoring guide](../test/scenarios/README.md) as the
  canonical manifest, fixture, evidence, and validation contract. Keep the
  closed machine registry only in
  [`REQUIRED_COVERAGE_KEYS`](tests/support/scenario.rs).
- Keep `conkit/tests/scenarios.rs` responsible for recursively discovered
  product scenarios and their coverage audit. Keep
  `tests/support/scenario.rs` as the registry/facade; place reusable mechanics
  in its `error`, `manifest`, `sandbox`, `steps`, `suite`, and `tree` children.
  Keep `tests/scenario_harness.rs` as the harness-regression facade over its
  `execution`, `filesystem`, `manifest`, and `suite` test children.
- Keep `conkit/tests/cli_help.rs` narrowly focused on the shared `conkit`
  display name and the test-owned process presentation environment; general
  grammar and scenario evidence belong in the checked-in leaves governed by
  the scenario guide.
- While changing manifests or the harness, run all seven focused scenario
  commands maintained in
  [the scenario authoring guide](../test/scenarios/README.md) before the
  workspace gates. Do not duplicate that command list here.
- Keep `conkit/tests/dependency_policy.rs` guarding:
  - the single cross-platform binary target name `conkit`
  - canonical workspace package and dependency names without Cargo aliases
  - the absence of compiler-private production dependencies
  - the absence of production-scope `#[cfg(test)]` shims outside local test
    modules
- Do not invoke the `rustc` executable directly from tests. Use Cargo-level
  validation, package tests, doctests, and scenario runners.
