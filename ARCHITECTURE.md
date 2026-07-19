# Conkit Architecture

`conkit`, also described as Contract Kit, is a three-member Rust workspace. Its
command-line adapter orchestrates two independent byte/catalog domain crates
for the `check`, `generate`, `archive`, and `diff` command families.

## Workspace boundary

```text
user and operating system
          |
          v
       conkit ------> conkit-signature
          |
          +--------> conkit-sketch

conkit-signature !-> conkit-sketch
conkit-sketch    !-> conkit-signature
conkit-signature !-> conkit
conkit-sketch    !-> conkit
```

- `conkit` owns the `conkit` command surface, `clap` parsing, explicit Rust
  crate-root and target-kind selection, filesystem and process boundaries, the
  shared executable identity, terminal output, local persistence, and
  cross-domain orchestration. See
  [conkit/ARCHITECTURE.md](conkit/ARCHITECTURE.md).
- `conkit-signature` owns signature contract parsing, Rust source extraction,
  generation, inventory comparison, reports, and signature semantic diffing.
  See
  [conkit-signature/ARCHITECTURE.md](conkit-signature/ARCHITECTURE.md).
- `conkit-sketch` owns language-neutral sketch parsing, normalization, matching,
  reports, generation, and semantic diffing. See
  [conkit-sketch/ARCHITECTURE.md](conkit-sketch/ARCHITECTURE.md).

The domain crates exchange logical catalog paths and bytes with callers. They
do not own local filesystem roots, command grammar, terminal behavior, process
exits, or persistence. Both domain crates independently validate the shared
combined-document link facts their operations need. `conkit-signature`
validates links alongside complete signature semantics and resolves linked
Rust spans into neutral seed data. `conkit-sketch` builds its own minimal
signature-link index for link validation, normalization, and matching without
depending on the signature crate. The CLI adapts signature seeds into sketch
requests while keeping the domains independent.

All three packages use the maintained semantic YAML stack for typed v2
documents and reports. Only the two domain crates depend on the lossless YAML
syntax-tree editor. They retain original document bytes, skip CST construction
for exact semantic no-ops, edit only owned nodes for real changes, and reparse
changed bytes before returning them. The CLI does not own or expose a generic
YAML value tree. Its narrow header validator shares one cumulative YAML budget
across every catalog in a command operation, including both sides of diff, and
charges alias replay from the existing all-content typed parse.

## Rust extraction boundary

Every signature-bearing v2 document records exactly one extraction mode,
`rust_syntax_v2` or `rust_compiler_v1`, plus `profile: rust_api_v1` and one or
more explicit crate roots whose `library` or `binary` kind is fixed before the
document reaches the signature domain. For a fresh catalog, the CLI may prove
exactly one conventional root-level `lib.rs` or `main.rs` and construct its
library or binary root; zero, multiple, nonconventional, and disconnected
layouts require explicit typed CLI roots.
The signature domain builds logical module identity from those roots, inline
modules, allowlisted out-of-line `mod` declarations, and allowlisted `#[path]`
targets rather than guessing it from arbitrary source paths. A document's
exact `files` allowlist therefore bounds UTF-8 decoding, Rust parsing, graph
traversal, lexical owner resolution, inventory projection, and label
allocation.

`rust_syntax_v2` remains a syntax extractor rather than a compiler. It retains
modeled declarations and semantic attributes, resolves supported local paths,
and emits deterministic capability warnings for facts such as conditional
compilation, macro expansion, and unresolved reexport targets. Unsupported
reachable syntax and invalid module, owner, attribute, or visibility state fail
closed. The CLI's default check mode permits warning-only results, strict mode
requires no diagnostics, and warning mode preserves diagnostics without failing
the completed check.

`rust_compiler_v1` is a separate opt-in capability. The CLI selects exactly
one Cargo package and library/binary target and invokes the pinned
`nightly-2026-07-01` Cargo/rustdoc toolchain with locked dependency resolution
and disabled toolchain auto-installation in a bounded isolated target
directory. A private Cargo-owned probe captures the composed rustdoc `cfg`
context, while rustdoc JSON supplies the authoritative target when no explicit
target was requested. Cargo—not either domain crate—owns filesystem discovery,
build processes, source revalidation, and local-crate source mapping. The CLI
never invokes the compiler executable directly and warns that selected build
scripts and procedural macros run unsandboxed with the user's permissions. A
private partial Serde projection reads only the rustdoc envelope, target and
privacy flags, root ID, and each item's ID, crate ID, and span needed for local
source mapping; unknown semantic fields remain untouched. The original JSON
bytes pass unchanged to `conkit-signature`, which performs the sole complete
rustdoc-schema decode and owns compiler-resolved canonical semantics and
digests. Existing contracts reject mode or Cargo crate-identity mismatches
before domain work begins.

## Domain runtime boundary

Both domain kits expose executor-neutral async operations over owned in-memory
catalogs. `conkit` constructs one application-owned Rayon pool and injects the
same `Arc<ThreadPool>` into both kits. Worker threads, active root operations,
and pending root operations are independent budgets in both domain libraries.
The CLI configures one active root operation per nominal domain and
`max_pending_operations = 0`, so it has no admitted domain queue and saturation
fails immediately; an active workflow may still use all shared workers
internally. Embedders may configure a bounded pending queue through each
domain's nominal work options. Dropping queued work releases admission;
dropping running work requests cooperative cancellation at file, module,
source-group, diagnostic, and render boundaries rather than attempting unsafe
thread termination. The CLI installs one process-level Ctrl-C/termination
handler whose atomic cancellation source wakes the root executor, drops pending
domain work, and is also observed by compiler extraction. An active Cargo
process group or Windows job is terminated and explicitly reaped. A command
that has already completed its final poll wins the race, so an atomic
persistence commit is never reported as canceled after it succeeds.

The CLI bounds filesystem catalog entries, aggregate bytes, and bytes per file
while reading through opened handles, then each nominal domain validates its
own catalog, YAML, extraction or matching, diagnostic, and output budgets
again. The duplicated domain types are intentional: conformance tests keep
their shared behavior aligned without adding a fourth package, shared error
enum, or lowest-common-denominator limit trait.

## Command orchestration

The CLI grammar lives in `conkit/args.rs`. Async dispatch and adaptation to
domain requests belong to the `conkit::command` module: `conkit/command.rs` is
the exhaustive `AppCommand` facade, while its `check`, `generate`, `archive`,
and `diff` children own the concrete workflows. The four commands cross
boundaries as follows:

- `check` selects direct root-level combined YAML documents, binds each
  document's `root` to `--source`, and reads the union of their exact `files`
  allowlists; separate documents may intentionally share source paths. A
  family-specific target delegates report rendering to that
  domain. `check all` asks both domains to check without domain-owned report
  files, then the CLI renders and persists one combined report.
- `generate` creates or updates combined documents. A fresh signature-bearing
  catalog receives typed roots from the CLI, either from the sole conventional
  root proof or repeatable `CRATE_ID=KIND:RELATIVE_PATH` values; existing
  documents retain their recorded extraction context. Signature generation
  preserves stable labels and valid sketch sections. All-family generation
  returns surviving linked-item seeds from that same parsed projection;
  sketch-only generation uses the separate signature resolver. Sketch
  generation refreshes only those records. A version-3 ownership journal,
  locked preflight, and sequential
  writes that are individually atomic persist the completed catalog. This
  recoverable coordination is not one atomic multi-file transaction.
- `archive` encodes the complete mixed contract catalog through the CLI-owned
  version-1 gzip codec, fully syncs a sibling temporary, then publishes a
  collision-safe local file without clobbering an existing name.
- `diff` decodes an archive once in the CLI, asks each domain for its own
  semantic changes, and prints signature entries before sketch entries. A
  changed result is successful command output, not a process failure.

The target `all` is CLI orchestration across the domains, not a shared domain
model. The operating-system path types accepted by `clap` are resolved and
validated before the CLI constructs domain requests from `FileCatalog` and
`CatalogPath` values.

Standalone signature and sketch reports are serialized by their owning domain
crates through borrowed views. For `check all`, the CLI owns only the combined
envelope and serializes embedded domain views; it does not mirror either
domain's report DTOs or copy their diagnostics. This keeps wire-field order and
domain evolution with the semantic owner while leaving report-path inference
and filesystem publication in `conkit`.

## Scenario boundary

`conkit/tests/scenarios.rs` recursively discovers every checked-in
`test/scenarios/**/scenario.yml`, sorts the manifests, and executes each leaf
once in a fresh temporary workspace. It also audits the union of declared
coverage against the closed machine registry in
[`REQUIRED_COVERAGE_KEYS`](conkit/tests/support/scenario.rs).

`conkit/tests/support/scenario.rs` is the reusable harness facade and owns the
closed coverage-key registry. Its focused `error`, `manifest`, `sandbox`,
`steps`, `suite`, and `tree` children own parsing, fixture copying, sandbox
paths, typed steps, process assertions, captures, Cargo workspace validation,
exhaustive tree comparison, and coverage aggregation.
`conkit/tests/scenario_harness.rs` is the independent harness-regression facade
over its `execution`, `filesystem`, `manifest`, and `suite` children. The
canonical manifest schema, fixture and step rules, evidence definitions,
coverage declarations, and focused validation commands belong in the
[scenario authoring guide](test/scenarios/README.md).

Keep this document structural. Product installation and usage belong in
`README.md`; human development and pull-request workflow belong in
`CONTRIBUTING.md`; community behavior and reporting belong in
`CODE_OF_CONDUCT.md`; operational agent instructions belong in `AGENTS.md`;
scenario authoring belongs in `test/scenarios/README.md`; and crate-specific
structure belongs in each crate's `ARCHITECTURE.md`.
