# Workspace Documentation Refresh Workflow

Use this file for the exact Contract Kit (`contract-kit`) workspace
documentation refresh sequence.

## Contents

- [Inputs](#inputs)
- [Inventory](#inventory)
- [Execution](#execution)
- [Validation](#validation)
- [Rules](#rules)

## Inputs

1. Treat current `HEAD` as the implementation source of truth.
2. Diff the current branch against `main` before deciding scope when `main`
   exists.
3. Use the official `AGENTS.md`, `ARCHITECTURE.md`, and Codex AGENTS docs as
   external guidance only when documentation structure or instruction layering
   is part of the refresh:
   - `https://agents.md/`
   - `https://architecture.md/`
   - `https://developers.openai.com/codex/guides/agents-md`

## Inventory

1. Inventory every repo-owned markdown file, including hidden repo-local skill
   docs:

   ```bash
   rg --hidden --files -g '*.md' -g '!.git/**' | sort
   ```

2. Reconcile the root docs (`README.md`, `ARCHITECTURE.md`, `AGENTS.md`,
   `SKILLS.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, and `RELEASING.md`), each
   crate's `AGENTS.md` and `ARCHITECTURE.md`, and
   `test/scenarios/AGENTS.md` plus `test/scenarios/README.md` against current
   implementation evidence. Include root audit, findings, plan, and other
   historical markdown in the inventory even when the correct action is to
   delete a stale unreferenced artifact.
3. Inventory every repo-local `SKILL.md`, `agents/openai.yaml`, and eval JSON
   file. This repository owns exactly twelve skills and twelve matching
   `agents/openai.yaml` files.
4. Preserve one canonical owner for repeated facts so the same validation
   command, skill boundary, or module claim is not maintained in many places.

## Execution

1. Refresh root `README.md`, `ARCHITECTURE.md`, `AGENTS.md`, `SKILLS.md`,
   `CHANGELOG.md`, `CONTRIBUTING.md`, and `RELEASING.md` when the branch changes
   their product, structure, operating, inventory, history, contributor, or
   release-owned facts.
2. Refresh nested crate and scenario docs when their module ownership,
   authoring rules, test flows, or command behavior changes.
3. Refresh skill docs when their owning workflow, references, templates,
   validation steps, or invocation policy changes.
4. Keep `README.md` product-facing, `ARCHITECTURE.md` structural,
   `AGENTS.md` operational, `SKILLS.md` as the skill inventory,
   `CONTRIBUTING.md` human-facing, `CHANGELOG.md` release-history-facing, and
   `RELEASING.md` focused on release operations.
5. Keep scenario schema and authoring detail in `test/scenarios/README.md`
   instead of duplicating it in root or CLI docs.
6. Keep archived or historical markdown clearly marked as non-normative.

## Validation

Review local links, role ownership, and stale command or test claims from the
workspace root. Inventory skills and metadata deterministically:

```bash
rg --hidden --files .agents/skills -g 'SKILL.md' | sort
rg --hidden --files .agents/skills -g 'openai.yaml' | sort
rg --hidden --files .agents/skills -g '*.json' | sort
```

Require exactly twelve `SKILL.md` files and twelve matching
`agents/openai.yaml` files. Run `skill-creator`'s `quick_validate.py` against
every skill:

```bash
validator="${CODEX_HOME:-$HOME/.codex}/skills/.system/skill-creator/scripts/quick_validate.py"
for skill_file in .agents/skills/*/SKILL.md; do
  python3 "$validator" "$(dirname "$skill_file")"
done
```

Parse every `agents/openai.yaml`, require all interface strings to be quoted,
require `short_description` to contain 25 through 64 characters, and require
`default_prompt` to name the exact `$skill-name`. Parse every eval JSON file as
JSON.

When scenario docs or harness claims change, run every focused scenario gate in
the canonical [scenario authoring guide](../../../../test/scenarios/README.md).

Run the checked, locked workspace gates for a workspace-wide refresh:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
git diff --check
```

## Rules

- Never suppress warnings, doctest failures, or skill validation failures.
- Keep `SKILLS.md` compact and keep detailed workflows in the owning skill
  docs and references.
- If the task reduces to public rustdoc only, redirect to
  `refresh-workspace-rustdocs`.
