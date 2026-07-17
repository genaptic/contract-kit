use super::document::{RustContractDocument, RustContractDocuments};
use crate::api::{ResolveSketchesResponse, ResolvedSketchSeed};
use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::RustProjectedSource;
use crate::languages::rust::parser::source_graph::{RustExtraction, RustRequiredModules};
use crate::languages::rust::rustdoc::RustCompilerExtraction;
use crate::limits::GeneratedOutputMeter;
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::languages::rust::parser) struct RustSketchDocumentPlan {
    contract: RustContractDocument,
    extraction: RustExtraction,
    required_modules: RustRequiredModules,
}

impl RustSketchDocumentPlan {
    pub(in crate::languages::rust::parser) fn new_syntax(
        contract: RustContractDocument,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        let mut required = BTreeSet::new();
        for signature in &contract.document.signatures {
            cancellation.checkpoint()?;
            if signature.sketch.is_none() {
                continue;
            }
            let entry = signature.entries.first().ok_or_else(|| {
                SignatureContractKitError::conversion_failed(format!(
                    "signature {} has no structural Rust item",
                    signature.label
                ))
            })?;
            required.insert(entry.id().module_id().clone());
        }
        if required.is_empty() {
            return Ok(None);
        }

        let metadata = contract.document.extraction.as_ref().ok_or_else(|| {
            SignatureContractKitError::missing_signature_extraction(&contract.location)
        })?;
        if !metadata.is_syntax() {
            return Err(SignatureContractKitError::parse_failed(
                contract.location.catalog_name(),
                format!(
                    "document {} records rust_compiler_v1 but the operation selected syntax extraction",
                    contract.location.document_index()
                ),
            ));
        }
        let extraction = metadata.to_rust_extraction(&contract.document.files, cancellation)?;

        Ok(Some(Self {
            contract,
            extraction,
            required_modules: RustRequiredModules::new(required, cancellation)?,
        }))
    }

    pub(in crate::languages::rust::parser) fn extraction(&self) -> &RustExtraction {
        &self.extraction
    }

    pub(in crate::languages::rust::parser) fn into_parts(
        self,
    ) -> (RustContractDocument, RustExtraction, RustRequiredModules) {
        (self.contract, self.extraction, self.required_modules)
    }
}

impl RustContractDocuments {
    pub(super) fn validate_unique_sketch_ids(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let mut declarations = BTreeMap::new();
        for document in &self.documents {
            cancellation.checkpoint()?;
            for sketch in &document.document.sketches {
                cancellation.checkpoint()?;
                if let Some(previous) = declarations.insert(sketch.id.as_str(), &document.location)
                {
                    return Err(SignatureContractKitError::parse_failed(
                        &document.location,
                        format!(
                            "duplicate sketch id {} is declared in both {previous} and {}",
                            sketch.id, document.location
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl RustContractDocument {
    pub(in crate::languages::rust::parser) fn append_sketch_seeds(
        &self,
        source: RustProjectedSource<'_>,
        seeds: &mut RustSketchSeeds,
        output: &mut GeneratedOutputMeter<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        seeds.append_document(&self.location, &self.document, source, output, cancellation)
    }

    pub(in crate::languages::rust::parser) fn append_compiler_sketch_seeds(
        &self,
        extraction: &RustCompilerExtraction,
        seeds: &mut RustSketchSeeds,
        output: &mut GeneratedOutputMeter<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let mut linked = false;
        for signature in &self.document.signatures {
            cancellation.checkpoint()?;
            if signature.sketch.is_some() {
                linked = true;
                break;
            }
        }
        if !linked {
            return Ok(());
        }
        let metadata = self.document.extraction.as_ref().ok_or_else(|| {
            SignatureContractKitError::missing_signature_extraction(&self.location)
        })?;
        metadata.validate_compiler_context(extraction.context(), &self.location, cancellation)?;
        self.append_sketch_seeds(extraction.projected_source(), seeds, output, cancellation)
    }
}

pub(in crate::languages::rust::parser) struct RustSketchSeeds {
    values: Vec<ResolvedSketchSeed>,
}

impl RustSketchSeeds {
    pub(in crate::languages::rust::parser) fn new() -> Self {
        Self { values: Vec::new() }
    }

    pub(super) fn append_document(
        &mut self,
        location: &super::input::RustYamlDocumentLocation,
        document: &super::input::RustYamlDocument,
        source: RustProjectedSource<'_>,
        output: &mut GeneratedOutputMeter<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        for signature in &document.signatures {
            cancellation.checkpoint()?;
            let Some(sketch_id) = signature.sketch.as_ref() else {
                continue;
            };
            let entry = signature.entries.first().ok_or_else(|| {
                SignatureContractKitError::conversion_failed(format!(
                    "signature {} has no structural Rust item",
                    signature.label
                ))
            })?;
            let source_entry = source.entry(entry.id()).ok_or_else(|| {
                SignatureContractKitError::conversion_failed(format!(
                    "linked Rust item {} is absent from its document source projection",
                    entry.id().render()
                ))
            })?;
            if source_entry.file() != &signature.file {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "linked Rust item {} moved from {} to {}",
                    entry.id().render(),
                    signature.file,
                    source_entry.file()
                )));
            }
            let source_text = source.source_text(source_entry)?;
            output.record(source_entry.file(), source_text.len())?;
            self.values.push(ResolvedSketchSeed {
                contract_file: location.catalog_name().clone(),
                document_index: location.document_index(),
                sketch_id: sketch_id.clone(),
                signature_type: signature.signature_type.as_str().to_owned(),
                file: signature.file.clone(),
                code: source_text.to_owned(),
            });
        }
        Ok(())
    }

    fn sort(&mut self, cancellation: &CancellationProbe) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        self.values.sort_by(|left, right| {
            left.contract_file
                .cmp(&right.contract_file)
                .then_with(|| left.document_index.cmp(&right.document_index))
                .then_with(|| left.sketch_id.cmp(&right.sketch_id))
        });
        cancellation.checkpoint()
    }

    pub(super) fn into_values(
        mut self,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<ResolvedSketchSeed>, SignatureContractKitError> {
        self.sort(cancellation)?;
        Ok(self.values)
    }

    pub(in crate::languages::rust::parser) fn into_response(
        mut self,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        self.sort(cancellation)?;
        Ok(ResolveSketchesResponse {
            seeds: self.values,
            capability_warnings: Vec::new(),
        })
    }
}
