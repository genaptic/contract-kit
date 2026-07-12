# Official guides and crate docs for adding Rust CLI commands

Use these docs when adding a command, updating dependencies, or validating implementation choices.

## Skill and Codex guidance

- OpenAI Codex Skills: https://developers.openai.com/codex/skills
- OpenAI Codex best practices: https://developers.openai.com/codex/learn/best-practices
- Agent Skills overview: https://agentskills.io/home
- Agent Skills specification: https://agentskills.io/specification
- Agent Skills best practices: https://agentskills.io/skill-creation/best-practices
- Optimizing skill descriptions: https://agentskills.io/skill-creation/optimizing-descriptions

## Rust CLI architecture

- Rust CLI book: https://rust-cli.github.io/book/
- CLI argument parsing chapter: https://rust-cli.github.io/book/tutorial/cli-args.html
- Rust command-line applications guide: https://www.rust-lang.org/what/cli/
- Cargo workspaces: https://doc.rust-lang.org/cargo/reference/workspaces.html
- Rust 2024 Cargo resolver: https://doc.rust-lang.org/edition-guide/rust-2024/cargo-resolver.html

## Parser, output, and completions

- `clap`: https://docs.rs/clap/latest/clap/
- `clap` derive tutorial: https://docs.rs/clap/latest/clap/_derive/_tutorial/
- `clap` derive reference: https://docs.rs/clap/latest/clap/_derive/
- `clap_complete`: https://docs.rs/clap_complete/latest/clap_complete/
- `clap-verbosity-flag`: https://docs.rs/clap-verbosity-flag/latest/clap_verbosity_flag/
- `serde`: https://serde.rs/
- `serde_json`: https://docs.rs/serde_json/latest/serde_json/

## Domain errors and diagnostics

- `thiserror`: https://docs.rs/thiserror/latest/thiserror/
- `anyhow`: https://docs.rs/anyhow/latest/anyhow/
- `miette`: https://docs.rs/miette/latest/miette/

## Filesystem-safe command implementation

- Rust `std::path`: https://doc.rust-lang.org/std/path/
- Rust `std::fs`: https://doc.rust-lang.org/std/fs/
- `fs-err`: https://docs.rs/fs-err/latest/fs_err/
- `directories`: https://docs.rs/directories/latest/directories/
- `tempfile`: https://docs.rs/tempfile/latest/tempfile/
- `walkdir`: https://docs.rs/walkdir/latest/walkdir/
- `ignore`: https://docs.rs/ignore/latest/ignore/
- `atomic-write-file`: https://docs.rs/atomic-write-file/latest/atomic_write_file/
- `same-file`: https://docs.rs/same-file/latest/same_file/
- `normpath`: https://docs.rs/normpath/latest/normpath/

## Logging and tests

- `tracing`: https://docs.rs/tracing/latest/tracing/
- `tracing-subscriber`: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/
- `assert_cmd`: https://docs.rs/assert_cmd/latest/assert_cmd/
- `assert_fs`: https://docs.rs/assert_fs/latest/assert_fs/
- `trycmd`: https://docs.rs/trycmd/latest/trycmd/
- cargo-nextest: https://nexte.st/
