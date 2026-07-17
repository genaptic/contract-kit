use super::{RustExtractionBackend, RustGenerationContext};
use crate::api::{GenerateResponse, ResolveSketchesResponse};
use crate::error::SignatureContractKitError;
use crate::files::FileCatalog;
use crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics;
use crate::languages::rust::parser::yaml::{
    RustContractDocuments, RustSketchDocumentPlan, RustSketchSeeds, RustYamlRenderer,
};
use crate::languages::rust::parser::{ParsedSignatureCheck, RustParsedFiles, SignatureParser};
use crate::limits::RustExtractionUsage;
use crate::work::CancellationProbe;
use std::collections::BTreeSet;

pub(in crate::languages::rust::parser) struct SyntaxBackend;

impl RustExtractionBackend for SyntaxBackend {
    fn check<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        contracts: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError> {
        contracts.validate_syntax_mode(cancellation)?;
        let allowlist = contracts.source_allowlist(cancellation)?;
        let parsed = RustParsedFiles::parse_allowlist(
            &allowlist,
            source_files,
            &parser.limits.rust,
            cancellation,
        )?;
        ParsedSignatureCheck::from_documents(
            contracts,
            &parsed,
            usage,
            RustCapabilityDiagnostics::new(&parser.limits.diagnostics),
            &parser.limits.diagnostics,
            cancellation,
        )
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
        plan.validate_syntax_mode(cancellation)?;
        let parsed = RustParsedFiles::parse_allowlist(
            &allowlist,
            source_files,
            &parser.limits.rust,
            cancellation,
        )?;
        RustYamlRenderer::render_syntax(
            parsed,
            plan,
            scope,
            usage,
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
        documents.validate_syntax_mode(cancellation)?;
        let mut plans = Vec::new();
        for document in documents.into_documents() {
            cancellation.checkpoint()?;
            if let Some(plan) = RustSketchDocumentPlan::new_syntax(document, cancellation)? {
                plans.push(plan);
            }
        }

        let mut linked_allowlist = BTreeSet::new();
        for plan in &plans {
            cancellation.checkpoint()?;
            for file in plan.extraction().files() {
                cancellation.checkpoint()?;
                linked_allowlist.insert(file.clone());
            }
        }
        let mut parsed = RustParsedFiles::deferred(&linked_allowlist, source_files, cancellation)?;
        let mut diagnostics = RustCapabilityDiagnostics::new(&parser.limits.diagnostics);
        let mut seeds = RustSketchSeeds::new();
        let mut output = parser.limits.output.meter(cancellation);

        for plan in plans {
            cancellation.checkpoint()?;
            let (contract, extraction, required_modules) = plan.into_parts();
            let projection = parsed.project_for_required_modules(
                &extraction,
                required_modules,
                &parser.limits,
                usage,
                &mut diagnostics,
                cancellation,
            )?;
            contract.append_sketch_seeds(
                parsed.projected_source(&projection),
                &mut seeds,
                &mut output,
                cancellation,
            )?;
        }

        let mut response = seeds.into_response(cancellation)?;
        response.capability_warnings = diagnostics.into_warning_messages(cancellation)?;
        Ok(response)
    }
}
