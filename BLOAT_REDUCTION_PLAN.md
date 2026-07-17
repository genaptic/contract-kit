# Comprehensive reductive refactor plan

No code was changed. This plan is based on the staged tree, the prior audit, the repository architecture rules, and all seven requested Rust skills.

The target is a net reduction of approximately:

- 1,800–3,300 production Rust lines.
- 4,100–5,500 test, fuzz, benchmark, and scenario lines.
- 6,000–9,000 workspace lines overall.
- 20 scenario directories and roughly 80 repeated scenario files.
- One direct production dependency from `conkit` (`rustdoc-types`).

Physical module splits are excluded from the reduction estimate because they mostly relocate surviving code.

## Change tags

- `[DELETION]` removes files, types, branches, tests, allocations, or duplicated behavior.
- `[MODIFICATION]` replaces or reorganizes existing code.
- `[ADDITION]` creates a new focused owner or module. Every addition below must be paired with a larger deletion; no phase may grow production code merely to reorganize it.

## Non-negotiable constraints

- Keep exactly the three existing crates. Do not create `conkit-core`.
- Keep archive encoding/decoding, filesystem capabilities, process execution, publication, catalog admission, and the shared Rayon pool in `conkit`.
- Keep signature and sketch limits, errors, worker admission, semantic parsing, and semantic diffing domain-local.
- Keep syntax and rustdoc lowering as separate concrete adapters.
- Keep CLI catalog admission followed by direct domain revalidation.
- Keep generate-all sequential because sketch generation consumes signature-produced seeds.
- Keep generation baseline/preflight/commit revalidation.
- Keep raw and semantic YAML checks where they enforce different guarantees.
- Keep the final post-edit sketch semantic reparse.
- Keep returned-output and scratch accounting distinct.
- Keep every fuzz target and corpus independent.
- Keep one application `futures_executor::block_on` boundary. Add no Tokio, `async_trait`, internal executor, or lock-across-`await`.
- Add no production test shims, Rust item type aliases, macro dispatch, trait objects, or standalone production helpers.
- Preserve CLI grammar, defaults, diagnostics, exit behavior, report schemas, byte ordering, source-coordinate semantics, cancellation, resource ceilings, and filesystem effects.
- The only proposed public API reductions are `SketchField::Normalization` and `DiffEntry::Changed.fields: BTreeSet<_> → Vec<_>`. Because `v0.1.0` is already released, these must be coordinated as a `0.2.0` change. If that release is not authorized, retain those two public shapes temporarily and perform the remaining plan non-breakingly.

## Phase overview

| Phase | Purpose | Expected net reduction |
|---|---|---:|
| 0 | Baseline and behavior lock | 0 |
| 1 | Replace brittle tests and shrink scenario matrix | 1,900–2,400 |
| 2 | Typed CLI extraction and layout state | 150–300 |
| 3 | Compiler projection, coordinates, process ownership | 250–500 |
| 4 | Contract parsing, bounded I/O, catalog, reports | 250–500 |
| 5 | Signature invariants, diagnostics, backend dispatch | 300–550 |
| 6 | Signature YAML input/output state | 400–700 |
| 7 | Rustdoc borrowing and semantic-diff allocation | 300–600 |
| 8 | Sketch collections, matching, limits, inventory | 450–800 |
| 9 | Sketch YAML/scalar editing | 250–550 |
| 10 | Public tests, fuzz harnesses, benchmarks | 1,200–1,900 |
| 11 | Final module splits, documentation, dead-code audit | roughly neutral |

---

# Phase 0 — Freeze behavior and measurements

No files change in this phase.

Record before implementation:

- The complete five-command workspace gate result.
- `git diff --cached --check`.
- Production/test line counts for every hotspot file.
- Exact standalone and combined YAML/JSON report bytes.
- Fresh and losslessly edited signature/sketch YAML bytes.
- Compiler mismatch and source-coordinate behavior.
- Capability-warning ordering.
- Diff ordering and digest values.
- Current one-worker matcher benchmark results.
- Current scenario coverage ownership.
- The direct dependency graph from Cargo metadata.

Create no broad new characterization suite. Reuse existing behavioral tests and add only narrow golden assertions needed before deleting brittle structural tests.

---

# Phase 1 — Replace structural test locks and reduce the scenario matrix

This must land before production reorganization so private names and statement order stop constraining the refactor.

## 1.1 Centralize source and dependency policy

- `[MODIFICATION]` [conkit/Cargo.toml](/Users/connorsanders/RustroverProjects/contract-kit/conkit/Cargo.toml): add `syn` as a dev dependency with `full` and `visit`. It is already locked and used elsewhere in the workspace.
- `[MODIFICATION]` [conkit/tests/dependency_policy.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/dependency_policy.rs):
    - Delete exact compiler/store method-body searches and statement-order assertions.
    - Delete the line-oriented `#[cfg(...)]` parser.
    - Use `cargo_metadata` for direct dependency ownership.
    - Use one recursive `syn::parse_file` visitor for production `#[cfg(test)]`, forbidden OS/process imports, and any remaining structural policy.
    - Keep compiler-private dependency, MSRV, manifest, and workspace-boundary checks.
    - Add the domain dependency rules currently duplicated in public API tests.
- `[DELETION]` Remove the source-spelling sections from:
    - [conkit/tests/check.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/check.rs)
    - [conkit/tests/generate.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/generate.rs)
    - [conkit-signature/tests/public_api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/tests/public_api.rs)
    - [conkit-sketch/tests/public_api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/tests/public_api.rs)
- `[MODIFICATION]` Replace them with behavioral coverage:
    - Persisted compiler/syntax incompatibility.
    - Compiler preflight before domain submission.
    - Cancellation before publication.
    - One AST-level enum-dispatch structural test using `syn`, checking topology rather than private source spelling.

## 1.2 Move the 24-case truth table into one binary integration test

- `[MODIFICATION]` [conkit/tests/check.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/check.rs): add a table-driven binary test covering:
    - Targets: all, signatures, sketches.
    - Mode spellings: explicit default, omitted, strict, warning.
    - Matching and mismatching catalogs.
    - Expected exit status, pass/fail semantics, and report family.

Use receiver-owned fixtures rather than free helper functions:

```rust
enum CheckTargetCase {
    All,
    Signatures,
    Sketches,
}

enum CheckModeCase {
    ExplicitDefault,
    Omitted,
    Strict,
    Warning,
}

struct CheckMatrixCase {
    target: CheckTargetCase,
    mode: CheckModeCase,
    matching: bool,
    expected_success: bool,
}

impl CheckMatrixCase {
    fn command_arguments(&self) -> Vec<&'static str> {
        // Closed target and mode matches.
    }

    fn run(&self, fixture: &CheckMatrixFixture) {
        // Build one test-local source/contract tree and assert observable CLI behavior.
    }
}
```

The test may share integration-test fixtures because the scenario-tree prohibition applies only to checked-in scenario leaves.

## 1.3 Retain four independent E2E leaves

Retain these leaves because together they cover all three targets, all four mode spellings, success/failure, report files, stdout/stderr, and filesystem shape:

1. `matrix-all-default-matching`
2. `matrix-all-omitted-mismatching`
3. `matrix-signatures-warning-mismatching`
4. `matrix-sketches-strict-mismatching`

- `[MODIFICATION]` Update their `scenario.yml` files to use semantic coverage keys such as:
    - `behavior.check.matching.passes`
    - `behavior.check.mismatching.default-fails`
    - `behavior.check.mismatching.warning-passes`
    - `behavior.check.mismatching.strict-fails`
- `[MODIFICATION]` Rename the retained signature-warning report to a mixed-case `.JSON` path so mixed-case extension inference remains E2E-covered.
- `[MODIFICATION]` [conkit/tests/support/scenario.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/support/scenario.rs):
    - Delete all 24 `behavior.check.matrix.*` keys.
    - Add the four semantic outcome keys above.
    - Retain report format/extension and surface target/mode keys only where independently useful.
    - Remove the duplicated `behavior.generate.adoption.idempotent` registry entry.
    - Keep the registry sorted and uniqueness-tested.
- `[MODIFICATION]` [conkit/tests/scenarios.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/scenarios.rs): delete `EXPECTED_SCENARIO_COUNT` and the exact 139-leaf test. Retain discovery, execution, and semantic coverage audit.
- `[MODIFICATION]` [test/scenarios/README.md](/Users/connorsanders/RustroverProjects/contract-kit/test/scenarios/README.md): replace the matrix-based example while retaining the no-shared-fixture rule.

## 1.4 Delete the other 20 leaves

- `[DELETION]` Delete these complete, independently owned directories:

```text
test/scenarios/check-rust/matrix-all-default-mismatching
test/scenarios/check-rust/matrix-all-omitted-matching
test/scenarios/check-rust/matrix-all-strict-matching
test/scenarios/check-rust/matrix-all-strict-mismatching
test/scenarios/check-rust/matrix-all-warning-matching
test/scenarios/check-rust/matrix-all-warning-mismatching

test/scenarios/check-rust/matrix-signatures-default-matching
test/scenarios/check-rust/matrix-signatures-default-mismatching
test/scenarios/check-rust/matrix-signatures-omitted-matching
test/scenarios/check-rust/matrix-signatures-omitted-mismatching
test/scenarios/check-rust/matrix-signatures-strict-matching
test/scenarios/check-rust/matrix-signatures-strict-mismatching
test/scenarios/check-rust/matrix-signatures-warning-matching

test/scenarios/check-rust/matrix-sketches-default-matching
test/scenarios/check-rust/matrix-sketches-default-mismatching
test/scenarios/check-rust/matrix-sketches-omitted-matching
test/scenarios/check-rust/matrix-sketches-omitted-mismatching
test/scenarios/check-rust/matrix-sketches-strict-matching
test/scenarios/check-rust/matrix-sketches-warning-matching
test/scenarios/check-rust/matrix-sketches-warning-mismatching
```

Do not introduce sibling references, symlinks, factories, or shared scenario fixtures.

## 1.5 Delete weaker test duplication

- `[DELETION]` [conkit/tests/cli_help.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/cli_help.rs): keep only the binary display-name and pinned-presentation-environment cases; delete predicate matrices already covered by exact help scenarios and argument-unit tests.
- `[DELETION]` [conkit/tests/domain_conformance.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/domain_conformance.rs): delete the empty-catalog worker determinism case; retain the stronger nonempty comparison.

### Phase gate

Run the check integration test, scenario harness tests, dependency policy test, and full workspace gates. No product output may change.

---

# Phase 2 — Typed CLI extraction and canonical layout state

## 2.1 Convert clap state exactly once

- `[MODIFICATION]` [conkit/args.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/args.rs):
    - Keep clap-facing primitive fields private to `SignatureOptions`.
    - Remove `Clone` from `SignatureOptions`.
    - Add one receiver that validates and returns a borrowed closed runtime value.
    - Represent target and feature selections as enums.
- `[ADDITION]` `conkit/contracts/extraction.rs`: add the sole CLI extraction reconciliation owner.
- `[MODIFICATION]` [conkit/contracts.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts.rs): declare the new private module.
- `[MODIFICATION]` [conkit/command/check.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/check.rs):
    - Delete `select_extraction`.
    - Use the coordinator.
    - Preserve explicit signature/sketch/all branches.
- `[MODIFICATION]` [conkit/command/generate.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/generate.rs):
    - Delete `all: bool`.
    - Delete the owned `SignatureOptions` copy.
    - Make `GenerationSelection::All` and `Signatures` carry a borrowed typed extraction request.
    - Delete the second requested-versus-persisted state machine.
    - Preserve signature-then-sketch sequencing.
- `[MODIFICATION]` [conkit/compiler.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs): `CompilerExtractor::extract` accepts only a validated `CompilerRequest<'_>`.

```rust
pub(crate) enum RequestedExtraction<'args> {
    Syntax,
    Compiler(CompilerRequest<'args>),
}

pub(crate) struct CompilerRequest<'args> {
    manifest: &'args Path,
    package: Option<&'args str>,
    target: CargoTarget<'args>,
    features: CargoFeatures<'args>,
    target_triple: Option<&'args str>,
}

pub(crate) enum CargoTarget<'args> {
    Automatic,
    Library,
    Binary(&'args str),
}

pub(crate) enum CargoFeatures<'args> {
    Default,
    Selected {
        names: &'args [String],
        include_default: bool,
    },
    All,
}

pub(crate) enum ExtractionUse<'layout> {
    Check {
        persisted: Option<&'layout LayoutExtraction>,
    },
    Generation {
        fresh: bool,
        persisted: Option<&'layout LayoutExtraction>,
        explicit_crates: &'layout [conkit_signature::RustCrateRoot],
    },
}

pub(crate) struct SignatureExtractionCoordinator<'args> {
    requested: RequestedExtraction<'args>,
}

impl SignatureExtractionCoordinator<'_> {
    pub(crate) fn reconcile<'layout>(
        &self,
        usage: ExtractionUse<'layout>,
    ) -> Result<ExtractionDecision<'_, 'layout>, CliError> {
        // One exhaustive requested/persisted match.
        // Check requires persisted compiler metadata.
        // Fresh generation may establish compiler metadata.
    }
}
```

`ExtractionDecision` should own the one compiler acquisition path: expected crates, whether an existing artifact must be validated, and whether fresh generation emits crate metadata.

## 2.2 Make layout extraction one valid state

- `[MODIFICATION]` [conkit/contracts/layout.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/layout.rs):
    - Replace parallel mode, context, origin, seen-crates, and crate-vector options with one enum.
    - Merge extraction while loading documents rather than rescanning all `document_plans` later.
    - Compare compiler contexts by reference.
    - Store document count rather than retaining plans solely for `is_empty`.
    - Return or take the canonical extraction without rebuilding it.
- `[MODIFICATION]` [conkit/contracts/document.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs): emit the data-carrying layout value directly.

```rust
#[derive(Clone, Debug)]
pub(crate) enum LayoutExtraction {
    Syntax {
        crates: Vec<conkit_signature::RustCrateRoot>,
        declared_at: DocumentOrigin,
    },
    Compiler {
        crates: Vec<conkit_signature::RustCrateRoot>,
        context: ContractCompilerContext,
        declared_at: DocumentOrigin,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct DocumentOrigin {
    contract_file: conkit_signature::CatalogPath,
    document_index: usize,
}

impl LayoutExtraction {
    fn merge(
        &mut self,
        incoming: Self,
        seen_crates: &mut BTreeSet<conkit_signature::RustCrateRoot>,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        // Syntax/Syntax and compatible Compiler/Compiler merge roots.
        // Mixed modes and incompatible compiler contexts fail here.
    }
}
```

### Phase gate

Test every requested/persisted pair, fresh versus existing generation, explicit crate roots, compiler artifact validation, and absence of raw `SignatureOptions` below the CLI boundary.

---

# Phase 3 — Reduce compiler extraction before splitting `compiler.rs`

Do all semantic deletion while code still lives in [conkit/compiler.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs). Split it only in Phase 11.

## 3.1 Deserialize only the CLI projection

- `[MODIFICATION]` Replace `rustdoc_types::Crate` decoding with a narrow projection containing:
    - Root ID.
    - Format version.
    - `includes_private`.
    - Target triple.
    - Item key, item ID, crate ID, and span.
- `[MODIFICATION]` Use a custom index visitor that accumulates a pre-sized `Vec`, sorts by ID, rejects duplicates and key/ID mismatches, and never constructs a second tree.
- `[DELETION]` Delete conkit’s semantic rustdoc item/type materialization.
- `[MODIFICATION]` Leave unrelated rustdoc fields accepted; do not use `deny_unknown_fields` on the intentionally partial document.
- `[MODIFICATION]` Keep projected fields required and fail closed on missing or malformed required data.
- `[MODIFICATION]` Pass the original rustdoc JSON bytes untouched to `conkit-signature`.
- `[MODIFICATION]` [conkit/Cargo.toml](/Users/connorsanders/RustroverProjects/contract-kit/conkit/Cargo.toml): remove direct `rustdoc-types`.
- `[MODIFICATION]` [Cargo.lock](/Users/connorsanders/RustroverProjects/contract-kit/Cargo.lock): remove it from the `conkit` package dependency list; the locked crate remains for `conkit-signature`.

```rust
#[derive(Deserialize)]
struct RustdocSourceDocument {
    root: RustdocId,
    index: RustdocSourceIndex,
    target: RustdocTarget,
    format_version: u32,
    includes_private: bool,
}

struct RustdocSourceIndex {
    items: Vec<RustdocSourceItem>,
}

// Manual Deserialize:
// 1. Read map entries directly into Vec.
// 2. Verify key == item.id.
// 3. Sort by item ID.
// 4. Reject adjacent duplicate IDs.

#[derive(Deserialize)]
struct RustdocSourceItem {
    id: RustdocId,
    crate_id: u32,
    span: Option<RustdocSourceSpan>,
}
```

## 3.2 Replace source-sized scalar tables

- `[DELETION]` Delete `CompilerSourceCoordinates.scalar_starts` and `line_scalar_starts`.
- `[MODIFICATION]` Group all retained local span endpoints by logical source file.
- `[MODIFICATION]` Sort and deduplicate `(line, scalar-column)` endpoints.
- `[MODIFICATION]` Resolve all endpoints during one UTF-8 scalar scan per source.
- `[MODIFICATION]` Charge endpoint requests against the existing source-mapping budget before allocation.
- `[MODIFICATION]` Preserve:
    - One-based Unicode-scalar columns.
    - CRLF behavior.
    - Inclusive end conversion.
    - EOF and invalid-range rejection.
    - Cancellation cadence.
    - Exact existing diagnostic categories.

```rust
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct SourceCoordinate {
    line: usize,
    column: usize,
}

struct RequestedEndpoint {
    coordinate: SourceCoordinate,
    item_id: u32,
    side: EndpointSide,
}

enum EndpointSide {
    Start,
    InclusiveEnd,
}

struct SourceEndpointResolver<'source> {
    source: &'source str,
    requests: Vec<RequestedEndpoint>,
}

impl SourceEndpointResolver<'_> {
    fn resolve(
        mut self,
        usage: &CompilerUsage,
    ) -> Result<Vec<ResolvedEndpoint>, CompilerError> {
        self.requests.sort_by_key(|request| request.coordinate);

        // Walk char_indices once while maintaining one-based line/column.
        // Start resolves to the current scalar's first byte.
        // InclusiveEnd resolves to byte + scalar.len_utf8().
        // Duplicate coordinates reuse the same resolved byte.
    }
}
```

This changes worst-case auxiliary memory from proportional to every source scalar to proportional to admitted rustdoc spans.

## 3.3 Consolidate probe and process cleanup ownership

- `[MODIFICATION]` Collapse `RustdocProbeRequest`, `RustdocProbeCapture`, and `RustdocProbeSession` into:
    - One boundary owner `RustdocProbe`.
    - One serialized `ProbeRecord`.
    - One closed state enum.
- `[MODIFICATION]` Consolidate invocation, monitoring, completion, and cleanup under a `CargoProcess` owner.
- `[DELETION]` Delete separate group and leader polling loops.
- `[MODIFICATION]` Use one reaper with a target enum and explicit policy.
- `[MODIFICATION]` Preserve group-first termination, leader fallback, reader draining, bounded evidence, cancellation behavior, and conclusive reap attempts.

```rust
pub(crate) struct RustdocProbe {
    state: ProbeState,
}

enum ProbeState {
    Child(ProbeChild),
    Parent(ProbeParent),
}

#[derive(Deserialize, Serialize)]
struct ProbeRecord {
    token: String,
    exit_code: u8,
    cfg_values: Vec<String>,
    rejection: Option<String>,
}

struct CargoProcess<'operation> {
    child: GroupChild,
    readers: BoundedPipes,
    usage: &'operation CompilerUsage,
    operation: CompilerOperation,
}

enum ReapTarget<'child> {
    Group(&'child mut GroupChild),
    Leader {
        child: &'child mut std::process::Child,
        was_terminated: bool,
    },
}

impl CargoProcess<'_> {
    fn reap(
        &mut self,
        target: ReapTarget<'_>,
        evidence: &mut ProcessCleanupEvidence,
    ) -> bool {
        // One poll/deadline/wait implementation with target-specific kill action.
    }
}
```

Unit tests should spawn the current test executable with an ignored helper test, keeping all helper behavior inside `#[cfg(test)]` modules rather than production shims.

### Phase gate

Required focused tests:

- Partial projection accepts extra fields and rejects missing required fields.
- Deterministic item ordering and key/ID mismatch.
- ASCII, Unicode, CRLF, duplicate, unsorted, EOF, and inclusive endpoints.
- Memory structure grows with endpoints, not source scalar count.
- Process success, nonzero exit, timeout, cancellation, group kill failure, leader fallback, pipe drain, and cleanup evidence.
- No direct `rustdoc-types` dependency in `conkit`.

---

# Phase 4 — Consolidate CLI document parsing, bounded I/O, catalog mechanics, and reports

## 4.1 Contract parsing context and YAML accounting

- `[MODIFICATION]` [conkit/contracts/document.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs):
    - Add one borrowed `ContractLocation`.
    - Move extraction conversion onto `ExtractionHeader`.
    - Move crate conversion onto `CrateHeader`.
    - Move compiler conversion onto `CompilerHeader`.
    - Delete repeated `(contract_file, document_index, cancellation)` parameters and repeated path formatting.
    - Replace separate resource limits/counters/mappings with one typed resource ledger.
    - Keep raw physical analysis and semantic replay as separate passes.

```rust
struct ContractLocation<'operation> {
    contract_file: &'operation CatalogPath,
    display_path: &'operation Path,
    document_index: usize,
    cancellation: &'operation ApplicationCancellation,
}

impl ContractLocation<'_> {
    fn checkpoint(&self) -> Result<(), CliError> {
        self.cancellation.checkpoint()
    }

    fn invalid(&self, message: impl Into<String>) -> CliError {
        CliError::ContractLayout {
            path: self.display_path.to_path_buf(),
            message: format!(
                "YAML document index {} {}",
                self.document_index,
                message.into()
            ),
        }
    }
}

enum ContractYamlResource {
    Documents,
    Depth,
    Nodes,
    Aliases,
    ScalarBytes,
}

#[derive(Clone, Copy, Default)]
struct ContractYamlCounters {
    documents: u64,
    depth: u64,
    nodes: u64,
    aliases: u64,
    scalar_bytes: u64,
}

impl ContractYamlCounters {
    fn charge(
        &mut self,
        resource: ContractYamlResource,
        amount: u64,
        limits: &ContractYamlCounters,
    ) -> Result<(), ContractYamlBreach> {
        // One checked-add and one resource-to-limit mapping.
    }
}
```

## 4.2 One concrete bounded writer

- `[ADDITION]` `conkit/bounded_output.rs`: add one cancellation-aware, byte-limited `Write` wrapper. It must know nothing about archive/report/store formats or publication.
- `[MODIFICATION]` [conkit/main.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/main.rs): declare the module.
- `[MODIFICATION]` [conkit/archive.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/archive.rs): delete archive JSON/gzip writer duplication and use the wrapper.
- `[MODIFICATION]` [conkit/report.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/report.rs): delete report writer duplication and use it around `AtomicWriteFile`.

```rust
struct BoundedOutput<'cancellation, W> {
    inner: W,
    cancellation: &'cancellation ApplicationCancellation,
    ceiling: u64,
    written: u64,
    failure: Option<BoundedOutputFailure>,
}

enum BoundedOutputFailure {
    Cancelled,
    Limit { observed_at_least: u64 },
}

impl<W: Write> Write for BoundedOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // Check cancellation, cap the request, delegate, and account actual bytes.
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Check cancellation before delegating.
    }
}
```

Archive and report callers retain their distinct error translation and publication behavior.

## 4.3 Consolidate bounded reads and capability paths

- `[MODIFICATION]` [conkit/catalog.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog.rs):
    - Add `CatalogReadBudget::read_file_with_ceiling`.
    - Make ordinary reads delegate to it.
    - Expose the operation cancellation probe through the budget.
- `[MODIFICATION]` [conkit/archive/source.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/archive/source.rs):
    - Delete the second compressed-input read loop.
    - Map external wire-ceiling failure before the catalog budget’s `finish_file`, preserving current error precedence.
- `[MODIFICATION]` [conkit/catalog/path.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/path.rs):
    - Consolidate parent traversal and final-component policy.
    - Use one access enum and one `CatalogLeaf` no-follow regular-file opener.
    - Preserve optional versus required behavior, symlink rejection, and capability-relative traversal.
- `[MODIFICATION]` [conkit/catalog/source.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/source.rs): use the shared leaf resolver/opener.
- `[MODIFICATION]` [conkit/catalog/store.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/store.rs): use the same leaf boundary.

## 4.4 Store-local atomic publication

- `[MODIFICATION]` [conkit/catalog/store.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/store.rs):
    - Introduce `ContractPublication`.
    - Delete duplicated close, cleanup, and error-combination branches.
    - Keep explicit abort reporting cleanup failures.
    - Use `Drop` only for best-effort fallback.
    - Keep the final cancellation checkpoint immediately before rename.
    - After rename, parent-directory sync is authoritative; do not report a later cancellation.

Do not merge this with [conkit/archive/publication.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/archive/publication.rs); their semantics differ.

## 4.5 Typed ownership and reconciliation cancellation

- `[MODIFICATION]` [conkit/catalog/ownership.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/ownership.rs):
    - Store `CatalogPath` directly in `OwnedFile`.
    - Preserve its string wire representation with custom serde.
    - Move owned-path conversion/error behavior onto receiver methods.
- `[MODIFICATION]` [conkit/catalog/reconciliation.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/reconciliation.rs):
    - Derive forward-operation cancellation from `CatalogReadBudget`.
    - Remove cancellation parameters passed alongside the same budget.
    - Remove repeated owned-path parsing.
    - Preserve cancellation-neutral rollback and cleanup.

## 4.6 Domain-owned report views

- `[MODIFICATION]` [conkit-signature/api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/api.rs): expose opaque borrowed standalone and embedded report views.
- `[MODIFICATION]` [conkit-sketch/api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/api.rs): expose the corresponding opaque view.
- `[MODIFICATION]` [conkit-signature/lib.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/lib.rs) and [conkit-sketch/lib.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/lib.rs): document the view methods; no new public DTO type is necessary.
- `[MODIFICATION]` [conkit-sketch/report.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/report.rs): serialize the domain view.
- `[MODIFICATION]` [conkit/report.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/report.rs):
    - Delete `SignatureCheckReport` and `SketchCheckReport`.
    - Keep only the combined envelope.
    - Serialize domain views manually.

Signature currently has different standalone and embedded field orders. Preserve both exactly:

```rust
enum CheckReportLayout {
    Standalone,
    Embedded,
}

struct CheckReportView<'response> {
    response: &'response CheckResponse,
    layout: CheckReportLayout,
}

impl CheckResponse {
    pub fn report_view(&self) -> impl serde::Serialize + '_ {
        CheckReportView {
            response: self,
            layout: CheckReportLayout::Standalone,
        }
    }

    pub fn embedded_report_view(&self) -> impl serde::Serialize + '_ {
        CheckReportView {
            response: self,
            layout: CheckReportLayout::Embedded,
        }
    }
}
```

`AllCheckReport` should hold response references and implement `Serialize` manually. Neither view includes `report_files`.

### Phase gate

Test exact and ceiling+1 writer/read behavior, cancellation, symlink policy, optional/required leaves, publication failure points, typed-path serde, reconciliation rollback, and byte-for-byte standalone/combined report equality.

---

# Phase 5 — Signature invariant, diagnostic, inventory, and backend consolidation

## 5.1 One semantic declaration kind

- `[MODIFICATION]` [conkit-signature/languages/rust/types/declaration.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/types/declaration.rs):
    - Move `RustItemKind` here.
    - Make `RustDeclaration::kind()` return it directly.
    - Own wire naming and structural repeatability here.
- `[DELETION]` Delete `RustDeclarationKind`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/signature_id.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/signature_id.rs): delete the duplicate enum and one-to-one conversions.
- `[MODIFICATION]` Update imports in parser, YAML, rustdoc, symbol table, and implementation types.

Keep the YAML kind distinct because it intentionally describes a different wire subset.

## 5.2 Make `RustExtraction` the invariant owner

- `[MODIFICATION]` [conkit-signature/languages/rust/parser/source_graph.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/source_graph.rs):
    - Add `RustExtraction::from_roots`.
    - Own nonempty crates, trimmed/unique IDs, `.rs` roots, allowlist membership, canonical files, and ordering.
- `[DELETION]` [conkit-signature/api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/api.rs): delete `GenerateDocument::validate_and_order_crates`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/yaml/document.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/document.rs): contextualize typed extraction errors instead of revalidating.
- `[MODIFICATION]` Compiler extraction keeps direct domain validation and its exactly-one-crate rule.

```rust
impl RustExtraction {
    pub(crate) fn from_roots(
        files: impl IntoIterator<Item = CatalogPath>,
        roots: impl IntoIterator<Item = RustCrateRoot>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        // Canonicalize files and roots, construct RustCrate values,
        // then enter the single invariant-preserving constructor.
    }
}
```

## 5.3 One operation-scoped diagnostic owner

- `[MODIFICATION]` [conkit-signature/languages/rust/parser/source_graph.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/source_graph.rs): remove diagnostics from `RustSourceGraph`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/mod.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs): remove them from `RustParsedProjection` and stop reinserting during document checking.
- `[MODIFICATION]` Inventory collection inserts directly into the operation collector.
- `[DELETION]` Delete final redundant `BTreeSet` reconstruction in `ParsedSignatureCheck::new`.
- `[MODIFICATION]` Consume the collector once when producing warnings/check output.

## 5.4 Collapse builder/pass lifecycle state

- `[DELETION]` `conkit-signature/languages/rust/parser/inventory_builder.rs`.
- `[ADDITION]` `conkit-signature/languages/rust/parser/inventory_collector.rs`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/mod.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs): use one mutable collector.

The collector owns symbol-table initialization, declaration pass, implementation pass, allocation, diagnostics, and final projection. It does not recreate itself between phases.

## 5.5 One closed syntax/compiler implementation family

- `[ADDITION]` `conkit-signature/languages/rust/parser/backend.rs`
- `[ADDITION]` `conkit-signature/languages/rust/parser/backend/syntax.rs`
- `[ADDITION]` `conkit-signature/languages/rust/parser/backend/compiler.rs`
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/mod.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs): replace repeated input matches with one dispatcher.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/yaml/render.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/render.rs): delete `RustYamlGenerationSource`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/yaml/sketch.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/sketch.rs): delete `RustSketchSource` and `RustSketchResolver`.

```rust
pub(crate) trait RustExtractionBackend {
    fn check(
        self,
        parser: &SignatureParser,
        source_files: FileCatalog,
        documents: RustContractDocuments,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError>;

    fn generate(
        self,
        parser: &SignatureParser,
        source_files: FileCatalog,
        plan: RustGenerationPlan,
        scope: ContractScope,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SignatureContractKitError>;

    fn resolve_sketches(
        self,
        parser: &SignatureParser,
        source_files: FileCatalog,
        documents: RustContractDocuments,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError>;
}

struct SyntaxBackend;

struct CompilerBackend {
    artifact: RustCompilerArtifact,
}

enum RustBackend {
    Syntax(SyntaxBackend),
    Compiler(CompilerBackend),
}

impl RustExtractionBackend for RustBackend {
    fn check(/* ... */) -> Result<ParsedSignatureCheck, SignatureContractKitError> {
        match self {
            Self::Syntax(backend) => backend.check(/* receiver-style call */),
            Self::Compiler(backend) => backend.check(/* receiver-style call */),
        }
    }

    // Same explicit two-arm dispatch for generation and sketch resolution.
}
```

Requirements:

- Exactly one uniquely named trait.
- Concrete implementations.
- One explicit exhaustive enum dispatcher.
- Receiver-style calls only; no UFCS payload dispatch.
- No `dyn`, macros, codegen, or `async_trait`.
- Renderer and sketch-seed construction consume implementation-neutral projections and source access.

## 5.6 Delete small wrappers

- `[DELETION]` [conkit-signature/languages/rust/parser/item_converter.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/item_converter.rs):
    - Delete `RustAssociatedConversion`; return `Vec<RustAssociatedItem>`.
    - Replace `type_converter_for_signature` and `type_converter_for_associated` with one generics-based receiver.
- `[DELETION]` [conkit-signature/languages/rust/parser/yaml/document.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/document.rs): inline `RustNewDocumentPlan` fields into `RustGenerationPlan::New`.
- `[MODIFICATION]` [conkit-signature/languages/rust/parser/mod.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs): store `SignatureLimits` directly in the already-`Arc`-owned parser; delete nested `Arc<SignatureLimits>`.

### Phase gate

A narrow `syn` structural test verifies trait, concrete implementations, dispatcher variants, and exhaustive receiver-style forwarding. Behavioral tests verify syntax/compiler parity, diagnostic order, extraction validation, and generated output.

---

# Phase 6 — Collapse signature YAML input and output state

## 6.1 Decode raw fields directly into named domain entries

- `[MODIFICATION]` [conkit-signature/languages/rust/parser/yaml/input.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs):
    - Retain the raw field-presence shape required to distinguish missing, null, forbidden, and required values.
    - Rename it if useful, but do not add another lifecycle stage.
    - Add one `RustYamlDocumentDecoder` that consumes the raw shape and produces `RustYamlNamedSignature`.
    - Delete shorthand/common intermediate carriers.
    - Consume nested input values by value to avoid clones.

```rust
struct RustYamlDocumentDecoder<'document, 'operation> {
    catalog_name: &'document CatalogPath,
    extraction: &'document RustYamlExtraction,
    item_ids: &'document mut RustItemIdAllocator,
    cancellation: &'operation CancellationProbe,
}

impl RustYamlDocumentDecoder<'_, '_> {
    fn decode(
        &mut self,
        label: String,
        mut raw: RustYamlRawSignature,
    ) -> Result<RustYamlNamedSignature, SignatureContractKitError> {
        raw.validate_shape()?;

        // Resolve common file/module/base ownership once.
        // Match signature_type and consume the relevant fields directly.
        // Reject forbidden leftovers and construct the final named entry.
    }
}
```

- `[DELETION]` Delete:
    - `RustYamlShorthandSignature`
    - `RustYamlSignatureCommonInput`
    - `RustYamlSignatureCommon`
    - Any raw→shorthand→common transition methods.

## 6.2 Delete micro-wrapper values

Move behavior onto actual DTOs or the decoder, then delete:

- `[DELETION]` `RustYamlCallableParts`
- `[DELETION]` `RustYamlImplementedTraitInput`
- `[DELETION]` `RustYamlFunctionAbiInput`
- `[DELETION]` `RustYamlVisibilityText`
- `[DELETION]` `RustYamlShorthandGenericsInput`
- `[DELETION]` `RustYamlMapField`
- `[DELETION]` `RustYamlReceiverText`

Use owned conversion where a parsed input is consumed once.

## 6.3 Rename genuinely bidirectional leaf codecs

- `[MODIFICATION]` Rename DTOs such as `RustYamlAttributesInput` only where they are truly shared by tolerant input and canonical output.
- Use neutral names such as `RustYamlAttributesValue`.
- Keep separate input/output types where accepted input is broader than emitted output.

## 6.4 Make generated-document origin valid by construction

- `[MODIFICATION]` [conkit-signature/languages/rust/parser/yaml/render.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/render.rs):
    - Replace two `Option`s and the empty-order sentinel with one enum.
    - Delete impossible-state error branches.

```rust
enum RustYamlDocumentOrigin {
    New,
    Existing {
        bytes: Arc<[u8]>,
        document: RustYamlDocument,
        signature_order: Vec<RustItemId>,
    },
}

struct RustYamlGeneratedDocument {
    location: RustYamlDocumentLocation,
    origin: RustYamlDocumentOrigin,
    proposed_document: RustYamlDocument,
    proposed_signature_order: Vec<RustItemId>,
    signatures: Vec<BTreeMap<String, RustYamlRenderedSignature>>,
}
```

## 6.5 Replace the 30-option output struct

- `[DELETION]` Delete `RustYamlShorthandSignatureOutput`.
- `[MODIFICATION]` Introduce common metadata plus a declaration-body enum.
- `[MODIFICATION]` Use explicit variant renames for wire names such as `enum`, `struct`, and `impl`.
- `[MODIFICATION]` Hand-write serialization if necessary to preserve exact key order.

```rust
#[derive(Serialize)]
struct RustYamlRenderedSignature {
    crate_id: RustCrateId,
    file: String,
    #[serde(flatten)]
    declaration: RustYamlRenderedDeclaration,
    #[serde(skip_serializing_if = "Option::is_none")]
    sketch: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "signature_type", rename_all = "snake_case")]
enum RustYamlRenderedDeclaration {
    Constant {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(rename = "type")]
        type_text: String,
        value: String,
    },
    Function {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        callable: RustYamlRenderedCallable,
    },
    Struct {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        fields: RustYamlRenderedFields,
        implementations: Vec<RustYamlRenderedImplementation>,
    },
    // Remaining supported declaration families with only valid fields.
}
```

### Phase gate

Golden tests must prove identical YAML bytes, key order, omitted defaults, lossless existing-document behavior, all declaration kinds, sketches, and output limit accounting.

---

# Phase 7 — Borrow rustdoc facts and semantic-diff context

## 7.1 Separate immutable rustdoc data from mutable lowering

- `[MODIFICATION]` [conkit-signature/languages/rust/rustdoc.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs):
    - Replace `RustdocConverter` with an immutable index/database and mutable inventory state.
    - Borrow `Item`, `ItemEnum`, types, variants, fields, and implementations.
    - Clone only final owned domain strings, paths, declarations, and output collections.
    - Preserve iterative traversal, cycle checks, multiple-parent checks, exports, and deterministic ordering.

```rust
struct RustdocIndex {
    context: RustCompilerExtractionContext,
    document: rustdoc_types::Crate,
    source_map: BTreeMap<u32, CompilerSourcePath>,
}

struct CompilerInventory<'operation, 'limits> {
    sources: RustSourceCatalog,
    limits: &'limits RustExtractionLimits,
    usage: &'operation mut RustExtractionUsage<'limits>,
    cancellation: &'operation CancellationProbe,
    entries: Vec<RustParsedEntry>,
    converted_items: BTreeSet<u32>,
    implementation_ids: BTreeSet<u32>,
    allocator: RustItemIdAllocator,
}
```

Use cohesive borrowed receiver owners:

- `RustdocModuleCollector`
- `RustdocDeclarationLowerer`
- `RustdocTypeLowerer`
- `RustdocProvenanceResolver`

These are concrete rustdoc-to-domain services, not a universal intermediate AST.

## 7.2 Remove the signature-side scalar index

- `[DELETION]` Delete `CompilerSourceFileIndex.scalar_starts`.
- `[MODIFICATION]` Group source-map byte endpoints by file.
- `[MODIFICATION]` Validate UTF-8 boundaries with `str::is_char_boundary`.
- `[MODIFICATION]` Resolve all admitted byte endpoints to line/scalar columns in one source pass.
- `[MODIFICATION]` Preserve the current rustdoc span/source-map agreement checks.

This closes the same source-sized allocation pattern on both sides of the compiler artifact boundary.

## 7.3 Borrow semantic-diff metadata and labels

- `[MODIFICATION]` [conkit-signature/inventory.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/inventory.rs):
    - Make `InventoryGroupDigest` borrow extraction/document buffers with `Option<&[u8]>`.
    - Make semantic indexes borrow labels from source inventories.
    - Own only digest values and counts.
    - Avoid self-referential structures.

## 7.4 Store digest bytes in fixed form

- `[MODIFICATION]` Store `SignatureDigest` internally as `[u8; 32]`.
- `[MODIFICATION]` Encode hex only at public/report boundaries.
- `[MODIFICATION]` When a digest participates in another digest, feed the same 64 lowercase hexadecimal bytes as before using a stack buffer, preserving all existing digest values.

```rust
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct SignatureDigest {
    bytes: [u8; 32],
}

impl SignatureDigest {
    fn hex_bytes(self) -> [u8; 64] {
        // Stack-only lowercase hexadecimal encoding.
    }

    fn render(self) -> String {
        String::from_utf8(self.hex_bytes().to_vec()).expect("hex is UTF-8")
    }
}
```

### Phase gate

Compare all pre-refactor digest goldens, semantic diff categories, label matching, rustdoc extraction results, compiler provenance, allocations under representative artifacts, and cancellation/limit behavior.

---

# Phase 8 — Simplify sketch collections, matching, inventory, limits, and workflows

## 8.1 Keep one canonical sorted sketch collection

- `[MODIFICATION]` [conkit-sketch/contract.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs):
    - Retain the already ID-sorted `Vec<SketchContract>`.
    - Add binary lookup.
    - Replace current/previous maps plus union set with a two-pointer merge.
- `[MODIFICATION]` [conkit-sketch/generate.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/generate.rs): use binary lookup instead of rebuilding `contracts_by_id`.

```rust
impl SketchContracts {
    fn get(&self, id: &SketchId) -> Option<&SketchContract> {
        self.entries
            .binary_search_by(|contract| contract.id().cmp(id))
            .ok()
            .map(|index| &self.entries[index])
    }

    fn diff_against(
        &self,
        previous: &Self,
        cancellation: &CancellationProbe,
    ) -> Result<DiffResponse, SketchContractKitError> {
        // Two indices, one ordered pass, no maps or union set.
    }
}
```

## 8.2 Canonical field vectors and unreachable normalization

This is the coordinated `0.2.0` checkpoint.

- `[MODIFICATION]` [conkit-sketch/api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/api.rs):
    - Change `DiffEntry::Changed.fields` to `Vec<SketchField>`.
    - Emit fields once in fixed canonical order.
    - Preserve serialized array ordering.
- `[DELETION]` Remove `SketchField::Normalization`, its dead comparison, docs, and synthetic serialization test.
- `[MODIFICATION]` [CHANGELOG.md](/Users/connorsanders/RustroverProjects/contract-kit/CHANGELOG.md): document both public API changes under Unreleased.

If `0.2.0` is not authorized, keep the public `BTreeSet` and variant while applying all other internal reductions.

## 8.3 One internal document locator

- `[MODIFICATION]` [conkit-sketch/contract.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs):
    - Add `ContractDocumentLocator`.
    - Use it in `SketchContract`, `SignatureLink`, and the declaration value.
    - Rename `PendingSketch` to `SketchDeclaration`.
    - Retain independent declaration/link copies of source file, signature label, and kind until agreement is validated.
    - Keep public `SketchSeed`, `SketchSnapshot`, and `SketchLocation` flat.

## 8.4 Delete one-field workflow wrappers

- `[DELETION]` Delete `SketchCheck`, `SketchDiff`, `SketchGenerator`, and `ReportFiles`.
- `[MODIFICATION]` Move behavior to private receiver methods on:
    - `CheckRequest`
    - `DiffRequest`
    - `GenerateRequest`
    - `ReportRequest`
- `[MODIFICATION]` Keep exactly one work-pool submission per public async operation.

## 8.5 One occurrence scanner and transactional diagnostics

- `[MODIFICATION]` [conkit-sketch/matcher.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs):
    - Use one source loop parameterized by `SketchOccurrence`.
    - Preserve first-hit exit for `AtLeastOne`.
    - Preserve complete counting and bounded spans for `ExactlyOne`.
    - Make `MatchEvaluation::Satisfied` a unit variant.
    - Put one-based coordinate construction on `MatchCandidatePosition`.
    - Keep the nearest-candidate scan only after a miss.
- `[MODIFICATION]` Make diagnostic reservation transactional:
    - Measure skeleton and escaped expansion.
    - Reserve exact count/bytes.
    - Materialize evidence.
    - Push and commit once.
    - Delete final reserialization.

```rust
enum MatchEvaluation {
    Satisfied,
    Missing,
    OccurrenceMismatch {
        actual: usize,
        spans: Vec<SourceLineSpan>,
        spans_truncated: bool,
    },
}

struct DiagnosticReservation {
    previous_count: u64,
    previous_bytes: u64,
    next_count: u64,
    next_bytes: u64,
}

impl DiagnosticBytes {
    fn reserve<T: Serialize + ?Sized>(
        &self,
        skeleton: &T,
        additional_bytes: u64,
        limits: &DiagnosticLimits,
        file: Option<&CatalogPath>,
    ) -> Result<DiagnosticReservation, SketchContractKitError> {
        // Measure once and validate without mutating committed counters.
    }

    fn commit(&mut self, reservation: DiagnosticReservation) {
        // Commit only after evidence and diagnostic insertion succeed.
    }
}
```

Do not add a first-line source index in this refactor. The corrected benchmark must first demonstrate at least a 20% improvement on 64/256/1000-sketch cases, no more than 5% regression on one-sketch cases, and deterministic limit accounting.

## 8.6 Derive inventory totals exactly once

- `[MODIFICATION]` [conkit-sketch/matcher.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs): aggregate matched outcomes directly.
- `[MODIFICATION]` [conkit-sketch/inventory.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/inventory.rs):
    - Construct counts from primitive scope facts, matched count, and diagnostics.
    - Make comparison construction infallible.
    - Retain debug assertions for internal invariants.
- `[DELETION]` Delete `InventoryError`, its wrapper/conversion in [conkit-sketch/error.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/error.rs), and tests manufacturing impossible count combinations.

## 8.7 Consolidate domain-local limit mechanics

- `[MODIFICATION]` [conkit-sketch/limits.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs):
    - Add a small private `LimitCharge`.
    - Centralize raw/semantic breach conversion.
    - Replace `observed_at_least: Option<u64>` plus `cancelled: bool` with `Option<OutputFailure>`.
    - Share only first-failure preservation between concrete writers.
    - Keep scratch reservations and returned-output totals independent.
    - Use `try_reserve` in returned output.

```rust
enum OutputFailure {
    Cancelled,
    Limit { observed_at_least: u64 },
    Allocation { message: String },
}
```

## 8.8 Emit normalization ranges in one pass

- `[MODIFICATION]` [conkit-sketch/normalize.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/normalize.rs): append line ranges while emitting normalized bytes and delete the second traversal.
- Preserve all CRLF, isolated CR, final terminator, arbitrary-byte, line-limit, byte-limit, cancellation, and idempotence behavior.

### Phase gate

Test sorted lookup/diff ordering, public field serialization, locator diagnostics, occurrence policy, exact diagnostic budgets, inventory counts, output failure precedence, and normalization parity.

---

# Phase 9 — Consolidate sketch YAML analysis and scalar editing

## 9.1 Combine raw budget and document metadata scans

- `[MODIFICATION]` [conkit-sketch/limits.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs): add a transactional raw event meter owned by `YamlBudget`.
- `[MODIFICATION]` [conkit-sketch/contract.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs):
    - Feed each parser event to both document metadata analysis and the meter.
    - Delete `YamlBudget::validate_source`, the separate raw parser pass, and production calls to `check_yaml_budget`.
    - Commit operation usage only after a complete valid stream.

```rust
struct RawYamlMeter<'budget, 'limits> {
    budget: &'budget mut YamlBudget<'limits>,
    path: &'budget CatalogPath,
    report: RawYamlReport,
    depth: usize,
}

impl RawYamlMeter<'_, '_> {
    fn observe(&mut self, event: &Event<'_>) -> Result<(), LimitExceeded> {
        // Documents, nodes, aliases, max depth, and scalar/tag bytes.
    }

    fn finish(self) -> Result<RawYamlReport, LimitExceeded> {
        // Commit only after balanced, successful parsing.
    }
}
```

Required proof:

- A test-only differential oracle against `serde_saphyr::budget::check_yaml_budget`.
- Exact boundaries for every raw resource.
- Syntax failure does not commit partial usage.
- Aggregate accounting remains exact across files, diff sides, and generation reparses.
- Typed semantic parsing and alias replay remain.

## 9.2 Replace scalar lifecycle carriers with one editor/codec

- `[MODIFICATION]` Replace:
    - `SketchScalarSource`
    - `SketchScalarRenderContext`
    - `SketchScalarNode`
    - `SketchScalarEnvelope*`
    - `SketchBlockPresentation`
    - `SketchScalarPresentation`
    - `SketchScalarRendering`
- `[MODIFICATION]` Keep `SketchCodeNode` as the concrete original-CST owner and add one cohesive scalar editor/candidate owner.
- `[MODIFICATION]` Represent inline versus block presentation with an enum.
- `[MODIFICATION]` Represent preferred versus safe fallback rendering with a closed data-carrying enum.

```rust
enum ScalarPresentation<'source> {
    Inline(InlineScalarStyle),
    Block {
        style: BlockScalarStyle,
        details: BlockPresentation<'source>,
    },
}

enum InlineScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

enum BlockScalarStyle {
    Literal,
    Folded,
}

enum ScalarEncoding<T> {
    Preferred(T),
    SafeFallback(T),
}

struct ScalarCandidate<'meter, 'limits> {
    source: ScratchText<'meter, 'limits>,
    node: yaml_edit::YamlNode,
    source_mapping_column: usize,
}
```

One receiver must own:

1. Original style inspection.
2. Preferred encoding.
3. Tag restoration.
4. Line-ending and indentation conversion.
5. CST validation.
6. Exact semantic string validation.
7. Dropping rejected preferred scratch before fallback allocation.
8. Deterministic double-quoted fallback.
9. Error contextualization.

Preserve anchors/aliases fail-closed, block comments, chomping, indicator order, per-scalar line endings, scratch peaks, and no-op fast paths.

## 9.3 Delete the rendered-document CST reparse

- `[ADDITION]` Add a private `VerifiedEditSet` inside the existing editor code.
- `[MODIFICATION]` It must:
    - Validate all edit ranges are UTF-8 boundaries, in bounds, sorted, and disjoint.
    - Copy every byte outside replacement ranges directly from the original.
    - Accept only independently CST- and semantically-validated scalar candidates.
    - Prove signature/extraction ranges are untouched by construction.
- `[DELETION]` Once those proofs are covered, delete the full rendered-output CST parse.
- `[MODIFICATION]` Retain:
    - Original CST parse for target discovery.
    - Per-candidate CST validation.
    - Final whole-document typed semantic reparse and equality check.

This removes one complete changed-document parse without weakening the authoritative semantic validation.

### Phase gate

Test aliases, anchors, every scalar style, tags, comments, CR/LF/CRLF, indentation, chomping, preferred/fallback scratch peaks, untouched byte ranges, malformed output rejection, no-op generation, and final semantic equality.

---

# Phase 10 — Reduce public tests, fuzz setup, and benchmarks

## 10.1 Prune public integration duplication

- `[MODIFICATION]` [conkit-signature/tests/public_api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/tests/public_api.rs):
    - Keep public builder/limits/Send boundaries, DTO serde, one check/generate/diff/resolve workflow, report layouts, representative extraction/diff cases, and capability-warning policy.
    - Delete documentation substring checks, private-module/export spelling checks, broad marker scans, and unit-level parser/digest matrices.
    - Move archive/dependency policy to the centralized Cargo metadata test.
- `[MODIFICATION]` [conkit-sketch/tests/public_api.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/tests/public_api.rs):
    - Keep async ownership/Send, public DTO and limits, one matching check, one warning/enforce mismatch, one report, one generate→check, one diff, representative parse/catalog errors, and unreferenced binary input.
    - Delete overlapping Unicode whitespace, CRLF, occurrence, counts, scalar edit, refresh, diff-field, locator, parser, and editor matrices already owned by unit modules.
    - Delete orphan fixture builders.

Expected sketch public-test deletions include the overlapping cases identified in the audit: exact-line whitespace families, occurrence policy matrix, bounded candidate evidence, count invariants, refresh seed matrices, scalar equivalence cases, field-by-field diff matrices, relocation/digest matrices, and the broad parser/editor block.

## 10.2 Consolidate fuzz harness setup

- `[ADDITION]` `fuzz/src/lib.rs`
- `[ADDITION]` `fuzz/src/signature.rs`
- `[ADDITION]` `fuzz/src/sketch.rs`
- `[MODIFICATION]` All eight files under `fuzz/fuzz_targets/`:
    - Use concrete signature/sketch harness owners.
    - Centralize the 256 KiB bound, thread-local kits, fixed paths, catalogs, and common requests.
    - Keep every binary and corpus separate.
    - `expect` all static path, catalog, kit, and fixed-valid workflow invariants.
    - Ignore only errors caused by arbitrary malformed input.
    - Use `SketchOccurrence` rather than string occurrence policy.

Do not add `conkit` to the fuzz workspace or expose archive internals.

## 10.3 Correct the matcher benchmark

- `[MODIFICATION]` [conkit-sketch/benches/matcher.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/benches/matcher.rs):
    - Delete the one/four-worker loop.
    - Build one one-worker kit because each iteration submits one root operation and production matching is sequential.
    - Remove worker count from IDs.
    - Use `SketchOccurrence`.
    - Retain cardinality, bytes, early/late hit, miss, common-prefix miss, duplicate, overlap, long-pattern, and invalid-byte cases.
    - Rename groups so they do not imply worker scaling.

### Phase gate

Check the independent fuzz workspace, compile the benchmark, compare the corrected one-worker baseline, and run the public integration tests plus full workspace gates.

---

# Phase 11 — Split surviving monoliths after deletion

These additions are relocations, not new architectural layers. Each facade keeps narrow visibility and unit tests move beside their owning private behavior.

## 11.1 `conkit` compiler

- `[MODIFICATION]` [conkit/compiler.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs): reduce to facade and top-level extractor orchestration.
- `[ADDITION]` `conkit/compiler/extractor.rs`
- `[ADDITION]` `conkit/compiler/probe.rs`
- `[ADDITION]` `conkit/compiler/limits.rs`
- `[ADDITION]` `conkit/compiler/process.rs`
- `[ADDITION]` `conkit/compiler/project.rs`
- `[ADDITION]` `conkit/compiler/source.rs`
- `[ADDITION]` `conkit/compiler/error.rs`

## 11.2 Contract-document parsing

- `[MODIFICATION]` [conkit/contracts/document.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs): keep catalog/document orchestration.
- `[ADDITION]` `conkit/contracts/document/header.rs`
- `[ADDITION]` `conkit/contracts/document/yaml.rs`

## 11.3 Signature YAML

- `[MODIFICATION]` Keep `yaml/input.rs` and `yaml/render.rs` as facades.
- `[ADDITION]` `yaml/input/contract.rs`
- `[ADDITION]` `yaml/input/declaration.rs`
- `[ADDITION]` `yaml/input/member.rs`
- `[ADDITION]` `yaml/input/metadata.rs`
- `[ADDITION]` `yaml/render/proposal.rs`
- `[ADDITION]` `yaml/render/lossless.rs`
- `[ADDITION]` `yaml/render/output.rs`

Existing behavioral tests remain under `yaml/tests/`; only tightly private unit tests move with their owners.

## 11.4 Signature rustdoc

- `[MODIFICATION]` [conkit-signature/languages/rust/rustdoc.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs): facade and extraction entry.
- `[ADDITION]` `rustdoc/artifact.rs`
- `[ADDITION]` `rustdoc/index.rs`
- `[ADDITION]` `rustdoc/modules.rs`
- `[ADDITION]` `rustdoc/declarations.rs`
- `[ADDITION]` `rustdoc/types.rs`
- `[ADDITION]` `rustdoc/provenance.rs`
- `[ADDITION]` `rustdoc/tests/{mod,artifact,modules,declarations,types,provenance}.rs`

## 11.5 Sketch contract and limits

- `[MODIFICATION]` [conkit-sketch/contract.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs): facade only.
- `[ADDITION]` `conkit-sketch/contract/model.rs`
- `[ADDITION]` `conkit-sketch/contract/document.rs`
- `[ADDITION]` `conkit-sketch/contract/resolve.rs`
- `[ADDITION]` `conkit-sketch/contract/diff.rs`
- `[ADDITION]` `conkit-sketch/contract/edit.rs`
- `[ADDITION]` `conkit-sketch/contract/edit/scalar.rs`
- `[MODIFICATION]` [conkit-sketch/limits.rs](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs): nominal public limit facade.
- `[ADDITION]` `conkit-sketch/limits/catalog.rs`
- `[ADDITION]` `conkit-sketch/limits/yaml.rs`
- `[ADDITION]` `conkit-sketch/limits/matching.rs`
- `[ADDITION]` `conkit-sketch/limits/output.rs`

## 11.6 Public integration test organization

Keep one integration crate per package.

- `[ADDITION]` `conkit-signature/tests/public_api/{support,async_contract,boundaries,workflows}.rs`
- `[ADDITION]` `conkit-sketch/tests/public_api/{support,async_contract,boundaries,workflows}.rs`
- `[MODIFICATION]` Each existing `tests/public_api.rs` becomes a small module root.

## 11.7 Documentation and ownership maps

- `[MODIFICATION]` [ARCHITECTURE.md](/Users/connorsanders/RustroverProjects/contract-kit/ARCHITECTURE.md): three-crate boundaries, partial CLI rustdoc projection, domain report views.
- `[MODIFICATION]` [conkit/ARCHITECTURE.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit/ARCHITECTURE.md): compiler, extraction coordinator, document, bounded-output, catalog, and report module maps.
- `[MODIFICATION]` [conkit-signature/ARCHITECTURE.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/ARCHITECTURE.md): backend family, canonical extraction, YAML and rustdoc ownership, report views.
- `[MODIFICATION]` [conkit-sketch/ARCHITECTURE.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/ARCHITECTURE.md): request-owned workflows, one-pass YAML analysis, scalar editor, matching/limit modules.
- `[MODIFICATION]` [conkit/AGENTS.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit/AGENTS.md), [conkit-signature/AGENTS.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/AGENTS.md), and [conkit-sketch/AGENTS.md](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/AGENTS.md): update stale ownership and test-placement rules.
- `[MODIFICATION]` [CHANGELOG.md](/Users/connorsanders/RustroverProjects/contract-kit/CHANGELOG.md): record public API cleanup and report that runtime/wire contract behavior is otherwise unchanged.
- `[DELETION]` Do not copy module descriptions into README or root operational docs.

---

# Audit finding coverage matrix

| Audit finding | Resolution |
|---|---|
| 1. Raw clap extraction state | Phase 2 typed request/coordinator; no raw options below args |
| 2. 512 MiB coordinate table | Phase 3 batched endpoints; Phase 7 removes signature-side table |
| 3. Double rustdoc decode | Phase 3 narrow CLI projection; full decode only in signature |
| 4. `compiler.rs` monolith | Phase 3 ownership deletion, then Phase 11 split |
| 5. Contract parsing context/budgets | Phase 4 `ContractLocation` and typed YAML counters |
| 6. Closed extraction family | Phase 5 private trait, concrete owners, explicit dispatcher |
| 7. Diagnostic cloning/reinsertion | Phase 5 one operation-scoped collector |
| 8. YAML origin/option soup | Phase 6 typed origin and declaration body |
| 9. Repeated extraction invariants | Phase 5 `RustExtraction::from_roots` |
| 10. Clone-heavy rustdoc converter | Phase 7 immutable index, mutable inventory, borrowed items |
| 11. Sketch contract/scalar monolith | Phase 9 scalar owner; Phase 11 split |
| 12. Repeated sketch YAML scans | Phase 9 combined event analysis and rendered-CST deletion |
| 13. Sketch limit duplication | Phase 8 `LimitCharge`, `OutputFailure`, concrete writers |
| 14. Impossible inventory validation | Phase 8 derive counts once; remove `InventoryError` |
| 15. Matcher repeated work | Phase 8 one scanner, transactional diagnostics, unit success |
| 16. Refactor-obstructing tests | Phases 1 and 10 behavioral/AST tests and pruning |
| 17. 24-leaf scenario matrix | Phase 1 table test plus four representative leaves |
| 18. CLI report mirrors | Phase 4 opaque domain-owned views |
| 19. CLI bounded I/O duplication | Phase 4 bounded writer/read/path/publication consolidation |
| 20. Layout parallel options | Phase 2 `LayoutExtraction` enum and merge receiver |
| 21. YAML decode lifecycle | Phase 6 raw fields directly through one decoder |
| 22. Builder/pass duplication | Phase 5 one inventory collector |
| 23. Duplicate declaration kinds | Phase 5 one semantic `RustItemKind` |
| 24. Diff context clones | Phase 7 borrowed buffers/labels and fixed digests |
| 25. Small signature wrappers | Phases 5–6 deletion list |
| 26. Sketch collection trees | Phase 8 sorted vector, binary lookup, two-way merge, canonical fields |
| 27. Normalization second traversal | Phase 8 one-pass range emission |
| 28. Repeated locator facts | Phase 8 `ContractDocumentLocator` |
| 29. Fuzz setup duplication | Phase 10 shared domain harness owners |
| 30. Misleading benchmark rows | Phase 10 one-worker typed benchmark |
| Empty determinism test | Phase 1 deletion |
| CLI help matrix | Phase 1 deletion |
| Reconciliation cancellation/path repetition | Phase 4 |
| Unobservable normalization field | Phase 8 coordinated `0.2.0` deletion |
| One-field sketch workflow wrappers | Phase 8 deletion |

---

# Final dead-symbol and dependency audit

The final tree should contain none of:

```text
RustDeclarationKind
RustInventoryBuilder
RustInventoryPass
RustYamlGenerationSource
RustSketchSource
RustSketchResolver
RustNewDocumentPlan
RustAssociatedConversion
RustYamlShorthandSignature
RustYamlSignatureCommonInput
RustYamlSignatureCommon
RustYamlCallableParts
RustYamlImplementedTraitInput
RustYamlFunctionAbiInput
RustYamlVisibilityText
RustYamlShorthandGenericsInput
RustYamlMapField
RustYamlReceiverText
RustYamlShorthandSignatureOutput
SketchCheck
SketchDiff
SketchGenerator
ReportFiles
InventoryError
CompilerSourceCoordinates::scalar_starts
CompilerSourceFileIndex::scalar_starts
Arc<SignatureLimits>
original_bytes: Option
original_document: Option
```

Cargo metadata must show:

- `conkit` no longer directly depends on `rustdoc-types`.
- `conkit-signature` remains the rustdoc schema owner.
- `conkit-sketch` remains independent of signature parsing and OS boundaries.
- No fourth production package or shared core crate.

# Validation sequence

After each phase, run narrowed package tests. At the final gate run:

```shell
git diff --cached --check
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

Additional required validation:

```shell
cargo test --locked -p conkit --test check
cargo test --locked -p conkit --test generate
cargo test --locked -p conkit --test dependency_policy
cargo test --locked -p conkit --test domain_conformance
cargo test --locked -p conkit --test scenario_harness
cargo test --locked -p conkit --test scenarios

cargo test --locked -p conkit-signature --lib
cargo test --locked -p conkit-signature --test public_api
cargo doc --locked -p conkit-signature --no-deps

cargo test --locked -p conkit-sketch --lib
cargo test --locked -p conkit-sketch --test public_api
cargo bench --locked -p conkit-sketch --bench matcher --no-run

cargo +nightly-2026-07-01 check \
  --manifest-path fuzz/Cargo.toml \
  --locked \
  --bins
```

Final acceptance requires:

- Byte-identical standalone and combined reports.
- Byte-identical no-op and unaffected lossless generation.
- Identical digest values and diff ordering.
- Identical CLI grammar, exit codes, stdout/stderr, and error precedence.
- No source-sized scalar index.
- No duplicated diagnostic snapshots.
- No new executor/runtime coupling.
- No production `#[cfg(test)]` shims.
- No direct compiler executable invocation.
- No cross-crate limits, errors, catalogs, pools, or persistence abstraction.
- Net workspace reduction within the 6,000–9,000-line target, or a written per-phase explanation for any preserved safety code that narrows that range.