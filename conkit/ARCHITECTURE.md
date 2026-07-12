# Conkit Crate Architecture

`conkit` is the Contract Kit process and filesystem adapter. The Cargo target is
`conkit`; help displays `conkit` on every supported platform.
Operational rules live in [AGENTS.md](AGENTS.md).

## Boundary

```text
argv + operating system
          |
          v
       conkit ------> conkit-signature
          |
          +--------> conkit-sketch
```

The CLI owns `clap`, OS paths, directory traversal, terminal output, exit
codes, reports, generated-document persistence, ownership metadata, and mixed
catalog archives. `conkit-signature` and `conkit-sketch` own domain semantics
over logical catalog paths and bytes and do not depend on each other. For
linked sketches, `conkit-signature` returns runtime-neutral source seeds;
`conkit` converts them into `conkit-sketch` requests.

The command surface is:

```text
conkit check <all|signatures|sketches> --source DIR --contracts DIR --output FILE [MODE]
conkit generate <all|signatures|sketches> --source DIR --contracts DIR [--adopt-existing]
conkit archive --contracts DIR --archive DIR [--gzip]
conkit diff --contracts DIR --archive FILE
```

`signature` and `sketch` are singular aliases. Check modes are optional and
mutually exclusive; omission and `--default` select default behavior. Report
extensions `.yml`, `.yaml`, and `.json` are case-insensitive. Omitted and
explicit `--gzip` both select the only archive format. A completed changed diff
still exits successfully.

## Execution layering

```text
main.rs
  -> one futures_executor::block_on boundary
  -> App
       -> args.rs (grammar)
       -> CommandContext
            -> SignatureContractKit (owned directly)
            -> SketchAdapter
            -> Output
       -> command.rs (AppCommand facade and exhaustive dispatch)
            -> command/check.rs
            -> command/generate.rs
            -> command/archive.rs
            -> command/diff.rs
                 -> contracts/* (document/layout/sketch adaptation)
                 -> catalog/* (paths, reads, persistence, ownership)
                 -> archive/* / report.rs / output.rs
                 -> conkit-signature + conkit-sketch
```

- `main.rs` owns error printing and `ExitCode`.
- `app.rs` parses once, initializes one context, and dispatches once.
- `args.rs` owns grammar only.
- `command.rs` owns only the `AppCommand` contract and exhaustive root
  dispatcher; its four children own verb-specific sequencing and direct
  `.await` calls.
- `context.rs` owns `SignatureContractKit` directly and initializes the
  substantive `SketchAdapter` from `contracts/sketch.rs`.
- `output.rs` owns successful human summaries; `error.rs` owns CLI failures.

No domain crate receives `Path`/`PathBuf`, terminal state, or process policy.
`contracts/sketch.rs` is the only place that converts between the independent
signature and sketch catalog types or adapts resolved signature seeds.

The `check` and `generate` workflows each match `ContractTarget` exhaustively
once, then delegate to concrete signatures, sketches, or all-family receiver
methods. The all-family branches sequence their dependent domain work
explicitly; routing uses neither optional response accumulators nor a generic
family runner.

## Contract layout boundary

`ContractDocumentPath` in `contracts/document.rs` is the checked identity for
a case-insensitive `.yml`/`.yaml` direct child of the contracts root. The same
module parses only the CLI-owned combined-document header. Canonical combined
documents contain `root`, `files`, `signatures`, and `sketches`, using
`sketches: []` when no sketches are linked. The CLI header reader and the
`conkit-signature` parser tolerate an omitted `sketches` field by treating it
as empty for signature-only reads; `conkit-sketch` requires the field
explicitly. Unknown top-level fields are rejected.

`ContractLayout` in `contracts/layout.rs` aggregates checked documents and
binds them to a `SourceTree` and `ContractsStore`. It resolves each document's
`root` relative to that document and requires the result to equal the selected
`--source`. `files` entries are portable relative Rust paths, exact allowlists,
and disjoint under portable ASCII-case comparison across documents. Missing
listed files fail; unlisted source files are never passed to a domain. Domain
crates then validate signature fields, global labels, links, sketches, and
semantic content.

## Check flow

```text
validate disjoint source/contracts/report paths
  -> SourceTree::open
  -> ContractsStore::read
  -> ContractLayout::load and require documents
  -> SourceTree::read_selected for the exact declared files
  -> signature check when selected
  -> sketch check when selected
  -> family report or CLI-owned combined report
  -> persist report before returning a semantic check failure
```

Operational failures do not announce success. Warning mode may retain
diagnostics while returning a passing result.

## Generation flow

For a missing/empty contracts root, the CLI selects all Rust source files,
constructs `GenerateTarget::New` for `main.yml`, and computes a portable root
relative to `--contracts`. Existing combined documents supply their own exact
file partitions.

```text
generate signatures:
  signature.generate(scope: Signatures)
    -> replace signatures
    -> preserve valid sketch links and records

generate sketches:
  require existing combined documents
    -> signature.resolve_sketches
    -> sketch.generate(exact seeds)
    -> refresh only linked code

generate all:
  signature.generate(scope: All)
    -> stale signature + linked sketch cleanup is allowed
    -> signature.resolve_sketches
    -> sketch.generate(exact surviving seeds)

all targets:
  one baseline-bound GeneratedContracts result
    -> ContractsStore reconciliation and persistence
```

A fresh `generate all` has zero sketches. Generation never invents links.
Stable structural items preserve user labels. Signature-only generation rejects
an update that would orphan a preserved sketch; all-family generation may
remove the stale signature and its linked record together.

`GeneratedContracts` immutably pairs the completed document catalog with the
exact non-metadata baseline used to compute it. Every signature/sketch await
finishes before `ContractsStore::write_generated` acquires the generation
lock, so a lock never crosses an await and cancellation before completed domain
work publishes no generated files.

## Catalog and ownership boundary

The catalog facade exposes capabilities instead of a generic read/write root:

- `catalog/path.rs` owns `PathRole`, `ResolvedPath`, portable logical identity,
  containment, overlap, relative paths, and filesystem-to-catalog mapping.
- `catalog/source.rs` owns `SourceTree`, deterministic Rust traversal, and
  exact allowlisted reads through one verified handle.
- `catalog/store.rs` owns `ContractsStore`, complete non-metadata contract
  reads, reserved-namespace inspection, generated output mapping, and
  individually atomic file writes. It also owns `ExistingOutputPolicy`,
  baseline-bound `GeneratedContracts`, and `GenerationReceipt`.
- `catalog/ownership.rs` owns only persisted version-3 manifest/journal values,
  digests, serialization, and intrinsic path, sorting, generation, and
  transition validation.
- `catalog/reconciliation.rs` owns runtime ownership loading, the generation
  lock, interrupted recovery, preflight, reservations, mutation, rollback, and
  final verification.

Ownership lives at `.contract-kit/generated-files.json`. Each owned combined
document appears once with its SHA-256 digest and must be a direct root-level
`.yml` or `.yaml` document. Every version other than 3 is rejected without
migration. An updating journal stores only its generation and complete
`before` and `after` catalogs; it carries no selected-family provenance.
`--adopt-existing` accepts an unowned document only when its existing bytes
equal the requested generated bytes.

Security-sensitive operations keep this order:

1. Command preparation proves source, contracts, report, and archive paths are
   disjoint as required before any mutation.
2. A selected source or contracts root may itself be a symlink to a directory;
   descendant symlinks are not followed.
3. A selected source path is component-validated, containment-proved, opened
   once, and verified as a regular-file handle. Its components and containment
   are revalidated, the opened handle is compared with the current path
   identity, and bytes are read from that same handle.
4. `ContractsStore` validates the reserved metadata namespace before walking
   the complete contracts catalog. Walking follows no links and excludes only
   recognized metadata.
5. Generation recovers an updating journal before parsing combined documents.
   Probing missing or committed ownership creates no metadata.
6. Generation captures the complete non-metadata baseline, then completes all
   sequential domain awaits without a generation lock.
7. Before writing, reconciliation validates the namespace, resolves the
   contracts root, acquires the lock, removes only recognized abandoned
   manifest temporaries, reloads and recovers ownership, re-reads the complete
   catalog, and requires it to equal the baseline.
8. Reconciliation validates every requested direct-root document and portable
   case identity, then preflights every requested and stale owned destination,
   including same-file alias checks, before mutation.
9. Reconciliation writes the updating journal, reserves missing outputs,
   immediately revalidates expectations, performs individually atomic
   sequential writes/removals, verifies the complete after-state, and writes
   the committed journal.

Generation is not one atomic multi-file transaction. It is a locked,
digest-backed, recoverable journal coordinating individually atomic file
writes. Reservation rollback removes only unchanged digest-bound markers and
preserves combined operation/cleanup errors. Report replacement and archive
publication remain separate persistence mechanisms because their durability
and collision semantics differ.

## Archive and diff flow

`archive.rs` owns the private payload-version-1 codec because archives carry
the complete mixed catalog. Encoding is deterministic JSON inside gzip with
`mtime(0)`. Decoding rejects trailing gzip bytes, invalid paths, duplicates,
unsupported versions, more than 100,000 entries, entries over 16 MiB,
compressed data over 64 MiB, and expanded data over 256 MiB.

`archive/publication.rs` writes, flushes, and syncs a newly created sibling
temporary, then uses a no-clobber hard link to expose the final timestamped
`*-archive.gzip`. Its state enum ensures the open handle is dropped before
publication and that cleanup cannot later delete a recreated temporary.
Publication failures explicitly clean the temporary and surface both the
publication and cleanup errors when cleanup also fails.

`archive/source.rs` opens an existing archive once, rejects symlinks and
non-regular entries, and enforces the compressed-size limit while reading from
that same handle. Metadata size is only an early rejection, so later growth or
path replacement cannot cause an unbounded read or switch the decoded bytes.

Diff validates paths, reads the current catalog, decodes the archive once,
then directly awaits signature diff followed by sketch diff. Formatting and
document relocation are semantic no-ops where the domain model says so.
Signature changes are printed before sketch changes.

## Platform, reports, and output

`platform.rs` owns the shared `conkit` executable name and the single
`PortablePathRules::validate_component` entry point. Portable logical and
generated path components use Windows-safe filename rules on every host;
`platform/windows_names.rs` owns those rules. Actual filesystem component
identity remains target-specific inside `catalog/path.rs`.

`report.rs` owns format inference, rendering, and individually atomic report
replacement. `output.rs` owns human summaries and writes through locked
`std::io::Write` handles so stdout errors propagate. Neither persistence model
is part of contracts reconciliation.

## Module map

- `main.rs`, `app.rs`, `args.rs`, `context.rs`: process boundary, application
  lifecycle, grammar, and initialized runtime handles.
- `command.rs`: `AppCommand` facade and exhaustive root dispatch.
- `command/{check,generate,archive,diff}.rs`: concrete command workflows.
- `contracts.rs`: cross-family target/check-mode routing facade.
- `contracts/document.rs`: `ContractDocumentPath` and combined-header parsing.
- `contracts/layout.rs`: document aggregation, source binding, and generation
  targets.
- `contracts/sketch.rs`: sketch kit ownership and cross-domain catalog/seed
  adaptation.
- `catalog.rs`: filesystem capability facade.
- `catalog/path.rs`: path security and portable logical identity.
- `catalog/source.rs`: source-tree walking and verified selected reads.
- `catalog/store.rs`: contracts reads and generated-file primitives.
- `catalog/ownership.rs`: persisted version-3 ownership model.
- `catalog/reconciliation.rs`: runtime locking, recovery, and mutation safety.
- `archive.rs`: mixed-catalog codec and limits.
- `archive/publication.rs`: no-clobber archive publication.
- `archive/source.rs`: bounded verified archive reads.
- `platform.rs`, `platform/windows_names.rs`: executable identity and portable
  component rules.
- `report.rs`, `output.rs`, `error.rs`: reports, stdout, and CLI errors.

## Tests

Integration tests under `conkit/tests` exercise the compiled binary. The
manifest-aware `tests/support/scenario.rs` file is the required facade and owns
the closed [`REQUIRED_COVERAGE_KEYS`](tests/support/scenario.rs) registry. Its
`error`, `manifest`, `sandbox`, `steps`, `suite`, and `tree` children own the
focused reusable mechanics. `tests/scenario_harness.rs` is the harness-test
facade over `execution`, `filesystem`, `manifest`, and `suite` child modules.
`tests/scenarios.rs` owns recursive product-scenario execution plus the global
coverage audit.

Every checked-in leaf is independently owned and found through recursive
discovery rather than hardcoded registration. The canonical manifest schema,
step grammar, fixtures, evidence rules, and authoring guidance remain only in
the [scenario guide](../test/scenarios/README.md).
