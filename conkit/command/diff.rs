//! Contract-diff command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::archive::ArchiveSource;
use crate::args::DiffCommand;
use crate::catalog::{ContractsStore, PathRole, ResolvedPath};
use crate::context::CommandContext;

impl AppCommand for DiffCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Contracts, self.contracts.clone())?,
            ResolvedPath::new(PathRole::ArchiveFile, self.archive.clone())?,
        ])?;

        let current = ContractsStore::new(self.contracts.clone()).read()?;
        let previous = ArchiveSource::new(self.archive.clone())
            .decode_contracts()
            .context("failed to decode contract archive")?;
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

        context.output().print_diff(&signatures, &sketches)?;
        Ok(())
    }
}
