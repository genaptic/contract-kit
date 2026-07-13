mod document;
mod generate;
mod sketch;

use super::{RustContractDocuments, RustYamlRenderer};
use crate::api::{ContractScope, GenerateDocument, GenerateResponse, GenerateTarget};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::SignatureInventory;
use crate::languages::rust::parser::RustSourceFiles;

struct RustYamlTestFixture {
    source_files: FileCatalog,
    source_inventory: SignatureInventory,
}

impl RustYamlTestFixture {
    fn new(source_files: FileCatalog) -> Self {
        let source_inventory = RustSourceFiles::from_catalog(source_files.clone())
            .parse_all()
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
        RustSourceFiles::from_catalog(self.source_files.clone())
            .parse_all_for_yaml()
            .expect("parsed source")
    }

    fn render_new(&self, contract_file: &str, files: &[&str]) -> GenerateResponse {
        self.render(GenerateTarget::New(GenerateDocument {
            contract_file: CatalogPath::new(contract_file).expect("contract path"),
            root: "../src".to_owned(),
            files: files
                .iter()
                .map(|file| CatalogPath::new(*file).expect("source path"))
                .collect(),
        }))
    }

    fn render_existing(&self, contract_files: FileCatalog) -> GenerateResponse {
        self.render(GenerateTarget::Existing(contract_files))
    }

    fn generated_inventory(&self, contract_files: FileCatalog) -> SignatureInventory {
        contract_inventory(contract_files).expect("generated contract inventory")
    }

    fn assert_generated_matches_source(&self, contract_files: FileCatalog) {
        let generated_inventory = self.generated_inventory(contract_files);
        let comparison = self
            .source_inventory
            .compare_against(&generated_inventory)
            .expect("comparison");
        assert!(
            comparison.diagnostics().is_empty(),
            "{:#?}",
            comparison.diagnostics()
        );
    }

    fn render(&self, target: GenerateTarget) -> GenerateResponse {
        RustYamlRenderer::new(self.parsed_for_yaml(), target, ContractScope::Signatures)
            .render()
            .expect("generation")
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
    RustContractDocuments::parse(catalog)?.into_inventory()
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
