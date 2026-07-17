# Finalized staged-diff bloat and cleanup audit

The staged tree is behaviorally sound, but materially overbuilt in four areas: compiler extraction, signature YAML/rustdoc lowering, sketch YAML/matching, and overlapping test infrastructure. The best reductive refactor stays inside the existing three crates and strengthens their adapters; a shared `conkit-core` would violate the intended architecture.

Scope audited:

- 448 staged files, approximately `+86,274/-11,301`.
- 149 Rust files, approximately `+76,104/-9,022`.
- `conkit`: 41 files, `+16,845/-1,799`.
- `conkit-signature`: 51 files, `+44,503/-5,229`.
- `conkit-sketch`: 18 files, `+14,731/-1,767`.
- Current Rust size is roughly 28.8K lines in `conkit`, 53.4K in `conkit-signature`, and 18.6K in `conkit-sketch`.

No files were changed.

## Highest-priority findings

### 1. Raw clap state leaks throughout compiler extraction

[`SignatureOptions`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/args.rs:161) represents compiler selection as a large `Option`/boolean matrix. It is validated at the clap boundary, revalidated by [`CompilerExtractor::extract`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:424), cloned into [`SignatureGenerationInput`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/generate.rs:30), and passed throughout compiler project/argument construction.

Check and generation then implement parallel requested-versus-persisted extraction state machines:

- [`check.rs:117`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/check.rs:117)
- [`generate.rs:196`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/generate.rs:196)

They duplicate mismatch messages, compiler-root validation, warnings, extraction, and artifact validation.

Refactor direction:

- Convert clap DTOs once into a closed runtime value such as `Syntax | Compiler(CompilerRequest)`.
- Give compiler selection typed target and feature values instead of `library`, `binary`, `all_features`, and related independent fields.
- Add one CLI-owned extraction planner reconciling requested and persisted modes.
- Preserve the small policy difference: check requires persisted compiler metadata, while fresh generation may establish it.
- Make `CompilerExtractor` accept only the validated compiler request.
- Remove `all: bool` and the `SignatureOptions` clone in [`generate.rs:44`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/command/generate.rs:44).

This should delete several validation branches and make invalid compiler states unrepresentable after argument parsing.

### 2. Compiler source-coordinate indexing can allocate roughly 512 MiB for one admitted 64 MiB file

[`CompilerSourceCoordinates`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:2599) stores a `usize` for every Unicode scalar in `scalar_starts`, plus line indexes. The catalog default allows a 64 MiB file at [`catalog.rs:62`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog.rs:62). On a 64-bit target, a 64 MiB ASCII source therefore creates approximately 512 MiB of scalar offsets, outside [`CompilerLimits`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:1155).

Refactor direction:

- Keep only line byte starts and scan the selected line for scalar columns.
- Alternatively, collect and deduplicate all requested span endpoints first and resolve them in one source pass.
- Meter any remaining auxiliary index.
- Preserve one-indexed Unicode-scalar column behavior and cancellation checkpoints.

This is both a bloat issue and a resource-boundary defect.

### 3. The CLI fully deserializes rustdoc JSON that `conkit-signature` immediately deserializes again

`conkit` reads and fully decodes the artifact at [`compiler.rs:532`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:532) and [`compiler.rs:775`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:775). `conkit-signature` then performs another full `rustdoc_types::Crate` deserialization at [`rustdoc.rs:581`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs:581).

The CLI only needs the envelope, target/private flags, root identifier, and each item’s ID, crate ID, and span for local source mapping. It should not materialize declaration/type semantics.

The source translator also copies up to one million index entries into a `BTreeMap` merely to sort them at [`compiler.rs:2485`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:2485).

Refactor direction:

- Deserialize a narrow private CLI projection containing only source-mapping facts.
- Leave the full rustdoc graph solely to `conkit-signature`.
- This may remove `rustdoc-types` from `conkit` production dependencies.
- Sort a pre-sized `Vec` of references rather than allocating a second tree.

Filesystem/source mapping must remain in `conkit`; semantic rustdoc lowering remains in `conkit-signature`.

### 4. `compiler.rs` is a multi-responsibility monolith

[`compiler.rs`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:1) is 4,781 lines: approximately 2,958 production lines and 1,823 test lines. It combines:

- private rustdoc probe protocol;
- extraction orchestration;
- resource ledgers and temporary trees;
- compiler identity/configuration;
- process groups, pipes, deadlines, cancellation, and cleanup;
- Cargo package/target resolution;
- source translation;
- a very large error enum.

The probe protocol is also split across same-root carrier families: `RustdocProbeRequest`, `RustdocProbeCapture`, and `RustdocProbeSession`. Process cleanup repeats group/leader polling and reap logic at [`compiler.rs:1645`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:1645) and [`compiler.rs:1705`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/compiler.rs:1705).

Refactor direction:

- First delete the duplicate rustdoc parse/index work.
- Consolidate the probe protocol into one cohesive boundary owner.
- Consolidate cleanup into one process-reap state owner carrying target and policy.
- Then split private modules such as `probe`, `limits`, `process`, `project`, `source`, and `error`.
- Retain one public/internal compiler facade and keep all process/filesystem ownership in `conkit`.

A file split by itself is organizational, not reductive; it should follow the ownership cleanup.

### 5. Contract-document parsing repeats context and budget mechanics

[`conkit/contracts/document.rs`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs:1) has grown to 1,314 production lines. `(contract_file, document_index, cancellation)` is threaded through many methods, and the same catalog-to-OS-path conversion is rebuilt throughout error construction.

`DocumentHeader::into_plan` contains a large extraction conversion even though [`ExtractionHeader`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs:129) owns the corresponding data.

The YAML budget path separately represents every resource in:

- limits;
- counters;
- checked arithmetic;
- raw-budget breach translation;
- semantic-budget breach translation.

See [`document.rs:383`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/document.rs:383) through approximately line 736.

Refactor direction:

- Introduce one document-location/parse-context value owning file, document index, cancellation, and contextual error construction.
- Move extraction and crate conversion to the corresponding header receivers.
- Introduce a typed private resource/counter owner with one accumulation method and one breach-to-resource mapping.
- Keep the raw scan and semantic replay; those two passes enforce different guarantees.

### 6. Syntax/compiler extraction is a closed implementation family without one dispatch contract

Extraction behavior is distributed across repeated matches in:

- [`parser/mod.rs:587`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs:587)
- [`parser/mod.rs:666`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs:666)
- [`yaml/sketch.rs:243`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/sketch.rs:243)
- [`yaml/render.rs:48`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/render.rs:48)

Secondary enums such as `RustYamlGenerationSource` and `RustSketchSource` repeat projection and source-text behavior.

Refactor direction:

- Add one private, uniquely named extraction-backend trait.
- Implement it for concrete syntax and compiler owners.
- Retain one explicit private enum dispatcher with exhaustive matching.
- Let rendering and sketch resolution consume implementation-neutral projection/source access.
- Do not introduce trait objects, macros, `async_trait`, or a universal Rust AST.

The syntax and rustdoc AST adapters themselves should remain concrete and separate.

### 7. Capability diagnostics are cloned and reinserted cumulatively

The same operation collector already flows through source-graph and inventory construction, but diagnostics are:

- cloned into every graph at [`source_graph.rs:778`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/source_graph.rs:778);
- reinserted into the same collector at [`inventory_builder.rs:57`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/inventory_builder.rs:57);
- cloned into every projection at [`inventory_builder.rs:265`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/inventory_builder.rs:265);
- reinserted again during document checking at [`parser/mod.rs:524`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs:524).

Later projections snapshot diagnostics accumulated for earlier documents, creating potentially quadratic cloning and deduplication.

Refactor direction:

- Remove diagnostic storage from `RustSourceGraph` and `RustParsedProjection`.
- Let one operation-scoped collector remain the sole owner.
- Consume it once when constructing final warnings/check output.

This is one of the clearest direct deletion opportunities in the staged signature implementation.

### 8. Signature YAML rendering has invalid-state and option-soup models

[`RustYamlGeneratedDocument`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/render.rs:325) represents new/existing origin using two independent `Option`s plus an empty-vector sentinel. Consumers repeatedly rediscover the state and contain errors for combinations the constructors should make impossible.

[`RustYamlShorthandSignatureOutput`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/render.rs:2412) is a roughly 30-field DTO containing a string signature kind and optional fields for every declaration family. Constructors initialize all irrelevant fields and mutate a subset afterward.

Refactor direction:

- Replace the origin fields with `New | Existing { bytes, document, signature_order }`.
- Replace the output option soup with common metadata plus a typed declaration-body enum.
- Use explicit serialization/flattening to retain the current YAML format.
- Share neutral bidirectional leaf codecs where input and output are genuinely symmetric. For example, `RustYamlAttributesInput` is already imported by the renderer, showing that its “Input” name does not reflect actual ownership.
- Keep distinct DTOs where tolerant input and canonical output truly differ.

### 9. Signature extraction and crate-layout invariants are reimplemented at several layers

Empty crate lists, `.rs` requirements, root allowlisting, ID validity/uniqueness, ordering, and conversion to sets/maps appear in:

- [`api.rs:1203`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/api.rs:1203)
- [`yaml/document.rs:85`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/document.rs:85)
- [`source_graph.rs:82`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/source_graph.rs:82)
- [`yaml/input.rs:684`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs:684)

Refactor direction:

- Make the domain `RustExtraction` value the invariant owner.
- Let public/YAML adapters provide raw values and contextualize typed construction errors.
- Delete preceding normalization/validation copies.
- Preserve direct domain validation; do not trust the CLI’s earlier validation.

### 10. `RustdocConverter` is clone-heavy because immutable artifact data and mutable lowering state share one owner

[`RustdocConverter`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs:907) holds the full rustdoc document, source map, sources, limits, usage, cancellation, outputs, visited sets, and allocator. Its approximately 2,500-line implementation covers module traversal, exports, declarations, implementations, generics, types, attributes, visibility, and source provenance.

The production portion contains approximately 205 `clone`/`to_owned`/`to_string` calls. Full `rustdoc_types::Item` or inner values are cloned repeatedly during module, field, variant, implementation, and associated-item lowering; examples begin at [`rustdoc.rs:1136`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs:1136), [`rustdoc.rs:1260`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs:1260), and [`rustdoc.rs:1574`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/rustdoc.rs:1574).

Refactor direction:

- Separate an immutable artifact/index/source view from mutable lowering state.
- Let lowering borrow items while mutating only the separate state owner.
- Give module/export traversal, declaration lowering, type lowering, and provenance cohesive receiver owners.
- Preserve concrete rustdoc-to-domain conversion; do not add a universal intermediate AST.

After that reduction, split the remaining artifact, module/export, type, declaration, and provenance modules.

### 11. `conkit-sketch/contract.rs` combines too many independent subdomains

[`contract.rs`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:1) is 5,361 lines: about 3,225 production and 2,136 test lines. It combines semantic diff/digests, catalog parsing, budget inspection, link resolution, lossless editing, scalar encoding, and semantic conversion.

The scalar edit path is particularly overmodeled. One value moves through:

- `SketchScalarSource`;
- `SketchScalarRenderContext`;
- `SketchCodeNode`;
- `SketchScalarNode`;
- `SketchScalarEnvelope*`;
- `SketchBlockPresentation`;
- `SketchScalarPresentation`;
- `SketchScalarRendering`.

Relevant spans include [`contract.rs:1210`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:1210), [`contract.rs:1764`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:1764), and [`contract.rs:2333`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:2333).

Refactor direction:

- Replace the lifecycle carrier family with one scalar codec/editor owner.
- Represent preferred versus safe-fallback rendering with a closed data-carrying enum.
- Centralize retagging, line-ending conversion, validation, and error mapping.
- Represent inline versus block presentation as an enum rather than `style + Option<block-data>`.
- Preserve fail-closed anchor/alias handling and final whole-document semantic validation.
- After deletion, split `model`, `parse`, `resolve`, `diff`, and `edit/scalar` within `conkit-sketch`.

### 12. Sketch YAML is scanned repeatedly

The current path performs:

1. a complete raw budget scan at [`limits.rs:180`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs:180);
2. another event scan for document/null/index metadata at [`contract.rs:958`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:958);
3. typed semantic parsing at [`contract.rs:787`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:787).

Changed generation also CST-parses the original, CST-parses rendered output, and finally performs the authoritative semantic reparse.

Refactor direction:

- Combine the raw budget scan and document metadata analysis into one budgeted/cancellable event analysis.
- Investigate whether edit construction can prove unchanged ranges without reparsing rendered CST.
- Keep the final semantic reparse at [`contract.rs:915`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:915).
- Keep the no-op fast path.

### 13. Sketch limits duplicate charge, breach, and bounded-writer machinery

[`conkit-sketch/limits.rs`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs:1) is 2,036 lines. It contains 24 `LimitExceeded::new` call sites, repeated raw/semantic YAML resource mappings, and similar failure/accounting logic in diagnostic, scratch, and returned-output writers.

`ReturnedOutput` stores mutually exclusive failure state as `observed_at_least: Option<u64>` plus `cancelled: bool` at [`limits.rs:1154`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/limits.rs:1154).

Refactor direction:

- Keep all public sketch-domain limit types nominal.
- Add a small private domain-local charge/breach owner.
- Share only the bounded append/failure core between writers.
- Use one `Option<OutputFailure>` enum for returned output.
- Do not share errors or limits across crates and do not add macros or aliases.

### 14. Sketch inventory validates counts that the producer already derived

The matcher derives failed and matched totals at [`matcher.rs:659`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:659). [`inventory.rs:221`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/inventory.rs:221) then accepts those derived values as inputs and validates relations that should already be structural invariants.

`InventoryError` and its error-wrapper plumbing exist primarily to represent impossible internal combinations.

Refactor direction:

- Pass primitive scope facts and diagnostics to one constructor.
- Derive all dependent totals exactly once.
- Make construction infallible after checked arithmetic.
- Remove `InventoryError` and tests that manufacture impossible internal states.
- Preserve public count fields if compatibility requires them.

### 15. Sketch matching performs avoidable repeated work

Several related reductions are available:

- `AtLeastOne` and `ExactlyOne` have parallel scan loops at [`matcher.rs:476`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:476).
- Each sketch scans the complete normalized source independently at [`matcher.rs:121`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:121).
- A miss performs another complete nearest-candidate pass.
- Candidate coordinates are constructed twice at [`matcher.rs:317`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:317) and [`matcher.rs:339`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:339).
- Not-matched diagnostics are measured, materialized, and then fully serialized again at [`matcher.rs:225`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/matcher.rs:225).
- `MatchEvaluation::Satisfied` carries an occurrence value whose only producer value is `1`.

Refactor direction:

- Add one concrete occurrence scanner parameterized by the closed policy enum.
- Preserve early exit for `AtLeastOne` and bounded span retention for `ExactlyOne`.
- Commit the exact diagnostic-byte reservation after evidence materialization instead of serializing the completed diagnostic again.
- Put coordinate creation on the position owner.
- Make `Satisfied` a unit variant.
- Consider a per-source first-line/candidate index only if benchmarks justify it.
- Keep the second nearest-candidate pass for misses unless a replacement retains its bounded-allocation behavior.

### 16. The test surface now obstructs reductive refactoring

Several staged tests assert source spelling rather than behavior:

- [`dependency_policy.rs:131`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/dependency_policy.rs:131) searches exact compiler/store function bodies and statement ordering.
- [`check.rs:198`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/check.rs:198) inspects command source ordering.
- [`generate.rs:317`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/generate.rs:317) asserts method names, source substrings, and absence of selected calls.
- [`conkit-sketch/tests/public_api.rs:1457`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/tests/public_api.rs:1457) uses substring scans and a hardcoded module list.
- [`conkit-signature/tests/public_api.rs:2234`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/tests/public_api.rs:2234) scans documentation and module/export spelling.
- [`dependency_policy.rs:444`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/dependency_policy.rs:444) implements an incomplete line-oriented Rust attribute parser.

These tests can reject harmless reorganizations and can accept semantically incorrect code that retains expected text.

Refactor direction:

- Replace ordering scans with behavioral state-machine tests.
- Test process lifecycle through a fake executable or process-state owner.
- Test publication cancellation on the store lifecycle owner.
- Use Cargo metadata for dependency ownership.
- If source-policy scanning must remain, parse Rust syntax or centralize it in one workspace test.
- Keep public API/serde/Send boundary tests; remove tests of exact private spelling.

There is also substantial behavioral duplication:

- `conkit-sketch/tests/public_api.rs` grew from 687 to 3,312 lines and from 21 to 79 tests. Exact whitespace, occurrence, scalar-edit, refresh, and count cases overlap exhaustive module-local tests.
- Keep one representative public check/generate/diff round trip plus public serde, Send, error, and limit boundaries. Leave exhaustive parser/matcher/editor matrices at the unit layer.
- Consolidate overlapping fixture families in [`public_api.rs:1219`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/tests/public_api.rs:1219) onward.

### 17. The 24-leaf check-mode scenario matrix is a costly combinatorial E2E representation

The matrix represents:

- three targets;
- four mode spellings;
- matching versus mismatching.

All 24 leaves copy the same contract, while the sources have only two distinct contents. Their manifests total 676 lines and the directories occupy approximately 384 KiB. The harness then hardcodes all 24 coverage keys at [`support/scenario.rs:62`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/support/scenario.rs:62), while [`scenarios.rs:7`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/scenarios.rs:7) freezes the exact total at 139 leaves.

The scenario guide correctly forbids shared fixtures inside the scenario system at [`test/scenarios/README.md:3`](/Users/connorsanders/RustroverProjects/contract-kit/test/scenarios/README.md:3).

Therefore, do not deduplicate these leaves through sibling references or shared files. Instead:

- move the target × mode × outcome truth table to one table-driven binary integration test;
- retain a small representative set of E2E leaves for report files, stdout/stderr, exit behavior, and filesystem shape;
- change coverage bookkeeping from exact leaf names/counts to semantic behavior requirements;
- delete the exact `139` count gate.

## Medium-priority findings

### 18. The CLI mirrors domain report schemas

Domain-owned report shapes exist in [`conkit-signature/api.rs:675`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/api.rs:675) and [`conkit-sketch/report.rs:116`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/report.rs:116), but [`conkit/report.rs:332`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/report.rs:332) independently recreates signature and sketch report DTOs.

The staged signature digest fields already required updating the CLI mirror.

Expose domain-owned serializable report views and let `conkit` own only the combined `{passed, signatures, sketches}` envelope and persistence.

### 19. `conkit` repeats bounded I/O mechanics in several owners

Concrete duplication exists between:

- archive and report bounded writers at [`archive.rs:270`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/archive.rs:270) and [`report.rs:234`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/report.rs:234);
- archive compressed reads and catalog reads at [`archive/source.rs:128`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/archive/source.rs:128) and [`catalog.rs:145`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog.rs:145);
- capability-relative path walkers/openers at [`catalog/path.rs:284`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/path.rs:284), [`catalog/path.rs:446`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/path.rs:446), and [`catalog/path.rs:644`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/path.rs:644);
- atomic-publication cleanup branches at [`catalog/store.rs:348`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/store.rs:348).

Refactor direction:

- Add one concrete `Write` wrapper for cancellation and bounded-byte accounting.
- Give `CatalogReadBudget` a receiver for bounded reads with an additional wire ceiling.
- Consolidate parent resolution and no-follow regular-file opening within the catalog-path owner.
- Introduce a store-local atomic-publication lifecycle value with `Drop` cleanup.
- Do not build a universal archive/report/store persistence abstraction; their publication and error semantics differ.

### 20. `ContractLayout::extraction` models one state through parallel options

[`layout.rs:254`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/layout.rs:254) separately tracks mode, compiler context, compiler location, seen crates, and crate vectors, then handles combinations parsing should have made impossible.

Use one private aggregate:

- absent;
- syntax with crates;
- compiler with crates, context, and origin.

Its merge receiver should own cross-document consistency. Also compare compiler contexts by reference rather than cloning all strings/vectors at [`layout.rs:112`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/contracts/layout.rs:112).

### 21. Signature YAML decoding has too many lifecycle representations

One signature passes through raw, shorthand, common-input, common, and named forms:

- [`input.rs:1448`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs:1448)
- [`input.rs:1625`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs:1625)
- [`input.rs:2569`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs:2569)
- [`input.rs:2955`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/input.rs:2955)

Keep the raw shape needed to distinguish missing, null, forbidden, and required fields, but decode from it directly into a named domain entry through one cohesive decoder owner.

Numerous micro-wrapper structs—such as `RustYamlCallableParts`, `RustYamlVisibilityText`, `RustYamlMapField`, and `RustYamlReceiverText`—temporarily borrow one value without owning a lasting invariant. Move that behavior onto the real DTO/domain owner or the decoder context.

### 22. `RustInventoryBuilder` and `RustInventoryPass` repeat almost the same state

[`inventory_builder.rs:23`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/inventory_builder.rs:23) constructs a second owner at [`inventory_builder.rs:79`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/inventory_builder.rs:79), forwarding most fields and lifetimes.

Collapse them into one inventory collector that initializes its symbol table and accumulators before running receiver-method phases.

### 23. Declaration kind is represented twice

[`RustDeclarationKind`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/types/declaration.rs:238) and [`RustItemKind`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/signature_id.rs:141) contain the same 16 semantic kinds, followed by a full one-to-one conversion.

Use one crate-private semantic kind for declaration identity and repeatability. Keep the YAML kind distinct because its wire subset intentionally excludes implementations.

### 24. Signature semantic diff copies temporary context buffers

[`InventoryGroupDigest`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/inventory.rs:610) clones optional extraction/document metadata buffers. [`InventorySemanticIndex`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/inventory.rs:708) then clones labels and digests into secondary maps.

Use temporary views borrowing metadata and labels from the inventories while owning only fixed-size digests.

### 25. Small signature wrappers can be deleted

- `RustAssociatedConversion` is only a `Vec` wrapper at [`item_converter.rs:1348`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/item_converter.rs:1348).
- Two type-converter constructors differ only in how they obtain `generics` at [`item_converter.rs:1266`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/item_converter.rs:1266).
- `RustGenerationPlan::New(RustNewDocumentPlan)` is a same-root one-use payload at [`yaml/document.rs:13`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/yaml/document.rs:13).
- `SignatureParser` contains a second `Arc<SignatureLimits>` even though the parser itself is already behind an `Arc`, at [`parser/mod.rs:34`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-signature/languages/rust/parser/mod.rs:34).

Return the associated-item vector directly, use one generics-based converter, inline the `New { layout, extraction }` fields, and let the sketch resolver borrow limits.

### 26. Sketch collections repeatedly rebuild trees

[`SketchContracts`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:85) stores an ID-sorted vector after collecting through a `BTreeMap`. Diff then creates two maps and a union set at [`contract.rs:119`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:119), while generation creates another lookup map at [`generate.rs:65`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/generate.rs:65).

Either retain a keyed canonical collection or use binary search and two-way merge over the sorted vector.

Likewise, [`SketchField`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/api.rs:742) changes are produced once in fixed order but stored in a `BTreeSet`. A canonical `Vec` would remove tree allocation and comparison.

### 27. Sketch normalization performs a second full traversal

[`normalize.rs:109`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/normalize.rs:109) builds normalized bytes and line counts; [`normalize.rs:147`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/normalize.rs:147) traverses the completed bytes again to create ranges.

Emit ranges while appending normalized newlines and finalize the final range once. Keep cached normalized bytes and ranges because matching, digesting, and diffing reuse them.

### 28. Sketch locator facts are repeated across several carriers

`SketchContract`, `SignatureLink`, and `PendingSketch` repeat contract file, document index, file, type, and identity fields around:

- [`contract.rs:233`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:233)
- [`contract.rs:3144`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:3144)
- [`contract.rs:3153`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/contract.rs:3153)

Introduce one internal document locator and rename `PendingSketch` to the domain fact it represents. Do not remove the separate declaration/link copies needed to validate agreement.

### 29. Fuzz harnesses repeat setup and sometimes hide regressions

Four sketch fuzz targets and several signature targets repeat input limits, thread-local kit creation, paths, catalog setup, and `block_on`:

- [`sketch_yaml.rs:13`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/sketch_yaml.rs:13)
- [`sketch_normalization.rs:13`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/sketch_normalization.rs:13)
- [`sketch_matching.rs:12`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/sketch_matching.rs:12)
- [`yaml_edit.rs:14`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/yaml_edit.rs:14)
- [`signature_yaml.rs:14`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/signature_yaml.rs:14)
- [`rust_syntax.rs:14`](/Users/connorsanders/RustroverProjects/contract-kit/fuzz/fuzz_targets/rust_syntax.rs:14)

Add fuzz-package-local concrete signature/sketch harness owners. Keep each fuzz binary and corpus separate.

Static path/catalog failures and valid fixed-contract operations currently return silently in several targets. Programmer-invariant setup should `expect`; only arbitrary malformed input should be discarded normally.

### 30. Half of the sketch matcher benchmark matrix does not measure worker scaling

[`benches/matcher.rs:60`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/benches/matcher.rs:60) runs every case with one and four workers, but permits one active root operation and submits one job per iteration. Production matching is sequential.

Remove the four-worker rows or replace them with a separate concurrent-root/scheduler benchmark. Also replace the benchmark’s string occurrence policy around [`matcher.rs:231`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/benches/matcher.rs:231) with a closed enum.

## Lower-priority deletions

- Remove the weaker empty-catalog worker-determinism test at [`domain_conformance.rs:517`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/domain_conformance.rs:517); the preceding nonempty comparison covers the same invariant more strongly.
- Trim [`cli_help.rs`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/tests/cli_help.rs:8) predicate matrices already covered by exact help scenarios and argument-unit tests. Keep environment pinning and any unique binary-level precedence case.
- Let `CatalogReadBudget` provide cancellation checkpoints instead of passing it alongside the same cancellation object in [`catalog/reconciliation.rs:395`](/Users/connorsanders/RustroverProjects/contract-kit/conkit/catalog/reconciliation.rs:395).
- Move repeated owned-path conversion onto its ownership value in reconciliation.
- `SketchField::Normalization` is currently deliberately unobservable under the one accepted normalization policy at [`api.rs:742`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/api.rs:742). Remove it until a second normalization mode can actually produce that diff category.
- One-field sketch workflow wrappers around requests in [`api.rs:450`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/api.rs:450), [`generate.rs:12`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/generate.rs:12), and [`report.rs:70`](/Users/connorsanders/RustroverProjects/contract-kit/conkit-sketch/report.rs:70) are optional deletion candidates. This is lower confidence because the checked-in architecture explicitly names those owners.

## Recommended reductive refactor order

1. Replace source-scraping tests with behavioral seams so they do not block reorganization.
2. Introduce the typed CLI extraction request/planner and eliminate raw `SignatureOptions` from compiler internals.
3. Fix compiler coordinate indexing, narrow CLI rustdoc decoding, and remove the temporary `BTreeMap`.
4. Remove signature diagnostic snapshots/reinsertion and centralize extraction dispatch.
5. Replace signature YAML sentinel/option-soup states and centralize extraction invariants.
6. Separate rustdoc immutable input from mutable lowering to eliminate full-item cloning.
7. Simplify sketch inventory counts and matcher diagnostic/policy loops.
8. Consolidate sketch scalar editing and raw YAML scans.
9. Consolidate domain-local limit accounting without sharing nominal types.
10. Prune overlapping public tests, the check-mode scenario matrix, redundant fuzz setup, and meaningless benchmark rows.
11. Only then split the remaining monolithic files into focused modules.

That sequence produces actual deletion before moving surviving code around.

## Consolidations that should not be attempted

The following apparent duplication is intentional:

- Do not add `conkit-core`.
- Do not unify `FileCatalog`, `CatalogPath`, work-pool types, resource limits, or error enums across `conkit-signature` and `conkit-sketch`.
- Keep independent active/pending admission and direct domain limit validation.
- Keep the application-owned shared Rayon pool in `conkit`.
- Keep signature and sketch semantic diffing in their respective domain crates.
- Keep compiler process/filesystem/source-coordinate ownership in `conkit`.
- Keep syntax and rustdoc lowering as separate concrete adapters.
- Keep CLI catalog admission followed by domain revalidation.
- Keep generation baseline/preflight/commit revalidation; it is TOCTOU protection.
- Keep generate-all sequential: sketch generation consumes signature-produced seeds.
- Keep raw YAML analysis plus semantic parsing where they enforce different budgets.
- Keep the final post-edit sketch semantic reparse.
- Keep returned-output and retained-scratch accounting distinct.
- Keep duplicate-rejecting custom `FileCatalog` serde visitors.
- Keep separate fuzz binaries/corpora.
- If scenario leaves remain, keep each leaf independently owned; do not introduce shared scenario fixtures or factories.
- Do not genericize the explicit `ContractTarget` check/generation branches into a universal family runner.

## Positive audit results

I found no production:

- Rust item type aliases;
- impermissible top-level free helper functions;
- macro-generated dispatch systems;
- `async_trait` use;
- Tokio coupling;
- lock held across `.await`;
- production `#[cfg(test)]` shims;
- direct compiler executable invocation;
- domain-owned process or filesystem boundary violations.

The domain work pools are proportionate and executor-neutral. The CLI’s single `futures_executor::block_on` boundary is appropriate, and synchronous Cargo/rustdoc extraction occurs before domain work is awaited.

Validation completed successfully:

- `git diff --cached --check`
- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets --all-features`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --doc`
- `cargo test --locked --workspace --all-targets`

All passed against the current staged tree.