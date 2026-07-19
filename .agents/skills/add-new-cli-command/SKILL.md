---
name: add-new-cli-command
description: "Add or change a command in Contract Kit's `conkit` Rust CLI. Use for clap grammar in `conkit/args.rs`, native async dispatch through `CommandContext` and `AppCommand`, CLI-owned filesystem and persistence work, catalog-based domain requests, command tests, scenarios, documentation, and checked locked validation. Do not use this skill as a generic Rust CLI template."
---

# Add New CLI Command

Extend the current `conkit` command surface without breaking its async dispatch,
catalog boundary, user-visible grammar, or scenario coverage.

## Current `conkit` architecture is authoritative

- Treat `conkit`, `conkit-signature`, and `conkit-sketch` as the existing workspace. Extend one
  of them before proposing another crate.
- Treat the `conkit` Cargo package, binary target, and displayed executable
  name as the same identity on every platform. The `.exe` suffix on Windows
  is only a platform filename convention.
- Define clap grammar in `conkit/args.rs`. Keep parser structs limited to raw
  `PathBuf` values, flags, and clap-owned validation.
- Adapt commands through the `conkit/command.rs` exhaustive facade and the
  owning `conkit/command/<verb>.rs` module using the existing native async
  contract:

  ```rust
  pub(crate) trait AppCommand {
      async fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()>;
  }
  ```

- Keep `App` orchestration in `conkit/app.rs` and shared runtime handles in
  `conkit/context.rs`. `CommandContext` owns one application-wide Rayon pool
  shared by both domains, their independent active/pending admission settings,
  the CLI's zero-pending policy, `CompilerExtractor`, `CatalogReadLimits`,
  process cancellation, and output. Do not construct a pool, compiler adapter,
  cancellation source, or filesystem limit policy per command.
- Confine OS paths, directory traversal, persistence, reports, archives, and
  ownership journals to CLI adapters. Pass `FileCatalog`, `CatalogPath`, byte
  payloads, and typed requests into `conkit-signature` and `conkit-sketch`.
- Keep requested-versus-persisted signature extraction reconciliation in the
  CLI-owned `SignatureExtractionCoordinator`. Compiler/Cargo execution stays
  in the CLI-owned `CompilerExtractor`; a domain receives only typed
  `RustExtractionInput`, never a manifest path or process responsibility.
- Reuse current dependencies and modules. Add a crate or dependency only when
  the requested behavior cannot fit the existing architecture and the user has
  authorized that expansion.

## Mandatory first steps

1. Load `$use-rust-best-practices-core` and
   `$use-rust-shell-cli-best-practices`.
2. Load `$use-rust-best-practices-testing` for behavioral or scenario changes,
   `$rust-code-structuring-best-practices` for new structs/enums/impls, and the
   dependency/platform skill for filesystem or Cargo changes.
3. Read root and CLI `AGENTS.md` and `ARCHITECTURE.md`, `README.md`, and
   `test/scenarios/README.md` before editing.
4. Inspect `conkit/args.rs`, `conkit/command.rs`, the relevant
   `conkit/command/<verb>.rs`, `conkit/app.rs`, `conkit/context.rs`, the
   relevant catalog capability, the owning domain API, and existing tests.

Suggested discovery commands:

```bash
find .agents/skills -maxdepth 4 -path '*/SKILL.md' -print 2>/dev/null | sort
find . -maxdepth 4 \( -name AGENTS.md -o -name ARCHITECTURE.md -o -name README.md \) -print | sort
find . -maxdepth 4 -name Cargo.toml -print | sort
rg "derive\((Parser|Subcommand|Args)|enum Command|trait AppCommand|CommandContext" conkit
rg "FileCatalog|CatalogPath|Request|Response" conkit conkit-signature conkit-sketch
rg --files test/scenarios conkit/tests
```

If `rg` is unavailable, use `grep -R`.

## Inputs to extract from the prompt

Determine:

- Command grammar: verb, subject, object, options, examples.
- Behavior: what the command should do and what side effects it has.
- Domain ownership: which business crate owns the logic.
- Filesystem scope: which paths are read, written, traversed, or removed.
- Resource and cancellation behavior: which existing operation-wide
  `CatalogReadBudget` is charged, where cancellation checkpoints occur, and
  which domain admission budget applies.
- Signature extraction: syntax or compiler mode, Cargo selection inputs, and
  any repeatable typed crate-root declarations.
- Output: stable stdout, reports or generated artifacts, and diagnostics.
- Error cases: not found, already exists, invalid input, permissions, partial failure.
- Tests required: success, failure, help, report/catalog output, filesystem
  side effects, and cross-platform paths.

Do not invent destructive behavior. Follow the existing generation ownership,
journal, collision, archive, and path-overlap safeguards.

## Implementation workflow

1. Mirror the existing `check`, `generate`, `archive`, and `diff` grammar before
   introducing a new verb, target, alias, or option.
2. Add parse-only clap data in `conkit/args.rs`.
3. Add an explicit exhaustive arm to `impl AppCommand for Command` and a native
   async receiver implementation for the command payload in its owning
   `conkit/command/<verb>.rs` module.
4. Resolve and validate OS paths in the CLI, including overlap and output
   policy checks. Reuse `CommandContext::catalog_read_limits` and process
   cancellation instead of creating command-local limits or cancellation.
5. Start one `CatalogReadBudget` for the complete filesystem-input operation,
   read inputs through budget-aware catalog APIs, construct a typed byte/catalog
   domain request, await the domain API, and persist returned catalogs in the
   CLI. Do not reset the ledger between recovery, source/contract reads,
   compiler source validation, archive decoding, or persistence preflight.
6. Put reusable contract semantics and typed errors in `conkit-signature` or
   `conkit-sketch`;
   keep clap, stdout/stderr, and filesystem I/O out of those crates.
7. Render stable user output through the current CLI output/report adapters.
8. Add the narrowest domain and CLI integration tests that prove the behavior.
9. Add positive and negative checked-in E2E cases when the external command
   contract changes. Use the manifest-aware runner and follow the
   [scenario guide](../../../test/scenarios/README.md).
10. Update product, architecture, and agent guidance that owns the changed
    behavior, then run locked validation.

## Command design checklist

For `conkit`, prefer the established command and target vocabulary:

```text
conkit check <all|signatures> --source DIR --contracts DIR --output FILE [MODE] [SIGNATURE EXTRACTION]
conkit check sketches --source DIR --contracts DIR --output FILE [MODE]
conkit generate <all|signatures> --source DIR --contracts DIR [--crate-root CRATE_ID=KIND:RELATIVE_PATH]... [SIGNATURE EXTRACTION] [--adopt-existing]
conkit generate sketches --source DIR --contracts DIR [--adopt-existing]
conkit archive --contracts DIR --archive DIR [--gzip]
conkit diff --contracts DIR --archive FILE
```

`SIGNATURE EXTRACTION` defaults to `--signature-extractor syntax`. Compiler
extraction is opt-in and uses the current Cargo-native grammar:

```text
--signature-extractor compiler --manifest-path FILE
  [--package SPEC] [--lib|--bin NAME]
  [--features FEATURES|--all-features] [--no-default-features]
  [--target TRIPLE]
```

`--features` is repeatable or comma-delimited. Generation's repeatable
`--crate-root CRATE_ID=KIND:RELATIVE_PATH` belongs only to `generate all` and
`generate signatures`; compiler/Cargo options belong only to signature-aware
check and generation targets.

Preserve existing singular aliases and mutually exclusive option groups. Avoid
inventing an unrelated CRUD surface such as:

```text
conkit contract-check PATH
conkit do-contract-thing PATH
```

Rules:

- Add a new verb only when an existing verb cannot express the action cleanly.
- Add a new target only when it denotes a coherent contract family.
- Use `ValueEnum` for closed sets.
- Use `ValueHint` for paths.
- Use `#[arg(long)]` for non-obvious options; use short flags sparingly and only when conventional.
- Match current output selection; reports infer YAML or JSON from their file
  extension rather than a global `--json` flag.

## Good `conkit` boundary pattern

Keep dispatch async and exhaustive:

```rust
impl AppCommand for Command {
    async fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()> {
        match self {
            Self::Check(command) => command.execute(ctx).await,
            Self::Generate(command) => command.execute(ctx).await,
            Self::Archive(command) => command.execute(ctx).await,
            Self::Diff(command) => command.execute(ctx).await,
            Self::New(command) => command.execute(ctx).await,
        }
    }
}
```

Adapt paths to catalogs before calling the domain. Generation commands must
preserve the current bounded recovery, immutable-baseline,
requested-versus-persisted extraction reconciliation, complete-domain-result,
and writer reconciliation flow. The representative signature branch below
elides warning/summary/adoption rendering and output-policy selection:

```rust
let source = SourceTree::open(self.source.clone())?
    .with_limits(ctx.catalog_read_limits());
let contracts = ContractsStore::new(self.contracts.clone())
    .with_limits(ctx.catalog_read_limits());
let mut catalog_reads = ctx
    .catalog_read_limits()
    .begin(ctx.cancellation());

contracts.recover_interrupted_generation_with_budget(&mut catalog_reads)?;
let baseline = contracts.read_optional_with_budget(&mut catalog_reads)?;
let layout = ContractLayout::load(
    &contracts,
    &source,
    &baseline,
    ctx.cancellation(),
)?;
let fresh = layout.is_empty();
let persisted = layout.extraction(ctx.cancellation())?;
let coordinator = SignatureExtractionCoordinator::new(requested, &contracts);
coordinator.validate_generation_roots(fresh, &crate_roots)?;
let source_files = layout.read_signature_sources(&source, &mut catalog_reads)?;
let decision = coordinator.reconcile(ExtractionUse::Generation {
    fresh,
    persisted,
    explicit_crates: &crate_roots,
})?;
let (extraction, generation_crates) = decision.acquire(
    ctx,
    &source,
    &source_files,
    &contracts,
    &mut catalog_reads,
)?;
let (source_files, target) = layout.into_signature_generation(
    &contracts,
    &source,
    source_files,
    generation_crates,
    ctx.cancellation(),
)?;
let conkit_signature::GenerateResponse {
    contract_files: documents,
    ..
} = ctx
    .signature()
    .generate(conkit_signature::GenerateRequest {
        source_files,
        target,
        scope: conkit_signature::ContractScope::Signatures,
        extraction,
    })
    .await
    .context("failed to generate signature contracts")?;
let generated = GeneratedContracts::new(baseline, documents);
ctx.cancellation().checkpoint()?;
contracts.write_generated_with_budget(
    generated,
    ExistingOutputPolicy::Reject,
    catalog_reads,
)?;
```

## Bad patterns to reject

```rust
// Bad: a filesystem path crosses into a domain request.
conkit_signature::NewRequest { source_root: self.source.clone() }
```

```rust
// Bad: library depends on clap and prints directly.
pub fn new_operation(matches: &clap::ArgMatches) -> anyhow::Result<()> { todo!() }
```

```rust
// Bad: stringly paths and non-atomic state updates.
let manifest = format!("{}/contract.yml", path);
std::fs::write(manifest, contents)?;
```

```rust
// Bad: command-local pool/admission and an unbounded filesystem read.
let pool = rayon::ThreadPoolBuilder::new().build()?;
let bytes = std::fs::read(source_root)?;
```

```rust
// Bad: a domain owns Cargo/compiler process behavior.
conkit_signature::GenerateRequest { manifest_path, cargo_features, .. }
```

## Testing checklist for every new command

Add tests for:

- `conkit <verb> --help` shows the new grammar.
- Success path with filesystem assertions when applicable.
- Important failure path with clear stderr.
- YAML/JSON reports or catalog bytes when supported.
- Paths with spaces and non-ASCII characters if the command touches the filesystem.
- Deterministic catalog entry/per-file/total-byte limit failures and process
  cancellation before publication when the command adds filesystem reads.
- No command-local worker pool or pending queue; tests that exercise domain
  work use the shared context and preserve the CLI's zero-pending behavior.
- No logs/progress on stdout when output is machine-readable.
- An independently owned manifest-aware E2E leaf with argv beginning with
  `conkit`, an exact exit code,
  explicit stdout/stderr expectations, and an exhaustive output tree when the
  command changes checked-in external behavior.
- Harness behavior itself only when the manifest schema or runner changes.
- Corruption/noise scenarios expressed with leaf-local assets and ordered
  sandbox steps, never by mutating a checked-in scenario input.

## Verification commands

Run every focused scenario command in the
[scenario guide](../../../test/scenarios/README.md) when checked-in external
behavior changes. Then run the checked, locked workspace gates:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

When a check cannot run, state exactly which check was skipped and why.

## Reference loading

- Read `references/command-addition-playbook.md` for a generic command-addition
  walkthrough. Its `app`/CRUD examples are non-normative; map them through the
  `conkit` rules above.
- Read `references/examples-good-and-bad-command-diffs.md` for generic diff
  shapes, subject to the same mapping.
- Read `references/official-guides-and-crates.md` when refreshing docs, versions, or dependencies.
