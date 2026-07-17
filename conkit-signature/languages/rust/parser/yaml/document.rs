use super::input::{RustYamlDocument, RustYamlDocumentLocation};
use crate::api::{GenerateDocument, GenerateTarget};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::SignatureInventory;
use crate::languages::rust::parser::source_graph::RustExtraction;
use crate::languages::rust::rustdoc::RustCompilerExtractionContext;
use crate::limits::{RustExtractionUsage, YamlUsage};
use crate::work::CancellationProbe;
use std::collections::BTreeSet;
use std::sync::Arc;

pub(in crate::languages::rust::parser) enum RustGenerationPlan {
    New {
        layout: GenerateDocument,
        extraction: RustExtraction,
    },
    Existing(RustContractDocuments),
}

impl RustGenerationPlan {
    pub(in crate::languages::rust::parser) fn parse(
        target: GenerateTarget,
        yaml_usage: &mut YamlUsage<'_>,
        usage: &mut RustExtractionUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match target {
            GenerateTarget::New(document) => {
                Self::from_new_document(document, yaml_usage, cancellation)
            }
            GenerateTarget::Existing(catalog) => {
                RustContractDocuments::parse(catalog, yaml_usage, usage, cancellation)
                    .map(Self::Existing)
            }
        }
    }

    pub(in crate::languages::rust::parser) fn source_allowlist(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<BTreeSet<CatalogPath>, SignatureContractKitError> {
        match self {
            Self::New { extraction, .. } => {
                let mut files = BTreeSet::new();
                for file in extraction.files() {
                    cancellation.checkpoint()?;
                    files.insert(file.clone());
                }
                Ok(files)
            }
            Self::Existing(documents) => documents.source_allowlist(cancellation),
        }
    }

    pub(in crate::languages::rust::parser) fn validate_syntax_mode(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        match self {
            Self::New { .. } => cancellation.checkpoint(),
            Self::Existing(documents) => documents.validate_syntax_mode(cancellation),
        }
    }

    pub(in crate::languages::rust::parser) fn validate_compiler_mode(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::New { extraction, .. } if extraction.crates().len() == 1 => Ok(()),
            Self::New { layout, .. } => Err(SignatureContractKitError::write_failed(
                &layout.contract_file,
                "rust_compiler_v1 generation requires exactly one selected crate target",
            )),
            Self::Existing(documents) => documents.compiler_document(cancellation).map(|_| ()),
        }
    }

    fn from_new_document(
        mut layout: GenerateDocument,
        yaml_usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        yaml_usage.record_documents(1, Some(&layout.contract_file))?;
        if !RustContractCatalogName::new(&layout.contract_file).is_root_yaml() {
            return Err(SignatureContractKitError::write_failed(
                &layout.contract_file,
                "combined contract file must be a direct root-level .yml or .yaml document",
            ));
        }
        if layout.root.trim().is_empty() {
            return Err(SignatureContractKitError::write_failed(
                &layout.contract_file,
                "root must not be empty",
            ));
        }
        if layout.files.is_empty() {
            return Err(SignatureContractKitError::write_failed(
                &layout.contract_file,
                "new signature document must list at least one source file",
            ));
        }
        let extraction = RustExtraction::from_roots(
            layout.files.iter().cloned(),
            layout.crates.iter().cloned(),
            cancellation,
        )
        .map_err(|source| Self::contextualize_error(&layout.contract_file, source))?;
        layout.files = extraction.files().iter().cloned().collect();

        Ok(Self::New { layout, extraction })
    }

    fn contextualize_error(
        contract_file: &CatalogPath,
        source: SignatureContractKitError,
    ) -> SignatureContractKitError {
        if source.is_operation_canceled() || source.limit_exceeded().is_some() {
            source
        } else {
            SignatureContractKitError::write_failed(contract_file, source.to_string())
        }
    }
}

pub(in crate::languages::rust::parser) struct RustContractDocuments {
    pub(super) documents: Vec<RustContractDocument>,
}

impl RustContractDocuments {
    pub(in crate::languages::rust::parser) fn parse(
        catalog: FileCatalog,
        yaml_usage: &mut YamlUsage<'_>,
        usage: &mut RustExtractionUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut documents = Vec::new();
        for (catalog_name, bytes) in catalog.into_entries() {
            if !RustContractCatalogName::new(&catalog_name).is_root_yaml() {
                continue;
            }
            cancellation.checkpoint()?;
            let parsed =
                RustContractDocument::parse_many(catalog_name, bytes, yaml_usage, cancellation)?;
            for document in &parsed {
                cancellation.checkpoint()?;
                usage.record_signatures(document.document.signatures.len())?;
                for signature in &document.document.signatures {
                    cancellation.checkpoint()?;
                    for entry in &signature.entries {
                        cancellation.checkpoint()?;
                        usage.record_items(entry.declaration().item_count(), Some(entry.file()))?;
                    }
                }
            }
            documents.extend(parsed);
        }

        let documents = Self { documents };
        documents.validate_unique_sketch_ids(cancellation)?;
        Ok(documents)
    }

    pub(in crate::languages::rust::parser) fn source_allowlist(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<BTreeSet<CatalogPath>, SignatureContractKitError> {
        let mut files = BTreeSet::new();
        for document in &self.documents {
            cancellation.checkpoint()?;
            if document.document.extraction.is_none() {
                continue;
            }
            for file in &document.document.files {
                cancellation.checkpoint()?;
                files.insert(file.clone());
            }
        }
        Ok(files)
    }

    pub(in crate::languages::rust::parser) fn documents(&self) -> &[RustContractDocument] {
        &self.documents
    }

    pub(in crate::languages::rust::parser) fn into_documents(self) -> Vec<RustContractDocument> {
        self.documents
    }

    pub(in crate::languages::rust::parser) fn validate_syntax_mode(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        for document in self
            .documents
            .iter()
            .filter(|document| document.document.extraction.is_some())
        {
            cancellation.checkpoint()?;
            let extraction = document.document.extraction.as_ref().ok_or_else(|| {
                SignatureContractKitError::missing_signature_extraction(&document.location)
            })?;
            if !extraction.is_syntax() {
                return Err(SignatureContractKitError::parse_failed(
                    document.location.catalog_name(),
                    format!(
                        "document {} records rust_compiler_v1 but the operation selected syntax extraction",
                        document.location.document_index()
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(in crate::languages::rust::parser) fn compiler_document(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<&RustContractDocument, SignatureContractKitError> {
        let mut compiler_document = None;
        for document in self
            .documents
            .iter()
            .filter(|document| document.document.extraction.is_some())
        {
            cancellation.checkpoint()?;
            let extraction = document.document.extraction.as_ref().ok_or_else(|| {
                SignatureContractKitError::missing_signature_extraction(&document.location)
            })?;
            if !extraction.is_compiler() {
                return Err(SignatureContractKitError::parse_failed(
                    document.location.catalog_name(),
                    format!(
                        "document {} records rust_syntax_v2 but the operation selected compiler extraction",
                        document.location.document_index()
                    ),
                ));
            }
            if compiler_document.replace(document).is_some() {
                return Err(SignatureContractKitError::parse_failed(
                    document.location.catalog_name(),
                    "rust_compiler_v1 schema version 1 supports exactly one signature-bearing document per artifact",
                ));
            }
        }
        compiler_document.ok_or_else(|| {
            SignatureContractKitError::conversion_failed(
                "compiler extraction requires one signature-bearing contract document",
            )
        })
    }

    pub(in crate::languages::rust::parser) fn into_inventory(
        self,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let mut inventories = Vec::with_capacity(self.documents.len());
        for document in self.documents {
            cancellation.checkpoint()?;
            inventories.push(document.into_inventory(cancellation)?);
        }

        SignatureInventory::merge_all(inventories, cancellation)
    }
}

pub(in crate::languages::rust::parser) struct RustContractDocument {
    pub(super) location: RustYamlDocumentLocation,
    pub(super) original_bytes: Arc<[u8]>,
    pub(super) document: RustYamlDocument,
}

impl RustContractDocument {
    pub(in crate::languages::rust::parser) fn rust_extraction(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<Option<RustExtraction>, SignatureContractKitError> {
        self.document
            .extraction
            .as_ref()
            .map(|metadata| metadata.to_rust_extraction(&self.document.files, cancellation))
            .transpose()
    }

    pub(in crate::languages::rust::parser) fn files(&self) -> &[CatalogPath] {
        &self.document.files
    }

    pub(in crate::languages::rust::parser) fn inventory_scope(&self) -> String {
        self.location.inventory_scope()
    }

    pub(in crate::languages::rust::parser) fn validate_compiler_context(
        &self,
        context: &RustCompilerExtractionContext,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        let metadata = self.document.extraction.as_ref().ok_or_else(|| {
            SignatureContractKitError::missing_signature_extraction(&self.location)
        })?;
        metadata.validate_compiler_context(context, &self.location, cancellation)?;
        Ok(self.inventory_scope())
    }

    fn parse_many(
        catalog_name: CatalogPath,
        bytes: Vec<u8>,
        yaml_usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<Self>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let documents =
            RustYamlDocument::parse_many(&catalog_name, &bytes, yaml_usage, cancellation)?;
        let original_bytes = Arc::<[u8]>::from(bytes);
        let mut materialized = Vec::with_capacity(documents.len());
        for parsed in documents {
            cancellation.checkpoint()?;
            materialized.push(Self {
                location: parsed.location,
                original_bytes: Arc::clone(&original_bytes),
                document: parsed.document,
            });
        }
        Ok(materialized)
    }

    fn into_inventory(
        self,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let Self {
            location,
            original_bytes: _,
            document,
        } = self;
        document.into_inventory(&location, cancellation)
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

#[cfg(test)]
mod tests {
    use super::RustGenerationPlan;
    use crate::error::SignatureContractKitError;
    use crate::files::CatalogPath;

    #[test]
    fn layout_context_preserves_operation_cancellation() {
        let contract_file = CatalogPath::new("main.yml").expect("fixture path");

        let error = RustGenerationPlan::contextualize_error(
            &contract_file,
            SignatureContractKitError::operation_canceled(),
        );

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn layout_context_wraps_noncontrol_flow_validation_errors() {
        let contract_file = CatalogPath::new("main.yml").expect("fixture path");

        let error = RustGenerationPlan::contextualize_error(
            &contract_file,
            SignatureContractKitError::conversion_failed("invalid crate layout"),
        );

        assert!(!error.is_operation_canceled());
        assert!(error.to_string().contains("main.yml"));
        assert!(error.to_string().contains("invalid crate layout"));
    }
}
