//! Contract-archive command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::archive::{ArchiveDestination, ArchiveFormatSelection};
use crate::args::ArchiveCommand;
use crate::catalog::{ContractsStore, PathRole, ResolvedPath};
use crate::context::CommandContext;

impl AppCommand for ArchiveCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Contracts, self.contracts.clone())?,
            ResolvedPath::new(PathRole::ArchiveDirectory, self.archive.clone())?,
        ])?;

        let contract_files = ContractsStore::new(self.contracts.clone()).read()?;
        let archive_bytes = ArchiveFormatSelection::from_gzip_flag(self.gzip)
            .encode(contract_files)
            .context("failed to archive contracts")?;
        let written = ArchiveDestination::new(self.archive.clone()).publish(archive_bytes)?;

        context.output().print_archive_summary(&written)?;
        Ok(())
    }
}
