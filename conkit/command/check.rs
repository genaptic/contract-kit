//! Contract-check command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::args::{CheckArgs, CheckCommand, CheckSubject};
use crate::catalog::{ContractsStore, PathRole, ResolvedPath, SourceTree};
use crate::context::CommandContext;
use crate::contracts::{ContractCheckMode, ContractLayout, ContractTarget, SketchCheckRequest};
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
}

impl AppCommand for CheckCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        match &self.subject {
            CheckSubject::All(args) => {
                ContractCheck::prepare(args, ContractTarget::All, context)?
                    .run()
                    .await
            }
            CheckSubject::Signatures(args) => {
                ContractCheck::prepare(args, ContractTarget::Signatures, context)?
                    .run()
                    .await
            }
            CheckSubject::Sketches(args) => {
                ContractCheck::prepare(args, ContractTarget::Sketches, context)?
                    .run()
                    .await
            }
        }
    }
}

impl<'context> ContractCheck<'context> {
    /// Validates filesystem inputs and reads the exact catalogs required by a check.
    fn prepare(
        args: &CheckArgs,
        target: ContractTarget,
        context: &'context CommandContext,
    ) -> anyhow::Result<Self> {
        let report = ReportDestination::new(args.output.clone())?;
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Source, args.source.clone())?,
            ResolvedPath::new(PathRole::Contracts, args.contracts.clone())?,
            ResolvedPath::new(PathRole::Report, args.output.clone())?,
        ])?;
        let source = SourceTree::open(args.source.clone())?;
        let contracts = ContractsStore::new(args.contracts.clone());
        let catalog = contracts.read()?;
        let layout = ContractLayout::load(&contracts, &source, &catalog)?;
        layout.require_documents(&contracts)?;
        let source_files = layout.read_sources(&source)?;
        let contract_files = layout.into_documents();

        Ok(Self {
            context,
            target,
            mode: Self::select_mode(args),
            report,
            source_files,
            contract_files,
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
            target: _,
        } = self;
        let response = context
            .signature()
            .check(conkit_signature::CheckRequest {
                source_files,
                contract_files,
                report: report.to_signature_request()?,
                scope: conkit_signature::ContractScope::Signatures,
                mode: mode.signature(),
            })
            .await
            .context("failed to check signature contracts")?;

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
            target: _,
        } = self;
        let signature_response = context
            .signature()
            .check(conkit_signature::CheckRequest {
                source_files: source_files.clone(),
                contract_files: contract_files.clone(),
                report: conkit_signature::ReportRequest::None,
                scope: conkit_signature::ContractScope::Signatures,
                mode: mode.signature(),
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
