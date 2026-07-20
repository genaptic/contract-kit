//! Exhaustive command dispatch.
//!
//! Each verb module adapts its parsed command into CLI-owned filesystem work,
//! domain requests, persistence, and user-facing output.

mod archive;
mod check;
mod diff;
mod generate;

use crate::args::Command;
use crate::context::CommandContext;

/// Execution contract shared by every parsed top-level command.
///
/// Implementations adapt CLI-owned paths and options into domain requests,
/// sequence the selected workflow, persist returned bytes when applicable,
/// and print the final user-facing result.
pub(crate) trait AppCommand {
    /// Executes a parsed command against initialized runtime context.
    ///
    /// # Errors
    ///
    /// Returns an error from the selected command's validation, domain work,
    /// persistence, report/archive handling, cancellation, or terminal output.
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()>;
}

impl AppCommand for Command {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        match self {
            Self::Check(command) => command.execute(context).await,
            Self::Generate(command) => command.execute(context).await,
            Self::Archive(command) => command.execute(context).await,
            Self::Diff(command) => command.execute(context).await,
        }
    }
}
