//! Contract-diff command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::archive::ArchiveSource;
use crate::args::DiffCommand;
use crate::catalog::{ContractsStore, PathRole, ResolvedPath};
use crate::context::CommandContext;
use crate::contracts::ContractFormatValidator;

impl AppCommand for DiffCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Contracts, self.contracts.clone())?,
            ResolvedPath::new(PathRole::ArchiveFile, self.archive.clone())?,
        ])?;

        let mut catalog_reads = context.catalog_read_limits().begin(context.cancellation());
        let current = ContractsStore::new(self.contracts.clone())
            .with_limits(context.catalog_read_limits())
            .read_with_budget(&mut catalog_reads)?;
        let mut validator = ContractFormatValidator::new(context.cancellation());
        validator.validate(&current)?;
        let previous = ArchiveSource::new(self.archive.clone())
            .decode_contracts(&mut catalog_reads)
            .context("failed to decode contract archive")?;
        validator
            .validate(&previous)
            .context("archived contracts must be recreated in contract format v2")?;
        let signatures = context
            .signature()
            .diff(conkit_signature::DiffRequest {
                current_contract_files: current.clone(),
                previous_contract_files: previous.clone(),
            })
            .await
            .context("failed to diff contracts")?;
        let sketches = context
            .sketch()
            .diff(current, previous)
            .await
            .context("failed to diff sketch contracts")?;

        context.cancellation().checkpoint()?;
        context
            .output()
            .print_diff(&signatures, &sketches, context.cancellation())?;
        Ok(())
    }
}
