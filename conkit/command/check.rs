//! Contract-check command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::args::{CheckArgs, CheckCommand, CheckSubject, SignatureCheckArgs};
use crate::catalog::{ContractsStore, PathRole, ResolvedPath, SourceTree};
use crate::context::CommandContext;
use crate::contracts::{
    ContractCheckMode, ContractLayout, ContractTarget, ExtractionUse, RequestedExtraction,
    SignatureExtractionCoordinator, SketchCheckRequest,
};
use crate::error::CliError;
use crate::report::ReportDestination;

/// Fully prepared inputs for one concrete contract-check workflow.
struct ContractCheck<'context> {
    context: &'context CommandContext,
    target: ContractTarget,
    mode: ContractCheckMode,
    report: ReportDestination,
    source_files: conkit_signature::FileCatalog,
    contract_files: conkit_signature::FileCatalog,
    extraction: conkit_signature::RustExtractionInput,
}

impl AppCommand for CheckCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        match &self.subject {
            CheckSubject::All(args) => {
                ContractCheck::prepare_signatures(args, ContractTarget::All, context)?
                    .run()
                    .await
            }
            CheckSubject::Signatures(args) => {
                ContractCheck::prepare_signatures(args, ContractTarget::Signatures, context)?
                    .run()
                    .await
            }
            CheckSubject::Sketches(args) => {
                ContractCheck::prepare(args, ContractTarget::Sketches, None, context)?
                    .run()
                    .await
            }
        }
    }
}

impl<'context> ContractCheck<'context> {
    fn prepare_signatures(
        args: &SignatureCheckArgs,
        target: ContractTarget,
        context: &'context CommandContext,
    ) -> anyhow::Result<Self> {
        let extraction = args.signature.requested_extraction()?;
        Self::prepare(&args.common, target, Some(extraction), context)
    }

    /// Validates filesystem inputs and reads the exact catalogs required by a check.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid report or overlapping path, an insecure
    /// or over-budget catalog read, invalid mandatory-v2 layout or extraction
    /// reconciliation, compiler acquisition failure, or cancellation.
    fn prepare(
        args: &CheckArgs,
        target: ContractTarget,
        requested: Option<RequestedExtraction<'_>>,
        context: &'context CommandContext,
    ) -> anyhow::Result<Self> {
        let report = ReportDestination::new(
            args.output.clone(),
            context.catalog_read_limits().per_file_bytes(),
            context.cancellation(),
        )?;
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Source, args.source.clone())?,
            ResolvedPath::new(PathRole::Contracts, args.contracts.clone())?,
            ResolvedPath::new(PathRole::Report, args.output.clone())?,
        ])?;
        let source =
            SourceTree::open(args.source.clone())?.with_limits(context.catalog_read_limits());
        let contracts =
            ContractsStore::new(args.contracts.clone()).with_limits(context.catalog_read_limits());
        let mut catalog_reads = context.catalog_read_limits().begin(context.cancellation());
        let catalog = contracts.read_with_budget(&mut catalog_reads)?;
        let layout = ContractLayout::load(&contracts, &source, &catalog, context.cancellation())?;
        layout.require_documents(&contracts)?;
        let persisted = if requested.is_some() {
            layout.extraction(context.cancellation())?
        } else {
            None
        };
        let source_files = layout.read_sources(&source, &mut catalog_reads)?;
        let extraction = match requested {
            Some(requested) => {
                let coordinator = SignatureExtractionCoordinator::new(requested, &contracts);
                let decision = coordinator.reconcile(ExtractionUse::Check { persisted })?;
                let (extraction, generation_crates) = decision.acquire(
                    context,
                    &source,
                    &source_files,
                    &contracts,
                    &mut catalog_reads,
                )?;
                debug_assert!(generation_crates.is_empty());
                extraction
            }
            None => conkit_signature::RustExtractionInput::Syntax,
        };
        let contract_files = layout.into_documents();

        Ok(Self {
            context,
            target,
            mode: Self::select_mode(args),
            report,
            source_files,
            contract_files,
            extraction,
        })
    }

    fn select_mode(args: &CheckArgs) -> ContractCheckMode {
        if args.strict {
            ContractCheckMode::Strict
        } else if args.warning {
            ContractCheckMode::Warning
        } else {
            let _ = args.default_mode;
            ContractCheckMode::Default
        }
    }

    async fn run(self) -> anyhow::Result<()> {
        match self.target {
            ContractTarget::Signatures => self.check_signatures().await,
            ContractTarget::Sketches => self.check_sketches().await,
            ContractTarget::All => self.check_all().await,
        }
    }

    async fn check_signatures(self) -> anyhow::Result<()> {
        let Self {
            context,
            mode,
            report,
            source_files,
            contract_files,
            extraction,
            target: _,
        } = self;
        let response = context
            .signature()
            .check(conkit_signature::CheckRequest {
                source_files,
                contract_files,
                report: report.to_signature_request()?,
                mode: mode.signature(),
                extraction,
            })
            .await
            .context("failed to check signature contracts")?;

        context.cancellation().checkpoint()?;
        report.write_signature_report(&response.report_files)?;
        if !response.passed {
            return Err(CliError::CheckFailed {
                target: ContractTarget::Signatures,
            }
            .into());
        }

        context.output().print_check_summary(&response)?;
        Ok(())
    }

    async fn check_sketches(self) -> anyhow::Result<()> {
        let Self {
            context,
            mode,
            report,
            source_files,
            contract_files,
            extraction: _,
            target: _,
        } = self;
        let response = context
            .sketch()
            .check(SketchCheckRequest::new(
                source_files,
                contract_files,
                report.to_sketch_request()?,
                mode.sketch(),
            ))
            .await?;

        context.cancellation().checkpoint()?;
        report.write_sketch_report(&response.report_files)?;
        if !response.passed {
            return Err(CliError::CheckFailed {
                target: ContractTarget::Sketches,
            }
            .into());
        }

        context.output().print_sketch_check_summary(&response)?;
        Ok(())
    }

    async fn check_all(self) -> anyhow::Result<()> {
        let Self {
            context,
            mode,
            report,
            source_files,
            contract_files,
            extraction,
            target: _,
        } = self;
        let signature_response = context
            .signature()
            .check(conkit_signature::CheckRequest {
                source_files: source_files.clone(),
                contract_files: contract_files.clone(),
                report: conkit_signature::ReportRequest::None,
                mode: mode.signature(),
                extraction,
            })
            .await
            .context("failed to check signature contracts")?;
        let sketch_response = context
            .sketch()
            .check(SketchCheckRequest::new(
                source_files,
                contract_files,
                conkit_sketch::ReportRequest::None,
                mode.sketch(),
            ))
            .await?;

        context.cancellation().checkpoint()?;
        report.write_all_check_report(&signature_response, &sketch_response)?;
        if !signature_response.passed || !sketch_response.passed {
            return Err(CliError::CheckFailed {
                target: ContractTarget::All,
            }
            .into());
        }

        context
            .output()
            .print_all_check_summary(&signature_response, &sketch_response)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::args::CheckArgs;
    use crate::contracts::ContractCheckMode;

    use super::ContractCheck;

    #[test]
    fn check_args_select_omitted_default_strict_and_warning_modes() {
        let cases = [
            (false, false, false, ContractCheckMode::Default),
            (true, false, false, ContractCheckMode::Default),
            (false, true, false, ContractCheckMode::Strict),
            (false, false, true, ContractCheckMode::Warning),
        ];

        for (default_mode, strict, warning, expected) in cases {
            let args = CheckArgs {
                source: "source".into(),
                contracts: "contracts".into(),
                output: "report.yml".into(),
                default_mode,
                strict,
                warning,
            };

            assert_eq!(ContractCheck::select_mode(&args), expected);
        }
    }
}
