## Summary

<!-- What changed, and why is this the right boundary for the change? -->

## Related issue

<!-- Use "Closes #123" when this PR should close an issue. -->

## Validation

<!-- Check only commands and reviews that you actually completed. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --locked --workspace --all-targets --all-features`
- [ ] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --locked --workspace --doc`
- [ ] `cargo test --locked --workspace --all-targets`
- [ ] Targeted scenario commands were run when scenarios or the harness changed.

## Impact review

- [ ] I considered Windows, Linux, and macOS behavior.
- [ ] I added or updated the narrowest useful tests.
- [ ] I updated product, architecture, scenario, or rustdoc documentation where needed.
- [ ] I called out breaking API, CLI, contract-format, or persistence changes below.
- [ ] I called out version or release-note implications below.

## Breaking changes and release notes

<!-- Write "None" when there are none. -->
