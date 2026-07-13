# Command addition playbook

Use this playbook to add a CLI command by extending the root `App` command
architecture from `$use-rust-shell-cli-best-practices`.

> **`conkit` mapping:** The `app archive project` walkthrough below is a generic,
> non-normative example. In this repository, define grammar in `conkit/args.rs`,
> implement native async `AppCommand` dispatch in `conkit/command.rs`, keep OS
> paths and writes in CLI adapters, and pass catalogs, logical catalog paths,
> bytes, and typed requests to `conkit-signature` or `conkit-sketch`. Do not introduce the
> example's `crates/app-*` layout or synchronous signatures into `conkit`.

## Table of contents

1. [Discover the existing app shape](#1-discover-the-existing-app-shape)
2. [Decide grammar and UX](#2-decide-grammar-and-ux)
3. [Extend clap parser types](#3-extend-clap-parser-types)
4. [Extend `AppCommand`](#4-extend-appcommand)
5. [Add domain API and behavior](#5-add-domain-api-and-behavior)
6. [Add tests and docs](#6-add-tests-and-docs)
7. [Validate and report](#7-validate-and-report)

The running example adds:

```text
app archive project PATH --output FILE [--format text|json]
```

## 1. Discover the existing app shape

Before editing, identify the existing symbols and owners:

```bash
find . -name Cargo.toml -maxdepth 4 | sort
rg "struct App|fn from_env|fn from_cli|fn run\\(" .
rg "derive\\((Parser|Args|Subcommand)" .
rg "enum Command|trait AppCommand|CommandContext" .
rg "print_json_or_text|print_json|print_text|OutputFormat" .
rg "assert_cmd|trycmd|cargo_bin|snapbox" tests crates conkit
```

Record:

- CLI crate and binary name.
- Root `App` runtime owner.
- Top-level `Cli` parser type.
- Root `Command` enum.
- `CommandContext` fields and accessors.
- `AppCommand` trait location.
- Existing command module layout.
- Output helper API.
- Domain crate that owns the new behavior.
- Integration and domain test conventions.

## 2. Decide grammar and UX

Use existing verbs first:

| User wants           | Preferred grammar                               |
|----------------------|-------------------------------------------------|
| Create a resource    | `app create <subject> <object>`                 |
| List resources       | `app list <subject>`                            |
| Show one resource    | `app show <subject> <object>`                   |
| Modify metadata      | `app update <subject> <object> [--field VALUE]` |
| Set config           | `app set config <key> <value>`                  |
| Remove resource      | `app remove <subject> <object> --force`         |
| Validate resource    | `app validate <subject> <object>`               |
| Export data          | `app export <subject> <object> --output FILE`   |
| Import data          | `app import <subject> FILE`                     |
| Generate completions | `app completion <shell>`                        |

For the archive example, define the public UX first:

```text
app archive project ./demo --output ./demo.app-archive
app archive project ./demo --output ./demo.app-archive --format json
app archive project ./missing --output ./missing.app-archive
app archive project ./demo --output ./existing.app-archive
app archive project --help
```

Expected output contract:

```text
Success human stdout:
  archived project ./demo to ./demo.app-archive

Success JSON stdout:
  { "project_path": "./demo", "archive_path": "./demo.app-archive" }

Failure stderr:
  failed to archive project
  caused by: project does not exist at ./missing
```

## 3. Extend clap parser types

Add the new verb to the root `Command` enum:

```rust
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    Create(CreateCommand),
    List(ListCommand),
    Show(ShowCommand),
    Update(UpdateCommand),
    Remove(RemoveCommand),
    Archive(ArchiveCommand),
    Set(SetCommand),
    Completion(CompletionCommand),
}
```

Add command parser data in the CLI crate. Keep this data parse-only; do not add
runtime context fields.

```rust
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueHint};

use crate::cli::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct ArchiveCommand {
    #[command(subcommand)]
    pub(crate) subject: ArchiveSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ArchiveSubject {
    Project {
        #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
        path: PathBuf,

        #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
        output: PathBuf,

        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
}
```

If the repository keeps all parser types in one `cli.rs`, import
`ArchiveCommand` there. If it uses one module per verb, re-export the command
type through the existing command module entrypoint.

## 4. Extend `AppCommand`

Update the root command contract with an explicit exhaustive arm:

```rust
impl AppCommand for Command {
    fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()> {
        match self {
            Self::Create(command) => command.execute(ctx),
            Self::List(command) => command.execute(ctx),
            Self::Show(command) => command.execute(ctx),
            Self::Update(command) => command.execute(ctx),
            Self::Remove(command) => command.execute(ctx),
            Self::Archive(command) => command.execute(ctx),
            Self::Set(command) => command.execute(ctx),
            Self::Completion(command) => command.execute(ctx),
        }
    }
}
```

Implement command execution on the command type. The command converts parsed CLI
data into a typed domain command, adds user-facing error context, and prints
through output helpers.

```rust
use anyhow::{Context, Result};

use crate::cli::{ArchiveCommand, ArchiveSubject, OutputFormat};
use crate::command::AppCommand;
use crate::context::CommandContext;

impl AppCommand for ArchiveCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            ArchiveSubject::Project {
                path,
                output,
                format,
            } => {
                let result = app_projects::archive_project(
                    ctx.app(),
                    app_projects::ArchiveProject {
                        path: path.clone(),
                        output: output.clone(),
                    },
                )
                .context("failed to archive project")?;

                match ctx.output().resolve(*format) {
                    OutputFormat::Json => ctx.output().print_json(&result),
                    OutputFormat::Text => ctx.output().print_text(format_args!(
                        "archived project {} to {}",
                        result.project_path.display(),
                        result.archive_path.display()
                    )),
                }
            }
        }
    }
}
```

Reject these shapes:

```rust
match cli.command {
    Command::Archive(command) => run_archive_command(&ctx, command),
}
```

```rust
fn run_archive_command(
    ctx: &CommandContext,
    command: ArchiveCommand,
) -> anyhow::Result<()> {
    command.execute(ctx)
}
```

The command type itself owns command execution parity through `AppCommand`.

## 5. Add domain API and behavior

Domain crates own business logic. They should not depend on `clap`, terminal
output, shell completions, or process exits.

`crates/app-projects/src/lib.rs`:

```rust
use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug)]
pub struct ArchiveProject {
    pub path: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ArchiveProjectOutput {
    pub project_path: PathBuf,
    pub archive_path: PathBuf,
}

pub fn archive_project(
    ctx: &app_core::AppContext,
    cmd: ArchiveProject,
) -> Result<ArchiveProjectOutput, Error> {
    ops::archive_project(ctx, cmd)
}
```

`crates/app-projects/src/error.rs`:

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("project does not exist at {path}")]
    NotFound { path: PathBuf },

    #[error("archive already exists at {path}")]
    ArchiveAlreadyExists { path: PathBuf },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

`crates/app-projects/src/ops.rs`:

```rust
use crate::{ArchiveProject, ArchiveProjectOutput, Error};

const MANIFEST_FILE: &str = "app-project.toml";

pub fn archive_project(
    ctx: &app_core::AppContext,
    cmd: ArchiveProject,
) -> Result<ArchiveProjectOutput, Error> {
    let project_path = ctx.resolve_path(cmd.path);
    let archive_path = ctx.resolve_path(cmd.output);

    if !project_path.join(MANIFEST_FILE).is_file() {
        return Err(Error::NotFound { path: project_path });
    }

    if archive_path.exists() {
        return Err(Error::ArchiveAlreadyExists { path: archive_path });
    }

    if let Some(parent) = archive_path.parent() {
        fs_err::create_dir_all(parent)?;
    }

    let body = format!("project={}\n", project_path.display());
    app_fs::atomic::write_string(&archive_path, &body)?;

    Ok(ArchiveProjectOutput {
        project_path,
        archive_path,
    })
}
```

Filesystem checklist:

- Resolve relative paths against `app_core::AppContext`.
- Create parent directories intentionally.
- Reject existing outputs unless overwrite is explicit.
- Use atomic writes for state-like outputs.
- Make symlink behavior explicit when traversal is involved.
- Clean up partial outputs when an operation can fail mid-write.

## 6. Add tests and docs

CLI integration tests should call the real binary:

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

#[test]
fn archive_project_success_json() {
    let temp = assert_fs::TempDir::new().unwrap();
    let project = temp.child("demo project");
    project.create_dir_all().unwrap();
    project
        .child("app-project.toml")
        .write_str("name = 'Demo'\n")
        .unwrap();
    let archive = temp.child("demo.app-archive");

    Command::cargo_bin("app")
        .unwrap()
        .args(["--json", "archive", "project"])
        .arg(project.path())
        .args(["--output"])
        .arg(archive.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("archive_path"))
        .stderr(predicate::str::is_empty());

    archive.assert(predicate::path::exists());
    temp.close().unwrap();
}

#[test]
fn archive_project_missing_fails() {
    let temp = assert_fs::TempDir::new().unwrap();
    let missing = temp.child("missing");
    let archive = temp.child("missing.app-archive");

    Command::cargo_bin("app")
        .unwrap()
        .args(["archive", "project"])
        .arg(missing.path())
        .args(["--output"])
        .arg(archive.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to archive project"));

    archive.assert(predicate::path::missing());
    temp.close().unwrap();
}

#[test]
fn archive_project_help_mentions_output() {
    Command::cargo_bin("app")
        .unwrap()
        .args(["archive", "project", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--output"));
}
```

Domain unit tests should cover behavior that does not require shell parsing:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn archive_rejects_existing_output() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("demo");
        let archive = temp.path().join("demo.app-archive");
        fs_err::create_dir_all(&project).unwrap();
        fs_err::write(project.join("app-project.toml"), "name = 'Demo'\n").unwrap();
        fs_err::write(&archive, "existing").unwrap();

        let ctx = app_core::AppContext {
            cwd: temp.path().to_path_buf(),
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            data_dir: temp.path().join("data"),
        };

        let error = archive_project(
            &ctx,
            ArchiveProject {
                path: PathBuf::from("demo"),
                output: PathBuf::from("demo.app-archive"),
            },
        )
        .unwrap_err();

        assert!(matches!(error, Error::ArchiveAlreadyExists { .. }));
    }
}
```

Documentation updates:

- Update README command examples if the repo maintains a command list.
- Update trycmd snapshots if the repo snapshots help output.
- Regenerate committed completion scripts only if the repo already commits them.

## 7. Validate and report

For a `conkit` command that changes checked-in external behavior, first run
every focused scenario command in the canonical
[scenario guide](../../../../test/scenarios/README.md). Then run:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

Review the diff for:

- The new command follows existing verb-subject-object grammar.
- `Cli` and command parser structs contain parsed data only.
- `AppCommand` is implemented for the new command.
- The root `Command` enum delegates exhaustively to receiver methods.
- Domain crates have no `clap`, stdout, stderr, progress bars, or process exits.
- Error messages are actionable.
- Tests assert behavior, not implementation details.
- JSON output is stable if public.
- No hard-coded platform assumptions were introduced.

Final response checklist:

- State the command added.
- Summarize command grammar and behavior.
- List tests/checks run.
- Mention any checks not run and why.
- Mention intentionally deferred behavior.
