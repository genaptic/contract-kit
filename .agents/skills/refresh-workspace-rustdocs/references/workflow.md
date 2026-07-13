# Workspace Rustdoc Refresh Workflow

Use this file for the exact Contract Kit (`contract-kit`) workspace rustdoc
refresh sequence.

## Inputs

1. Treat current `HEAD` as the implementation source of truth.
2. Use the official rustdoc writing guide as external guidance when crate-root
   docs, examples, or rustdoc sections need confirmation:
   - `https://doc.rust-lang.org/stable/rustdoc/how-to-write-documentation.html`
3. Use the populated `conkit`, `conkit-signature`, and `conkit-sketch` source
   files plus Cargo manifests as the local authority for current expectations.

## Inventory

1. Inventory every rustdoc-visible public item for the crates in scope.
2. Mark which items need crate-root or module `//!` docs, runnable examples,
   `# Errors`, `# Panics`, or `# Safety`.
3. Identify examples that may justify `no_run` because they require
   subprocesses, file system fixtures, or external tools.

## Execution

1. Refresh crate-root and public-module `//!` docs first.
2. Refresh public item `///` docs after the crate and module framing is
   correct.
3. Prefer runnable examples for pure or local APIs and reserve `no_run` for
   real subprocess or environment-dependent entrypoints.
4. Update doctests and workspace validation expectations together when rustdoc
   examples change.
5. Keep the pass rustdoc-only. Report markdown drift separately and route it to
   `refresh-workspace-agentic-documentation` rather than editing repo-owned
   Markdown as part of this workflow.

## Validation

Run these commands from the populated workspace root:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

## Rules

- Never suppress rustdoc warnings, doctest failures, or lint failures.
- Never leave compile-only helper shells on pure local examples when the
  example can actually run.
- If the task reduces to markdown-only docs, redirect to
  `refresh-workspace-agentic-documentation`.
