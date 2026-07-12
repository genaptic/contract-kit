# Full CRUD Rust shell CLI example

This reference is the canonical code shape for a modern Rust shell CLI. It is
intentionally exhaustive so future command work can copy the architecture
without inventing missing pieces.

It is **not** the architecture specification for `conkit`. Treat its `app`,
`crates/app-*`, CRUD grammar, synchronous `AppCommand`, and domain `PathBuf`
types as generic teaching material. In `conkit`, follow `conkit/args.rs`, the native
async dispatcher in `conkit/command.rs`, CLI-owned filesystem/catalog adapters,
and catalog/byte requests into `conkit-signature` and `conkit-sketch`.

## Table of contents

1. [Workspace and dependencies](#1-workspace-and-dependencies)
2. [CLI crate runtime architecture](#2-cli-crate-runtime-architecture)
3. [CLI grammar and command execution](#3-cli-grammar-and-command-execution)
4. [Output and logging](#4-output-and-logging)
5. [Core, filesystem, project, and config crates](#5-core-filesystem-project-and-config-crates)
6. [Tests and CI checks](#6-tests-and-ci-checks)
7. [Good and bad pattern summary](#7-good-and-bad-pattern-summary)

The example command surface is:

```text
app create project PATH [--name NAME] [--template TEMPLATE]
app list project [--format text|json]
app show project PATH [--format text|json]
app update project PATH [--name NAME] [--description DESCRIPTION]
app remove project PATH --force
app show config
app set config KEY VALUE
app completion SHELL
```

## 1. Workspace and dependencies

Root `Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = [
    "crates/app-cli",
    "crates/app-core",
    "crates/app-fs",
    "crates/app-projects",
    "crates/app-config",
]
default-members = ["crates/app-cli"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
anyhow = "1"
atomic-write-file = "0.3"
clap = "4"
clap_complete = "4"
clap-verbosity-flag = "3"
directories = "6"
fs-err = "3"
serde = "1"
serde_json = "1"
tempfile = "3"
thiserror = "2"
toml_edit = "0.25"
tracing = "0.1"
tracing-subscriber = "0.3"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
expect_used = "warn"
unwrap_used = "warn"
```

`crates/app-cli/Cargo.toml`:

```toml
[package]
name = "app-cli"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "app"
path = "src/main.rs"

[lints]
workspace = true

[dependencies]
app-core = { path = "../app-core" }
app-config = { path = "../app-config" }
app-projects = { path = "../app-projects" }
anyhow = { workspace = true }
clap = { workspace = true, features = ["derive", "env", "wrap_help"] }
clap_complete = { workspace = true }
clap-verbosity-flag = { workspace = true, features = ["tracing"] }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tracing-subscriber = { workspace = true, features = ["env-filter", "fmt"] }

[dev-dependencies]
assert_cmd = "2"
assert_fs = "1"
predicates = "3"
```

## 2. CLI crate runtime architecture

`clap` parses argv into `Cli`. It does not construct the runtime application.
The CLI crate owns a root `App` that initializes runtime state after parsing and
executes commands through `AppCommand`.

`crates/app-cli/src/main.rs`:

```rust
mod app;
mod cli;
mod command;
mod context;
mod logging;
mod output;

use std::process::ExitCode;

use app::App;

fn main() -> ExitCode {
    match App::from_env().and_then(|app| app.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}
```

`crates/app-cli/src/app.rs`:

```rust
use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::command::AppCommand;
use crate::context::CommandContext;

#[derive(Debug)]
pub(crate) struct App {
    cli: Cli,
    context: CommandContext,
}

impl App {
    pub(crate) fn from_env() -> Result<Self> {
        Self::from_cli(Cli::parse())
    }

    pub(crate) fn from_cli(cli: Cli) -> Result<Self> {
        let context = CommandContext::initialize(&cli)?;
        Ok(Self { cli, context })
    }

    pub(crate) fn run(&self) -> Result<()> {
        self.cli.command.execute(&self.context)
    }
}
```

`crates/app-cli/src/context.rs`:

```rust
use anyhow::{Context, Result};

use crate::cli::{Cli, GlobalOptions};
use crate::logging;
use crate::output::Output;

#[derive(Debug)]
pub(crate) struct CommandContext {
    app: app_core::AppContext,
    global: GlobalOptions,
    output: Output,
}

impl CommandContext {
    pub(crate) fn initialize(cli: &Cli) -> Result<Self> {
        logging::init(&cli.global).context("failed to initialize logging")?;

        let app = app_core::AppContext::discover(app_core::DiscoverOptions {
            cwd: cli.global.cwd.clone(),
        })
        .context("failed to initialize app context")?;

        let output = Output::from_global(&cli.global);

        Ok(Self {
            app,
            global: cli.global.clone(),
            output,
        })
    }

    pub(crate) fn app(&self) -> &app_core::AppContext {
        &self.app
    }

    pub(crate) fn global(&self) -> &GlobalOptions {
        &self.global
    }

    pub(crate) fn output(&self) -> &Output {
        &self.output
    }
}
```

## 3. CLI grammar and command execution

`crates/app-cli/src/cli.rs`:

```rust
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum, ValueHint};
use clap_complete::Shell;
use clap_verbosity_flag::{InfoLevel, Verbosity};

#[derive(Debug, Parser)]
#[command(
    name = "app",
    version,
    about = "Manage local app projects",
    propagate_version = true,
    arg_required_else_help = true
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalOptions,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Args, Clone)]
pub(crate) struct GlobalOptions {
    #[command(flatten)]
    pub(crate) verbosity: Verbosity<InfoLevel>,

    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[arg(long, global = true, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Create(CreateCommand),
    List(ListCommand),
    Show(ShowCommand),
    Update(UpdateCommand),
    Remove(RemoveCommand),
    Set(SetCommand),
    Completion(CompletionCommand),
}

#[derive(Debug, Args)]
pub(crate) struct CreateCommand {
    #[command(subcommand)]
    pub(crate) subject: CreateSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CreateSubject {
    Project {
        #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
        path: PathBuf,

        #[arg(long)]
        name: Option<String>,

        #[arg(long)]
        template: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ListCommand {
    #[command(subcommand)]
    pub(crate) subject: ListSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ListSubject {
    Project {
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ShowCommand {
    #[command(subcommand)]
    pub(crate) subject: ShowSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ShowSubject {
    Config,
    Project {
        #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
        path: PathBuf,

        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct UpdateCommand {
    #[command(subcommand)]
    pub(crate) subject: UpdateSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum UpdateSubject {
    Project {
        #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
        path: PathBuf,

        #[arg(long)]
        name: Option<String>,

        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct RemoveCommand {
    #[command(subcommand)]
    pub(crate) subject: RemoveSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RemoveSubject {
    Project {
        #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
        path: PathBuf,

        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct SetCommand {
    #[command(subcommand)]
    pub(crate) subject: SetSubject,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SetSubject {
    Config { key: String, value: String },
}

#[derive(Debug, Args)]
pub(crate) struct CompletionCommand {
    #[arg(value_enum)]
    pub(crate) shell: Shell,
}
```

`crates/app-cli/src/command.rs`:

```rust
use anyhow::{Context, Result};
use clap::CommandFactory;

use crate::cli::{
    Cli, Command, CompletionCommand, CreateCommand, CreateSubject, ListCommand, ListSubject,
    OutputFormat, RemoveCommand, RemoveSubject, SetCommand, SetSubject, ShowCommand, ShowSubject,
    UpdateCommand, UpdateSubject,
};
use crate::context::CommandContext;

pub(crate) trait AppCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()>;
}

impl AppCommand for Command {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match self {
            Self::Create(command) => command.execute(ctx),
            Self::List(command) => command.execute(ctx),
            Self::Show(command) => command.execute(ctx),
            Self::Update(command) => command.execute(ctx),
            Self::Remove(command) => command.execute(ctx),
            Self::Set(command) => command.execute(ctx),
            Self::Completion(command) => command.execute(ctx),
        }
    }
}

impl AppCommand for CreateCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            CreateSubject::Project {
                path,
                name,
                template,
            } => {
                let output = app_projects::create_project(
                    ctx.app(),
                    app_projects::CreateProject {
                        path: path.clone(),
                        name: name.clone(),
                        template: template.clone(),
                    },
                )
                .context("failed to create project")?;

                ctx.output().print_json_or_text(&output, None, |out| {
                    format!("created project {}", out.path.display())
                })
            }
        }
    }
}

impl AppCommand for ListCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            ListSubject::Project { format } => {
                let output = app_projects::list_projects(ctx.app())
                    .context("failed to list projects")?;

                match ctx.output().resolve(*format) {
                    OutputFormat::Json => ctx.output().print_json(&output),
                    OutputFormat::Text => ctx.output().print_project_rows(&output.projects),
                }
            }
        }
    }
}

impl AppCommand for ShowCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            ShowSubject::Config => {
                let output = app_config::show_config(ctx.app())
                    .context("failed to show config")?;
                ctx.output().print_json_or_text(&output, None, |out| {
                    format!(
                        "editor: {}\ndefault_template: {}",
                        out.editor.as_deref().unwrap_or("<unset>"),
                        out.default_template
                    )
                })
            }
            ShowSubject::Project { path, format } => {
                let output = app_projects::show_project(
                    ctx.app(),
                    app_projects::ShowProject { path: path.clone() },
                )
                .context("failed to show project")?;

                match ctx.output().resolve(*format) {
                    OutputFormat::Json => ctx.output().print_json(&output),
                    OutputFormat::Text => ctx.output().print_text(format_args!(
                        "{}\n  name: {}\n  template: {}",
                        output.path.display(),
                        output.manifest.name,
                        output.manifest.template
                    )),
                }
            }
        }
    }
}

impl AppCommand for UpdateCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            UpdateSubject::Project {
                path,
                name,
                description,
            } => {
                let output = app_projects::update_project(
                    ctx.app(),
                    app_projects::UpdateProject {
                        path: path.clone(),
                        name: name.clone(),
                        description: description.clone(),
                    },
                )
                .context("failed to update project")?;

                ctx.output().print_json_or_text(&output, None, |out| {
                    format!("updated project {}", out.path.display())
                })
            }
        }
    }
}

impl AppCommand for RemoveCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            RemoveSubject::Project { path, force } => {
                let output = app_projects::remove_project(
                    ctx.app(),
                    app_projects::RemoveProject {
                        path: path.clone(),
                        force: *force,
                    },
                )
                .context("failed to remove project")?;

                ctx.output().print_json_or_text(&output, None, |out| {
                    format!("removed project {}", out.path.display())
                })
            }
        }
    }
}

impl AppCommand for SetCommand {
    fn execute(&self, ctx: &CommandContext) -> Result<()> {
        match &self.subject {
            SetSubject::Config { key, value } => {
                let output = app_config::set_config(
                    ctx.app(),
                    app_config::SetConfig {
                        key: key.clone(),
                        value: value.clone(),
                    },
                )
                .context("failed to set config")?;

                ctx.output().print_json_or_text(&output, None, |out| {
                    format!("set config {}", out.key)
                })
            }
        }
    }
}

impl AppCommand for CompletionCommand {
    fn execute(&self, _ctx: &CommandContext) -> Result<()> {
        let mut command = Cli::command();
        let bin_name = command.get_name().to_owned();
        clap_complete::generate(self.shell, &mut command, bin_name, &mut std::io::stdout());
        Ok(())
    }
}
```

## 4. Output and logging

`crates/app-cli/src/output.rs`:

```rust
use std::fmt;

use anyhow::Result;

use crate::cli::{GlobalOptions, OutputFormat};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Output {
    default_format: OutputFormat,
}

impl Output {
    pub(crate) fn from_global(global: &GlobalOptions) -> Self {
        let default_format = if global.json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        };
        Self { default_format }
    }

    pub(crate) fn resolve(&self, requested: Option<OutputFormat>) -> OutputFormat {
        requested.unwrap_or(self.default_format)
    }

    pub(crate) fn print_json<T>(&self, value: &T) -> Result<()>
    where
        T: serde::Serialize,
    {
        println!("{}", serde_json::to_string_pretty(value)?);
        Ok(())
    }

    pub(crate) fn print_text(&self, args: fmt::Arguments<'_>) -> Result<()> {
        println!("{args}");
        Ok(())
    }

    pub(crate) fn print_json_or_text<T>(
        &self,
        value: &T,
        requested: Option<OutputFormat>,
        render_text: impl FnOnce(&T) -> String,
    ) -> Result<()>
    where
        T: serde::Serialize,
    {
        match self.resolve(requested) {
            OutputFormat::Json => self.print_json(value),
            OutputFormat::Text => self.print_text(format_args!("{}", render_text(value))),
        }
    }

    pub(crate) fn print_project_rows(&self, projects: &[app_projects::ProjectSummary]) -> Result<()> {
        for project in projects {
            println!("{}\t{}", project.path.display(), project.name);
        }
        Ok(())
    }
}
```

`crates/app-cli/src/logging.rs`:

```rust
use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::cli::GlobalOptions;

pub(crate) fn init(global: &GlobalOptions) -> Result<()> {
    let level = global.verbosity.tracing_level_filter().to_string();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .try_init()?;

    Ok(())
}
```

## 5. Core, filesystem, project, and config crates

`crates/app-core/src/lib.rs`:

```rust
mod context;

pub use context::{AppContext, DiscoverOptions, Error};
```

`crates/app-core/src/context.rs`:

```rust
use std::path::PathBuf;

use directories::ProjectDirs;

#[derive(Debug, Clone)]
pub struct AppContext {
    pub cwd: PathBuf,
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DiscoverOptions {
    pub cwd: Option<PathBuf>,
}

impl AppContext {
    pub fn discover(options: DiscoverOptions) -> Result<Self, Error> {
        let cwd = match options.cwd {
            Some(path) => path,
            None => std::env::current_dir()?,
        };

        let dirs = ProjectDirs::from("com", "example", "app")
            .ok_or(Error::ProjectDirsUnavailable)?;

        Ok(Self {
            cwd,
            config_dir: dirs.config_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
        })
    }

    pub fn resolve_path(&self, path: PathBuf) -> PathBuf {
        if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not determine platform-specific app directories")]
    ProjectDirsUnavailable,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

`crates/app-fs/src/lib.rs`:

```rust
pub mod atomic;
pub mod ensure;
```

`crates/app-fs/src/atomic.rs`:

```rust
use std::io::Write;
use std::path::Path;

use atomic_write_file::AtomicWriteFile;

pub fn write_string(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent)?;
    }

    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(contents.as_bytes())?;
    file.commit()
}
```

`crates/app-fs/src/ensure.rs`:

```rust
use std::path::Path;

pub fn ensure_dir_does_not_exist(path: &Path) -> Result<(), std::io::Error> {
    match fs_err::metadata(path) {
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("path already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
```

`crates/app-projects/src/lib.rs`:

```rust
mod error;
mod manifest;
mod ops;

use std::path::PathBuf;

use serde::Serialize;

pub use error::Error;
pub use manifest::ProjectManifest;

#[derive(Debug)]
pub struct CreateProject {
    pub path: PathBuf,
    pub name: Option<String>,
    pub template: Option<String>,
}

#[derive(Debug)]
pub struct ShowProject {
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct UpdateProject {
    pub path: PathBuf,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug)]
pub struct RemoveProject {
    pub path: PathBuf,
    pub force: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CreateProjectOutput {
    pub path: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug, Serialize)]
pub struct ListProjectsOutput {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Serialize)]
pub struct ShowProjectOutput {
    pub path: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug, Serialize)]
pub struct UpdateProjectOutput {
    pub path: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug, Serialize)]
pub struct RemoveProjectOutput {
    pub path: PathBuf,
    pub removed: bool,
}

pub fn create_project(
    ctx: &app_core::AppContext,
    cmd: CreateProject,
) -> Result<CreateProjectOutput, Error> {
    ops::create_project(ctx, cmd)
}

pub fn list_projects(ctx: &app_core::AppContext) -> Result<ListProjectsOutput, Error> {
    ops::list_projects(ctx)
}

pub fn show_project(
    ctx: &app_core::AppContext,
    cmd: ShowProject,
) -> Result<ShowProjectOutput, Error> {
    ops::show_project(ctx, cmd)
}

pub fn update_project(
    ctx: &app_core::AppContext,
    cmd: UpdateProject,
) -> Result<UpdateProjectOutput, Error> {
    ops::update_project(ctx, cmd)
}

pub fn remove_project(
    ctx: &app_core::AppContext,
    cmd: RemoveProject,
) -> Result<RemoveProjectOutput, Error> {
    ops::remove_project(ctx, cmd)
}
```

`crates/app-projects/src/error.rs`:

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("project already exists at {path}")]
    AlreadyExists { path: PathBuf },

    #[error("project does not exist at {path}")]
    NotFound { path: PathBuf },

    #[error("refusing to remove project without --force: {path}")]
    RefusingToRemoveWithoutForce { path: PathBuf },

    #[error("invalid project manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlDe(#[from] toml_edit::de::Error),

    #[error(transparent)]
    TomlSer(#[from] toml_edit::ser::Error),
}
```

`crates/app-projects/src/manifest.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectManifest {
    pub name: String,
    pub template: String,
    pub description: Option<String>,
}

impl Default for ProjectManifest {
    fn default() -> Self {
        Self {
            name: "project".to_owned(),
            template: "basic".to_owned(),
            description: None,
        }
    }
}
```

`crates/app-projects/src/ops.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::{
    CreateProject, CreateProjectOutput, Error, ListProjectsOutput, ProjectManifest,
    ProjectSummary, RemoveProject, RemoveProjectOutput, ShowProject, ShowProjectOutput,
    UpdateProject, UpdateProjectOutput,
};

const MANIFEST_FILE: &str = "app-project.toml";

pub fn create_project(
    ctx: &app_core::AppContext,
    cmd: CreateProject,
) -> Result<CreateProjectOutput, Error> {
    let path = ctx.resolve_path(cmd.path);

    if path.exists() {
        return Err(Error::AlreadyExists { path });
    }

    fs_err::create_dir_all(&path)?;

    let default_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned();

    let manifest = ProjectManifest {
        name: cmd.name.unwrap_or(default_name),
        template: cmd.template.unwrap_or_else(|| "basic".to_owned()),
        description: None,
    };

    write_manifest(&path, &manifest)?;
    remember_project(ctx, &path)?;

    Ok(CreateProjectOutput { path, manifest })
}

pub fn list_projects(ctx: &app_core::AppContext) -> Result<ListProjectsOutput, Error> {
    let index_path = project_index_path(ctx);
    if !index_path.exists() {
        return Ok(ListProjectsOutput {
            projects: Vec::new(),
        });
    }

    let contents = fs_err::read_to_string(&index_path)?;
    let mut projects = Vec::new();

    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let path = PathBuf::from(line);
        if let Ok(manifest) = read_manifest(&path) {
            projects.push(ProjectSummary {
                path,
                name: manifest.name,
            });
        }
    }

    projects.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(ListProjectsOutput { projects })
}

pub fn show_project(
    ctx: &app_core::AppContext,
    cmd: ShowProject,
) -> Result<ShowProjectOutput, Error> {
    let path = ctx.resolve_path(cmd.path);
    let manifest = read_manifest_or_not_found(&path)?;
    Ok(ShowProjectOutput { path, manifest })
}

pub fn update_project(
    ctx: &app_core::AppContext,
    cmd: UpdateProject,
) -> Result<UpdateProjectOutput, Error> {
    let path = ctx.resolve_path(cmd.path);
    let mut manifest = read_manifest_or_not_found(&path)?;

    if let Some(name) = cmd.name {
        manifest.name = name;
    }

    if let Some(description) = cmd.description {
        manifest.description = Some(description);
    }

    write_manifest(&path, &manifest)?;
    Ok(UpdateProjectOutput { path, manifest })
}

pub fn remove_project(
    ctx: &app_core::AppContext,
    cmd: RemoveProject,
) -> Result<RemoveProjectOutput, Error> {
    let path = ctx.resolve_path(cmd.path);

    if !path.exists() {
        return Err(Error::NotFound { path });
    }

    if !cmd.force {
        return Err(Error::RefusingToRemoveWithoutForce { path });
    }

    fs_err::remove_dir_all(&path)?;
    forget_project(ctx, &path)?;

    Ok(RemoveProjectOutput {
        path,
        removed: true,
    })
}

fn manifest_path(project_path: &Path) -> PathBuf {
    project_path.join(MANIFEST_FILE)
}

fn read_manifest_or_not_found(path: &Path) -> Result<ProjectManifest, Error> {
    if !manifest_path(path).exists() {
        return Err(Error::NotFound {
            path: path.to_path_buf(),
        });
    }

    read_manifest(path)
}

fn read_manifest(path: &Path) -> Result<ProjectManifest, Error> {
    let manifest_path = manifest_path(path);
    let contents = fs_err::read_to_string(&manifest_path)?;
    let manifest: ProjectManifest = toml_edit::de::from_str(&contents).map_err(|error| {
        Error::InvalidManifest {
            path: manifest_path,
            message: error.to_string(),
        }
    })?;
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &ProjectManifest) -> Result<(), Error> {
    let contents = toml_edit::ser::to_string_pretty(manifest)?;
    app_fs::atomic::write_string(&manifest_path(path), &contents)?;
    Ok(())
}

fn project_index_path(ctx: &app_core::AppContext) -> PathBuf {
    ctx.data_dir.join("projects.txt")
}

fn remember_project(ctx: &app_core::AppContext, path: &Path) -> Result<(), Error> {
    let index_path = project_index_path(ctx);
    let mut entries = read_index(&index_path)?;
    let entry = path.to_path_buf();
    if !entries.iter().any(|existing| existing == &entry) {
        entries.push(entry);
    }
    write_index(&index_path, &entries)
}

fn forget_project(ctx: &app_core::AppContext, path: &Path) -> Result<(), Error> {
    let index_path = project_index_path(ctx);
    let mut entries = read_index(&index_path)?;
    entries.retain(|entry| entry != path);
    write_index(&index_path, &entries)
}

fn read_index(path: &Path) -> Result<Vec<PathBuf>, Error> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs_err::read_to_string(path)?;
    Ok(contents.lines().map(PathBuf::from).collect())
}

fn write_index(path: &Path, entries: &[PathBuf]) -> Result<(), Error> {
    let mut contents = String::new();
    for entry in entries {
        contents.push_str(&entry.display().to_string());
        contents.push('\n');
    }
    app_fs::atomic::write_string(path, &contents)?;
    Ok(())
}
```

`crates/app-config/src/lib.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct SetConfig {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub editor: Option<String>,
    pub default_template: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            editor: None,
            default_template: "basic".to_owned(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SetConfigOutput {
    pub key: String,
}

pub fn show_config(ctx: &app_core::AppContext) -> Result<AppConfig, Error> {
    let path = config_path(ctx);
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let contents = fs_err::read_to_string(path)?;
    Ok(toml_edit::de::from_str(&contents)?)
}

pub fn set_config(ctx: &app_core::AppContext, cmd: SetConfig) -> Result<SetConfigOutput, Error> {
    let mut config = show_config(ctx)?;

    match cmd.key.as_str() {
        "editor" => config.editor = Some(cmd.value),
        "default_template" => config.default_template = cmd.value,
        other => return Err(Error::UnknownKey(other.to_owned())),
    }

    let contents = toml_edit::ser::to_string_pretty(&config)?;
    app_fs::atomic::write_string(&config_path(ctx), &contents)?;

    Ok(SetConfigOutput { key: cmd.key })
}

fn config_path(ctx: &app_core::AppContext) -> std::path::PathBuf {
    ctx.config_dir.join("config.toml")
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown config key: {0}")]
    UnknownKey(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlDe(#[from] toml_edit::de::Error),

    #[error(transparent)]
    TomlSer(#[from] toml_edit::ser::Error),
}
```

## 6. Tests and CI checks

`crates/app-cli/tests/projects.rs`:

```rust
use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

#[test]
fn create_show_update_remove_project() {
    let temp = assert_fs::TempDir::new().unwrap();
    let project = temp.child("demo project");

    Command::cargo_bin("app")
        .unwrap()
        .args(["--cwd"])
        .arg(temp.path())
        .args(["create", "project"])
        .arg(project.path())
        .args(["--name", "Demo", "--template", "basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("created project"))
        .stderr(predicate::str::is_empty());

    project
        .child("app-project.toml")
        .assert(predicate::path::exists());

    Command::cargo_bin("app")
        .unwrap()
        .args(["show", "project"])
        .arg(project.path())
        .args(["--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"Demo\""));

    Command::cargo_bin("app")
        .unwrap()
        .args(["update", "project"])
        .arg(project.path())
        .args(["--description", "A test project"])
        .assert()
        .success();

    Command::cargo_bin("app")
        .unwrap()
        .args(["remove", "project"])
        .arg(project.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--force"));

    Command::cargo_bin("app")
        .unwrap()
        .args(["remove", "project"])
        .arg(project.path())
        .arg("--force")
        .assert()
        .success();

    project.assert(predicate::path::missing());
    temp.close().unwrap();
}

#[test]
fn nested_help_mentions_project_subjects() {
    Command::cargo_bin("app")
        .unwrap()
        .args(["create", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("project"));
}
```

`crates/app-projects/src/ops.rs` unit test excerpt:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_project_changes_manifest_fields() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = app_core::AppContext {
            cwd: temp.path().to_path_buf(),
            config_dir: temp.path().join("config"),
            cache_dir: temp.path().join("cache"),
            data_dir: temp.path().join("data"),
        };

        let created = create_project(
            &ctx,
            CreateProject {
                path: PathBuf::from("demo"),
                name: Some("Demo".to_owned()),
                template: Some("basic".to_owned()),
            },
        )
        .unwrap();

        let updated = update_project(
            &ctx,
            UpdateProject {
                path: PathBuf::from("demo"),
                name: Some("Renamed".to_owned()),
                description: Some("Updated by test".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(created.path, temp.path().join("demo"));
        assert_eq!(updated.manifest.name, "Renamed");
        assert_eq!(
            updated.manifest.description.as_deref(),
            Some("Updated by test")
        );
    }
}
```

CI checks:

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo check --locked --workspace --all-targets --all-features
      - run: cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
      - run: cargo test --locked --workspace --doc
      - run: cargo test --locked --workspace --all-targets
```

## 7. Good and bad pattern summary

Good:

- `main` calls `App::from_env()` and maps `anyhow::Error` to `ExitCode`.
- `clap` produces typed `Cli` data only.
- `CommandContext` is initialized after parsing and carries shared runtime state.
- `AppCommand` is implemented by the root `Command` enum and every verb command.
- Root command execution uses explicit exhaustive match arms with receiver-method calls.
- Domain crates expose typed command structs, typed outputs, and typed errors.
- Domain crates do not depend on `clap`, stdout, stderr, terminal colors, or process exits.
- `PathBuf` is used at CLI and domain boundaries.
- Human output is explicit and JSON output is serialized from stable structs.
- Tests call the real binary and assert filesystem side effects.

Bad:

- `clap::Command` treated as the runtime application container.
- `AppContext` or service clients stored inside clap-derived parser structs.
- Free helper functions such as `create(ctx, global, cmd)` used as command parity.
- `Command::Create(command) => create(ctx, command)` instead of `command.execute(ctx)`.
- Passing `clap::ArgMatches` into a library crate.
- Domain functions printing directly or calling `std::process::exit`.
- String concatenation for paths.
- `std::fs::write` for config or state updates that should be atomic.
