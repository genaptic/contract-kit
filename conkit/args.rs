//! Clap grammar for the `conkit` executable.
//!
//! The types in this module describe the stable command-line interface only.
//! They intentionally carry raw `PathBuf` values and simple flags; command
//! execution modules are responsible for converting parsed input into domain
//! requests.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueHint};

/// Root parser for all CLI input.
#[derive(Debug, Parser)]
#[command(version, about = "Contract Kit", arg_required_else_help = true)]
pub(crate) struct Cli {
    /// Top-level command selected by the user.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Top-level command families exposed by the executable.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Check source files against existing contract files.
    Check(CheckCommand),
    /// Generate contract files from source files.
    Generate(GenerateCommand),
    /// Archive the current contract catalog.
    Archive(ArchiveCommand),
    /// Compare current contracts with an archived catalog.
    Diff(DiffCommand),
}

/// Parsed arguments for `conkit check`.
#[derive(Debug, Args)]
pub(crate) struct CheckCommand {
    /// Contract family to check.
    #[command(subcommand)]
    pub(crate) subject: CheckSubject,
}

/// Contract targets accepted by `conkit check`.
#[derive(Debug, Subcommand)]
pub(crate) enum CheckSubject {
    /// Check all implemented contract families.
    All(CheckArgs),
    /// Check signature contracts only.
    #[command(alias = "signature")]
    Signatures(CheckArgs),
    /// Check sketch contracts only.
    #[command(alias = "sketch")]
    Sketches(CheckArgs),
}

/// Shared filesystem and mode flags for check commands.
#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("check-mode")
        .args(["default_mode", "strict", "warning"])
        .multiple(false)
))]
pub(crate) struct CheckArgs {
    /// Root directory containing source files to inspect.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) source: PathBuf,

    /// Root directory containing contract files to compare against.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Report file to write for the requested check.
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub(crate) output: PathBuf,

    /// Use the domain crate's default check mode.
    #[arg(long = "default", group = "check-mode")]
    pub(crate) default_mode: bool,

    /// Treat contract diagnostics as check failures.
    #[arg(long, group = "check-mode")]
    pub(crate) strict: bool,

    /// Emit diagnostics without failing the contract check.
    #[arg(long, group = "check-mode")]
    pub(crate) warning: bool,
}

/// Parsed arguments for `conkit generate`.
#[derive(Debug, Args)]
pub(crate) struct GenerateCommand {
    /// Contract family to generate.
    #[command(subcommand)]
    pub(crate) subject: GenerateSubject,
}

/// Contract targets accepted by `conkit generate`.
#[derive(Debug, Subcommand)]
pub(crate) enum GenerateSubject {
    /// Generate every implemented contract family.
    All(GenerateArgs),
    /// Generate signature contracts only.
    #[command(alias = "signature")]
    Signatures(GenerateArgs),
    /// Generate sketch contracts only.
    #[command(alias = "sketch")]
    Sketches(GenerateArgs),
}

/// Shared filesystem flags for generate commands.
#[derive(Debug, Args)]
pub(crate) struct GenerateArgs {
    /// Root directory containing source files to inspect.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) source: PathBuf,

    /// Root directory where generated contract files should be written.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Adopt matching pre-existing generated outputs into managed ownership.
    #[arg(long)]
    pub(crate) adopt_existing: bool,
}

/// Parsed arguments for `conkit archive`.
#[derive(Debug, Args)]
pub(crate) struct ArchiveCommand {
    /// Root directory containing the current contract files.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Directory where a timestamped archive file should be created.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) archive: PathBuf,

    /// Select gzip archive output.
    #[arg(long)]
    pub(crate) gzip: bool,
}

/// Parsed arguments for `conkit diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffCommand {
    /// Root directory containing the current contract files.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Archive file to compare against the current contract catalog.
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub(crate) archive: PathBuf,
}
