# Official guides and crate docs for modern Rust shell CLIs

Use these links when refreshing dependencies, checking API details, or explaining why a recommendation exists. Prefer official documentation and docs.rs over blog posts.

## Table of contents

- [Skill authoring standards](#skill-authoring-standards)
- [Rust and Cargo](#rust-and-cargo)
- [Rust CLI guidance](#rust-cli-guidance)
- [CLI parser and shell integration](#cli-parser-and-shell-integration)
- [Error handling and diagnostics](#error-handling-and-diagnostics)
- [Filesystem and paths](#filesystem-and-paths)
- [Serialization and config](#serialization-and-config)
- [Logging, progress, and terminal behavior](#logging-progress-and-terminal-behavior)
- [Testing](#testing)
- [Release, audit, and distribution](#release-audit-and-distribution)
- [Version policy for generated code](#version-policy-for-generated-code)

## Skill authoring standards

- OpenAI Codex Skills: https://developers.openai.com/codex/skills
- Agent Skills overview: https://agentskills.io/home
- Agent Skills specification: https://agentskills.io/specification
- Agent Skills best practices: https://agentskills.io/skill-creation/best-practices
- Optimizing skill descriptions: https://agentskills.io/skill-creation/optimizing-descriptions
- Evaluating skill output quality: https://agentskills.io/skill-creation/evaluating-skills

## Rust and Cargo

- Latest Rust releases: https://blog.rust-lang.org/releases/latest/
- The Rust Programming Language: https://doc.rust-lang.org/book/
- Rust standard library: https://doc.rust-lang.org/std/
- `std::path`: https://doc.rust-lang.org/std/path/
- `std::process::ExitCode`: https://doc.rust-lang.org/std/process/struct.ExitCode.html
- `std::io::IsTerminal`: https://doc.rust-lang.org/std/io/trait.IsTerminal.html
- Cargo Book: https://doc.rust-lang.org/cargo/
- Cargo workspaces: https://doc.rust-lang.org/cargo/reference/workspaces.html
- Workspace dependencies: https://doc.rust-lang.org/cargo/reference/workspaces.html#the-dependencies-table
- Rust 2024 resolver: https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html

## Rust CLI guidance

- Rust CLI book: https://rust-cli.github.io/book/
- Argument parsing chapter: https://rust-cli.github.io/book/tutorial/cli-args.html
- Packaging CLI apps: https://rust-cli.github.io/book/tutorial/packaging.html
- Rust command-line apps guide: https://www.rust-lang.org/what/cli/

## CLI parser and shell integration

- `clap`: https://docs.rs/clap/latest/clap/
- `clap` derive tutorial: https://docs.rs/clap/latest/clap/_derive/_tutorial/
- `clap` derive reference: https://docs.rs/clap/latest/clap/_derive/
- `clap_complete`: https://docs.rs/clap_complete/latest/clap_complete/
- `clap_complete::Shell`: https://docs.rs/clap_complete/latest/clap_complete/aot/enum.Shell.html
- `clap-verbosity-flag`: https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/

## Error handling and diagnostics

- Rust error handling: https://doc.rust-lang.org/book/ch09-00-error-handling.html
- `thiserror`: https://docs.rs/thiserror/latest/thiserror/
- `anyhow`: https://docs.rs/anyhow/latest/anyhow/
- `miette`: https://docs.rs/miette/latest/miette/

## Filesystem and paths

- `std::fs`: https://doc.rust-lang.org/std/fs/
- `std::path::Path`: https://doc.rust-lang.org/std/path/struct.Path.html
- `std::path::PathBuf`: https://doc.rust-lang.org/std/path/struct.PathBuf.html
- `fs-err`: https://docs.rs/fs-err/latest/fs_err/
- `directories`: https://docs.rs/directories/latest/directories/
- `tempfile`: https://docs.rs/tempfile/latest/tempfile/
- `walkdir`: https://docs.rs/walkdir/latest/walkdir/
- `ignore`: https://docs.rs/ignore/latest/ignore/
- `atomic-write-file`: https://docs.rs/atomic-write-file/latest/atomic_write_file/
- `same-file`: https://docs.rs/same-file/latest/same_file/
- `normpath`: https://docs.rs/normpath/latest/normpath/
- `camino`: https://docs.rs/camino/latest/camino/

## Serialization and config

- Serde overview: https://serde.rs/
- Serde derive: https://serde.rs/derive.html
- `toml_edit`: https://docs.rs/toml_edit/latest/toml_edit/
- `figment`: https://docs.rs/figment/latest/figment/
- `serde_json`: https://docs.rs/serde_json/latest/serde_json/

## Logging, progress, and terminal behavior

- `tracing`: https://docs.rs/tracing/latest/tracing/
- `tracing-subscriber`: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/
- `tracing_subscriber::EnvFilter`: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html
- `indicatif`: https://docs.rs/indicatif/latest/indicatif/

## Testing

- `assert_cmd`: https://docs.rs/assert_cmd/latest/assert_cmd/
- `assert_fs`: https://docs.rs/assert_fs/latest/assert_fs/
- `predicates`: https://docs.rs/predicates/latest/predicates/
- `trycmd`: https://docs.rs/trycmd/latest/trycmd/
- `insta`: https://docs.rs/insta/latest/insta/
- cargo-nextest: https://nexte.st/
- cargo-nextest running tests: https://nexte.st/docs/running/

## Release, audit, and distribution

- GitHub Actions runners: https://docs.github.com/actions/using-github-hosted-runners/about-github-hosted-runners
- GitHub Rust CI guide: https://docs.github.com/actions/tutorials/build-and-test-code/rust
- `cargo-deny`: https://docs.rs/cargo-deny/latest/cargo_deny/
- cargo-deny book: https://embarkstudios.github.io/cargo-deny/
- `cargo-dist`: https://axodotdev.github.io/cargo-dist/book/
- `cross`: https://github.com/cross-rs/cross

## Version policy for generated code

- Prefer the repository's existing MSRV and lockfile policy if present.
- For new Rust 2024 CLI work without a repository toolchain policy, use the
  current stable release from the official
  [Rust release feed](https://blog.rust-lang.org/releases/latest/) instead of
  copying a version captured in an example.
- Prefer semver-compatible dependency requirements in examples (`clap = "4"`, `serde = "1"`) unless a repo pins exact versions.
- Use `cargo add` to resolve current compatible dependency versions instead of copying stale examples blindly.
