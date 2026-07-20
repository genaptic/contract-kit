use super::super::document::{RustContractDocument, RustGenerationPlan};
use super::super::input::{
    RustYamlDocument, RustYamlDocumentLocation, RustYamlExtraction, RustYamlNamedSignature,
    RustYamlSketch,
};
use super::super::sketch::RustSketchSeeds;
use super::output::{RustYamlLabelPlanner, RustYamlOutput, RustYamlRenderedSignature};
use crate::api::{ContractScope, GenerateResponse, SignatureGenerationCounts};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics;
use crate::languages::rust::parser::{RustParsedEntry, RustParsedFiles, RustParsedProjection};
use crate::languages::rust::rustdoc::RustCompilerExtraction;
use crate::languages::rust::types::declaration::RustDeclaration;
use crate::limits::{GeneratedOutputMeter, RustExtractionUsage, SignatureLimits, YamlUsage};
use crate::work::CancellationProbe;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(in crate::languages::rust::parser) struct RustYamlRenderer;

impl RustYamlRenderer {
    pub(in crate::languages::rust::parser) fn render_syntax(
        parsed: RustParsedFiles,
        plan: RustGenerationPlan,
        scope: ContractScope,
        usage: &mut RustExtractionUsage<'_>,
        yaml_usage: &mut YamlUsage<'_>,
        limits: &SignatureLimits,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut diagnostics = RustCapabilityDiagnostics::new(&limits.diagnostics);
        let mut output = limits.output.meter(cancellation);
        let mut resolved_sketch_seeds = RustSketchSeeds::new();
        let documents = match plan {
            RustGenerationPlan::New { layout, extraction } => {
                let projection = parsed.project_for_extraction(
                    &extraction,
                    usage,
                    &mut diagnostics,
                    &limits.diagnostics,
                    cancellation,
                )?;
                let groups = RustYamlSourceGroups::from_projection(&projection, cancellation)?;
                let extraction =
                    RustYamlExtraction::from_rust_extraction(&extraction, cancellation)?;
                vec![RustYamlGeneratedDocument::from_new(
                    layout,
                    extraction,
                    &groups,
                    cancellation,
                )?]
            }
            RustGenerationPlan::Existing(documents) => {
                let mut generated_documents = Vec::new();
                for RustContractDocument {
                    location,
                    original_bytes,
                    document: existing,
                } in documents.documents
                {
                    cancellation.checkpoint()?;
                    let extraction =
                        RustYamlGeneratedDocument::required_extraction(&existing, &location)?;
                    let extraction =
                        extraction.to_rust_extraction(&existing.files, cancellation)?;
                    let projection = parsed.project_for_extraction(
                        &extraction,
                        usage,
                        &mut diagnostics,
                        &limits.diagnostics,
                        cancellation,
                    )?;
                    let generated = RustYamlGeneratedDocument::refreshed_existing(
                        location,
                        original_bytes,
                        existing,
                        &projection,
                        scope,
                        cancellation,
                    )?;
                    if matches!(scope, ContractScope::All) {
                        resolved_sketch_seeds.append_document(
                            &generated.location,
                            &generated.proposed_document,
                            parsed.projected_source(&projection),
                            &mut output,
                            cancellation,
                        )?;
                    }
                    generated_documents.push(generated);
                }
                generated_documents
            }
        };
        let mut response = Self::finish(
            documents,
            resolved_sketch_seeds,
            output,
            yaml_usage,
            cancellation,
        )?;
        response.capability_warnings = diagnostics.into_warning_messages(cancellation)?;
        Ok(response)
    }

    pub(in crate::languages::rust::parser) fn render_compiler(
        extracted: RustCompilerExtraction,
        plan: RustGenerationPlan,
        scope: ContractScope,
        yaml_usage: &mut YamlUsage<'_>,
        limits: &SignatureLimits,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut output = limits.output.meter(cancellation);
        let mut resolved_sketch_seeds = RustSketchSeeds::new();
        let documents = match plan {
            RustGenerationPlan::New {
                layout,
                extraction: requested_extraction,
            } => {
                let extraction =
                    RustYamlExtraction::from_compiler_context(extracted.context(), cancellation)?;
                let expected = extraction.to_rust_extraction(&layout.files, cancellation)?;
                if expected != requested_extraction {
                    return Err(SignatureContractKitError::write_failed(
                        &layout.contract_file,
                        "compiler artifact target does not match the requested crate root",
                    ));
                }
                let groups =
                    RustYamlSourceGroups::from_projection(extracted.projection(), cancellation)?;
                vec![RustYamlGeneratedDocument::from_new(
                    layout,
                    extraction,
                    &groups,
                    cancellation,
                )?]
            }
            RustGenerationPlan::Existing(documents) => {
                RustYamlGeneratedDocument::from_existing_compiler(
                    documents.documents,
                    &extracted,
                    scope,
                    &mut output,
                    &mut resolved_sketch_seeds,
                    cancellation,
                )?
            }
        };
        Self::finish(
            documents,
            resolved_sketch_seeds,
            output,
            yaml_usage,
            cancellation,
        )
    }

    pub(super) fn finish(
        documents: Vec<RustYamlGeneratedDocument>,
        resolved_sketch_seeds: RustSketchSeeds,
        mut output: GeneratedOutputMeter<'_>,
        yaml_usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let mut signature_count = 0_usize;
        let mut preserved_sketch_count = 0_usize;
        for document in &documents {
            cancellation.checkpoint()?;
            signature_count = signature_count
                .checked_add(document.signatures.len())
                .ok_or_else(|| {
                    SignatureContractKitError::conversion_failed(
                        "generated signature count overflowed usize",
                    )
                })?;
            preserved_sketch_count = preserved_sketch_count
                .checked_add(document.proposed_document.sketches.len())
                .ok_or_else(|| {
                    SignatureContractKitError::conversion_failed(
                        "preserved sketch count overflowed usize",
                    )
                })?;
        }
        let mut counts = SignatureGenerationCounts {
            document_count: documents.len(),
            signature_count,
            preserved_sketch_count,
            semantically_changed_document_count: 0,
            byte_changed_document_count: 0,
        };
        let mut by_file = BTreeMap::<CatalogPath, Vec<RustYamlGeneratedDocument>>::new();
        for document in documents {
            cancellation.checkpoint()?;
            by_file
                .entry(document.location.catalog_name().clone())
                .or_default()
                .push(document);
        }
        let mut contract_files = FileCatalog::new();
        for (catalog_name, mut documents) in by_file {
            cancellation.checkpoint()?;
            documents.sort_by_key(|document| document.location.document_index());
            cancellation.checkpoint()?;
            let bytes = RustYamlOutput::new(&catalog_name, documents, cancellation)?.render(
                &mut counts,
                &mut output,
                yaml_usage,
                cancellation,
            )?;
            contract_files.insert(catalog_name, bytes)?;
        }
        Ok(GenerateResponse {
            contract_files,
            counts,
            resolved_sketch_seeds: resolved_sketch_seeds.into_values(cancellation)?,
            capability_warnings: Vec::new(),
        })
    }
}

pub(super) struct RustYamlSourceGroup<'a> {
    pub(super) primary: &'a RustParsedEntry,
    pub(super) implementations: Vec<&'a RustParsedEntry>,
}

impl RustYamlSourceGroup<'_> {
    pub(super) fn structural_key(&self) -> &RustItemId {
        self.primary.id()
    }
}

struct RustYamlSourceGroups<'a> {
    groups: Vec<RustYamlSourceGroup<'a>>,
}

impl<'a> RustYamlSourceGroups<'a> {
    fn from_projection(
        projection: &'a RustParsedProjection,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut primary = BTreeMap::new();
        let mut implementations = Vec::new();
        for entry in projection.entries() {
            cancellation.checkpoint()?;
            match entry.declaration() {
                RustDeclaration::Implementation(_) => implementations.push(entry),
                _ => {
                    let previous = primary.insert(
                        entry.id().clone(),
                        RustYamlSourceGroup {
                            primary: entry,
                            implementations: Vec::new(),
                        },
                    );
                    if previous.is_some() {
                        return Err(SignatureContractKitError::conversion_failed(format!(
                            "duplicate projected Rust identity {} entered signature rendering",
                            entry.id().render()
                        )));
                    }
                }
            }
        }

        for implementation in implementations {
            cancellation.checkpoint()?;
            let RustDeclaration::Implementation(value) = implementation.declaration() else {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation group contains a non-implementation signature",
                ));
            };
            let Some(group) = primary.get_mut(value.owner().id()) else {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "cannot fold implementation {} into resolved owner {}",
                    implementation.id().render(),
                    value.owner().id().render()
                )));
            };
            group.implementations.push(implementation);
        }
        let mut groups = Vec::with_capacity(primary.len());
        for group in primary.into_values() {
            cancellation.checkpoint()?;
            groups.push(group);
        }
        Ok(Self { groups })
    }

    fn groups(&self) -> &[RustYamlSourceGroup<'a>] {
        &self.groups
    }
}

pub(super) enum RustYamlDocumentOrigin {
    New,
    Existing {
        bytes: Arc<[u8]>,
        document: Box<RustYamlDocument>,
        signature_order: Vec<RustItemId>,
    },
}

pub(super) struct RustYamlGeneratedDocument {
    pub(super) location: RustYamlDocumentLocation,
    pub(super) origin: RustYamlDocumentOrigin,
    pub(super) proposed_document: RustYamlDocument,
    pub(super) proposed_signature_order: Vec<RustItemId>,
    pub(super) signatures: Vec<BTreeMap<String, RustYamlRenderedSignature>>,
}

impl Serialize for RustYamlGeneratedDocument {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct RustYamlGeneratedDocumentRef<'a> {
            contract_version: u16,
            root: &'a str,
            files: &'a [CatalogPath],
            #[serde(skip_serializing_if = "Option::is_none")]
            extraction: Option<&'a RustYamlExtraction>,
            signatures: &'a [BTreeMap<String, RustYamlRenderedSignature>],
            sketches: &'a [RustYamlSketch],
        }

        RustYamlGeneratedDocumentRef {
            contract_version: self.proposed_document.contract_version,
            root: &self.proposed_document.root,
            files: &self.proposed_document.files,
            extraction: self.proposed_document.extraction.as_ref(),
            signatures: &self.signatures,
            sketches: &self.proposed_document.sketches,
        }
        .serialize(serializer)
    }
}

impl RustYamlGeneratedDocument {
    fn from_new(
        layout: crate::api::GenerateDocument,
        extraction: RustYamlExtraction,
        groups: &RustYamlSourceGroups<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let selected = groups.groups();
        let labels = RustYamlLabelPlanner::new().plan(selected, cancellation)?;
        let mut signatures = Vec::new();
        let mut proposed_signatures = Vec::new();
        let mut proposed_signature_order = Vec::new();
        for group in selected {
            cancellation.checkpoint()?;
            let label = labels[group.structural_key()].clone();
            let (id, signature, output) =
                Self::generated_signature(group, None, label, cancellation)?;
            proposed_signature_order.push(id);
            proposed_signatures.push(signature);
            signatures.push(output);
        }
        let proposed_document = RustYamlDocument {
            contract_version: 2,
            root: layout.root.clone(),
            files: layout.files.clone(),
            extraction: Some(extraction),
            signatures: proposed_signatures,
            sketches: Vec::new(),
        };
        Ok(Self {
            location: RustYamlDocumentLocation::new(layout.contract_file, 0),
            origin: RustYamlDocumentOrigin::New,
            proposed_document,
            proposed_signature_order,
            signatures,
        })
    }

    fn from_existing_compiler(
        existing_documents: Vec<RustContractDocument>,
        extracted: &RustCompilerExtraction,
        scope: ContractScope,
        output: &mut GeneratedOutputMeter<'_>,
        resolved_sketch_seeds: &mut RustSketchSeeds,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<Self>, SignatureContractKitError> {
        let mut documents = Vec::with_capacity(existing_documents.len());
        for RustContractDocument {
            location,
            original_bytes,
            document: existing,
        } in existing_documents
        {
            cancellation.checkpoint()?;
            let metadata = Self::required_extraction(&existing, &location)?;
            metadata.validate_compiler_context(extracted.context(), &location, cancellation)?;
            let generated = Self::refreshed_existing(
                location,
                original_bytes,
                existing,
                extracted.projection(),
                scope,
                cancellation,
            )?;
            if matches!(scope, ContractScope::All) {
                resolved_sketch_seeds.append_document(
                    &generated.location,
                    &generated.proposed_document,
                    extracted.projected_source(),
                    output,
                    cancellation,
                )?;
            }
            documents.push(generated);
        }
        Ok(documents)
    }

    fn required_extraction<'document>(
        document: &'document RustYamlDocument,
        location: &RustYamlDocumentLocation,
    ) -> Result<&'document RustYamlExtraction, SignatureContractKitError> {
        document.extraction.as_ref().ok_or_else(|| {
            SignatureContractKitError::conversion_failed(format!(
                "cannot generate signatures into {location} without extraction metadata"
            ))
        })
    }

    fn refreshed_existing(
        location: RustYamlDocumentLocation,
        original_bytes: Arc<[u8]>,
        existing: RustYamlDocument,
        projection: &RustParsedProjection,
        scope: ContractScope,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let original_signature_order = existing.signature_order(cancellation)?;
        let groups = RustYamlSourceGroups::from_projection(projection, cancellation)?;
        let selected = groups.groups();
        let labels = RustYamlLabelPlanner::from_existing(&location, &existing, cancellation)?
            .plan(selected, cancellation)?;
        let mut existing_by_key = BTreeMap::new();
        for signature in &existing.signatures {
            cancellation.checkpoint()?;
            if let Some(entry) = signature.entries.first() {
                existing_by_key.insert(
                    entry.id().clone(),
                    (signature.label.clone(), signature.sketch.clone()),
                );
            }
        }
        let mut selected_keys = BTreeSet::new();
        for group in selected {
            cancellation.checkpoint()?;
            selected_keys.insert(group.structural_key().clone());
        }
        let mut stale_sketches = BTreeSet::new();
        for signature in &existing.signatures {
            cancellation.checkpoint()?;
            let Some(entry) = signature.entries.first() else {
                continue;
            };
            if !selected_keys.contains(entry.id())
                && let Some(sketch) = &signature.sketch
            {
                match scope {
                    ContractScope::Signatures => {
                        return Err(SignatureContractKitError::conversion_failed(format!(
                            "removing signature {} would orphan preserved sketch {sketch}",
                            signature.label
                        )));
                    }
                    ContractScope::All => {
                        stale_sketches.insert(sketch.clone());
                    }
                }
            }
        }
        let mut proposed_sketches = Vec::with_capacity(existing.sketches.len());
        for sketch in &existing.sketches {
            cancellation.checkpoint()?;
            if !stale_sketches.contains(&sketch.id) {
                proposed_sketches.push(sketch.clone());
            }
        }
        let mut signatures = Vec::new();
        let mut proposed_signatures = Vec::new();
        let mut proposed_signature_order = Vec::new();
        for group in selected {
            cancellation.checkpoint()?;
            let key = group.structural_key();
            let (label, sketch) = existing_by_key
                .get(key)
                .cloned()
                .unwrap_or_else(|| (labels[key].clone(), None));
            let (id, signature, output) =
                Self::generated_signature(group, sketch, label, cancellation)?;
            proposed_signature_order.push(id);
            proposed_signatures.push(signature);
            signatures.push(output);
        }
        let proposed_document = RustYamlDocument {
            contract_version: existing.contract_version,
            root: existing.root.clone(),
            files: existing.files.clone(),
            extraction: existing.extraction.clone(),
            signatures: proposed_signatures,
            sketches: proposed_sketches,
        };
        Ok(Self {
            location,
            origin: RustYamlDocumentOrigin::Existing {
                bytes: original_bytes,
                document: Box::new(existing),
                signature_order: original_signature_order,
            },
            proposed_document,
            proposed_signature_order,
            signatures,
        })
    }

    fn generated_signature(
        group: &RustYamlSourceGroup<'_>,
        sketch: Option<String>,
        label: String,
        cancellation: &CancellationProbe,
    ) -> Result<
        (
            RustItemId,
            RustYamlNamedSignature,
            BTreeMap<String, RustYamlRenderedSignature>,
        ),
        SignatureContractKitError,
    > {
        cancellation.checkpoint()?;
        let output = RustYamlRenderedSignature::from_group(group, sketch, &label, cancellation)?;
        let signature_type = output.signature_type();
        let mut entries = std::iter::once(group.primary)
            .chain(group.implementations.iter().copied())
            .map(|entry| {
                cancellation.checkpoint()?;
                Ok((*entry).clone())
            })
            .collect::<Result<Vec<_>, SignatureContractKitError>>()?;
        if let Some((_, implementations)) = entries.split_first_mut() {
            cancellation.checkpoint()?;
            implementations.sort_by(|left, right| left.id().cmp(right.id()));
            cancellation.checkpoint()?;
        }
        let signature = RustYamlNamedSignature {
            label: label.clone(),
            crate_id: group.primary.id().module_id().crate_id().clone(),
            file: group.primary.file().clone(),
            signature_type,
            sketch: output.sketch().map(str::to_owned),
            entries,
        };
        Ok((
            group.structural_key().clone(),
            signature,
            BTreeMap::from([(label, output)]),
        ))
    }

    pub(super) fn serialize(
        &self,
        output: &mut GeneratedOutputMeter<'_>,
    ) -> Result<Vec<u8>, SignatureContractKitError> {
        output.serialize_yaml(self.location.catalog_name(), self)
    }

    pub(super) fn existing_origin(
        &self,
    ) -> Result<(&RustYamlDocument, &[RustItemId]), SignatureContractKitError> {
        match &self.origin {
            RustYamlDocumentOrigin::Existing {
                document,
                signature_order,
                ..
            } => Ok((document, signature_order)),
            RustYamlDocumentOrigin::New => {
                Err(SignatureContractKitError::unsupported_lossless_edit(
                    &self.location,
                    "new semantic document entered existing-document lossless editing",
                ))
            }
        }
    }

    pub(super) fn unchanged(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SignatureContractKitError> {
        match &self.origin {
            RustYamlDocumentOrigin::New => Ok(false),
            RustYamlDocumentOrigin::Existing { document, .. } => {
                document.render_eq(&self.proposed_document, cancellation)
            }
        }
    }

    pub(super) fn signatures_unchanged(&self) -> bool {
        match &self.origin {
            RustYamlDocumentOrigin::New => false,
            RustYamlDocumentOrigin::Existing { document, .. } => {
                document.signatures == self.proposed_document.signatures
            }
        }
    }

    pub(super) fn sketches_unchanged(&self) -> bool {
        match &self.origin {
            RustYamlDocumentOrigin::New => false,
            RustYamlDocumentOrigin::Existing { document, .. } => {
                document.sketches == self.proposed_document.sketches
            }
        }
    }

    pub(super) fn signature_preview_required(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SignatureContractKitError> {
        let RustYamlDocumentOrigin::Existing {
            document,
            signature_order,
            ..
        } = &self.origin
        else {
            return Ok(true);
        };
        if self.proposed_signature_order.is_empty() || signature_order.is_empty() {
            return Ok(true);
        }
        let original_by_id = document.signatures_by_id(cancellation)?;
        let proposed_by_id = self.proposed_document.signatures_by_id(cancellation)?;
        for id in &self.proposed_signature_order {
            cancellation.checkpoint()?;
            let Some(original) = original_by_id.get(id) else {
                return Ok(true);
            };
            if proposed_by_id.get(id) != Some(original) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn sketch_preview_required(&self) -> bool {
        !self.sketches_unchanged() && self.proposed_document.sketches.is_empty()
    }
}
