# Routing Matrix

Use this file after the rustdoc refresh goal is understood and before writing
the plan or editing files.

## Always Consider

- `use-rust-best-practices-core` for baseline Rust quality guidance.
- `use-rust-best-practices-testing` for doctests, examples, and validation
  command choices.
- `use-rust-best-practices-architecture` when rustdoc changes expose crate,
  module, or target ownership questions.

## In-Scope Crates

- `conkit` for the populated `conkit` executable and CLI-module rustdoc surface.
- `conkit-signature` for signature parsing and matching APIs.
- `conkit-sketch` for sketch parsing, checking, generation, and diff APIs.

## Boundaries

- Route markdown-only refreshes to `refresh-workspace-agentic-documentation`.
- Treat all three workspace members as populated; inspect their current
  manifests and sources instead of deferring validation for a skeletal state.
- Keep narrow item-level rustdoc edits in the owning crate unless the request
  needs a coordinated workspace-wide pass.
- Do not invent unavailable rustdoc contract tests, docs-site scripts, or
  crate-specific contract skills.

## Output Contract

- Start with a decision-complete plan when the request is made from `/plan`.
- Continue into edits and validation when execution is allowed by the
  surrounding Codex flow.
- Update examples and checked, locked workspace validation together when
  rustdoc expectations move together.
