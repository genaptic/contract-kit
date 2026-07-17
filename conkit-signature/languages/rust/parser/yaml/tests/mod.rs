mod document;
mod generate;
mod sketch;

use super::{
    RustContractDocuments as ParsedRustContractDocuments,
    RustGenerationPlan as ParsedRustGenerationPlan, RustYamlRenderer,
};
use crate::api::{
    ContractScope, GenerateDocument, GenerateResponse, GenerateTarget, RustCrateKind, RustCrateRoot,
};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{InventoryComparison, SignatureInventory};
use crate::languages::rust::parser::source_graph::{RustCapabilityDiagnostics, RustExtraction};
use crate::languages::rust::parser::{ParsedSignatureCheck, RustParsedFiles};
use std::collections::BTreeSet;

struct RustYamlTestFixture {
    source_files: FileCatalog,
    source_inventory: SignatureInventory,
}

struct RustContractDocuments;

impl RustContractDocuments {
    fn parse(
        catalog: FileCatalog,
        limits: &crate::limits::YamlLimits,
    ) -> Result<ParsedRustContractDocuments, SignatureContractKitError> {
        let rust_limits = crate::limits::RustExtractionLimits::default();
        let mut usage = rust_limits.usage();
        let mut yaml_usage = limits.usage();
        ParsedRustContractDocuments::parse(
            catalog,
            &mut yaml_usage,
            &mut usage,
            &crate::work::CancellationProbe::new(),
        )
    }
}

struct RustGenerationPlan;

impl RustGenerationPlan {
    fn parse(
        target: GenerateTarget,
        limits: &crate::limits::YamlLimits,
    ) -> Result<ParsedRustGenerationPlan, SignatureContractKitError> {
        let rust_limits = crate::limits::RustExtractionLimits::default();
        let mut usage = rust_limits.usage();
        let mut yaml_usage = limits.usage();
        ParsedRustGenerationPlan::parse(
            target,
            &mut yaml_usage,
            &mut usage,
            &crate::work::CancellationProbe::new(),
        )
    }
}

impl RustYamlTestFixture {
    fn new(source_files: FileCatalog) -> Self {
        let parsed = Self::parse_sources(&source_files);
        let limits = crate::limits::SignatureLimits::default();
        let mut usage = limits.rust.usage();
        let cancellation = crate::work::CancellationProbe::new();
        let mut diagnostics = RustCapabilityDiagnostics::new(&limits.diagnostics);
        let source_inventory = parsed
            .project_for_extraction(
                &Self::extraction(&source_files),
                &mut usage,
                &mut diagnostics,
                &limits.diagnostics,
                &cancellation,
            )
            .expect("source projection")
            .into_inventory(None, &cancellation)
            .expect("source inventory");
        Self {
            source_files,
            source_inventory,
        }
    }

    fn source_inventory(&self) -> &SignatureInventory {
        &self.source_inventory
    }

    fn parsed_for_yaml(&self) -> crate::languages::rust::parser::RustParsedFiles {
        Self::parse_sources(&self.source_files)
    }

    fn render_plan(
        &self,
        plan: ParsedRustGenerationPlan,
        scope: ContractScope,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        self.render_plan_with_limits(plan, scope, crate::limits::SignatureLimits::default())
    }

    fn render_plan_with_limits(
        &self,
        plan: ParsedRustGenerationPlan,
        scope: ContractScope,
        limits: crate::limits::SignatureLimits,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let mut usage = limits.rust.usage();
        let mut yaml_usage = limits.yaml.usage();
        let cancellation = crate::work::CancellationProbe::new();
        RustYamlRenderer::render_syntax(
            self.parsed_for_yaml(),
            plan,
            scope,
            &mut usage,
            &mut yaml_usage,
            &limits,
            &cancellation,
        )
    }

    fn render_new(&self, contract_file: &str, files: &[&str]) -> GenerateResponse {
        let files = files
            .iter()
            .map(|file| CatalogPath::new(*file).expect("source path"))
            .collect::<Vec<_>>();
        let root = files.first().cloned().expect("fixture crate root");
        self.render(GenerateTarget::New(GenerateDocument {
            contract_file: CatalogPath::new(contract_file).expect("contract path"),
            root: "../src".to_owned(),
            files,
            crates: vec![RustCrateRoot {
                id: "sample".to_owned(),
                root,
                kind: RustCrateKind::Library,
            }],
        }))
    }

    fn render_existing(&self, contract_files: FileCatalog) -> GenerateResponse {
        self.render(GenerateTarget::Existing(contract_files))
    }

    fn assert_generated_matches_source(&self, contract_files: FileCatalog) {
        let comparison = self.compare_contract_files(contract_files);
        assert!(
            comparison.diagnostics().is_empty(),
            "{:#?}",
            comparison.diagnostics()
        );
    }

    fn compare_contract_files(&self, contract_files: FileCatalog) -> InventoryComparison {
        let limits = crate::limits::SignatureLimits::default();
        let mut usage = limits.rust.usage();
        let cancellation = crate::work::CancellationProbe::new();
        let diagnostics = RustCapabilityDiagnostics::new(&limits.diagnostics);
        let documents = parse_contracts(contract_files).expect("generated contract documents");
        let parsed = self.parsed_for_yaml();
        ParsedSignatureCheck::from_documents(
            documents,
            &parsed,
            &mut usage,
            diagnostics,
            &limits.diagnostics,
            &cancellation,
        )
        .expect("warning-aware projected inventories")
        .compare(&limits.diagnostics, &cancellation)
        .expect("comparison")
    }

    fn render(&self, target: GenerateTarget) -> GenerateResponse {
        let limits = crate::limits::SignatureLimits::default();
        let mut usage = limits.rust.usage();
        let mut yaml_usage = limits.yaml.usage();
        let cancellation = crate::work::CancellationProbe::new();
        let plan =
            ParsedRustGenerationPlan::parse(target, &mut yaml_usage, &mut usage, &cancellation)
                .expect("generation plan");
        RustYamlRenderer::render_syntax(
            self.parsed_for_yaml(),
            plan,
            ContractScope::Signatures,
            &mut usage,
            &mut yaml_usage,
            &limits,
            &cancellation,
        )
        .expect("generation")
    }

    fn parse_sources(source_files: &FileCatalog) -> RustParsedFiles {
        let allowlist = source_files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<BTreeSet<_>>();
        RustParsedFiles::parse_allowlist(
            &allowlist,
            source_files.clone(),
            &crate::limits::RustExtractionLimits::default(),
            &crate::work::CancellationProbe::new(),
        )
        .expect("parsed source")
    }

    fn extraction(source_files: &FileCatalog) -> RustExtraction {
        let cancellation = crate::work::CancellationProbe::new();
        let files = source_files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<BTreeSet<_>>();
        let root = files
            .iter()
            .find(|path| path.as_str() == "lib.rs")
            .or_else(|| files.iter().next())
            .cloned()
            .expect("fixture source root");
        RustExtraction::from_roots(
            files,
            [RustCrateRoot {
                id: "sample".to_owned(),
                root,
                kind: RustCrateKind::Library,
            }],
            &cancellation,
        )
        .expect("fixture extraction")
    }
}

fn rendered(catalog: &FileCatalog, name: &str) -> String {
    String::from_utf8(
        catalog
            .get(&CatalogPath::new(name).expect("path"))
            .expect("generated file")
            .to_vec(),
    )
    .expect("utf8")
}

fn catalog_with(path: &str, bytes: &[u8]) -> FileCatalog {
    catalog([(path, bytes)])
}

fn contract_inventory(
    catalog: FileCatalog,
) -> Result<SignatureInventory, SignatureContractKitError> {
    let cancellation = crate::work::CancellationProbe::new();
    parse_contracts(catalog)?.into_inventory(&cancellation)
}

fn parse_contracts(
    catalog: FileCatalog,
) -> Result<ParsedRustContractDocuments, SignatureContractKitError> {
    let limits = crate::limits::SignatureLimits::default();
    let mut usage = limits.rust.usage();
    let mut yaml_usage = limits.yaml.usage();
    ParsedRustContractDocuments::parse(
        catalog,
        &mut yaml_usage,
        &mut usage,
        &crate::work::CancellationProbe::new(),
    )
}

fn catalog<const N: usize>(entries: [(&str, &[u8]); N]) -> FileCatalog {
    let mut catalog = FileCatalog::new();
    for (path, bytes) in entries {
        catalog
            .insert(
                CatalogPath::new(path).expect("catalog path"),
                bytes.to_vec(),
            )
            .expect("insert");
    }
    catalog
}
