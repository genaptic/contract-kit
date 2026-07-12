# Conkit Agent Guide

## Repository Expectations

- Treat the `contract-kit` repository as the home of the `conkit` project,
  also described as Contract Kit.
- Keep repo-local Codex skills under `.agents/skills/<skill-name>`.
- Do not install these repo skills into `${CODEX_HOME:-$HOME/.codex}/skills`;
  they are project-scoped assets.
- Prefer the repository's existing shape before adding new crates, commands, or
  validation scripts.
- Do not invent architecture beyond the current workspace names and checked-in
  scenarios.
- Keep mixed-catalog archive encoding and decoding in `conkit`;
  `conkit-signature` and `conkit-sketch` own their respective contract
  semantics, including semantic diffing.
- Keep `README.md` product-facing, `ARCHITECTURE.md` structural, and this file
  operational. Crate-specific rules belong in each crate's nested `AGENTS.md`.
- Treat `test/scenarios/README.md` as the canonical scenario-manifest authoring
  guide and `test/scenarios/AGENTS.md` as the operational instructions for that
  subtree. Link to the guide instead of duplicating its schema in root or crate
  documentation.

## Skill Routing

- Treat [SKILLS.md](SKILLS.md) as the compact inventory and ownership map for
  repo-local skills; keep operating detail in each owning skill.
- Use `$use-rust-best-practices-core` for any Rust planning, editing, review,
  debugging, or validation task.
- Add the specialized Rust best-practice skills when the task is about their
  specific domain: architecture, testing, async, networking, abstractions, or
  dependencies/platforms.
- Use `$use-rust-shell-cli-best-practices` when designing, implementing,
  reviewing, or refactoring the `conkit` shell CLI.
- Use `$add-new-cli-command` when adding or changing a `conkit` CLI command.
- Use `$rust-code-structuring-best-practices` when a task touches structs,
  enums, receiver methods, builders, repeated parameter groups, type aliases,
  or standalone-function placement.
- Use `$refresh-workspace-agentic-documentation` only for an explicit
  workspace-wide markdown refresh.
- Use `$refresh-workspace-rustdocs` only for an explicit workspace-wide
  rustdoc refresh.

## Validation Defaults

- For skill edits, validate every `.agents/skills/*/SKILL.md` with the
  `skill-creator` quick validator when PyYAML is available.
- For `agents/openai.yaml`, keep interface strings quoted, keep
  `short_description` between 25 and 64 characters, and make every
  `default_prompt` mention the exact `$skill-name`.
- For repo-local skill discovery, verify that exactly twelve `SKILL.md` files
  exist under `.agents/skills`.
- Use checked, locked Cargo validation for workspace changes. Unless a narrower
  nested guide is sufficient, run:

  ```shell
  cargo fmt --all -- --check
  cargo check --locked --workspace --all-targets --all-features
  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  cargo test --locked --workspace --doc
  cargo test --locked --workspace --all-targets
  ```

- For scenario or harness changes, run the targeted scenario tests in
  `test/scenarios/README.md` before the workspace gates.
- Do not invoke the `rust[c]` executable directly in tests; use Cargo-level
  checks, doctests, package tests, and scenario runners instead.
- Do not interweave test shims into production Rust code with `#[cfg(test)]`
  impls, methods, imports, fields, trait impls, or constructors. `#[cfg(test)]`
  is allowed for local `mod tests` blocks; test-only helpers belong inside
  those modules or under `tests/`.
- Production manifests must not depend on compiler-private crates whose names
  contain the `rust[c]` prefix. The workspace test
  `compiler_private_dependencies_are_not_declared` enforces this for current
  production manifests.
