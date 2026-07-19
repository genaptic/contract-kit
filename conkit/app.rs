//! Top-level application runtime for the executable.
//!
//! This module keeps `main` thin by assigning the stable `conkit` display name,
//! parsing clap input once, installing process cancellation, initializing the
//! shared command context, and dispatching exactly one typed command.

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};

use crate::args::Cli;
use crate::command::AppCommand;
use crate::context::{ApplicationCancellation, CommandContext};

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
    /// Returns an error if process signal registration, shared-pool or domain
    /// initialization, cancellation, or the selected command fails.
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
    /// Returns an error if validated clap matches cannot be converted to typed
    /// CLI state, the process signal handler cannot be installed, available
    /// parallelism cannot be queried, the shared Rayon pool cannot be built, or
    /// either domain service cannot be initialized.
    pub(crate) fn from_env() -> Result<Self> {
        let matches = Cli::command()
            .name(crate::platform::EXECUTABLE_NAME)
            .bin_name(crate::platform::EXECUTABLE_NAME)
            .get_matches();
        let cli = Cli::from_arg_matches(&matches)?;

        let cancellation = ApplicationCancellation::process()?;
        Self::from_cli(cli, cancellation)
    }

    /// Initializes runtime state independently of process argument parsing.
    ///
    /// # Errors
    ///
    /// Returns an error if available parallelism cannot be queried, the shared
    /// Rayon pool cannot be built, or either domain service cannot be initialized.
    pub(crate) fn from_cli(cli: Cli, cancellation: ApplicationCancellation) -> Result<Self> {
        let context = CommandContext::initialize(cancellation)?;

        Ok(Self { cli, context })
    }

    /// Executes the selected top-level command.
    ///
    /// # Errors
    ///
    /// Returns an error when process cancellation wins the command race or the
    /// selected command reports a validation, domain, persistence, reporting,
    /// archive, or output failure.
    pub(crate) async fn run(&self) -> Result<()> {
        self.context
            .cancellation()
            .race(self.cli.command.execute(&self.context))
            .await?
    }
}
