---
name: use-rust-shell-cli-best-practices
description: Use this skill when designing, implementing, reviewing, or refactoring a modern Rust shell CLI for macOS, Linux, and Windows. Applies to Cargo workspaces, root App runtime structs, CommandContext, AppCommand traits, clap derive command trees, verb-subject-object commands, filesystem paths, business logic crates, typed errors, config, JSON/text output, logging, shell completions, integration tests, and CRUD command patterns.
---

# Use Rust Shell CLI Best Practices

Apply this skill whenever touching the Rust shell CLI. Treat the CLI as a stable user interface and a thin adapter over domain crates.

## Contract Kit repository notes

- Treat the current three-member workspace as authoritative: `conkit` owns the
  `conkit` binary target on every platform; `conkit-signature` and
  `conkit-sketch` own runtime-neutral contract semantics. The `.exe` suffix on
  Windows is only a platform filename convention.
- Treat `conkit/args.rs` as grammar authority, `conkit/app.rs` as root runtime
  orchestration, `conkit/command.rs` as exhaustive async dispatch, the
  `conkit/command/` children as verb-owned CLI-to-domain workflows, and
  `conkit/context.rs` as shared runtime state with a direct signature kit and
  substantive sketch adapter.
- Keep OS paths and persistence in `conkit`. Domain requests use `FileCatalog`,
  `CatalogPath`, bytes, and typed options instead of `PathBuf` roots.
- Reuse the current modules, crates, and locked dependencies before proposing
  a new package or dependency.
- Treat every `app`, `app-cli`, or CRUD example in bundled references as a
  non-normative teaching example. Map its principle to the current `conkit`
  architecture; never copy its layout or synchronous signatures literally.

## Load supporting context only when needed

- Read `references/official-guides-and-crates.md` when checking current docs, crate choices, versions, or official guidance.
- Read `references/full-crud-cli-example.md` only when a generic end-to-end
  example is useful; apply the `conkit` mapping above.
- Read `references/bad-patterns.md` during review, cleanup, or when code smells like CLI/domain coupling.

## Non-negotiable architecture rules

1. Keep the binary CLI crate thin. `main` starts the runtime and delegates to
   `App::from_env_and_run`; it does not own command or domain logic.
2. Put business logic in library crates. Domain crates must not depend on `clap`, terminal colors, stdout/stderr, shell completions, or process exits.
3. Use `clap` only to parse, validate, render help, and produce typed `Cli`/`Command` values. Do not make `clap` the runtime app.
4. Construct `CommandContext` after parsing. It owns the reusable signature and
   sketch handles plus CLI output policy.
5. Route command execution through
   `pub(crate) trait AppCommand { async fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()>; }`.
6. Implement `AppCommand` for the root `Command` enum with explicit exhaustive match arms that call receiver methods on every `{Verb}Command`.
7. Preserve the existing `conkit <verb> [target] [options]` surface: `check`,
   `generate`, `archive`, and `diff`.
8. Use `clap` v4 derive (`Parser`, `Args`, `Subcommand`, `ValueEnum`) for grammar. Prefer nested enums for verb-subject structure.
9. Accept filesystem paths as `PathBuf` only at OS-facing CLI boundaries. Read
   them into validated catalogs before constructing domain requests.
10. Use typed domain request/response structs carrying catalogs, logical catalog
    paths, bytes, and domain options. Convert inside the relevant async
    `AppCommand::execute` implementation.
11. Use `thiserror` for public/library errors. Use `anyhow` or `miette` only in the CLI/application layer for context and friendly reports.
12. Keep stdout for command results and machine-readable output. Put logs, warnings, progress, and diagnostics on stderr.
13. Add or update tests with every behavior change. Use domain tests, CLI
    integration tests, and manifest-aware E2E scenarios at their appropriate
    boundaries.

## Repository layout and dependencies

Do not replace the current workspace with a generic template. Its dependency
direction is:

```text
conkit -> conkit-signature, conkit-sketch
conkit-signature -> no conkit crate
conkit-sketch -> no conkit crate
```

Respect `Cargo.lock` with `--locked`. Do not use generic "latest compatible"
advice to change versions. Load the dependency/platform skill and obtain the
required authorization before expanding the dependency surface.

## Command grammar rules

Preserve the current grammar:

```text
conkit check <all|signatures|sketches> --source DIR --contracts DIR --output FILE [--default|--strict|--warning]
conkit generate <all|signatures|sketches> --source DIR --contracts DIR [--adopt-existing]
conkit archive --contracts DIR --archive DIR [--gzip]
conkit diff --contracts DIR --archive FILE
```

`signature` and `sketch` remain singular aliases. An omitted check mode is
equivalent to `--default`. Reports infer YAML or JSON from the output extension.
`--gzip` is optional and currently selects the only archive format.

Good CLI grammar:

```rust
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    Check(CheckCommand),
    Generate(GenerateCommand),
    Archive(ArchiveCommand),
    Diff(DiffCommand),
}
```

Bad CLI grammar:

```rust
// Bad: bypasses established verbs/targets and uses string paths.
enum Command {
    ContractCheck { directory: String },
}
```

## Root App And CLI/Domain Boundary

`clap` creates parsed data. The root `App` creates runtime state and executes
commands:

Every command implements the same crate-private execution contract:

```rust
pub(crate) trait AppCommand {
    async fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()>;
}
```

Good generation execution adapts OS paths into a catalog request, computes a
complete document catalog outside the persistent lock, and ties that result to
the exact input baseline before reconciliation:

```rust
let source = SourceTree::open(self.source.clone())?;
let contracts = ContractsStore::new(self.contracts.clone());

contracts.recover_interrupted_generation()?;
let baseline = contracts.read_optional()?;
let layout = ContractLayout::load(&contracts, &source, &baseline)?;
let (source_files, target) =
    layout.into_signature_generation(&contracts, &source)?;
let conkit_signature::GenerateResponse {
    contract_files: documents,
    ..
} = ctx
    .signature()
    .generate(conkit_signature::GenerateRequest {
        source_files,
        target,
        scope: conkit_signature::ContractScope::Signatures,
    })
    .await?;
let generated = GeneratedContracts::new(baseline, documents);
contracts.write_generated(generated, ExistingOutputPolicy::Reject)?;
```

Bad coupling:

```rust
// Bad: domain crate now knows clap and owns user-facing CLI behavior.
conkit_signature::generate_from_clap_matches(matches)?;
```

## Filesystem rules

- Use `Path`, `PathBuf`, `OsStr`, and `OsString` for OS-facing paths.
- Convert validated filesystem trees to `FileCatalog` before crossing into a
  domain crate; use `CatalogPath` for logical paths inside catalogs.
- Use `path.join("child")`, not string concatenation.
- Use `fs-err` instead of `std::fs` when the error should mention the operation/path.
- Use `tempfile` or `assert_fs` in tests; never hard-code `/tmp` or Windows-only paths.
- Reuse `ResolvedPath`, `SourceTree`, `ContractsStore`, archive/report adapters,
  and ownership reconciliation instead of creating parallel filesystem helpers.
- Preserve explicit overlap, symlink, collision, ownership, and rollback rules.

## Error, output, and logging rules

- Domain functions return `Result<T, domain::Error>` with `thiserror`.
- CLI handlers may return `anyhow::Result<()>` or `miette::Result<()>` and add user-facing context.
- Do not call `std::process::exit` below `main`.
- Do not log to stdout. Configure `tracing-subscriber` to write to stderr.
- Keep human output and serialized reports intentionally formatted; do not rely
  on `Debug` output in production.

## Testing rules

Before finishing CLI work, run the narrowest meaningful checks, then the workspace checks if practical:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

Test at least:

- `--help` and nested command help.
- Success and failure cases.
- YAML/JSON report behavior if supported.
- Filesystem paths with spaces and non-ASCII characters.
- Existing destination behavior.
- No accidental output on stderr for successful machine-readable commands.
- No progress/logging on stdout.
- Checked-in positive and negative scenarios for stable external behavior.
  Follow the canonical [scenario guide](../../../test/scenarios/README.md):
  manifests are recursively discovered, every argv begins with `conkit`, every
  leaf receives an isolated sandbox with its sibling `input/` copied
  implicitly, and output trees are exhaustive.

When scenario behavior changes, run every focused scenario command in the
[scenario guide](../../../test/scenarios/README.md) before the workspace
checks above.

## Review checklist

A change is not done until:

- The command follows the existing verb/target grammar.
- The CLI crate is still only an adapter.
- Domain logic lives in the right library crate.
- OS paths remain in CLI adapters; domain inputs remain catalog/byte based.
- Errors have context without leaking implementation details.
- Output is predictable for humans and scripts.
- Tests cover the new behavior and relevant failure modes.
- Formatting, linting, and tests have been run or the skipped checks are explicitly reported.

## Canonical references

- OpenAI Codex Skills: https://developers.openai.com/codex/skills
- Agent Skills specification: https://agentskills.io/specification
- Agent Skills best practices: https://agentskills.io/skill-creation/best-practices
- Rust CLI book: https://rust-cli.github.io/book/
- Rust command-line apps guide: https://www.rust-lang.org/what/cli/
- Cargo workspaces: https://doc.rust-lang.org/cargo/reference/workspaces.html
- Rust 2024 resolver: https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html
- clap derive docs: https://docs.rs/clap/latest/clap/_derive/
