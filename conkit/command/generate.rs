//! Contract-generation command orchestration.

use anyhow::Context;

use super::AppCommand;
use crate::args::{GenerateArgs, GenerateCommand, GenerateSubject};
use crate::catalog::{
    ContractsStore, ExistingOutputPolicy, GeneratedContracts, GenerationReceipt, PathRole,
    ResolvedPath, SourceTree,
};
use crate::context::CommandContext;
use crate::contracts::{
    ContractLayout, ContractTarget, SketchGenerateRequest, SketchGenerateResponse,
};
use crate::output::GenerateSummary;

/// CLI-owned state for one complete generation workflow.
struct ContractGeneration<'context> {
    context: &'context CommandContext,
    target: ContractTarget,
    source: SourceTree,
    contracts: ContractsStore,
    policy: ExistingOutputPolicy,
}

impl AppCommand for GenerateCommand {
    async fn execute(&self, context: &CommandContext) -> anyhow::Result<()> {
        match &self.subject {
            GenerateSubject::All(args) => {
                ContractGeneration::prepare(args, ContractTarget::All, context)?
                    .run()
                    .await
            }
            GenerateSubject::Signatures(args) => {
                ContractGeneration::prepare(args, ContractTarget::Signatures, context)?
                    .run()
                    .await
            }
            GenerateSubject::Sketches(args) => {
                ContractGeneration::prepare(args, ContractTarget::Sketches, context)?
                    .run()
                    .await
            }
        }
    }
}

impl<'context> ContractGeneration<'context> {
    /// Validates roots and selects the generated-output collision policy.
    fn prepare(
        args: &GenerateArgs,
        target: ContractTarget,
        context: &'context CommandContext,
    ) -> anyhow::Result<Self> {
        ResolvedPath::ensure_disjoint(&[
            ResolvedPath::new(PathRole::Source, args.source.clone())?,
            ResolvedPath::new(PathRole::Contracts, args.contracts.clone())?,
        ])?;
        let source = SourceTree::open(args.source.clone())?;
        let contracts = ContractsStore::new(args.contracts.clone());

        Ok(Self {
            context,
            target,
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
        self.contracts.recover_interrupted_generation()?;

        let baseline = self.contracts.read_optional()?;
        let layout = ContractLayout::load(&self.contracts, &self.source, &baseline)?;
        let (documents, summary) = match self.target {
            ContractTarget::Signatures => self.generate_signatures(layout).await?,
            ContractTarget::Sketches => self.generate_sketches(layout).await?,
            ContractTarget::All => self.generate_all(layout).await?,
        };

        let receipt: GenerationReceipt = self
            .contracts
            .write_generated(GeneratedContracts::new(baseline, documents), self.policy)?;

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
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        let (source_files, target) =
            layout.into_signature_generation(&self.contracts, &self.source)?;
        let conkit_signature::GenerateResponse {
            contract_files,
            signature_count,
            sketch_count: _,
        } = self
            .context
            .signature()
            .generate(conkit_signature::GenerateRequest {
                source_files,
                target,
                scope: conkit_signature::ContractScope::Signatures,
            })
            .await
            .context("failed to generate signature contracts")?;

        Ok((
            contract_files,
            GenerateSummary::Signatures {
                count: signature_count,
            },
        ))
    }

    async fn generate_sketches(
        &self,
        layout: ContractLayout,
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        let (source_files, contract_files) =
            layout.into_sketch_generation(&self.contracts, &self.source)?;
        let conkit_signature::ResolveSketchesResponse { seeds } = self
            .context
            .signature()
            .resolve_sketches(conkit_signature::ResolveSketchesRequest {
                source_files,
                contract_files: contract_files.clone(),
            })
            .await
            .context("failed to resolve linked sketch source")?;
        let response: SketchGenerateResponse = self
            .context
            .sketch()
            .generate(SketchGenerateRequest::new(contract_files, seeds))
            .await?;
        let (contract_files, sketch_count) = response.into_parts();

        Ok((
            contract_files,
            GenerateSummary::Sketches {
                count: sketch_count,
            },
        ))
    }

    async fn generate_all(
        &self,
        layout: ContractLayout,
    ) -> anyhow::Result<(conkit_signature::FileCatalog, GenerateSummary)> {
        let (source_files, target) =
            layout.into_signature_generation(&self.contracts, &self.source)?;
        let conkit_signature::GenerateResponse {
            contract_files,
            signature_count,
            sketch_count: _,
        } = self
            .context
            .signature()
            .generate(conkit_signature::GenerateRequest {
                source_files: source_files.clone(),
                target,
                scope: conkit_signature::ContractScope::All,
            })
            .await
            .context("failed to generate signature contracts")?;
        let conkit_signature::ResolveSketchesResponse { seeds } = self
            .context
            .signature()
            .resolve_sketches(conkit_signature::ResolveSketchesRequest {
                source_files,
                contract_files: contract_files.clone(),
            })
            .await
            .context("failed to resolve linked sketch source")?;
        let response: SketchGenerateResponse = self
            .context
            .sketch()
            .generate(SketchGenerateRequest::new(contract_files, seeds))
            .await?;
        let (contract_files, sketch_count) = response.into_parts();

        Ok((
            contract_files,
            GenerateSummary::All {
                signature_count,
                sketch_count,
            },
        ))
    }
}
