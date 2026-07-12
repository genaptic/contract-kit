# Good and bad command addition examples

This file shows how to add `app archive project PATH --output FILE` by
extending the root `AppCommand` architecture.

> **`conkit` mapping:** These are generic, non-normative diff shapes. For `conkit`,
> use `conkit/args.rs`, native async `AppCommand` receiver methods in
> `conkit/command.rs`, CLI-owned filesystem adapters, catalog/byte domain
> requests, and manifest-aware scenarios. Do not copy the example's CRUD
> grammar, package names, domain `PathBuf` requests, or synchronous methods.

## Table of contents

- [Good diff shape](#good-diff-shape)
  - [CLI grammar](#cli-grammar)
  - [Root command delegation](#root-command-delegation)
  - [Command execution](#command-execution)
  - [Domain crate API](#domain-crate-api)
  - [Domain errors](#domain-errors)
  - [Domain operation](#domain-operation)
- [Bad diff shape](#bad-diff-shape)
  - [Root enum arm calls a free helper](#bad-root-enum-arm-calls-a-free-helper)
  - [Trait method uses invalid receiver syntax](#bad-trait-method-uses-invalid-receiver-syntax)
  - [Command name bypasses grammar](#bad-command-name-bypasses-grammar)
  - [Business work in the CLI crate](#bad-does-business-work-in-the-cli-crate)
  - [Domain crate prints](#bad-domain-crate-prints)
  - [Domain crate depends on clap matches](#bad-domain-crate-depends-on-clap-matches)
  - [One giant enum for every command shape](#bad-one-giant-enum-for-every-command-shape)
- [Good test diff](#good-test-diff)
- [Bad test diff](#bad-test-diff)
- [Good final review comment](#good-final-review-comment)

## Good diff shape

### CLI grammar

```diff
 #[derive(Debug, Subcommand)]
 pub enum Command {
     Create(CreateCommand),
     List(ListCommand),
     Show(ShowCommand),
     Update(UpdateCommand),
     Remove(RemoveCommand),
+    Archive(ArchiveCommand),
     Set(SetCommand),
     Completion(CompletionCommand),
 }
+
+#[derive(Debug, Args)]
+pub struct ArchiveCommand {
+    #[command(subcommand)]
+    pub subject: ArchiveSubject,
+}
+
+#[derive(Debug, Subcommand)]
+pub enum ArchiveSubject {
+    Project {
+        #[arg(value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
+        path: PathBuf,
+
+        #[arg(long, value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
+        output: PathBuf,
+
+        #[arg(long, value_enum)]
+        format: Option<OutputFormat>,
+    },
+}
```

Why this is good:

- Adds a clear verb.
- Keeps `project` as the subject.
- Uses `PathBuf` and shell completion hints.
- Keeps output format explicit and consistent.
- Keeps parser structs free of runtime state.

### Root command delegation

```diff
 impl AppCommand for Command {
     fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()> {
         match self {
             Self::Create(command) => command.execute(ctx),
             Self::List(command) => command.execute(ctx),
             Self::Show(command) => command.execute(ctx),
             Self::Update(command) => command.execute(ctx),
             Self::Remove(command) => command.execute(ctx),
+            Self::Archive(command) => command.execute(ctx),
             Self::Set(command) => command.execute(ctx),
             Self::Completion(command) => command.execute(ctx),
         }
     }
 }
```

Why this is good:

- The root enum stays exhaustive.
- New variants must provide the same `AppCommand` contract.
- Dispatch calls receiver methods on command payloads.

### Command execution

```diff
+impl AppCommand for ArchiveCommand {
+    fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()> {
+        match &self.subject {
+            ArchiveSubject::Project {
+                path,
+                output,
+                format,
+            } => {
+                let result = app_projects::archive_project(
+                    ctx.app(),
+                    app_projects::ArchiveProject {
+                        path: path.clone(),
+                        output: output.clone(),
+                    },
+                )
+                .context("failed to archive project")?;
+
+                match ctx.output().resolve(*format) {
+                    OutputFormat::Json => ctx.output().print_json(&result),
+                    OutputFormat::Text => ctx.output().print_text(format_args!(
+                        "archived project {} to {}",
+                        result.project_path.display(),
+                        result.archive_path.display()
+                    )),
+                }
+            }
+        }
+    }
+}
```

Why this is good:

- Command behavior is attached to `ArchiveCommand`.
- The CLI layer adapts parsed data into a typed domain command.
- The domain crate owns business behavior.
- The CLI layer owns user-facing output.
- `anyhow::Context` is attached at a useful command boundary.

### Domain crate API

```diff
+#[derive(Debug)]
+pub struct ArchiveProject {
+    pub path: PathBuf,
+    pub output: PathBuf,
+}
+
+#[derive(Debug, Serialize)]
+pub struct ArchiveProjectOutput {
+    pub project_path: PathBuf,
+    pub archive_path: PathBuf,
+}
+
+pub fn archive_project(
+    ctx: &app_core::AppContext,
+    cmd: ArchiveProject,
+) -> Result<ArchiveProjectOutput, Error> {
+    ops::archive_project(ctx, cmd)
+}
```

### Domain errors

```diff
 pub enum Error {
     #[error("project does not exist at {path}")]
     NotFound { path: PathBuf },
+
+    #[error("archive already exists at {path}")]
+    ArchiveAlreadyExists { path: PathBuf },

     #[error(transparent)]
     Io(#[from] std::io::Error),
 }
```

### Domain operation

```rust
pub fn archive_project(
    ctx: &app_core::AppContext,
    cmd: ArchiveProject,
) -> Result<ArchiveProjectOutput, Error> {
    let project_path = ctx.resolve_path(cmd.path);
    let archive_path = ctx.resolve_path(cmd.output);

    if !project_path.join("app-project.toml").is_file() {
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

## Bad diff shape

### Bad: root enum arm calls a free helper

```rust
match cli.command {
    Command::Archive(command) => archive_command(&ctx, &global, command),
}

fn archive_command(
    ctx: &app_core::AppContext,
    global: &GlobalOptions,
    command: ArchiveCommand,
) -> anyhow::Result<()> {
    let result = app_projects::archive_project(
        ctx,
        app_projects::ArchiveProject {
            path: PathBuf::new(),
            output: PathBuf::new(),
        },
    )?;
    crate::output::print_output(global, &result)
}
```

Problems:

- Command execution is not attached to `ArchiveCommand`.
- The new command can drift away from the shared `AppCommand` contract.
- Root command handling becomes a collection of loose helpers.

### Bad: trait method uses invalid receiver syntax

```rust
pub trait AppCommand {
    fn execute(command: &Self, ctx: &CommandContext) -> anyhow::Result<()>;
}
```

Problems:

- This is an associated function, not a receiver method on the command value.
- Rust command execution should use a named receiver such as `&self`.
- The intended contract is `fn execute(&self, ctx: &CommandContext)`.

### Bad: command name bypasses grammar

```diff
 pub enum Command {
+    ArchiveProject { path: String, output: String },
 }
```

Problems:

- Not verb-subject-object.
- Uses `String` for paths.
- Will not scale when archive supports more subjects.

### Bad: does business work in the CLI crate

```rust
impl AppCommand for ArchiveCommand {
    fn execute(&self, _ctx: &CommandContext) -> anyhow::Result<()> {
        if let ArchiveSubject::Project { path, output, format: _ } = &self.subject {
            let manifest = format!("{}/app-project.toml", path.display());
            if !std::path::Path::new(&manifest).exists() {
                eprintln!("missing project");
                std::process::exit(2);
            }

            std::fs::write(output, std::fs::read_to_string(manifest)?)?;
            println!("done");
        }

        Ok(())
    }
}
```

Problems:

- Stringly paths.
- `std::process::exit` outside `main`.
- `eprintln!` mixed with structured errors.
- `std::fs::write` can leave partial output.
- Domain logic cannot be reused or unit tested.

### Bad: domain crate prints

```rust
pub fn archive_project(cmd: ArchiveProject) -> Result<(), Error> {
    println!("archiving {}", cmd.path.display());
    println!("created {}", cmd.output.display());
    Ok(())
}
```

Problems:

- Library code breaks stdout contract.
- JSON output cannot be trusted.
- Tests must capture output instead of checking data.

### Bad: domain crate depends on clap matches

```rust
pub fn archive_project(matches: &clap::ArgMatches) -> anyhow::Result<()> {
    let project = matches.get_one::<String>("path").unwrap();
    println!("archiving {project}");
    Ok(())
}
```

Problems:

- Domain behavior is coupled to one parser crate.
- Tests, GUI callers, and future services must fake CLI matches.
- Typed domain errors and outputs disappear.

### Bad: one giant enum for every command shape

```rust
pub enum Command {
    CreateProject { path: PathBuf },
    ListProjects,
    ShowProject { path: PathBuf },
    UpdateProject { path: PathBuf, name: Option<String> },
    RemoveProject { path: PathBuf, force: bool },
    ArchiveProject { path: PathBuf, output: PathBuf },
    SetConfig { key: String, value: String },
    ShowConfig,
}
```

This can work for tiny CLIs, but it tends to become noisy and inconsistent.
Prefer nested verb and subject enums when the CLI uses a verb-subject structure.

## Good test diff

```diff
+#[test]
+fn archive_project_help_mentions_output() {
+    assert_cmd::Command::cargo_bin("app")
+        .unwrap()
+        .args(["archive", "project", "--help"])
+        .assert()
+        .success()
+        .stdout(predicates::str::contains("--output"));
+}
+
+#[test]
+fn archive_project_rejects_existing_output() {
+    let temp = assert_fs::TempDir::new().unwrap();
+    let project = temp.child("demo");
+    project.create_dir_all().unwrap();
+    project.child("app-project.toml").write_str("name = 'Demo'\n").unwrap();
+    let output = temp.child("demo.archive");
+    output.write_str("already here").unwrap();
+
+    assert_cmd::Command::cargo_bin("app")
+        .unwrap()
+        .args(["archive", "project"])
+        .arg(project.path())
+        .args(["--output"])
+        .arg(output.path())
+        .assert()
+        .failure()
+        .stderr(predicates::str::contains("archive already exists"));
+
+    temp.close().unwrap();
+}
```

## Bad test diff

```rust
#[test]
fn archive_project_does_not_panic() {
    archive_project("demo".to_string(), "demo.zip".to_string()).unwrap();
}
```

Problems:

- Does not invoke the real binary.
- Uses string paths.
- Does not assert output file, stderr, stdout, or help behavior.
- Writes into the developer's working directory.

## Good final review comment

```text
Added `app archive project PATH --output FILE`.

Changed:
- `crates/app-cli/src/cli.rs`: added ArchiveCommand and ArchiveSubject.
- `crates/app-cli/src/command.rs`: implemented AppCommand for ArchiveCommand and updated root Command delegation.
- `crates/app-projects/src/lib.rs`: added ArchiveProject input/output.
- `crates/app-projects/src/ops.rs`: implemented archive creation with PathBuf and atomic output write.
- `crates/app-cli/tests/archive.rs`: added help, success, and existing-output failure tests.

Validated:
- cargo fmt --all -- --check
- cargo check --locked --workspace --all-targets --all-features
- cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
- cargo test --locked --workspace --doc
- cargo test --locked --workspace --all-targets
```
