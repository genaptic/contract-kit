# Scenario Agent Guide

These instructions apply to every file below `test/scenarios`. Read the
[scenario authoring guide](README.md) before authoring or changing a manifest.

## Operational invariants

- Treat the authoring guide as the sole authority for the manifest schema,
  typed steps, evidence levels, path rules, fixture and mutation rules, and
  targeted validation. Link to it instead of repeating those details.
- Keep every scenario as an independently owned leaf discovered recursively.
  Do not add hardcoded registration, sibling references, shared fixtures,
  fixture factories, or symlinks.
- Keep checked-in inputs, assets, and expected trees immutable during test
  execution. A scenario may mutate only its fresh sandbox; after validation,
  verify that no checked-in file changed as a test side effect.
- Preserve exact help, version, alias, usage-error, and conflict snapshots in
  their own fixtureless `cli/<scenario-name>/` leaves. Never weaken an exact
  process assertion or exhaustive tree merely to make a scenario pass.
- Declare only truthful keys from
  [`REQUIRED_COVERAGE_KEYS`](../../conkit/tests/support/scenario.rs), using the
  evidence levels defined by the authoring guide. Reachability alone is not
  behavioral evidence.
- Use the closed typed mutation steps for ordered scenario state. Keep
  mechanics that cannot be represented safely in a manifest in focused Rust
  integration tests; never add a production test hook.
- Keep contract fixtures on the canonical combined format documented in the
  [product guide](../../README.md#contract-format), and keep platform,
  archive, ownership, and exhaustive-tree evidence observable as required by
  the authoring guide.

## Validation

Run the targeted commands in the [scenario authoring guide](README.md) before
the root workspace gates. Then inspect the worktree and verify that no
checked-in manifest, fixture, overlay, archive, or golden changed as a test
side effect.
