---
name: refresh-workspace-rustdocs
description: "Refresh Contract Kit public rustdoc end to end across the conkit, conkit-signature, and conkit-sketch crates. Use when public Rust APIs, crate or module docs, doctestable examples, or workspace rustdoc validation need a coordinated refresh against current HEAD. Do not use it for markdown-only documentation refreshes or narrow single-item rustdoc edits; use `refresh-workspace-agentic-documentation` for markdown-only work."
---

# Refresh Workspace Rustdocs

## Purpose

Refresh the public rustdoc surface, doctestable examples, and rustdoc-contract
expectations across the Contract Kit (`contract-kit`) workspace.

## Trigger Boundary

Use this skill for workspace-wide rustdoc refreshes that span one or more
crates or shared rustdoc validation. Do not use it for markdown-only
documentation refreshes; route that work to `refresh-workspace-agentic-documentation`.

## Prerequisites

- Read [references/routing.md](references/routing.md).
- Read [references/workflow.md](references/workflow.md).
- Start from current `HEAD` and the in-scope public surface.
- Inspect the populated `conkit`, `conkit-signature`, and `conkit-sketch`
  manifests and source
  files before editing crate-owned rustdoc.

## Workflow

1. Inventory the rustdoc-visible public surface for every crate in scope before
   writing edits.
2. If the request is made from `/plan`, produce a decision-complete plan first,
   then continue into execution only when the surrounding Codex flow permits
   file edits.
3. Browse the official rustdoc writing guide live when example structure,
   crate-root docs, or rustdoc sections need external confirmation.
4. Refresh crate-root and module `//!` docs first, then public item `///` docs,
   examples, and required sections such as `# Errors` or `# Panics`.
5. Tighten rustdoc examples and workspace validation together whenever
   expectations change.
6. Keep edits confined to Rust source rustdoc and its executable examples;
   route markdown drift discovered during the audit to the agentic-doc skill.
7. Run the documented checked, locked rustdoc gates before closing the task.

## Output Rules

- Keep examples runnable unless the item truly requires live services,
  credentials, or external processes.
- Never suppress rustdoc warnings, lints, or doctest failures.
- Route markdown-only documentation refreshes to
  `refresh-workspace-agentic-documentation`.
- Do not fold `README.md`, `ARCHITECTURE.md`, `AGENTS.md`, scenario guides, or
  repo-local skill docs into a rustdoc-only pass.
- Keep narrow item-level rustdoc work local to the owning crate when the task
  does not require a workspace-wide coordination pass.

## References

- [references/routing.md](references/routing.md)
- [references/workflow.md](references/workflow.md)
- [references/examples.md](references/examples.md)
