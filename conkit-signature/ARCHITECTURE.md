# conkit-signature Crate Architecture

`conkit-signature` is the Contract Kit domain library for signature contracts. It is a
byte-in, byte-out crate: callers provide logical catalog entries and bytes, and
the crate returns logical catalog entries and bytes.

Operational rules and exact validation commands live in
[AGENTS.md](AGENTS.md). This document owns the structural boundary, public
surface, formats, data flows, and module map.

## External Domain Boundary

The crate is storage-agnostic. It does not know whether bytes came from local
disk, GitHub, S3, a database, memory, or a test fixture.

Callers own:

- filesystem discovery and directory walking
- converting operating-system paths into logical catalog names
- reading source and contract bytes from any backing store
- writing returned report or contract bytes to any backing store
- encoding and decoding cross-domain archive transport
- output merge policy, output collision policy, and stale-output cleanup
- CLI argument parsing, user-facing command shape, terminal output, and exit
  codes
- sketch generation, sketch comparison, and generated-code behavior

`conkit-signature` owns:

- validating logical catalog paths with `CatalogPath`
- storing ordered in-memory byte entries with `FileCatalog`
- parsing source bytes and contract bytes into neutral signature inventories
- comparing source and contract inventories for `check`
- diffing current and previous contract inventories for `diff`
- rendering report bytes for `check`
- generating Rust signature contract YAML bytes for `generate`
- scheduling CPU-heavy parse, render, and compare work without tying callers
  to an async runtime

In the wider `conkit` workspace, `conkit` should adapt command-line paths and
options into `FileCatalog` requests, then persist returned catalogs. The
`conkit-sketch` crate owns sketch-specific behavior. `conkit-signature` should
not absorb either boundary.

## Public Surface

`SignatureContractKit` is the public behavior handle. It is constructed through
`SignatureContractKitBuilder`.

Public async methods:

- `check(CheckRequest) -> CheckResponse`
- `generate(GenerateRequest) -> GenerateResponse`
- `resolve_sketches(ResolveSketchesRequest) -> ResolveSketchesResponse`
- `diff(DiffRequest) -> DiffResponse`

Public boundary types include:

- `FileCatalog`: deterministic in-memory catalog of logical paths to bytes
- `CatalogPath`: validated relative UTF-8 logical path using `/`
- `FileCatalogError`: catalog path and duplicate-entry errors
- `SignatureContractKitError`: crate-level typed error wrapper
- `WorkOptions` and `WorkerPool`: independent worker-pool, active-operation, and pending-operation configuration
- `RustExtractionInput`: the closed syntax/compiler operation selector
- `RustCompilerArtifact`, `RustCompilerCrate`, `CompilerSourcePath`,
  `CompilerSourceProvenance`, and their typed validation failures: the
  versioned compiler-artifact boundary supplied by a filesystem/process-owning
  host
- request and response DTOs in `api.rs`

Public request and response DTOs do not include operating-system paths, storage
provider details, filesystem roots, or written-file lists. Output-producing
operations return a `FileCatalog`; callers decide where those bytes go.
`CheckResponse` also exposes opaque borrowed standalone and embedded report
views implementing `Serialize`; the standalone view preserves the domain's
wire order, while the embedded view lets a caller compose a cross-domain
envelope without recreating or cloning signature report state.

## Catalog Boundary

`FileCatalog` is a private-map wrapper over deterministic logical path order.
It rejects duplicate paths instead of replacing existing bytes and exposes
only catalog-oriented accessors and iteration.

`CatalogPath` is a validated UTF-8 logical name rather than an operating-system
path. Serde encodes it as a scalar string and validates that string through
`CatalogPath::new` during deserialization; legacy object forms are rejected.
This representation allows nonempty catalogs to round-trip through JSON map
keys in deterministic logical-path order without bypassing path validation.

## Data Flows

`generate`:

```text
source FileCatalog
  + RustExtractionInput::{Syntax | Compiler(artifact)}
  -> SignatureParser
  -> allowlist-bounded syntax graph or validated rustdoc extraction
  -> Rust YAML renderer
  -> GenerateResponse { contract_files, counts, all-scope linked seeds }
```

`check`:

```text
source FileCatalog -> SignatureParser -> source SignatureInventory
contract FileCatalog -> SignatureParser -> contract SignatureInventory
source inventory + contract inventory -> comparison -> CheckResponse
optional report request -> report FileCatalog
```

`diff`:

```text
current contract FileCatalog -> SignatureParser -> current SignatureInventory
previous contract FileCatalog -> SignatureParser -> previous SignatureInventory
current inventory + previous inventory -> DiffResponse
```

Archive decoding is deliberately outside signature comparison so a caller can
decode one mixed catalog and pass the same previous bytes to independent
contract families.

Only catalog, inventory, and signature-domain error values cross the
shared/language boundary. Rust parser and renderer implementation types stay
inside the Rust subtree.

## Rust YAML Contract Format

Rust signature contract YAML is a crate-owned byte format. It is not a public
Rust DTO surface. Public APIs continue to accept and return `FileCatalog`
values; callers decide where those bytes come from and where generated bytes
are written.

`generate` renders the canonical combined YAML format:

- mandatory `contract_version: 2`, document-level `root`, and exact portable
  `files` allowlist
- exactly one closed extraction mode, `rust_syntax_v2` or `rust_compiler_v1`,
  plus `profile: rust_api_v1`, with a nonempty set of explicit logical crate
  IDs, allowlisted root paths, and `library` or `binary` target kinds for every
  signature-bearing document
- for `rust_compiler_v1`, one complete compiler mapping containing the
  artifact/rustdoc schema versions, compiler/extractor versions, target triple,
  Cargo package and selected target, sorted feature and cfg values, and true
  macro-expansion/name-resolution capabilities
- one-entry user-named `signatures` maps with `signature_type`
- trait associated constants, associated types, and methods in one ordered
  `items` sequence; implementation blocks remain descriptor-scoped beneath
  their resolved local owner and contain their own canonical `items` sequence
- module signatures with semantic visibility and attributes, `inline: true`
  for inline bodies, and optional `path` text for `#[path]` declarations
- signature-to-sketch links and nested one-entry `sketches` maps whose bodies
  repeat the linked file, signature label, signature type, matching policy,
  and code; generated documents always emit `sketches`, using `sketches: []`
  when empty
- user-facing text for visibility, generics, `where` predicates, types,
  callable qualifiers, ABI, variadic parameters, receivers, fields, variants,
  discriminants, trait supertraits, implementation polarity, implementation
  qualifiers, statics, macros, and type aliases

The extraction mode and profile form a closed v2 pair; unknown values fail
without fallback. Crate target kind and logical module identity are never
inferred from a physical basename. Every source file decoded, parsed, traversed,
resolved, projected, or used for label allocation must first belong to that
document's exact allowlist. Compiler artifacts are likewise source-mapped only
to allowlisted catalog entries, and their selected crate root and kind must
agree with the document metadata.

`check` parses user-provided Rust contract YAML through the same combined
dialect with the maintained semantic parser. Raw, kind-specific Serde values
convert immediately into the single typed document and Rust domain values;
document-render equality is intentionally distinct from API-digest
canonicalization. Wire-only DTOs stay private under
`languages/rust/parser/yaml/` and must not leak through public exports, shared
modules, fixtures, or CLI behavior.

Each operation parses every participating document exactly once into a sorted
`RustContractDocuments` set. File allowlists are duplicate-free within each
document but may overlap across documents; participating source bytes are
parsed once and projected into document-local inventories and label plans.
Signature labels are unique within a document and may repeat across document-
local label plans; sketch IDs remain catalog-wide identities. Checking borrows
the source allowlist union from that set and then consumes the same parsed
documents into the neutral inventory. Linked-sketch resolution uses the same
document-set owner.

Only direct children of the contract catalog with case-insensitive `.yml` or
`.yaml` extensions participate. Every physical document is indexed and parsed
strictly; missing or unsupported `contract_version`, `version`/`language`,
duplicate keys, unknown fields, flattened or mixed shapes, duplicate labels
within one document, duplicate global sketch IDs, duplicate files within one
document, unlisted signature files, and invalid links are errors. Nested YAML
files are ignored. Canonical documents always include the `sketches` field.

One operation-owned YAML usage counter spans every participating physical file,
both sides of a diff, and every changed-document verification reparse. Raw
nodes and scalar bytes are charged once during resource preflight; the final
typed-parser budget report then charges only alias-replayed nodes and scalar
bytes beyond that raw report. The typed parser receives only the operation's
remaining replay capacity while still being allowed to consume the current
file's already-recorded raw events. Its internal replay-event cap is twice the
remaining node count because a balanced container contributes one semantic
node but both a start and an end event; the semantic node budget remains the
exact public authority rather than exposing a second replay-event limit. A
replay-cap diagnostic converts events back to the proven lower bound of one
node per two events before composing operation-wide evidence.

Existing generation compares the complete proposed typed document before
loading the lossless syntax tree. A semantic no-op returns the original bytes.
For a real change, the editor replaces only affected signature nodes, preserves
the sketch section, reparses the edited bytes with the semantic parser, and
returns them only when the reparsed document equals the proposal.

Returned output and generated editing scratch have independent nominal byte
budgets. Scratch accounting measures simultaneously retained logical text,
releases reservations on scope exit, and reports the first crossing before
allocation. A changed physical document owns its previews and replacements;
the editor streams untouched spans and edits into the returned-output writer,
then releases all document-local scratch before processing the next document.
Removal-only and semantic no-op paths allocate no scratch, and no complete
edited-source clone is constructed. Returned output and live scratch each
default to 512 MiB and may be configured independently.

Each changed physical document independently selects LF, CRLF, or CR from its
first retained line break, with LF as the no-break fallback. Retained source
spans bypass conversion and remain byte-exact; only generated preview and
replacement text adopts the document presentation. Source-column and removal
range discovery recognize all three line-break forms, including CR-only YAML,
without allocating a converted replacement buffer.

The combined dialect must be lossless for every field represented by the Rust
parser domain model. Generated documents should parse back to the same
inventory digest for all Rust item kinds that `conkit-signature` supports. When the
domain model learns a new Rust signature fact, the combined renderer,
parser, and round-trip tests in `languages/rust/parser/yaml/tests/` should change
together.

Module declarations are signatures and independently provide source-graph
edges. Their canonical shape retains visibility, ordered semantic attributes,
inline versus out-of-line form, and an optional raw `#[path]` override. The
source graph separately follows inline modules, allowlisted out-of-line
modules, and `#[path]` edges from explicit crate roots. Free items carry their
canonical crate and `module_path`; the same physical file may participate in
more than one logical crate/module context. Structurally repeatable items that
share a crate, module, item kind, and semantic name use their one-based
declaration occurrence as structural identity. YAML signature-vector order
reconstructs the same identities. Neutral linked-sketch resolution uses that
identity and retained exact source provenance to return Rust item text. A
compiler-generated origin remains valid signature/file identity but cannot
serve a linked sketch that requests exact source bytes.

The graph never falls back to a filename-derived effective module. An
out-of-line edge whose target is absent from `files`, an ambiguous conventional
target, a cycle, a multiply claimed logical edge, or a conditional `#[path]`
that syntax extraction cannot choose is a typed failure with source evidence.
Ordinary conditional module syntax that does not change the selected target is
retained and reported as capability evidence.

Implementation ownership is resolved in a second pass against the logical
module graph and explicit import bindings. Resolution starts in the impl's
lexical module, understands supported `self`, `super`, and `crate` paths, and
never falls back to a global bare-name search. Supported implementations
normalize to a source-declared local struct, enum, union, or type alias; their
physical block location and equivalent owner spelling are nonsemantic.
Ambiguous, unresolved, external, or unsupported owners fail with source-site
and canonical-candidate evidence. Owner applications must be bare or apply the
implementation's declared type, lifetime, and const parameters unchanged and
in order; specialized, reordered, nested, and qualifier-segment applications
are rejected before normalization. Blocks with the same canonical descriptor
merge into one deterministically sorted associated-item set.

Syntax and compiler extraction use the same parsed-entry merger. Compiler impl
IDs are registered once, converted in sorted order after module containment,
grouped by canonical owner and descriptor, assigned deterministic exact-source
or compiler-generated provenance, and charged against extraction limits only
after the merged entry is finalized. Contract-origin entries never enter
extractor-side merging.

Visibility is stored semantically as public, crate-wide, a canonical ancestor
module, or private. Inherited visibility, `pub(self)`, and `pub(in self)` are
equivalent; crate spellings are equivalent; and valid relative restrictions
resolve against the declaring module. Invalid non-ancestor restrictions fail
with the requested spelling and resolved target evidence. A function named
`main` is an ordinary function and keeps its real visibility in every crate and
module context.

Callable parameter patterns remain rendering metadata so generated YAML can
preserve binding shape. `rust_api_v1` canonical bytes include receiver form,
parameter order and types, qualifiers, ABI, variadic state, generics, and
constraints, but exclude ordinary binding names, destructuring patterns, and a
variadic binding name.

## Extraction Capability Boundary

`rust_syntax_v2` parses allowlisted source with `syn`; it does not run Cargo or
validate a complete crate, expand macros, evaluate `cfg`/`cfg_attr`, resolve a
reexport target, normalize aliases or type paths, or determine compiler-level
API reachability. Modeled syntax is retained in canonical contracts. Facts that
need those compiler capabilities produce sorted, deduplicated warnings rather
than silent confidence.

Retained conditional syntax is accepted only after its predicate satisfies the
Rust Reference grammar: identifiers and string-valued options, `true`/`false`,
and iteratively validated `all`, `any`, and single-argument `not` forms. A valid
zero-attribute `cfg_attr(predicate,)` is a source-level semantic no-op and is
omitted; canonical YAML continues to reject an encoded empty attribute list.
The implicit Rust representation is canonical: explicit `repr(Rust)` is
omitted while any alignment, packing, or integer modifiers remain represented.

Default checking permits warning-only results but fails inventory or extraction
errors. Strict checking requires no diagnostics. Warning mode retains all
diagnostics while passing the completed check. Unsupported reachable item,
associated-item, foreign-item, macro, attribute, graph, owner, or visibility
state fails closed instead of being discarded. This syntax capability boundary
is the documented limit of `rust_syntax_v2`, not a silent claim of compiler
equivalence.

`rust_compiler_v1` consumes a host-produced, schema-versioned rustdoc JSON
artifact and source-provenance map. The crate validates artifact version,
rustdoc format, compiler/extractor metadata, Cargo package/target identity,
target triple, sorted feature/cfg context, public reachability, and every source
mapping before conversion. Exact provenance must correspond to a rustdoc span,
its logical filename, and the same one-indexed Unicode-scalar line/column range;
compiler-generated provenance is valid only when rustdoc has no span. A missing
mapping is tolerated only for an unreachable item. Validation groups admitted
byte endpoints by exact source file, verifies UTF-8 boundaries, and resolves
all requested line/scalar coordinates in one cooperatively checkpointed source
pass. Auxiliary memory therefore scales with admitted mappings rather than
every source scalar. It then lowers
compiler-resolved declarations into the same
`RustDeclaration`, document projection, inventory, rendering, and digest graph
used by syntax extraction. It performs no filesystem discovery, Cargo command,
toolchain installation, or subprocess work. Unsupported or incomplete rustdoc
facts fail closed; there is no implicit syntax fallback.

Compiler lowering follows rustdoc's already evaluated reachable item graph, so
cfg-disabled declarations do not enter the projection and spanless
macro-generated public declarations remain modeled. Exact-span declarations
retain their logical byte range. Compiler-generated declarations retain typed
crate-root ownership for file identity and signatures, but linked-sketch
resolution fails with an actionable no-exact-source error for those items.
Public module containment is lowered with an iterative depth-first work stack.
An active module ID is a cycle, a completed ID under another parent is multiple
containment, and exports are emitted only after admitted children. Malformed
artifacts therefore fail without recursive stack growth while preserving
canonical module paths and postorder export behavior. Every admitted non-root
public module is also emitted as a first-class module declaration after
containment validation. Rustdoc does not retain inline versus out-of-line
shape, so compiler lowering obtains that shape and any raw `#[path]` override
only from exact source provenance. Because rustdoc may report only the module
header, the validated header range is resolved against the cached parsed source
to exactly one complete `ItemMod`; the complete declaration span becomes the
entry provenance. Missing, ambiguous, or spanless module shape fails closed
instead of being omitted or guessed. The compiler-generated crate root remains
containment only and is never rendered as a `mod` declaration.
Direct public reexports resolve to canonical target paths. Glob reexports
expand the effective public item set exposed in the rustdoc index, including
recursive public uses, deterministic identical-target deduplication,
explicit-over-glob precedence, cycle termination, and conflicting-name errors.
An external module represented only by an `ItemSummary` has no enumerable
target set in this single-crate artifact and therefore fails explicitly;
externally owned module items are supported when the artifact actually exposes
their complete index entries.

Resolved type-alias applications normalize recursively to their underlying
compiler types while alias declarations remain first-class signatures. The
normalizer substitutes ordered lifetime, type, and const arguments, fills type
and const defaults, handles nested aliases, and rejects kind/arity mismatches,
associated-item constraints on alias applications, and exact alias cycles.
Other rustdoc item or type facts without a lossless `rust_api_v1`
representation remain typed extraction errors rather than silent omissions.

Representation hints are canonicalized at the shared semantic owner.
Alignment-one packing therefore has one value and renders as bare `packed`;
source or compiler spellings equivalent to `packed(1)` compare and digest
identically while larger packing values remain explicit.

The YAML input boundary accepts exactly `signature_type` and immediately
converts raw Serde values into a closed kind-specific body. Known fields on the
wrong kind are errors even when empty. Named aggregate fields retain source
declaration order. ABI field presence is semantic: omission is implicit Rust,
`abi: extern` is unnamed extern, and values such as `Rust` or `C` are explicit
ABI names.

## Shared Module Ownership

- `lib.rs`: declares crate modules and re-exports only the public API and
  boundary types.
- `api.rs`: owns request and response DTOs, the builder, public behavior
  wiring, direct top-level async work submission, complete check and
  current-then-previous diff workflows, report byte rendering, and borrowed
  standalone/embedded report views. The views serialize domain state without
  widening the public DTO surface or copying report collections.
- `error.rs`: owns `SignatureContractKitError`, contextual operation errors,
  and conversions from catalog and language-neutral inventory errors.
- `files.rs`: owns `FileCatalog`, `CatalogPath`, path validation,
  deterministic iteration, duplicate rejection, and catalog errors.
- `work.rs`: owns `AsyncWorkPool`, `WorkOptions`, `WorkerPool`, and cooperative
  cancellation. `AsyncWorkPool::execute` rejects a full admission queue,
  asynchronously acquires an active-operation permit, and completes through a
  runtime-neutral one-shot channel. The worker observes cancellation between
  cohesive work groups, captures panics, and forwards them to resume unwinding
  on the polling thread.
- `inventory.rs`: owns language-neutral opaque signature IDs, digests,
  inventory merge, inventory compare, and inventory diff.
- `languages/mod.rs`: exposes the concrete private `SignatureParser` owner to
  shared API code.

Shared modules must remain language-neutral. They may call the concrete private
parser owner, but they must not import
Rust YAML DTOs, Rust canonical forms, or Rust parser domain structs.
Parser execution remains synchronous and `Send + Sync` inside one admitted root
operation; parser implementations do not accept or submit to the work pool.

## Rust Module Ownership

- `languages/rust/mod.rs`: declares the Rust parser, parsed-source cache, and
  Rust parser domain type subtree.
- `languages/rust/source.rs`: owns the allowlist-filtered physical-source
  cache, one-time UTF-8/`syn` parsing, and exact source spans.
- `languages/rust/rustdoc.rs`: compiler-artifact facade and extraction entry.
- `languages/rust/rustdoc/artifact.rs`: versioned artifact/schema validation.
- `languages/rust/rustdoc/index.rs`: immutable rustdoc graph and item lookup.
- `languages/rust/rustdoc/modules.rs`: iterative containment and public export
  traversal.
- `languages/rust/rustdoc/declarations.rs`: borrowed declaration and
  associated-item lowering.
- `languages/rust/rustdoc/types.rs`: borrowed type and alias normalization.
- `languages/rust/rustdoc/provenance.rs`: exact/generated source validation and
  batched coordinate resolution.
- `languages/rust/rustdoc/tests/*`: artifact, module, declaration, type, and
  provenance unit tests beside their private owners.
- `languages/rust/parser/mod.rs`: owns the concrete parser implementation,
  target-first source selection, document-local projections, parsed Rust
  entries, Rust canonical byte conversion, and conversion into neutral
  inventory entries.
- `languages/rust/parser/backend.rs` plus `backend/{syntax,compiler}.rs`: one
  private closed extraction-backend trait, two concrete implementations, and
  one explicit exhaustive enum dispatcher for check, generation, and linked
  sketch resolution.
- `languages/rust/parser/source_graph.rs`: owns explicit crate roots and the
  allowlist-bounded logical module graph.
- `languages/rust/parser/symbol_table.rs`: owns lexical declarations/imports
  and fail-closed implementation-owner resolution.
- `languages/rust/parser/inventory_collector.rs`: one receiver owning the
  two-pass graph walk, implementation merging, identity allocation, and the
  operation-scoped capability collector.
- `languages/rust/parser/item_converter.rs`: converts `syn` items into Rust
  parser domain structs.
- `languages/rust/parser/signature_id.rs`: owns Rust item identity rendering
  and conversion into opaque shared `SignatureId` values.
- `languages/rust/parser/type_converter.rs`: owns Rust type text and `syn`
  type conversion.
- `languages/rust/parser/visibility_converter.rs`: owns Rust visibility
  conversion.
- `languages/rust/parser/yaml/mod.rs`: private combined-YAML façade.
- `languages/rust/parser/yaml/document.rs`: one parsed document set,
  deterministic ordering, and global validation.
- `languages/rust/parser/yaml/input.rs`: strict raw Serde facade and direct
  YAML-to-domain decoder.
- `languages/rust/parser/yaml/input/{contract,declaration,member,metadata}.rs`:
  document shape, kind-specific declaration bodies, nested members, and shared
  wire metadata respectively.
- `languages/rust/parser/yaml/render.rs`: deterministic render facade.
- `languages/rust/parser/yaml/render/proposal.rs`: source grouping,
  collision-free labels, and typed document proposals.
- `languages/rust/parser/yaml/render/lossless.rs`: affected-node discovery and
  verified streaming edits.
- `languages/rust/parser/yaml/render/output.rs`: canonical typed wire
  serialization and output accounting.
- `languages/rust/parser/yaml/sketch.rs`: linked Rust-item source resolution.
- `languages/rust/parser/yaml/type_text.rs`: wire/domain Rust type-text
  conversion.
- `languages/rust/parser/yaml/tests/*`: document, generation, round-trip, and
  linked-source tests.
- `languages/rust/types/*`: owns Rust parser domain structs and canonical
  forms for exhaustive declarations, typed attributes, associated and foreign
  items, functions, structs, enums, traits, implementations, unions, statics,
  modules, macros, aliases, callables, primitive types, and base item metadata.

Rust-specific identity, type conversion, YAML DTOs, canonical forms, and parser
domain structs belong under `languages/rust`. Shared inventory stores opaque
IDs and digest bytes, not Rust-specific structures.

## Determinism And Work

Determinism is part of the crate contract:

- Source parse partials, contract parse partials, generated contract files,
  reports, inventory comparisons, and inventory diffs must be stable across
  runs.
- Parallel parsing and rendering must sort by logical `CatalogPath` before
  merging or returning results.
- Generated report bytes must not recursively include `report_files`.

Public async methods schedule CPU work, not file loading. `FileCatalog` already
contains bytes in memory, and operation futures remain executor-neutral. Direct
`.await` is the normal integration. A spawned task owns its request and the kit
or an `Arc` clone; the resulting owning future and its output satisfy
`Send + 'static`.

Every operation must obtain admission without blocking its executor before it
submits one complete root workflow to the configured Rayon pool. `WorkerPool`
selects a runtime-default, dedicated, or caller-shared pool;
`max_in_flight_operations` independently bounds active root workflows and
`max_pending_operations` bounds admitted-but-waiting workflows. Nested Rayon
work remains part of its active root workflow.

The admitted signature workflows are complete domain operations:

- `check` parses the contract and source inventories, compares them, converts
  the response, and renders any requested report in one worker operation.
- `generate` parses source and renders the generated documents in one worker
  operation. All-scope generation resolves every surviving linked sketch from
  that same parsed projection and returns the seeds beside the documents.
- `resolve_sketches` performs validation and source resolution in one worker
  operation.
- `diff` parses current and then previous contracts sequentially, computes the
  semantic diff, and converts the response in one worker operation.

Dropping a future before execution releases its admission and active-operation
permits. Dropping a started future requests cooperative cancellation, which is
observed between parsing, module, inventory, comparison, diagnostic, and render
groups. The scheduler never terminates a worker thread unsafely. Admission
bounds both active workflows and pending owned catalogs, and queue saturation
returns a typed error. No FIFO scheduling guarantee is made.

The work bridge completes through a runtime-neutral one-shot channel. Worker
panics resume unwinding on the polling thread, while recoverable failures remain
typed domain errors. Operations only transform owned in-memory catalogs, so a
discarded result cannot leave partial external side effects.

## Concrete Parser And Backend Rules

The opaque public handle directly owns one concrete `SignatureParser` and one
work pool. Public receiver methods submit complete operations; the parser owns
the shared limits used by those operations and remains synchronous inside the
admitted worker job. Rust extraction is the one justified closed implementation
family: a uniquely named private trait is implemented by concrete syntax and
compiler owners, and one private data-carrying enum forwards through explicit
exhaustive receiver-style matches. It uses no trait objects, macro generation,
`async_trait`, or universal intermediate Rust AST. The syntax and rustdoc
adapters remain concrete and separate after their results enter the shared
parsed-declaration graph.

## Tests And Boundary Policy

Unit tests live next to the implementation they exercise. Public API and
boundary tests live under `conkit-signature/tests`.

`tests/public_api.rs` is the single integration-crate root over focused
`public_api/{support,async_contract,boundaries,workflows}.rs` children. Those
children contain compile-time contracts for the public builder, kit, directly
borrowed futures, and owning spawned-task shapes plus the representative
executor-neutral boundary workflows. The local `work.rs` tests
cover bounded admission, cancellation before admission, best-effort queued
cancellation, run-to-completion after start, worker-thread execution, panic
forwarding, and worker-channel loss. They coordinate with explicit channels,
manual polling, and bounded completion guards rather than sleeps or production
test seams.

The `tests/public_api/boundaries.rs` child protects the architecture through
behavioral public-surface tests for nominal request, response, limit, report,
serde, and error contracts. Workspace source-policy enforcement—including
filesystem/process ownership, production `#[cfg(test)]` shims, dependency
boundaries, and private implementation topology—is centralized in
`conkit/tests/dependency_policy.rs` rather than duplicated through substring
scans in this crate's public integration suite.

Production modules must have the same type, method, trait-impl, and import
shape in test and non-test builds. Test-only conveniences belong inside
`#[cfg(test)] mod tests` blocks or under crate-level `tests/`, not as
`#[cfg(test)]` items interwoven through production impls.

Combined Rust YAML behavior is tested under
`languages/rust/parser/yaml/tests/`. These unit tests cover rejection of the
non-combined `version`/`language` shape, parsing of hand-authored canonical
combined documents, document-local file/label validation, catalog-wide sketch-
ID and link validation, linked source resolution, and generated round trips
against source inventories. They also
cover repeated item-macro occurrence identity, numeric ordering beyond nine,
label retention during regeneration, and occurrence-aware sketch resolution.
The `files.rs` tests protect scalar-only `CatalogPath` Serde, legacy object-form
rejection, and deterministic nonempty `FileCatalog` map-key round trips.
Compiler-artifact unit tests live under `languages/rust/rustdoc/tests/` beside
their artifact, module, declaration, type, and provenance owners and cover
schema/rustdoc/crate mismatches, tagged exact/generated provenance, exact
Unicode line/column-to-byte agreement, provenance contradictions, source-map
gaps, public reachability, direct/recursive/cyclic/colliding reexports,
macro-generated declarations, cfg-selected artifacts, generic alias argument
and default substitution, type/owner resolution, syntax/compiler agreement on
the supported overlap, deterministic lowering, and compiler-mode
generation/check/resolution through the public API.
