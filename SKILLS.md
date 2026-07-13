# Conkit Repo Skills

Repo-local Codex skills live under `.agents/skills`. Codex discovers these
skills from the repository root and from nested working directories.

## Implicit Rust Guidance

- `$use-rust-best-practices-core`: baseline Rust quality guidance for any Rust
  task.
- `$use-rust-best-practices-architecture`: crate, module, workspace, target,
  and test placement guidance.
- `$use-rust-best-practices-async`: async Rust, Tokio, cancellation, and
  concurrency guidance.
- `$use-rust-best-practices-abstractions`: traits, generics, lifetimes, errors,
  enum dispatch, and API-shape guidance.
- `$use-rust-best-practices-dependencies-platforms`: Cargo hygiene,
  dependency risk, MSRV policy, and portability guidance.
- `$use-rust-best-practices-networking`: HTTP, gRPC, retries, timeouts, client
  reuse, and pooling guidance.
- `$use-rust-best-practices-testing`: unit, integration, doctest, binary smoke,
  and e2e test guidance.
- `$use-rust-shell-cli-best-practices`: shell CLI architecture, clap command
  grammar, filesystem behavior, output, errors, and CLI tests.
- `$add-new-cli-command`: workflow for adding or changing `conkit` CLI commands
  while preserving CLI/domain boundaries and tests.
- `$rust-code-structuring-best-practices`: strict struct, enum, receiver
  method, builder, type-alias, and standalone-function guidance.

## Explicit Refresh Workflows

- `$refresh-workspace-agentic-documentation`: explicit-only workflow for
  refreshing repo-owned markdown docs and skill docs.
- `$refresh-workspace-rustdocs`: explicit-only workflow for refreshing public
  rustdoc comments and runnable examples across `conkit`, `conkit-signature`,
  and `conkit-sketch` while preserving the workspace's current crate
  boundaries.

## Configuration Notes

- Keep these skills project-scoped; do not copy them to the user skill
  directory unless the goal changes to global reuse.
- Keep long workflow detail inside the owning skill references, not this file.
- Restart Codex or open a fresh session if newly moved or edited skills do not
  appear in the skill selector.
