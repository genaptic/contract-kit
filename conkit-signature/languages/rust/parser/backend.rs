mod compiler;
mod syntax;

use self::compiler::CompilerBackend;
use self::syntax::SyntaxBackend;
use super::yaml::{RustContractDocuments, RustGenerationPlan};
use super::{ParsedSignatureCheck, SignatureParser};
use crate::api::{ContractScope, GenerateResponse, ResolveSketchesResponse, RustExtractionInput};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::files::FileCatalog;
use crate::limits::{RustExtractionUsage, YamlUsage};
use crate::work::CancellationProbe;
use std::collections::BTreeSet;

pub(super) struct RustGenerationContext<'operation, 'limits, 'yaml_limits> {
    pub(super) parser: &'limits SignatureParser,
    pub(super) source_files: FileCatalog,
    pub(super) allowlist: BTreeSet<CatalogPath>,
    pub(super) plan: RustGenerationPlan,
    pub(super) scope: ContractScope,
    pub(super) usage: &'operation mut RustExtractionUsage<'limits>,
    pub(super) yaml_usage: &'operation mut YamlUsage<'yaml_limits>,
    pub(super) cancellation: &'operation CancellationProbe,
}

pub(super) trait RustExtractionBackend {
    fn check<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        contracts: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError>;

    fn generate(
        self,
        context: RustGenerationContext<'_, '_, '_>,
    ) -> Result<GenerateResponse, SignatureContractKitError>;

    fn resolve_sketches<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        documents: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError>;
}

pub(super) enum RustBackend {
    Syntax(SyntaxBackend),
    Compiler(CompilerBackend),
}

impl RustBackend {
    pub(super) fn from_input(input: RustExtractionInput) -> Self {
        match input {
            RustExtractionInput::Syntax => Self::Syntax(SyntaxBackend),
            RustExtractionInput::Compiler(artifact) => {
                Self::Compiler(CompilerBackend::new(artifact))
            }
        }
    }
}

impl RustExtractionBackend for RustBackend {
    fn check<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        contracts: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError> {
        match self {
            Self::Syntax(backend) => {
                backend.check(parser, source_files, contracts, usage, cancellation)
            }
            Self::Compiler(backend) => {
                backend.check(parser, source_files, contracts, usage, cancellation)
            }
        }
    }

    fn generate(
        self,
        context: RustGenerationContext<'_, '_, '_>,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        match self {
            Self::Syntax(backend) => backend.generate(context),
            Self::Compiler(backend) => backend.generate(context),
        }
    }

    fn resolve_sketches<'limits>(
        self,
        parser: &'limits SignatureParser,
        source_files: FileCatalog,
        documents: RustContractDocuments,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        match self {
            Self::Syntax(backend) => {
                backend.resolve_sketches(parser, source_files, documents, usage, cancellation)
            }
            Self::Compiler(backend) => {
                backend.resolve_sketches(parser, source_files, documents, usage, cancellation)
            }
        }
    }
}
