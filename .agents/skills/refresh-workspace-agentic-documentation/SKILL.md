---
name: refresh-workspace-agentic-documentation
description: "Refresh Contract Kit workspace documentation end to end across AGENTS.md, ARCHITECTURE.md, README.md, SKILLS.md, focused guides, and repo skill docs. Use when a branch changes behavior, module ownership, validation flows, repo-local skills, or documentation expectations enough that repo-owned markdown must be reconciled against current HEAD and the diff vs main. Do not use it for public-rustdoc-only refreshes; use `refresh-workspace-rustdocs` for workspace-wide rustdoc work."
---

# Refresh Workspace Agentic Documentation

## Purpose

Refresh repo-owned Contract Kit (`contract-kit`) markdown docs and repo-local skill documentation as
one workspace change.

## Trigger Boundary

Use this skill for workspace-wide documentation refreshes that span root docs,
crate docs, focused guides, or skill docs. Do not use it for narrow rustdoc-only
work; route that to `refresh-workspace-rustdocs`.

## Prerequisites

- Read [references/routing.md](references/routing.md).
- Read [references/workflow.md](references/workflow.md).
- Start from current `HEAD` and the diff against `main`.
- Inspect the root docs, nested crate docs, scenario authoring docs, and
  `.agents/skills/**` before deciding scope.

## Workflow

1. Start by identifying the in-scope markdown set from the diff against
   `main`, the complete root/crate/scenario documentation inventory, and the
   current skill inventory.
2. If the request is made from `/plan`, produce a decision-complete plan first,
   then continue into execution only when the surrounding Codex flow permits
   file edits.
3. Browse the official `AGENTS.md`, `ARCHITECTURE.md`, and Codex AGENTS docs
   live when structure or instruction-layering guidance matters.
4. Refresh root docs, crate docs, scenario docs, focused guides, and skill
   docs together instead of treating them as separate tasks.
5. Keep `SKILLS.md` as the compact inventory for repo-local skills under
   `.agents/skills`.
6. Validate local links, role ownership, every repo-local skill and metadata
   file, and the checked, locked Cargo gates before closing the task.

## Output Rules

- Keep role ownership clear across `README.md`, `ARCHITECTURE.md`, `AGENTS.md`,
  nested scenario guides, focused guides, and skill docs.
- Never suppress warnings, doc-test failures, or skill validation failures.
- Route rustdoc-only refreshes to `refresh-workspace-rustdocs` instead of
  absorbing them here.
- Keep `SKILLS.md` compact and leave long operating detail in the owning skill
  docs and references.

## References

- [references/routing.md](references/routing.md)
- [references/workflow.md](references/workflow.md)
- [references/examples.md](references/examples.md)
