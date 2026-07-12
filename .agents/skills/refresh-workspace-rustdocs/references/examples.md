# Canonical Prompt Examples

Use these prompts when manually validating that `refresh-workspace-rustdocs`
owns workspace-wide rustdoc refreshes rather than general markdown work.

## Manual Command Pattern

Run from the repository root. Use a fresh temporary Codex home and capture the
last message to a file:

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-rustdocs-routing.txt \
  $'Use $refresh-workspace-rustdocs ...'
```

For a real execution pass, switch the sandbox to `workspace-write`.

## Workspace Rustdoc Refresh Happy Path

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-rustdocs-happy.txt \
  $'Use $refresh-workspace-rustdocs from /plan. Refresh public rustdoc comments and runnable examples across conkit, conkit-signature, and conkit-sketch against current HEAD.'
```

Expected checks:

- The response starts with a decision-complete plan.
- The response treats public rustdoc, doctests, and workspace validation as one
  coordinated refresh.
- The response does not invent unavailable external validation gates.

## No-Op Verification

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-rustdocs-noop.txt \
  $'Use $refresh-workspace-rustdocs from /plan. Verify whether the current branch has any public rustdoc drift and describe the no-op outcome if public docs are already aligned. Do not implement.'
```

Expected checks:

- The response preserves the verification or no-op outcome cleanly.
- The response does not invent markdown catalog or skill-doc updates when the
  task is rustdoc-only.

## Boundary Check

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-rustdocs-boundary.txt \
  $'Use $refresh-workspace-rustdocs from /plan. Refresh README.md, ARCHITECTURE.md, AGENTS.md, and skill docs without changing public rustdoc comments.'
```

Expected checks:

- The response redirects toward `refresh-workspace-agentic-documentation`.
- The response does not claim ownership of general markdown-only refresh work.
