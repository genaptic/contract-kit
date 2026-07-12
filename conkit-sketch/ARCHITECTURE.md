# conkit-sketch Crate Architecture

`conkit-sketch` is the Contract Kit domain library for language-neutral code snippet
contracts. It is storage-agnostic and byte-in, byte-out: callers provide
logical catalog names plus bytes, and the crate returns diagnostics, reports,
and updated combined-contract bytes in catalogs.

Operational rules and validation commands live in [AGENTS.md](AGENTS.md). This
document owns the structural boundary, public surface, data flows, and module
map.

## External Domain Boundary

The dependency direction is one-way:

```text
embedded caller --------> conkit-sketch
storage adapter --------> conkit-sketch

user and operating system
          |
          v
       conkit -----------> conkit-sketch
          |
          +-------------> conkit-signature

conkit-signature !-> conkit-sketch
conkit-sketch    !-> conkit-signature
conkit-signature !-> conkit
conkit-sketch    !-> conkit
```

`conkit` is one caller, not a required gateway. `conkit-sketch` does not know whether
catalog bytes came from local disk, memory, GitHub, object storage, a database,
an embedded application, or a test fixture.

Callers own:

- filesystem discovery and directory walking
- converting operating-system paths into logical catalog names
- reading and writing bytes through any storage provider
- output roots, merge policy, collision policy, and persistence
- command grammar, terminal output, process exits, and user-facing errors
- cross-domain orchestration between signature and sketch contracts
- any signature-assisted discovery that produces snippet seeds

`conkit-sketch` owns:

- logical catalog path validation and deterministic in-memory catalogs
- sketch YAML parsing and contract validation
- language-neutral source and snippet normalization
- matching, diagnostics, check counts, and check-mode behavior
- semantic sketch catalog diffing
- YAML and JSON report bytes
- linked-sketch refresh inside combined contract documents
- runtime-neutral scheduling of its CPU-bound work

The crate builds only the minimal signature index needed to resolve top-level
signature-owned sketch links: document, label, file, and signature type. A
caller may use signature-derived source spans to construct `SketchSeed` values,
but `conkit-sketch` does not parse Rust or depend on `conkit-signature`.

## Public Surface

`SketchContractKit` is the public behavior handle and is constructed through
`SketchContractKitBuilder`.

Public async operations:

- `check(CheckRequest) -> CheckResponse`
- `generate(GenerateRequest) -> GenerateResponse`
- `diff(DiffRequest) -> DiffResponse`

Public boundary types:

- `FileCatalog`, `CatalogPath`, and `FileCatalogError`
- `SketchContractKitError`
- `CheckRequest`, `CheckResponse`, and `CheckMode`
- `SketchCheckCounts` and `SketchDiagnostic`
- `ReportRequest` and `ReportFormat`
- `GenerateRequest`, `GenerateResponse`, and `SketchSeed`
- `DiffRequest`, `DiffResponse`, and `DiffEntry`
- `WorkOptions` and `WorkParallelism`

Public DTOs use in-memory catalog bytes and logical catalog paths. They do not
contain `Path`, `PathBuf`, filesystem roots, storage-provider handles, or lists
of locally written files. Output-producing operations return `FileCatalog`
values so callers choose where and how to persist them. Requests, responses,
and their public enum fields remain serializable, deserializable, and
comparable at this boundary.

The builder configures the crate-owned CPU pool. `WorkParallelism::RuntimeDefault`
lets Rayon choose its worker count, while `WorkParallelism::Fixed` accepts a
nonzero explicit count.

## Catalog Boundary

`FileCatalog` is a private-map wrapper over deterministic logical path order.
It rejects duplicate paths rather than replacing existing bytes and exposes
only catalog-oriented accessors and iteration.

`CatalogPath` is a UTF-8 logical name, not an operating-system path. It is:

- nonempty and relative
- separated with `/`
- free of backslashes, colons, and NUL bytes
- free of empty, `.`, and `..` components

It serializes as a scalar string and deserializes through `CatalogPath::new`.
That allows a nonempty catalog to round-trip through JSON map keys without
bypassing path validation.

The byte boundary is intentionally broader than UTF-8 source text. Checking
can normalize malformed UTF-8 source lines with a byte-preserving fallback.
Resolved generation seeds carry UTF-8 strings because they replace YAML scalar
content.

## Check Data Flow

```text
CheckRequest
  source FileCatalog
  contract FileCatalog
  mode
  report request
        |
        v
SketchContractDocuments::from_catalog
  -> parse each direct root-level .yaml/.yml document once
  -> retain original selected-document bytes
  -> retain nested and non-YAML entries as passthrough
  -> contracts() validates links and builds SketchContracts
        |
        v
SourceCatalog::from_catalog
  -> retain and normalize only referenced source entries
        |
        v
SketchMatcher
  -> parallel per-sketch comparison
  -> deterministic diagnostic sort
        |
        v
SketchInventoryComparison
  -> counts and mode-dependent passed value
        |
        v
ReportFiles
  -> optional YAML/JSON report FileCatalog
        |
        v
CheckResponse
```

`source_file_count` describes the complete supplied source catalog even though
only referenced source entries are normalized. `contract_file_count` describes
all considered `.yaml` and `.yml` combined documents, including documents with
no linked sketches.

Report generation happens after comparison and serializes only `passed`,
`counts`, and `diagnostics`; it does not recursively serialize `report_files`.

## Contract Format And Parsing

Sketches live inside the canonical combined document shape:

```yaml
root: ../src
files: [lib.rs]
signatures:
  - answer_signature:
      file: lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
sketches:
  - answer_body:
    signature_type: function
    code: |
      pub fn answer() -> u8 {
          42
      }
```

The parser considers direct root-level case-insensitive `.yaml` and `.yml`
catalog entries and ignores nested YAML. Each selected document must contain
exactly `root`, `files`, `signatures`, and `sketches`; use `sketches: []` when
the list is empty. The `version`/`language` shape and other mixed shapes are
rejected. The `conkit-sketch` crate validates the combined facts needed by its
domain:

- `root` is a nonempty user-facing string;
- `files` contains unique validated logical paths and does not overlap another
  parsed document's list;
- top-level signature entries have globally unique nonempty labels, a listed
  `file`, a nonempty `signature_type`, and an optional string `sketch` link;
- flattened sketch entries contain exactly one null-valued ID beside
  `signature_type` and `code`;
- sketch IDs are globally unique and code remains nonempty after normalization;
- every sketch is referenced by exactly one top-level signature in the same
  document, every link resolves, and both signature types match.

The `conkit-signature` crate owns complete validation of all other signature
fields; `conkit-sketch` intentionally does not duplicate a Rust signature
parser. Malformed YAML, unknown root fields, duplicate labels or IDs, invalid
paths, orphan, ambiguous, missing, or cross-document links, kind mismatches,
and empty values fail with `SketchContractKitError` before matching. A missing
target source file or a valid snippet that does not match produces a check
diagnostic instead.

## Normalization And Matching

Contract snippets and selected source files pass through the same
language-neutral normalization kernel:

- valid UTF-8 lines use `char::is_whitespace`, including Unicode whitespace
- malformed UTF-8 lines treat ASCII whitespace as whitespace and preserve all
  other bytes
- consecutive whitespace within a line becomes one ASCII space
- leading and trailing whitespace disappears
- empty normalized lines are removed
- line order and every non-whitespace token byte are preserved

`SketchSnippet` performs this normalization once when a contract is
constructed. Matching and semantic comparison borrow the cached
`NormalizedSnippet` instead of rebuilding it.

Semantic diffing keys entries by sketch ID. For the same ID, comparison uses
the linked logical source file, linked signature label, `signature_type`, and
normalized code. The containing contract document, YAML formatting, YAML
comments, and mapping order are nonsemantic. Comments and other tokens inside
the `code` scalar pass through snippet normalization with the rest of the code,
so they remain semantic.

A sketch checks only the logical source path named by its linked signature. It
passes when its normalized lines occur as one contiguous ordered window in the
normalized source. The source may contain unrelated lines before or after that
window. Changed tokens, missing or reordered lines, or nonempty normalized
lines inserted inside the candidate window fail.

Matching runs in parallel across sketches. Diagnostics are sorted afterward by
sketch ID, optional file, and diagnostic kind so worker scheduling cannot
change returned or rendered results.

Mode behavior is currently:

- `Warning`: retain diagnostics and set `passed` to `true`
- `Default`: fail when diagnostics exist
- `Strict`: fail when diagnostics exist

Each parsed sketch contributes at most one diagnostic. Therefore
`failed_sketch_count` equals `diagnostics.len()`, and `matched_sketch_count`
equals `sketch_count - failed_sketch_count`.

## Generation Data Flow

```text
GenerateRequest
  combined contract FileCatalog
  Vec<SketchSeed>
        |
        v
SketchContractDocuments::from_catalog
  -> one parsed combined-document set
  -> contracts() borrows it for global validation
        |
        v
SketchRefreshSeeds
  -> require one seed per linked sketch
  -> validate document, ID, kind, file, and code
        |
        v
SketchContractDocuments::refresh
  -> render only targeted documents
  -> reuse original bytes for untargeted root documents
  -> append nested and non-YAML passthrough unchanged
        |
        v
GenerateResponse { contract_files, sketch_count }
```

`SketchContractDocuments` is the sole parsed combined-document owner used by
validation and generation, so a refresh does not parse the YAML catalog a
second time.

Each `SketchSeed` carries `contract_file`, `sketch_id`, `signature_type`,
`file`, and exact `code`. This mirrors the runtime-neutral output of a caller's
signature resolver without coupling the crates. Seed IDs must be unique, every
seed must match an existing linked sketch exactly, and every linked sketch must
have one seed. Code must remain nonempty after normalization.

Generation is a combined-document update, not sketch discovery. It changes
only each flattened sketch's `code` scalar. Root values, file allowlists,
signatures, signature-owned links, IDs, and kinds remain intact. Documents
without links, nested YAML entries, and non-YAML catalog entries remain
byte-for-byte unchanged. A
fresh combined document with no explicit sketch links therefore produces zero
sketches rather than creating whole-file sketches.

## Runtime And Dispatch

The public handle is an opaque struct over a private inner enum. The handle and
the local backend payload implement the same private
`SketchContractKitBackend` trait. Dispatch uses explicit exhaustive match arms,
so adding another backend is a compile-time-visible change.

Public methods bridge into the handle's private trait implementation. The
local implementation submits one finite top-level job to `AsyncWorkPool`.
That pool owns a Rayon thread pool and sends the completed result through a
futures oneshot channel. The returned future is not tied to Tokio or another
async runtime, and the crate never calls `block_on` internally.

Within a check job, per-sketch matching may use Rayon parallel iteration.
Deterministic sorting occurs before values cross the public boundary.

## Module Ownership

- `lib.rs`: crate policy, module declarations, and intentional public
  re-exports.
- `api.rs`: public requests/responses, builder, opaque handle, private backend
  dispatch, and top-level check/generate/diff composition.
- `files.rs`: `FileCatalog`, `CatalogPath`, deterministic catalog access, path
  validation, duplicate rejection, and catalog errors.
- `error.rs`: the public builder/check/generate/diff error wrapper and
  contextual internal error construction.
- `id.rs`: the private sketch-ID invariant plus contract and seed
  normalization.
- `work.rs`: `WorkOptions`, `WorkParallelism`, the Rayon pool, and the
  runtime-neutral oneshot result bridge.
- `contract.rs`: the parsed document set, original-byte and passthrough
  preservation, strict validation, minimal signature-link indexing,
  catalog-wide uniqueness, targeted refresh/rendering, flattened sketch
  resolution, and deterministic ID-based semantic diffing.
- `normalize.rs`: valid- and malformed-UTF-8 line normalization plus
  contiguous sequence comparison.
- `matcher.rs`: referenced-source selection, normalized source indexing, and
  parallel per-sketch matching.
- `inventory.rs`: diagnostics, deterministic diagnostic ordering, counts,
  comparison invariants, and check-mode response construction.
- `report.rs`: YAML and JSON report rendering into a returned catalog.
- `generate.rs`: resolved-seed validation and generation orchestration.
- `tests/public_api.rs`: public API, async behavior, serialization, error
  context, generation/check round trips, and source-boundary scanners.

No module in this crate owns filesystem IO, CLI presentation, cross-domain
reports, Rust parsing, or source-span resolution.

## Determinism And Failure Boundaries

Determinism is part of the crate contract:

- catalogs use ordered maps and reject duplicate logical paths
- contract IDs are unique across the complete parsed catalog
- parallel diagnostics are sorted before response construction
- refreshed output files retain deterministic catalog path order
- unaffected documents and non-YAML entries retain their original bytes
- report bytes and refreshed YAML derive from validated ordered values

Recoverable catalog, parse, conversion, render, inventory, and worker failures
return through the public typed error. Match failures remain data in
`SketchDiagnostic` and affect `passed` according to `CheckMode`.

## Tests And Boundary Scanners

Unit tests live beside private implementation details. Public API and
architecture regression tests live under `conkit-sketch/tests`.

Together, unit and public API tests exercise malformed-byte normalization,
catalog serde, async checking, both report formats, linked-sketch refresh,
check-mode behavior, binary catalog entries, refreshed-contract round trips,
semantic added/removed/changed diff entries, and no-op diffs for formatting,
YAML comments, mapping order, and contract-file relocation. Token or comment
changes inside sketch code remain semantic. Public source scanners prevent
domain boundary regressions such as filesystem IO, OS-path DTOs, `clap`, Tokio,
`async_trait`, `conkit_signature::` imports, process exits, and
production-scope `#[cfg(test)]` shims.
