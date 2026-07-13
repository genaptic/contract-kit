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

- `conkit` owns the `conkit` command surface, `clap` parsing, filesystem and
  process boundaries, the shared executable identity, terminal output, local
  persistence, and cross-domain orchestration. See
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

## Domain runtime boundary

Both domain kits expose executor-neutral async operations over owned in-memory
catalogs. Each operation asynchronously awaits per-kit admission, then runs one
complete root workflow on a reusable Rayon pool. The selected worker count is
both the worker budget and the admitted-root capacity. Callers own bounds on
the pending tasks and catalogs they create, as well as their waiting deadlines;
a deadline does not preempt finite CPU work that has already started. Detailed
cancellation and worker behavior belongs in each domain crate's architecture.

## Command orchestration

The CLI grammar lives in `conkit/args.rs`. Async dispatch and adaptation to
domain requests belong to the `conkit::command` module: `conkit/command.rs` is
the exhaustive `AppCommand` facade, while its `check`, `generate`, `archive`,
and `diff` children own the concrete workflows. The four commands cross
boundaries as follows:

- `check` selects direct root-level combined YAML documents, binds each
  document's `root` to `--source`, and reads only its disjoint `files`
  allowlist. A family-specific target delegates report rendering to that
  domain. `check all` asks both domains to check without domain-owned report
  files, then the CLI renders and persists one combined report.
- `generate` creates or updates combined documents. Signature generation
  preserves stable labels and valid sketch sections; the signature resolver
  then extracts seeds for existing links, and sketch generation refreshes only
  those records. A version-3 ownership journal, locked preflight, and
  sequential writes that are individually atomic persist the completed
  catalog. This recoverable coordination is not one atomic multi-file
  transaction.
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

Keep this document structural. Product usage belongs in `README.md`,
operational agent instructions belong in `AGENTS.md`, scenario authoring belongs
in `test/scenarios/README.md`, and crate-specific structure belongs in each
crate's `ARCHITECTURE.md`.
