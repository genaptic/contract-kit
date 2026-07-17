//! Contract-archive command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::archive::{ArchiveDestination, ArchiveFormatSelection};
use crate::args::ArchiveCommand;
use crate::catalog::{ContractsStore, PathRole, ResolvedPath};
use crate::context::CommandContext;
use crate::contracts::ContractFormatValidator;

impl AppCommand for ArchiveCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Contracts, self.contracts.clone())?,
            ResolvedPath::new(PathRole::ArchiveDirectory, self.archive.clone())?,
        ])?;

        let mut catalog_reads = context.catalog_read_limits().begin(context.cancellation());
        let contract_files = ContractsStore::new(self.contracts.clone())
            .with_limits(context.catalog_read_limits())
            .read_with_budget(&mut catalog_reads)?;
        ContractFormatValidator::new(context.cancellation()).validate(&contract_files)?;
        context.cancellation().checkpoint()?;
        let archive_bytes = ArchiveFormatSelection::from_gzip_flag(self.gzip)
            .encode(contract_files, context.cancellation())
            .context("failed to archive contracts")?;
        context.cancellation().checkpoint()?;
        let written = ArchiveDestination::new(self.archive.clone(), context.cancellation())
            .publish(archive_bytes)?;

        context.output().print_archive_summary(&written)?;
        Ok(())
    }
}
