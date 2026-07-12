//! Aggregate binding between combined documents and source-tree paths.

use std::collections::BTreeMap;
use std::path::Component;

use conkit_signature::{CatalogPath, FileCatalog};

use super::document::{ContractDocument, ContractDocumentPath};
use crate::catalog::{ContractsStore, PathRole, PortableCatalogPathKey, ResolvedPath, SourceTree};
use crate::error::CliError;
use crate::platform::PortablePathRules;

/// Validated root-level combined documents and their source allowlist.
#[derive(Debug)]
pub(crate) struct ContractLayout {
    documents: FileCatalog,
    source_paths: Vec<CatalogPath>,
}

impl ContractLayout {
    /// Parses all direct root-level YAML documents and binds them to `source`.
    ///
    /// # Errors
    ///
    /// Returns an error if the source root cannot be resolved, a combined
    /// document is malformed or targets another source root, or source claims
    /// overlap under portable case comparison.
    pub(crate) fn load(
        contracts: &ContractsStore,
        source: &SourceTree,
        catalog: &FileCatalog,
    ) -> Result<Self, CliError> {
        let canonical_source = fs_err::canonicalize(source.path()).map_err(|source_error| {
            CliError::ContractLayout {
                path: source.path().to_path_buf(),
                message: format!("failed to canonicalize selected source root: {source_error}"),
            }
        })?;
        if !canonical_source.is_dir() {
            return Err(CliError::ContractLayout {
                path: source.path().to_path_buf(),
                message: "selected source root is not a directory".to_owned(),
            });
        }

        let mut documents = FileCatalog::new();
        let mut claimed =
            BTreeMap::<PortableCatalogPathKey, (ContractDocumentPath, CatalogPath)>::new();

        for (catalog_path, bytes) in catalog.iter() {
            let Ok(document_path) = ContractDocumentPath::try_from(catalog_path.clone()) else {
                continue;
            };
            let document = ContractDocument::parse(
                document_path,
                bytes.to_vec(),
                contracts.path(),
                source.path(),
                &canonical_source,
            )?;
            let (document_path, bytes, source_paths) = document.into_parts();

            for logical in source_paths {
                let portable = PortableCatalogPathKey::new(&logical);
                if let Some((previous_document, previous_path)) =
                    claimed.insert(portable, (document_path.clone(), logical.clone()))
                {
                    return Err(CliError::ContractLayout {
                        path: contracts
                            .path()
                            .join(document_path.as_catalog_path().as_str()),
                        message: format!(
                            "listed source path {} overlaps {} claimed by {}",
                            logical.as_str(),
                            previous_path.as_str(),
                            previous_document.as_catalog_path().as_str()
                        ),
                    });
                }
            }

            documents.insert(document_path.into_catalog_path(), bytes)?;
        }

        Ok(Self {
            documents,
            source_paths: claimed
                .into_values()
                .map(|(_, source_path)| source_path)
                .collect(),
        })
    }

    fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Consumes the layout into its participating combined documents.
    pub(crate) fn into_documents(self) -> FileCatalog {
        self.documents
    }

    /// Reads exactly the source files claimed by the combined documents.
    ///
    /// # Errors
    ///
    /// Returns an error when an allowlisted file cannot be securely opened,
    /// validated, or represented in the source catalog.
    pub(crate) fn read_sources(&self, source: &SourceTree) -> Result<FileCatalog, CliError> {
        source.read_selected(&self.source_paths)
    }

    /// Requires at least one root-level combined contract document.
    ///
    /// # Errors
    ///
    /// Returns an error when the layout contains no combined documents.
    pub(crate) fn require_documents(&self, contracts: &ContractsStore) -> Result<(), CliError> {
        if self.is_empty() {
            Err(CliError::ContractLayout {
                path: contracts.path().to_path_buf(),
                message: "no root-level .yml or .yaml contract documents were found".to_owned(),
            })
        } else {
            Ok(())
        }
    }

    /// Builds the source catalog and signature generation target.
    ///
    /// # Errors
    ///
    /// Returns an error when source inputs cannot be securely read or a fresh
    /// document root cannot be represented as a portable relative path.
    pub(crate) fn into_signature_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
    ) -> Result<(FileCatalog, conkit_signature::GenerateTarget), CliError> {
        if !self.is_empty() {
            let source_files = self.read_sources(source)?;
            return Ok((
                source_files,
                conkit_signature::GenerateTarget::Existing(self.documents),
            ));
        }

        self.new_signature_generation(contracts, source)
    }

    /// Builds the exact source and document catalogs for sketch generation.
    ///
    /// # Errors
    ///
    /// Returns an error when no combined document exists or an allowlisted
    /// source file cannot be securely read.
    pub(crate) fn into_sketch_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
    ) -> Result<(FileCatalog, FileCatalog), CliError> {
        self.require_documents(contracts)?;
        let source_files = self.read_sources(source)?;
        Ok((source_files, self.documents))
    }

    fn new_signature_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
    ) -> Result<(FileCatalog, conkit_signature::GenerateTarget), CliError> {
        let source_files = source.read_rust_sources()?;
        let contract_root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())?;
        let source_root = ResolvedPath::new(PathRole::Source, source.path().to_path_buf())?;
        let relative = contract_root.relative_path_to(&source_root)?;
        let mut root_parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::ParentDir => root_parts.push("..".to_owned()),
                Component::Normal(value) => {
                    PortablePathRules::validate_component(value)?;
                    root_parts.push(
                        value
                            .to_str()
                            .ok_or(CliError::NonUtf8PathComponent)?
                            .to_owned(),
                    );
                }
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(CliError::ContractLayout {
                        path: contracts.path().to_path_buf(),
                        message: "cannot represent the selected source as a relative contract root"
                            .to_owned(),
                    });
                }
            }
        }

        let document = conkit_signature::GenerateDocument {
            contract_file: CatalogPath::new("main.yml")?,
            root: root_parts.join("/"),
            files: source_files.iter().map(|(path, _)| path.clone()).collect(),
        };

        Ok((
            source_files,
            conkit_signature::GenerateTarget::New(document),
        ))
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;
    use conkit_signature::{CatalogPath, FileCatalog};

    use super::ContractLayout;
    use crate::catalog::{ContractsStore, SourceTree};

    #[test]
    fn loads_root_documents_and_ignores_non_documents() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source
            .child("lib.rs")
            .write_str("pub fn answer() -> u8 { 42 }\n")
            .expect("source file");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                CatalogPath::new("main.YmL").expect("document path"),
                b"root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n".to_vec(),
            )
            .expect("root document");
        catalog
            .insert(
                CatalogPath::new("nested/ignored.yaml").expect("nested path"),
                b"not: a contract\n".to_vec(),
            )
            .expect("nested document");

        let layout = ContractLayout::load(&contracts, &source, &catalog).expect("valid layout");
        let (sources, target) = layout
            .into_signature_generation(&contracts, &source)
            .expect("existing generation input");

        assert_eq!(sources.len(), 1);
        let conkit_signature::GenerateTarget::Existing(documents) = target else {
            panic!("expected existing-document target");
        };
        assert_eq!(documents.len(), 1);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn aggregates_multiple_documents_and_their_exact_sources() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("first.rs").touch().expect("first source");
        source.child("second.rs").touch().expect("second source");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        for (document, source_file) in [("first.yml", "first.rs"), ("second.yaml", "second.rs")] {
            catalog
                .insert(
                    CatalogPath::new(document).expect("document path"),
                    format!("root: ../src\nfiles: [{source_file}]\nsignatures: []\nsketches: []\n")
                        .into_bytes(),
                )
                .expect("document");
        }

        let layout = ContractLayout::load(&contracts, &source, &catalog).expect("valid layout");
        let sources = layout.read_sources(&source).expect("selected sources");

        assert_eq!(sources.len(), 2);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn rejects_portably_overlapping_file_allowlists() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("lib.rs").touch().expect("source file");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        for (document, source_file) in [("first.yml", "lib.rs"), ("second.yaml", "LIB.rs")] {
            catalog
                .insert(
                    CatalogPath::new(document).expect("document path"),
                    format!("root: ../src\nfiles: [{source_file}]\nsignatures: []\nsketches: []\n")
                        .into_bytes(),
                )
                .expect("document");
        }

        let error =
            ContractLayout::load(&contracts, &source, &catalog).expect_err("overlap must fail");

        assert!(error.to_string().contains("overlaps"));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn requires_at_least_one_combined_document() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let layout =
            ContractLayout::load(&contracts, &source, &FileCatalog::new()).expect("empty layout");

        let error = layout
            .require_documents(&contracts)
            .expect_err("documents are required");

        assert!(error.to_string().contains("no root-level"));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn fresh_generation_targets_main_with_a_relative_root_and_rust_files() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("lib.rs").touch().expect("Rust source");
        source.child("notes.txt").touch().expect("ignored source");
        let contracts = temp.child("contracts");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let layout = ContractLayout::load(&contracts, &source, &FileCatalog::new())
            .expect("empty generation layout");

        let (sources, target) = layout
            .into_signature_generation(&contracts, &source)
            .expect("generation input");

        assert_eq!(sources.len(), 1);
        let conkit_signature::GenerateTarget::New(document) = target else {
            panic!("expected new-document target");
        };
        assert_eq!(document.contract_file.as_str(), "main.yml");
        assert_eq!(document.root, "../src");
        assert_eq!(document.files.len(), 1);
        assert_eq!(document.files[0].as_str(), "lib.rs");
        temp.close().expect("close temporary root");
    }
}
