//! Contract-generation command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::args::{GenerateArgs, GenerateCommand, GenerateSubject};
use crate::catalog::{
    CatalogReadBudget, ContractsStore, ExistingOutputPolicy, GeneratedContracts, GenerationReceipt,
    PathRole, ResolvedPath, SourceTree,
};
use crate::context::CommandContext;
use crate::contracts::{
    ContractLayout, ExtractionUse, RequestedExtraction, SignatureExtractionCoordinator,
    SketchGenerateRequest, SketchGenerateResponse,
};
use crate::error::CliError;
use crate::output::GenerateSummary;

/// CLI-owned state for one complete generation workflow.
struct ContractGeneration<'args, 'context> {
    context: &'context CommandContext,
    selection: GenerationSelection<'args>,
    source: SourceTree,
    contracts: ContractsStore,
    policy: ExistingOutputPolicy,
}

enum GenerationSelection<'args> {
    All {
        extraction: RequestedExtraction<'args>,
        crate_roots: Vec<conkit_signature::RustCrateRoot>,
    },
    Signatures {
        extraction: RequestedExtraction<'args>,
        crate_roots: Vec<conkit_signature::RustCrateRoot>,
    },
    Sketches,
}

impl AppCommand for GenerateCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        match &self.subject {
            GenerateSubject::All(args) => {
                let extraction = args.signature.requested_extraction()?;
                let crate_roots = args
                    .crate_roots
                    .iter()
                    .map(crate::args::CrateRootArg::to_domain)
                    .collect::<Result<Vec<_>, _>>()?;
                ContractGeneration::prepare(
                    &args.common,
                    GenerationSelection::All {
                        extraction,
                        crate_roots,
                    },
                    context,
                )?
                .run()
                .await
            }
            GenerateSubject::Signatures(args) => {
                let extraction = args.signature.requested_extraction()?;
                let crate_roots = args
                    .crate_roots
                    .iter()
                    .map(crate::args::CrateRootArg::to_domain)
                    .collect::<Result<Vec<_>, _>>()?;
                ContractGeneration::prepare(
                    &args.common,
                    GenerationSelection::Signatures {
                        extraction,
                        crate_roots,
                    },
                    context,
                )?
                .run()
                .await
            }
            GenerateSubject::Sketches(args) => {
                ContractGeneration::prepare(args, GenerationSelection::Sketches, context)?
                    .run()
                    .await
            }
        }
    }
}

impl<'args, 'context> ContractGeneration<'args, 'context> {
    /// Validates roots and selects the generated-output collision policy.
    fn prepare(
        args: &GenerateArgs,
        selection: GenerationSelection<'args>,
        context: &'context CommandContext,
    ) -> anyhow::Result<Self> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Source, args.source.clone())?,
            ResolvedPath::new(PathRole::Contracts, args.contracts.clone())?,
        ])?;
        let source =
            SourceTree::open(args.source.clone())?.with_limits(context.catalog_read_limits());
        let contracts =
            ContractsStore::new(args.contracts.clone()).with_limits(context.catalog_read_limits());

        Ok(Self {
            context,
            selection,
            source,
            contracts,
            policy: if args.adopt_existing {
                ExistingOutputPolicy::AdoptMatching
            } else {
                ExistingOutputPolicy::Reject
            },
        })
    }

    async fn run(self) -> anyhow::Result<()> {
        let mut catalog_reads = self
            .context
            .catalog_read_limits()
            .begin(self.context.cancellation());
        self.contracts
            .recover_interrupted_generation_with_budget(&mut catalog_reads)?;
        let baseline = self
            .contracts
            .read_optional_with_budget(&mut catalog_reads)?;
        let layout = ContractLayout::load(
            &self.contracts,
            &self.source,
            &baseline,
            self.context.cancellation(),
        )?;
        let (documents, summary) = match &self.selection {
            GenerationSelection::Signatures {
                extraction,
                crate_roots,
            } => {
                self.generate_signatures(layout, *extraction, crate_roots, &mut catalog_reads)
                    .await?
            }
            GenerationSelection::Sketches => {
                self.generate_sketches(layout, &mut catalog_reads).await?
            }
            GenerationSelection::All {
                extraction,
                crate_roots,
            } => {
                self.generate_all(layout, *extraction, crate_roots, &mut catalog_reads)
                    .await?
            }
        };

        self.context.cancellation().checkpoint()?;
        let receipt: GenerationReceipt = self.contracts.write_generated_with_budget(
            GeneratedContracts::new(baseline, documents),
            self.policy,
            catalog_reads,
        )?;

        self.context.output().print_generate_summary(summary)?;
        if matches!(self.policy, ExistingOutputPolicy::AdoptMatching) {
            self.context
                .output()
                .print_adoption_summary(receipt.adopted_count())?;
        }

        Ok(())
    }

    async fn generate_signatures(
        &self,
        layout: ContractLayout,
        requested: RequestedExtraction<'args>,
        crate_roots: &[conkit_signature::RustCrateRoot],
        catalog_reads: &mut CatalogReadBudget,
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        let (source_files, target, extraction) =
            self.signature_generation(layout, requested, crate_roots, catalog_reads)?;
        let conkit_signature::GenerateResponse {
            contract_files,
            counts,
            resolved_sketch_seeds: _,
            capability_warnings,
        } = self
            .context
            .signature()
            .generate(conkit_signature::GenerateRequest {
                source_files,
                target,
                scope: conkit_signature::ContractScope::Signatures,
                extraction,
            })
            .await
            .context("failed to generate signature contracts")?;
        self.context.output().print_signature_capability_warnings(
            &capability_warnings,
            self.context.cancellation(),
        )?;

        Ok((contract_files, GenerateSummary::Signatures { counts }))
    }

    fn signature_generation(
        &self,
        layout: ContractLayout,
        requested: RequestedExtraction<'args>,
        crate_roots: &[conkit_signature::RustCrateRoot],
        catalog_reads: &mut CatalogReadBudget,
    ) -> anyhow::Result<(
        conkit_signature::FileCatalog,
        conkit_signature::GenerateTarget,
        conkit_signature::RustExtractionInput,
    )> {
        let fresh = layout.is_empty();
        let persisted = layout.extraction(self.context.cancellation())?;
        let coordinator = SignatureExtractionCoordinator::new(requested, &self.contracts);
        coordinator.validate_generation_roots(fresh, crate_roots)?;
        let source_files = layout.read_signature_sources(&self.source, catalog_reads)?;
        let decision = coordinator.reconcile(ExtractionUse::Generation {
            fresh,
            persisted,
            explicit_crates: crate_roots,
        })?;
        let (extraction, generation_crates) = decision.acquire(
            self.context,
            &self.source,
            &source_files,
            &self.contracts,
            catalog_reads,
        )?;
        let (source_files, target) = layout.into_signature_generation(
            &self.contracts,
            &self.source,
            source_files,
            generation_crates,
            self.context.cancellation(),
        )?;
        Ok((source_files, target, extraction))
    }

    async fn generate_sketches(
        &self,
        layout: ContractLayout,
        catalog_reads: &mut CatalogReadBudget,
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        if layout
            .extraction(self.context.cancellation())?
            .is_some_and(|value| value.is_compiler())
        {
            return Err(CliError::ContractLayout {
                path: self.contracts.path().to_path_buf(),
                message: "sketch-only generation cannot recreate a compiler artifact because compiler flags are intentionally signature-only; run `conkit generate all --signature-extractor compiler --manifest-path FILE ...`"
                    .to_owned(),
            }
            .into());
        }
        let (source_files, contract_files) =
            layout.into_sketch_generation(&self.contracts, &self.source, catalog_reads)?;
        let conkit_signature::ResolveSketchesResponse {
            seeds,
            capability_warnings,
        } = self
            .context
            .signature()
            .resolve_sketches(conkit_signature::ResolveSketchesRequest {
                source_files,
                contract_files: contract_files.clone(),
                extraction: conkit_signature::RustExtractionInput::Syntax,
            })
            .await
            .context("failed to resolve linked sketch source")?;
        self.context.output().print_signature_capability_warnings(
            &capability_warnings,
            self.context.cancellation(),
        )?;
        let response: SketchGenerateResponse = self
            .context
            .sketch()
            .generate(SketchGenerateRequest::new(contract_files, seeds))
            .await?;
        let (contract_files, counts) = response.into_parts();

        Ok((contract_files, GenerateSummary::Sketches { counts }))
    }

    async fn generate_all(
        &self,
        layout: ContractLayout,
        requested: RequestedExtraction<'args>,
        crate_roots: &[conkit_signature::RustCrateRoot],
        catalog_reads: &mut CatalogReadBudget,
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        let (source_files, target, extraction) =
            self.signature_generation(layout, requested, crate_roots, catalog_reads)?;
        let conkit_signature::GenerateResponse {
            contract_files,
            counts: signature_counts,
            resolved_sketch_seeds,
            capability_warnings,
        } = self
            .context
            .signature()
            .generate(conkit_signature::GenerateRequest {
                source_files,
                target,
                scope: conkit_signature::ContractScope::All,
                extraction,
            })
            .await
            .context("failed to generate signature contracts")?;
        self.context.output().print_signature_capability_warnings(
            &capability_warnings,
            self.context.cancellation(),
        )?;
        let response: SketchGenerateResponse = self
            .context
            .sketch()
            .generate(SketchGenerateRequest::new(
                contract_files,
                resolved_sketch_seeds,
            ))
            .await?;
        let (contract_files, sketch_counts) = response.into_parts();

        Ok((
            contract_files,
            GenerateSummary::All {
                signatures: signature_counts,
                sketches: sketch_counts,
            },
        ))
    }
}
