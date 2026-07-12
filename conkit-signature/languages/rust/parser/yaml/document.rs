use super::input::RustYamlDocument;
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::SignatureInventory;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::languages::rust::parser) struct RustContractDocuments {
    pub(super) documents: Vec<RustContractDocument>,
}

impl RustContractDocuments {
    pub(in crate::languages::rust::parser) fn parse(
        catalog: FileCatalog,
    ) -> Result<Self, SignatureContractKitError> {
        let mut documents = catalog
            .into_entries()
            .filter(|(catalog_name, _)| RustContractCatalogName::new(catalog_name).is_root_yaml())
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(catalog_name, bytes)| RustContractDocument::parse(catalog_name, bytes))
            .collect::<Result<Vec<_>, _>>()?;

        documents.sort_by(|left, right| left.catalog_name.cmp(&right.catalog_name));

        let parsed = Self { documents };
        parsed.validate_global_keys()?;
        Ok(parsed)
    }

    pub(in crate::languages::rust::parser) fn source_allowlist(&self) -> BTreeSet<CatalogPath> {
        self.documents
            .iter()
            .flat_map(|document| document.document.files.iter().cloned())
            .collect()
    }

    pub(in crate::languages::rust::parser) fn into_inventory(
        self,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let inventories = self
            .documents
            .into_iter()
            .map(RustContractDocument::into_inventory)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SignatureInventory::merge_all(inventories)?)
    }

    fn validate_global_keys(&self) -> Result<(), SignatureContractKitError> {
        let mut files = BTreeMap::<&CatalogPath, &CatalogPath>::new();
        let mut labels = BTreeMap::<&str, &CatalogPath>::new();
        let mut sketches = BTreeMap::<&str, &CatalogPath>::new();

        for document in &self.documents {
            for file in &document.document.files {
                if let Some(previous) = files.insert(file, &document.catalog_name) {
                    return Err(SignatureContractKitError::parse_failed(
                        &document.catalog_name,
                        format!(
                            "source file {file} is listed by both {previous} and {}",
                            document.catalog_name
                        ),
                    ));
                }
            }
            for signature in &document.document.signatures {
                if let Some(previous) =
                    labels.insert(signature.label.as_str(), &document.catalog_name)
                {
                    return Err(SignatureContractKitError::parse_failed(
                        &document.catalog_name,
                        format!(
                            "signature label {} is defined by both {previous} and {}",
                            signature.label, document.catalog_name
                        ),
                    ));
                }
            }
            for sketch in &document.document.sketches {
                if let Some(previous) = sketches.insert(sketch.id.as_str(), &document.catalog_name)
                {
                    return Err(SignatureContractKitError::parse_failed(
                        &document.catalog_name,
                        format!(
                            "sketch id {} is defined by both {previous} and {}",
                            sketch.id, document.catalog_name
                        ),
                    ));
                }
            }
        }

        Ok(())
    }
}

pub(super) struct RustContractDocument {
    pub(super) catalog_name: CatalogPath,
    pub(super) document: RustYamlDocument,
}

impl RustContractDocument {
    fn parse(catalog_name: CatalogPath, bytes: Vec<u8>) -> Result<Self, SignatureContractKitError> {
        let document = RustYamlDocument::parse(&catalog_name, &bytes)?;
        Ok(Self {
            catalog_name,
            document,
        })
    }

    fn into_inventory(self) -> Result<SignatureInventory, SignatureContractKitError> {
        let Self {
            catalog_name,
            document,
        } = self;
        document.into_inventory(&catalog_name)
    }
}

pub(super) struct RustContractCatalogName<'a> {
    path: &'a CatalogPath,
}

impl<'a> RustContractCatalogName<'a> {
    pub(super) fn new(path: &'a CatalogPath) -> Self {
        Self { path }
    }

    pub(super) fn is_root_yaml(&self) -> bool {
        let value = self.path.as_str();
        if value.contains('/') {
            return false;
        }
        value.rsplit_once('.').is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
        })
    }
}
