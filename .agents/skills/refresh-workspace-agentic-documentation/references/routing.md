# Routing Matrix

Use this file after the documentation refresh goal is understood and before
writing the plan or editing files.

## Always Consider

- `use-rust-best-practices-core` when Rust examples, validation commands, or
  workspace wording are in scope.
- `use-rust-best-practices-testing` when test scenario documentation,
  doctests, examples, or validation flows are in scope.
- `use-rust-shell-cli-best-practices` when CLI grammar, command behavior,
  process output, or CLI architecture is being documented.
- `skill-creator` when any repo-local skill or its metadata is updated.

## In-Scope Markdown

- Root docs: `AGENTS.md`, `ARCHITECTURE.md`, `README.md`, and `SKILLS.md`.
- Crate docs: `conkit/AGENTS.md`, `conkit/ARCHITECTURE.md`,
  `conkit-signature/AGENTS.md`, `conkit-signature/ARCHITECTURE.md`,
  `conkit-sketch/AGENTS.md`, and `conkit-sketch/ARCHITECTURE.md`.
- Scenario docs: `test/scenarios/AGENTS.md` and
  `test/scenarios/README.md`.
- Repo-local skill docs under `.agents/skills/**`.
- Other focused guides when they exist and are referenced from an owning root,
  crate, or scenario document.
- Scenario documentation whenever manifests, fixtures, harness behavior, or
  any `conkit` command's documented end-to-end behavior changes.

## Boundaries

- Route public Rust API docs and doctestable examples to
  `refresh-workspace-rustdocs`.
- Do not invent documentation gate suites or ledger files that do not exist in
  this repository.
- Keep `SKILLS.md` compact; leave long workflow details in each owning skill.
- Treat Rust sources, Cargo manifests, scenario manifests, fixtures, and
  goldens as implementation evidence, not markdown-refresh outputs, unless the
  user separately authorizes changing them.

## Output Contract

- Start with a decision-complete plan when the request is made from `/plan`.
- Continue into edits and validation when execution is allowed by the
  surrounding Codex flow.
- Refresh owning docs and skill metadata together when a skill's behavior or
  inventory changes, then validate the entire skill and metadata inventory.
