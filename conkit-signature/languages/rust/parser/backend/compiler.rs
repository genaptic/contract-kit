use super::{RustExtractionBackend, RustGenerationContext};
use crate::api::{GenerateResponse, ResolveSketchesResponse};
use crate::error::SignatureContractKitError;
use crate::files::FileCatalog;
use crate::languages::rust::parser::yaml::{
    RustContractDocuments, RustSketchSeeds, RustYamlRenderer,
};
use crate::languages::rust::parser::{ParsedSignatureCheck, SignatureParser};
use crate::languages::rust::rustdoc::RustCompilerArtifact;
use crate::limits::RustExtractionUsage;
use crate::work::CancellationProbe;

pub(in crate::languages::rust::parser) struct CompilerBackend {
    artifact: RustCompilerArtifact,
}

impl CompilerBackend {
    pub(in crate::languages::rust::parser) fn new(artifact: RustCompilerArtifact) -> Self {
        Self { artifact }
    }
}

impl RustExtractionBackend for CompilerBackend {
    fn check<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        contracts: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError> {
        let allowlist = contracts
            .compiler_document(cancellation)?
            .files()
            .iter()
            .cloned()
            .collect();
        let extracted = self.artifact.extract(
            source_files,
            &allowlist,
            &parser.limits.rust,
            usage,
            cancellation,
        )?;
        ParsedSignatureCheck::from_compiler_documents(contracts, extracted, cancellation)
    }

    fn generate(
        self,
        context: RustGenerationContext<'_, '_, '_>,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let RustGenerationContext {
            parser,
            source_files,
            allowlist,
            plan,
            scope,
            usage,
            yaml_usage,
            cancellation,
        } = context;
        plan.validate_compiler_mode(cancellation)?;
        let extracted = self.artifact.extract(
            source_files,
            &allowlist,
            &parser.limits.rust,
            usage,
            cancellation,
        )?;
        RustYamlRenderer::render_compiler(
            extracted,
            plan,
            scope,
            yaml_usage,
            &parser.limits,
            cancellation,
        )
    }

    fn resolve_sketches<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        documents: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        let allowlist = documents
            .compiler_document(cancellation)?
            .files()
            .iter()
            .cloned()
            .collect();
        let extracted = self.artifact.extract(
            source_files,
            &allowlist,
            &parser.limits.rust,
            usage,
            cancellation,
        )?;
        let mut seeds = RustSketchSeeds::new();
        let mut output = parser.limits.output.meter(cancellation);
        for contract in documents.into_documents() {
            cancellation.checkpoint()?;
            contract.append_compiler_sketch_seeds(
                &extracted,
                &mut seeds,
                &mut output,
                cancellation,
            )?;
        }
        seeds.into_response(cancellation)
    }
}
