# Canonical Prompt Examples

Use these prompts when manually validating that
`refresh-workspace-agentic-documentation` owns workspace-wide markdown refreshes
rather than narrow rustdoc work.

## Manual Command Pattern

Run from the repository root. Use a fresh temporary Codex home and capture the
last message to a file:

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-agentic-documentation-routing.txt \
  $'Use $refresh-workspace-agentic-documentation ...'
```

For a real execution pass, switch the sandbox to `workspace-write`.

## Workspace Refresh Happy Path

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-agentic-documentation-happy.txt \
  $'Use $refresh-workspace-agentic-documentation from /plan. Refresh all repo-owned Contract Kit markdown docs, reconcile the repo-local skill inventory, and run the documented validation checks.'
```

Expected checks:

- The response starts with a decision-complete plan.
- The response treats root docs and skill docs as one coordinated refresh.
- The response does not invent unavailable documentation gates.

## No-Op Audit

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-agentic-documentation-noop.txt \
  $'Use $refresh-workspace-agentic-documentation from /plan. Check whether the current branch introduces any workspace documentation drift and describe the no-op outcome if the docs are already aligned. Do not implement.'
```

Expected checks:

- The response preserves the audit or no-op outcome cleanly.
- The response does not invent rustdoc-only changes when the branch only needs
  markdown verification.

## Boundary Check

```bash
mkdir -p /tmp/codex-skill-forwardtest
CODEX_HOME=/tmp/codex-skill-forwardtest codex exec --ephemeral --sandbox read-only \
  -C . \
  -o /tmp/refresh-workspace-agentic-documentation-boundary.txt \
  $'Use $refresh-workspace-agentic-documentation from /plan. Refresh the public rustdoc comments and doctestable examples across all crates without changing markdown docs.'
```

Expected checks:

- The response redirects toward `refresh-workspace-rustdocs`.
- The response does not claim ownership of pure rustdoc work.
