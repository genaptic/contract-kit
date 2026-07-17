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
conkit generate <all|signatures> --source DIR --contracts DIR \
  [--crate-root CRATE_ID=KIND:RELATIVE_PATH]... [SIGNATURE EXTRACTION] [--adopt-existing]
conkit generate sketches --source DIR --contracts DIR [--adopt-existing]
conkit archive --contracts DIR --archive DIR [--gzip]
conkit diff --contracts DIR --archive FILE
```

`signature` and `sketch` are singular aliases. Check modes are optional and
mutually exclusive. Omission and `--default` map to signature `Default` and
sketch `Enforce`; `--strict` maps to signature `Strict` and sketch `Enforce`;
`--warning` maps to `Warning` in both domains. Sketch normalization and
occurrence remain contract-owned rather than check-mode behavior. Report extensions `.yml`,
`.yaml`, and `.json` are case-insensitive. Omitted and explicit `--gzip` both
select the only archive format. A completed changed diff still exits
successfully.

`SIGNATURE EXTRACTION` is either the default portable syntax mode or:

```text
--signature-extractor compiler --manifest-path FILE
  [--package SPEC] [--lib|--bin NAME]
  [--features FEATURES|--all-features] [--no-default-features]
  [--target TRIPLE]
```

These flags belong only to `all` and `signatures`; compiler selection never
changes check severity semantics. `--target` accepts only a concrete Rust
target triple; the Cargo `host-tuple` alias and custom target paths are rejected
so the literal target recorded in an artifact is deterministic.

## Execution layering

```text
main.rs
  -> one futures_executor::block_on boundary
  -> App
       -> args.rs (grammar)
       -> CommandContext
            -> one shared Arc<rayon::ThreadPool>
            -> CompilerExtractor (Cargo process owner)
            -> SignatureContractKit (owned directly)
            -> SketchAdapter
            -> CatalogReadLimits
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
- `context.rs` owns `SignatureContractKit` and one concrete `CompilerExtractor`
  directly and initializes the
  substantive `SketchAdapter` from `contracts/sketch.rs`. It constructs one
  Rayon pool for both domains, keeps worker count independent from each
  domain's active and pending root-operation budgets, and owns the CLI catalog
  read policy.
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
documents declare `contract_version: 2`, `root`, `files`, `signatures`, and
`sketches`, using `sketches: []` when no sketches are linked.
Signature-bearing documents declare `mode: rust_syntax_v2` or
`mode: rust_compiler_v1`,
`profile: rust_api_v1`, and at least one explicitly typed crate root that
appears in `files`. Each root records its logical crate ID, allowlisted source
path, and `library` or `binary` kind; the domain never recomputes that kind from
the stored path. The CLI header reader rejects missing or unsupported contract
versions, duplicate keys, invalid extraction headers, and unknown top-level
fields. The domain parsers additionally validate their owned signature and
sketch payloads, including rejection of flattened sketch records. There is no
v1 compatibility parser; callers recreate pre-v2 catalogs.

`ContractLayout` in `contracts/layout.rs` aggregates checked documents and
binds them to a `SourceTree` and `ContractsStore`. It resolves each document's
`root` relative to that document and requires the result to equal the selected
`--source`. `files` entries are portable relative Rust paths and exact,
duplicate-free allowlists within each document. Separate documents may share a
source path; the CLI reads the union once and the signature domain projects it
back into document-local inventories. Missing listed files fail; unlisted
source files are never passed to a domain. Domain crates then validate
signature fields, document-local labels, global sketch IDs, links, sketches,
and semantic content.

Within each exact allowlist, `conkit-signature` constructs logical crate/module
identity from explicit roots, inline modules, out-of-line `mod` declarations,
and `#[path]`; the CLI neither infers modules from source filenames nor searches
for undeclared graph inputs. The syntax extractor reports capability warnings
for retained facts that need compiler context and fails closed for unsupported
reachable syntax or invalid graph, owner, attribute, and visibility state.

Compiler extraction is opt-in and Cargo-native. The `compiler` facade exposes
one validated extractor while its `project`, `process`, `probe`, `source`, and
`limits` children own Cargo selection, child execution, probe transport,
source translation, and resource accounting respectively. The facade
resolves exactly one package and library/binary target through locked Cargo
metadata, invokes the pinned `nightly-2026-07-01` Cargo/rustdoc toolchain, and
never invokes the compiler executable directly or auto-installs a missing
toolchain. A private one-shot Cargo-owned rustdoc probe captures
rustdoc-specific `cfg` arguments,
including Cargo configuration and environment flags; `cfg(doc)` is added
explicitly and rustdoc JSON is the authoritative target when the user did not
pass one. The CLI warns first because Cargo runs selected build scripts and
procedural macros unsandboxed with the user's permissions. Each command uses an
isolated target directory, bounded output/artifacts, a timeout, and bounded
best-effort cleanup for its Unix process group or Windows job object. Cleanup
evidence augments rather than replaces the primary failure. After Cargo
starts, artifact-tree inspection polls child completion first and scans a
still-mutating target tree at a separately rate-limited cadence. Only a
descendant `NotFound` is transient during those live scans; root disappearance
and every other walk or metadata error remain fatal. Once Cargo exits and its
pipe readers are drained, one mandatory quiescent scan enforces the exact entry
and byte limits before any artifact is consumed, including after an expected
nonzero probe. The CLI then stream-compares selected source bytes through the
same operation-wide filesystem budget and cancellation probe, without
constructing a second catalog. It decodes a narrow private rustdoc source
projection rather than the semantic `rustdoc_types::Crate`: only envelope,
target/private, root-ID, and item ID/crate/span facts are materialized, while
the original artifact bytes are retained for the signature domain. Source
translation maps only local-crate (`crate_id == 0`) spans whose canonical
physical files are in the exact source catalog. Unbound or unallowlisted spans
are omitted so the signature domain decides whether they are publicly
reachable and therefore
required; spanless compiler-generated items receive typed ownership by the
selected logical crate root, never fabricated zero offsets. It passes raw JSON,
tagged source provenance, schema and semantic extractor versions, normalized
compiler identity and host, target triple, Cargo package/target, sorted
features, and complete cfg values to `conkit-signature`; the domain owns
validation and semantics. Library rustdoc JSON omits private items. Cargo
necessarily includes private items in binary rustdoc JSON, but the signature
domain admits only public children reachable through public modules in either
artifact shape; documentation-hidden public items remain contract-visible.

The CLI uses the maintained semantic YAML parser only for its typed header and
contract-version validation, including before archive publication and after
archive decoding. `contracts/document.rs` owns catalog/document orchestration;
its `header` child owns typed combined-header conversion and its `yaml` child
owns raw and semantic resource analysis. One command-side usage owner
cumulatively bounds documents,
nodes, aliases, and materialized scalar bytes across every physical file; diff
reuses that owner for both current and archived catalogs. The physical stream
scan supplies document indexes and cancellation checkpoints, while the single
all-content typed pass reports alias replay against the same operation budget.
Its internal replay-event cap is twice the remaining semantic-node allowance
because container starts and ends are separate events; the parser's semantic
node counter remains the exact public limit and diagnostic authority.
Signature and sketch payload semantics remain owned by the domain parsers.
Lossless YAML syntax trees remain inside the two domain crates and are created
lazily only for documents whose typed semantics actually change. Exact semantic
no-ops return their original bytes; every changed result is reparsed before it
crosses the domain boundary.

## Check flow

```text
validate disjoint source/contracts/report paths
  -> SourceTree::open
  -> ContractsStore::read
  -> ContractLayout::load and require documents
  -> SourceTree::read_selected for the exact declared files
  -> for rust_compiler_v1, reconcile command and persisted modes and run Cargo
     before any domain await
  -> signature check when selected
  -> sketch check when selected
  -> family report or CLI-owned combined report
  -> persist report before returning a semantic check failure
```

Operational failures do not announce success. Warning mode may retain
diagnostics while returning a passing result.

## Generation flow

For a missing/empty contracts root, the CLI selects all Rust source files,
then accepts exactly one root-level conventional `lib.rs` as a library or
`main.rs` as a binary when that root is unambiguous. Zero, multiple,
nonconventional, and disconnected root layouts require repeatable
`--crate-root CRATE_ID=KIND:RELATIVE_PATH` values for every logical crate.
Explicit `KIND` is `library` or `binary`, and explicit nonconventional root
filenames are valid. The CLI constructs `GenerateTarget::New` for `main.yml`
and computes a portable root relative to `--contracts`. Existing combined
documents supply their own exact file partitions and extraction context and
reject command-line root overrides.

Fresh compiler generation derives its crate identity from the selected Cargo
target unless an explicit root is supplied for validation. Existing compiler
documents require explicit compiler selection and require Cargo root/kind to
match persisted identity. Syntax/compiler mismatches fail before any domain
await. `generate all` consumes one compiler artifact in one signature-domain
generation request, which also returns linked-sketch seeds from that same
parsed projection.

```text
generate signatures:
  signature.generate(scope: Signatures)
    -> replace signatures
    -> preserve valid sketch links and records

generate sketches:
  require existing combined documents
    -> signature.resolve_sketches
    -> sketch.generate(FullRefresh, exact seeds)
    -> refresh only linked code

generate all:
  signature.generate(scope: All)
    -> stale signature + linked sketch cleanup is allowed
    -> return surviving linked-item seeds from the same parsed projection
    -> sketch.generate(FullRefresh, exact surviving seeds)

all targets:
  one baseline-bound GeneratedContracts result
    -> ContractsStore reconciliation and persistence
```

A fresh `generate all` has zero sketches. Generation never invents links.
Stable structural items preserve user labels. Signature-only generation rejects
an update that would orphan a preserved sketch; all-family generation may
remove the stale signature and its linked record together. CLI summaries retain
the domains' cohesive counts: signature document/semantic/byte-change totals
and sketch linked/refreshed/exact-change/changed-document totals. Partial sketch
refresh remains a library/editor capability and is not exposed as CLI grammar.

Diff output derives changed state from entries, identifies each domain's digest
version and current contract digest, categorizes signature changes, and prints
sketch field flags plus old/current document, source, policy, and code-digest
snapshots. Document relocation remains locator metadata rather than a semantic
change, while a relocated semantic edit retains both locators.

`GeneratedContracts` immutably pairs the completed document catalog with the
exact non-metadata baseline used to compute it. Every signature/sketch await
finishes before `ContractsStore::write_generated` acquires the generation
lock, so a lock never crosses an await and cancellation before completed domain
work publishes no generated files. `ApplicationCancellation` owns the one
process signal registration and races command execution through an atomic flag
plus executor waker. Compiler extraction observes the same flag and terminates
and reaps its Cargo process group or Windows job. If a synchronous persistence
step completes during its final poll, that completed result wins instead of
returning a false cancellation after the commit.

## Catalog and ownership boundary

The catalog facade exposes capabilities instead of a generic read/write root:

`CatalogReadLimits` defaults to 10,000 participating entries, 512 MiB per
CLI invocation, and 64 MiB per file, matching the domain catalog defaults while
remaining a nominal CLI filesystem policy. Each command carries one mutable
budget through every catalog read it performs. Generation therefore charges
initial contract/source reads, interrupted-generation recovery, source snapshot
revalidation, and reconciliation rereads to the same operation-wide ledger;
archive and diff likewise retain one ledger for all of their own reads. Focused
test helpers start an isolated ledger only when they intentionally model a new
invocation. The resulting in-memory catalogs are validated again by the
selected domains.

Diff charges three distinct input layers to that one ledger: current contract
files, the verified physical gzip entry and its actual compressed bytes, and
each decoded archived contract entry. The independent 64 MiB compressed-wire
ceiling remains archive policy; catalog entry, per-file, and aggregate limits
are enforced before decoding without charging the physical gzip twice.

Compensating rollback cleanup is the only exception: it uses a fresh,
independently bounded, cancellation-neutral ledger so an exhausted or canceled
forward operation cannot strand digest-bound reservation markers. That cleanup
ledger is unavailable to normal command progress and is not a general budget
reset.

- `catalog/path.rs` owns `PathRole`, `ResolvedPath`, portable logical identity,
  containment, overlap, relative paths, filesystem-to-catalog mapping, and the
  verified no-follow regular-descendant open shared by catalog readers.
- `catalog/source.rs` owns `SourceTree`, deterministic Rust traversal, and
  exact allowlisted reads through verified handles.
- `catalog/store.rs` owns `ContractsStore`, complete non-metadata contract
  reads through verified handles, reserved-namespace inspection, generated
  output mapping, and individually atomic file writes. It also owns
  `ExistingOutputPolicy`, baseline-bound `GeneratedContracts`, and
  `GenerationReceipt`.
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
3. Every participating source or contract path is component-validated,
   containment-proved, and opened once with final-component symlink/reparse
   following disabled. Its components and containment are revalidated, the
   regular-file handle is compared with the current path identity, and bytes
   are read from that same handle under entry, per-file, and aggregate byte
   budgets.
4. `ContractsStore` validates the reserved metadata namespace before walking
   the complete contracts catalog. Walking follows no links, excludes only
   recognized metadata, and applies the same verified-handle bounded-read
   policy before allocating each participating entry. Metadata size is an
   early rejection; the opened handle is still read through `limit + 1`
   evidence because a file may grow.
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

Each generated-file replacement observes cancellation through the checkpoint
immediately before its destination rename. That rename is the publication
boundary: once it succeeds, parent-directory synchronization alone determines
success or failure, and a later cancellation request cannot report already
published bytes as uncommitted.

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

`archive/source.rs` opens an existing archive once with atomic final-component
no-follow behavior, rejects symlinks and non-regular entries, and enforces the
compressed-size limit while reading from that same handle. It reserves the
physical archive as one participating catalog entry, preflights opened-handle
metadata, reads to the tighter archive or catalog evidence boundary, and
commits actual compressed bytes before decoding. Metadata size is only an early
rejection, so later growth or path replacement cannot cause an unbounded read,
switch the decoded bytes, or bypass the operation-wide catalog budget.

Diff validates paths, reads the current catalog, decodes the archive once,
validates both catalogs through one cumulative CLI YAML budget, then directly
awaits signature diff followed by sketch diff. Formatting and document
relocation are semantic no-ops where the domain model says so. Signature
changes are printed before sketch changes.

## Platform, reports, and output

`platform.rs` owns the shared `conkit` executable name and the single
`PortablePathRules::validate_component` entry point. Portable logical and
generated path components use Windows-safe filename rules on every host;
`platform/windows_names.rs` owns those rules. Actual filesystem component
identity remains target-specific inside `catalog/path.rs`.

`report.rs` owns format inference, rendering, and individually atomic report
replacement. Standalone reports serialize through borrowed domain-owned views;
the CLI's combined report owns only its outer envelope and embeds those views
without recreating signature or sketch report DTOs. `bounded_output.rs`
provides the concrete cancellation-aware byte-ceiling writer reused by archive
and report serialization while each caller retains its own error translation
and publication policy. `output.rs` owns human summaries and writes through
locked `std::io::Write` handles so stdout errors propagate. Neither persistence
model is part of contracts reconciliation.

## Module map

- `main.rs`, `app.rs`, `args.rs`, `context.rs`: process boundary, application
  lifecycle, grammar, and initialized runtime handles.
- `command.rs`: `AppCommand` facade and exhaustive root dispatch.
- `command/{check,generate,archive,diff}.rs`: concrete command workflows.
- `compiler.rs`: facade, top-level concrete Cargo/rustdoc extraction
  orchestration, and final artifact assembly.
- `compiler/extractor.rs`: validated Cargo execution helpers, narrow rustdoc
  source projection, and compiler identity/configuration validation.
- `compiler/probe.rs`: private child/parent rustdoc probe protocol.
- `compiler/limits.rs`: compiler operation limits, deadlines, and resource
  accounting.
- `compiler/process.rs`: process-group/job lifecycle, pipes, deadlines,
  termination, conclusive reap attempts, and bounded cleanup evidence.
- `compiler/project.rs`: Cargo metadata, package, target, feature, and isolated
  project resolution.
- `compiler/source.rs`: batched local-span endpoint resolution and logical
  source-provenance translation.
- `compiler/error.rs`: compiler extraction error enum.
- `contracts.rs`: cross-family target/check-mode routing facade.
- `contracts/extraction.rs`: typed requested-versus-persisted signature
  extraction coordination for check and generation.
- `contracts/document.rs`: catalog selection and physical-document
  orchestration.
- `contracts/document/header.rs`: strict combined-header DTO conversion.
- `contracts/document/yaml.rs`: command-local raw/semantic YAML resource
  analysis and accounting.
- `contracts/layout.rs`: document aggregation, source binding, and generation
  targets.
- `contracts/sketch.rs`: sketch kit ownership and cross-domain catalog/seed
  adaptation.
- `catalog.rs`: filesystem capability facade plus CLI entry/per-file/aggregate
  read limits and per-catalog accounting.
- `catalog/path.rs`: path security and portable logical identity.
- `catalog/source.rs`: source-tree walking and verified selected reads.
- `catalog/store.rs`: contracts reads and generated-file primitives.
- `catalog/ownership.rs`: persisted version-3 ownership model.
- `catalog/reconciliation.rs`: runtime locking, recovery, and mutation safety.
- `archive.rs`: mixed-catalog codec and limits.
- `archive/publication.rs`: no-clobber archive publication.
- `archive/source.rs`: bounded verified archive reads.
- `bounded_output.rs`: concrete cancellation-aware byte-limited writer shared
  only by archive and report encoding.
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

The scheduled compiler-compatibility job in
`.github/workflows/hardening.yml` exercises the pinned nightly through Cargo
against `test/fixtures/compiler-api`, covering macro-generated API, direct and
recursive glob reexports, exact public module declarations, evaluated `cfg`,
and generic alias normalization.
The CLI never installs that toolchain or invokes the compiler executable
directly.
