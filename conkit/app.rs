//! Top-level application runtime for the executable.
//!
//! This module keeps `main` thin by owning clap parsing, platform-specific
//! executable naming, context initialization, and command dispatch.

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};

use crate::args::Cli;
use crate::command::AppCommand;
use crate::context::CommandContext;

/// Parsed CLI state plus initialized runtime dependencies.
pub(crate) struct App {
    cli: Cli,
    context: CommandContext,
}

impl App {
    /// Parses process arguments, initializes runtime state, and executes the command.
    ///
    /// # Errors
    ///
    /// Returns an error if application construction or command execution fails.
    pub(crate) async fn from_env_and_run() -> Result<()> {
        Self::from_env()?.run().await
    }

    /// Builds an application from the current process environment.
    ///
    /// The displayed executable name is shared across every supported host.
    /// Clap terminates the process after rendering requested help or version
    /// output, or after reporting invalid arguments.
    ///
    /// # Errors
    ///
    /// Returns an error if validated clap matches cannot be converted to the
    /// typed CLI state or runtime dependency initialization fails.
    pub(crate) fn from_env() -> Result<Self> {
        let matches = Cli::command()
            .name(crate::platform::EXECUTABLE_NAME)
            .bin_name(crate::platform::EXECUTABLE_NAME)
            .get_matches();
        let cli = Cli::from_arg_matches(&matches)?;

        Self::from_cli(cli)
    }

    /// Initializes runtime state independently of process argument parsing.
    ///
    /// # Errors
    ///
    /// Returns an error if a domain adapter cannot be initialized.
    pub(crate) fn from_cli(cli: Cli) -> Result<Self> {
        let context = CommandContext::initialize()?;

        Ok(Self { cli, context })
    }

    /// Executes the selected top-level command.
    ///
    /// # Errors
    ///
    /// Returns an error reported by the selected command.
    pub(crate) async fn run(&self) -> Result<()> {
        self.cli.command.execute(&self.context).await
    }
}
