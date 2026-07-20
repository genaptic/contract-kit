# Bad Rust CLI patterns and better replacements

Use this file when reviewing or refactoring shell CLI code.

> **`conkit` mapping:** The `app`/CRUD snippets are generic and non-normative.
> Translate their principles to `conkit/args.rs`, native async dispatch in
> `conkit/command.rs`, and the application-owned `CommandContext`: one shared
> Rayon pool, independent zero-pending domain admission, process cancellation,
> `CatalogReadLimits`, and `CompilerExtractor`. Keep OS paths, one bounded
> `CatalogReadBudget`, Cargo processes, extraction reconciliation, and
> persistence in the CLI; pass only catalog/byte requests with typed extraction
> to domains.

## Table of contents

- [App architecture anti-patterns](#app-architecture-anti-patterns)
  - [Treating `clap::Command` as the runtime app](#bad-treating-clapcommand-as-the-runtime-app)
  - [Putting runtime state inside parser structs](#bad-putting-runtime-state-inside-clap-derived-parser-structs)
  - [Using free command helpers](#bad-free-command-helpers-instead-of-appcommand)
- [Passing `clap` types into business logic](#1-passing-clap-types-into-business-logic)
- [Treating paths as strings](#2-treating-paths-as-strings)
- [Using stdout for logs or progress](#3-using-stdout-for-logs-or-progress)
- [Calling `std::process::exit` in libraries or handlers](#4-calling-stdprocessexit-in-libraries-or-handlers)
- [Using `anyhow` as the public library error type](#5-using-anyhow-as-the-public-library-error-type)
- [Non-atomic config writes](#6-non-atomic-config-writes)
- [Hard-coded Unix assumptions](#7-hard-coded-unix-assumptions)
- [Accidental symlink traversal](#8-accidental-symlink-traversal)
- [Relying on shell glob expansion](#9-relying-on-shell-glob-expansion)
- [Debug formatting as public output](#10-debug-formatting-as-public-output)
- [No command integration tests](#11-no-command-integration-tests)
- [Conflating `list`, `show`, and `status`](#12-conflating-list-show-and-status)
- [Hidden mutable globals](#13-hidden-mutable-globals)
- [Over-broad command names](#14-over-broad-command-names)
- [Finishing without verification](#15-finishing-without-verification)

## App architecture anti-patterns

### Bad: treating `clap::Command` as the runtime app

```rust
fn main() {
    let matches = clap::Command::new("app")
        .subcommand(clap::Command::new("create"))
        .get_matches();

    if matches.subcommand_matches("create").is_some() {
        println!("created");
    }
}
```

Problems:

- `clap::Command` is a parser definition, not the runtime application.
- Runtime setup, output policy, and command behavior are smeared into `main`.
- Tests must fake parser matches instead of constructing a typed app.

Good:

```rust
fn main() -> std::process::ExitCode {
    match App::from_env().and_then(|app| app.run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
```

### Bad: putting runtime state inside clap-derived parser structs

```rust
#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    pub app: app_core::AppContext,
}
```

Problems:

- `clap` can parse argv, but it cannot discover application directories or
  construct service clients.
- Parser data and runtime state now have different lifecycles inside one type.
- Tests cannot parse help or command grammar without constructing runtime state.

Good:

```rust
#[derive(Debug, clap::Parser)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalOptions,

    #[command(subcommand)]
    pub command: Command,
}

pub(crate) struct CommandContext {
    app: app_core::AppContext,
    output: Output,
}
```

### Bad: free command helpers instead of `AppCommand`

```rust
match cli.command {
    Command::Create(command) => create(&ctx, &global, command),
    Command::Remove(command) => remove(&ctx, &global, command),
}

fn create(
    ctx: &app_core::AppContext,
    global: &GlobalOptions,
    command: CreateCommand,
) -> anyhow::Result<()> {
    let output = app_projects::create_project(
        ctx,
        app_projects::CreateProject {
            path: PathBuf::new(),
            template: None,
        },
    )?;
    print_output(global, &output)
}
```

Problems:

- Command behavior is detached from the command type that owns the parsed data.
- New commands can skip the shared command contract.
- Root command execution becomes a bag of loose helpers.

Good:

```rust
pub(crate) trait AppCommand {
    fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()>;
}

impl AppCommand for Command {
    fn execute(&self, ctx: &CommandContext) -> anyhow::Result<()> {
        match self {
            Self::Create(command) => command.execute(ctx),
            Self::Remove(command) => command.execute(ctx),
        }
    }
}
```

## 1. Passing `clap` types into business logic

Bad:

```rust
// app-projects/src/lib.rs
pub fn create_project(matches: &clap::ArgMatches) -> anyhow::Result<()> {
    let path = matches.get_one::<String>("path").unwrap();
    std::fs::create_dir_all(path)?;
    println!("created {path}");
    Ok(())
}
```

Why it is bad:

- The domain crate now depends on the argument parser.
- It cannot be reused by tests, a GUI, a daemon, or a future REPL without fake CLI matches.
- It prints directly, so output policy is scattered.

Good:

```rust
// app-projects/src/lib.rs
#[derive(Debug)]
pub struct CreateProject {
    pub path: std::path::PathBuf,
    pub template: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CreateProjectOutput {
    pub path: std::path::PathBuf,
}

pub fn create_project(
    ctx: &app_core::AppContext,
    cmd: CreateProject,
) -> Result<CreateProjectOutput, Error> {
    let path = ctx.resolve_path(cmd.path);
    fs_err::create_dir_all(&path)?;
    Ok(CreateProjectOutput { path })
}
```

```rust
// app-cli/src/command.rs
let output = app_projects::create_project(
    &ctx,
    app_projects::CreateProject { path, template },
)?;
print_json_or_text(&global, &output)?;
```

## 2. Treating paths as strings

Bad:

```rust
#[derive(clap::Parser)]
struct Args {
    path: String,
}

let manifest = format!("{}/app-project.toml", args.path);
```

Why it is bad:

- OS paths are not guaranteed to be UTF-8.
- String concatenation mishandles separators and prefixes.
- Windows, UNC, and relative path behavior become accidental.

Good:

```rust
#[derive(clap::Parser)]
struct Args {
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    path: std::path::PathBuf,
}

let manifest = args.path.join("app-project.toml");
```

## 3. Using stdout for logs or progress

Bad:

```rust
println!("opening {}", path.display());
println!("{}", serde_json::to_string(&data)?);
```

Why it is bad:

- Piped JSON becomes invalid.
- Scripts cannot reliably parse command output.

Good:

```rust
tracing::debug!(path = %path.display(), "opening project manifest");
println!("{}", serde_json::to_string_pretty(&data)?);
```

Configure `tracing-subscriber` with a stderr writer:

```rust
tracing_subscriber::fmt()
    .with_writer(std::io::stderr)
    .init();
```

## 4. Calling `std::process::exit` in libraries or handlers

Bad:

```rust
pub fn remove_project(path: &Path) {
    if !path.exists() {
        eprintln!("not found");
        std::process::exit(2);
    }
}
```

Good:

```rust
pub fn remove_project(path: &Path) -> Result<RemoveProjectOutput, Error> {
    if !path.exists() {
        return Err(Error::NotFound { path: path.to_path_buf() });
    }
    fs_err::remove_dir_all(path)?;
    Ok(RemoveProjectOutput { removed: true })
}
```

Only `main` maps errors to process exit codes.

## 5. Using `anyhow` as the public library error type

Bad:

```rust
pub fn parse_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let text = fs_err::read_to_string(path)?;
    Ok(toml_edit::de::from_str(&text)?)
}
```

Good:

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn parse_manifest(path: &Path) -> Result<Manifest, Error> {
    let text = fs_err::read_to_string(path)?;
    toml_edit::de::from_str(&text).map_err(|err| Error::InvalidManifest {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}
```

Use `anyhow::Context` in the CLI adapter:

```rust
let manifest = app_projects::parse_manifest(path)
    .with_context(|| format!("failed to parse {}", path.display()))?;
```

## 6. Non-atomic config writes

Bad:

```rust
std::fs::write(config_path, contents)?;
```

Why it is bad:

- A crash or interruption can leave a partial file.
- Another process may read a half-written config.

Good:

```rust
use std::io::Write;
use atomic_write_file::AtomicWriteFile;

let mut file = AtomicWriteFile::open(config_path)?;
file.write_all(contents.as_bytes())?;
file.commit()?;
```

## 7. Hard-coded Unix assumptions

Bad:

```rust
let path = PathBuf::from(format!("/tmp/{name}"));
let config = PathBuf::from(format!("{}/.config/app/config.toml", home));
```

Good:

```rust
let temp = tempfile::tempdir()?;
let dirs = directories::ProjectDirs::from("com", "example", "app")
    .ok_or(Error::ProjectDirsUnavailable)?;
let config = dirs.config_dir().join("config.toml");
```

## 8. Accidental symlink traversal

Bad:

```rust
for entry in walkdir::WalkDir::new(root).follow_links(true) {
    let entry = entry?;
    fs_err::remove_file(entry.path())?;
}
```

Good:

```rust
pub struct CleanOptions {
    pub root: PathBuf,
    pub follow_links: bool,
}

let walker = walkdir::WalkDir::new(&opts.root).follow_links(opts.follow_links);
for entry in walker {
    let entry = entry?;
    if entry.file_type().is_file() {
        fs_err::remove_file(entry.path())?;
    }
}
```

Expose symlink behavior in the command if it matters:

```rust
#[arg(long)]
follow_links: bool,
```

## 9. Relying on shell glob expansion

Bad UX:

```text
app remove project *.tmp
```

This may expand on Unix shells but not in many Windows contexts.

Good UX:

```text
app remove project --glob "*.tmp"
```

Then implement globbing explicitly and test it on Windows.

## 10. Debug formatting as public output

Bad:

```rust
println!("{output:#?}");
```

Good:

```rust
match format {
    OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&output)?),
    OutputFormat::Text => println!("{}\t{}", output.path.display(), output.name),
}
```

Human output is part of the user interface. Design it intentionally.

## 11. No command integration tests

Bad:

```rust
#[test]
fn create_project_name_defaults() {
    assert_eq!(default_name("demo"), "demo");
}
```

This is fine as a unit test but insufficient for a CLI command.

Good:

```rust
#[test]
fn create_project_command_creates_manifest() {
    let temp = assert_fs::TempDir::new().unwrap();
    let project = temp.child("demo");

    assert_cmd::Command::cargo_bin("app")
        .unwrap()
        .args(["create", "project"])
        .arg(project.path())
        .assert()
        .success();

    project.child("app-project.toml").assert(predicates::path::exists());
    temp.close().unwrap();
}
```

## 12. Conflating `list`, `show`, and `status`

Bad:

```text
app project --all
app project --details PATH
app project --status PATH
```

Good:

```text
app list project
app show project PATH
app status project PATH
```

Use verbs for actions and subjects for resource types. Keep command discovery simple.

## 13. Hidden mutable globals

Bad:

```rust
static mut CONFIG_DIR: Option<PathBuf> = None;
```

Good:

```rust
#[derive(Debug, Clone)]
pub struct AppContext {
    pub cwd: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

pub fn run(ctx: &AppContext, cmd: CreateProject) -> Result<Output, Error> {
    // explicit dependencies
}
```

## 14. Over-broad command names

Bad:

```text
app run project PATH
app do project PATH
app process project PATH
```

Good:

```text
app create project PATH
app update project PATH --name NAME
app remove project PATH --force
```

Use domain-specific verbs with concrete effects.

## 15. Finishing without verification

Bad final state:

- Code compiles only in the agent's head.
- No tests were added.
- No help output was checked.
- No Windows path behavior was considered.

Good final checklist:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

If any check cannot be run, report the exact reason and what remains unverified.
