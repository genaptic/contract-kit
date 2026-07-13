# Releasing Contract Kit

Contract Kit releases are coordinated across the workspace. A release tag is
`vX.Y.Z`, every workspace package has version `X.Y.Z`, the two libraries are
published to crates.io, and `conkit` binaries are attached to the corresponding
GitHub Release. A merge to `main` never publishes a release.

## One-time repository setup

1. In GitHub, create an environment named `release`. Restrict it to `main`, add
   required reviewers, and prevent self-review when the repository plan allows
   it.
2. Keep the default `GITHUB_TOKEN` permission read-only. The release workflow
   grants write permissions only to its final, environment-protected job.
3. Enable immutable releases in the repository settings so a published tag and
   its assets cannot be changed after the release becomes public.
4. Provision the labels referenced by `.github/release.yml`. The configuration
   categorizes pull requests by label but does not create repository labels.
   These commands are idempotent and normalize the label metadata:

   ```shell
   gh label create breaking-change --repo genaptic/contract-kit --color B60205 \
     --description "Introduces a backwards-incompatible change." --force
   gh label create ignore-for-release --repo genaptic/contract-kit --color EDEDED \
     --description "Exclude this pull request from generated release notes." --force
   gh label create dependencies --repo genaptic/contract-kit --color 0366D6 \
     --description "Pull requests that update a dependency file." --force
   ```

   Apply `breaking-change` to incompatible changes and `ignore-for-release` to
   pull requests that should not appear in generated notes. Dependabot applies
   `dependencies` by default, and maintainers may use it for other dependency
   updates.
5. For the initial `0.0.1` release, create a crates.io API token that is allowed
   to publish new crates. Store it as the `CARGO_REGISTRY_TOKEN` secret on the
   `release` environment. Revoke it immediately after both crates are published.
6. After `conkit-signature` and `conkit-sketch` exist on crates.io, configure a
   trusted publisher for each crate using repository
   `genaptic/contract-kit`, workflow `release.yml`, and environment `release`.
   Future runs use crates.io's short-lived OIDC credential and do not need a
   registry secret.
7. Protect `main` and require `CI / Required` and `Rustdocs / Build preview`
   before merge. Require pull requests and disallow force pushes and tag
   rewrites.

The two crate names must still be available when the first publish starts.
Crates.io publication is permanent, so verify account ownership and token scope
before approving the release environment.

## Prepare a release

1. Update `[workspace.package].version` in `Cargo.toml`.
2. Regenerate `Cargo.lock` and update the exact `conkit --version` scenario in
   `test/scenarios/cli/version/scenario.yml`.
3. Move the release notes out of `Unreleased` in `CHANGELOG.md` and add the
   version links.
4. Open a pull request and wait for every required CI and rustdoc check to pass.
5. Merge that pull request into `main`.

## Publish a release

From the Actions tab, run the `Release` workflow on `main`:

- enter the manifest version without the leading `v`;
- select `bootstrap_crates_io` only for `0.0.1` or another first publication
  that cannot use trusted publishing yet; and
- select `confirm_release` before dispatching.

The workflow re-runs the checked, locked workspace gates and package dry-runs,
builds and smoke-tests each native executable, creates checksummed archives,
and waits for the `release` environment approval. The protected job creates a
lightweight tag at the validated release commit, verifies that the remote tag
resolves to that exact commit, and creates the draft GitHub Release only from
the verified tag. It then publishes any library version that is not already on
crates.io and makes the GitHub Release public only after both publishes succeed.
Docs.rs automatically queues versioned documentation for each published
library.

The publish step is safe to retry after a partial crates.io publication. It
packages each library locally, then accepts an existing version only when the
crates.io metadata names the expected crate and version, the version is not
yanked, and the metadata checksum and downloaded archive both match the local
package. After an attempted publish it polls for that complete postcondition,
because Cargo can time out while waiting for a successful upload to appear.

A failed run may intentionally leave a draft GitHub Release and tag at the
release commit. Fix a transient cause and use **Re-run failed jobs** on the
original workflow run so it retains the same `main` commit. If the matching
package is yanked, a maintainer must explicitly run
`cargo yank --undo --vers X.Y.Z CRATE` before retrying. A checksum mismatch is
terminal for that version because crates.io versions are immutable: investigate,
delete the abandoned draft and its tag, and prepare a higher version. Never move
an existing release tag or dispatch the same version from a newer commit.

GitHub may represent a draft created without an existing tag with an
`untagged-*` URL while deferring creation of the Git ref. The release workflow
therefore creates and verifies the tag before asking GitHub to create the draft,
and `--verify-tag` prevents draft creation from silently relying on a deferred
tag. If an older workflow left an untagged draft, first confirm that neither
library was published, then delete the abandoned draft, merge the workflow fix,
and dispatch the version again from the fixed `main`. Re-running the older run
would reuse its original workflow definition.

## Expected artifacts

Each GitHub Release contains:

- `conkit-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`;
- `conkit-vX.Y.Z-x86_64-pc-windows-msvc.zip`;
- `conkit-vX.Y.Z-x86_64-apple-darwin.tar.gz`;
- `conkit-vX.Y.Z-aarch64-apple-darwin.tar.gz`; and
- `SHA256SUMS`.

GitHub artifact attestations record build provenance for the archives and
checksum file. Verify a downloaded artifact with `gh attestation verify` and
the repository name before installing it.
