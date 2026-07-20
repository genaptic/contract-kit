use super::declaration::{RustYamlDocumentDecoder, RustYamlRawEntry, RustYamlSignatureType};
use super::metadata::RustYamlShorthandCatalogPath;
use crate::api::{RustCrateKind, RustCrateRoot};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::inventory::{SignatureGroupContext, SignatureInventory};
use crate::languages::rust::parser::RustParsedEntry;
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemIdAllocator};
use crate::languages::rust::parser::source_graph::{RustCrateId, RustExtraction};
use crate::languages::rust::rustdoc::{
    RUST_COMPILER_ARTIFACT_SCHEMA_VERSION, RustCompilerExtractionContext,
};
use crate::limits::YamlUsage;
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize, de};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlDocumentLocation {
    catalog_name: CatalogPath,
    document_index: usize,
}

impl RustYamlDocumentLocation {
    pub(in crate::languages::rust::parser::yaml) fn new(
        catalog_name: CatalogPath,
        document_index: usize,
    ) -> Self {
        Self {
            catalog_name,
            document_index,
        }
    }

    pub(in crate::languages::rust::parser::yaml) fn catalog_name(&self) -> &CatalogPath {
        &self.catalog_name
    }

    pub(in crate::languages::rust::parser::yaml) fn document_index(&self) -> usize {
        self.document_index
    }

    pub(in crate::languages::rust::parser::yaml) fn inventory_scope(&self) -> String {
        format!("{}::document:{}", self.catalog_name, self.document_index)
    }
}

impl std::fmt::Display for RustYamlDocumentLocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} document {}",
            self.catalog_name, self.document_index
        )
    }
}

pub(in crate::languages::rust::parser::yaml) struct RustYamlParsedDocument {
    pub(in crate::languages::rust::parser::yaml) location: RustYamlDocumentLocation,
    pub(in crate::languages::rust::parser::yaml) document: RustYamlDocument,
}

#[derive(Clone, Eq, PartialEq)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlDocument {
    pub(in crate::languages::rust::parser::yaml) contract_version: u16,
    pub(in crate::languages::rust::parser::yaml) root: String,
    pub(in crate::languages::rust::parser::yaml) files: Vec<CatalogPath>,
    pub(in crate::languages::rust::parser::yaml) extraction: Option<RustYamlExtraction>,
    pub(in crate::languages::rust::parser::yaml) signatures: Vec<RustYamlNamedSignature>,
    pub(in crate::languages::rust::parser::yaml) sketches: Vec<RustYamlSketch>,
}

impl RustYamlDocument {
    pub(in crate::languages::rust::parser::yaml) fn parse_many(
        catalog_name: &CatalogPath,
        bytes: &[u8],
        usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustYamlParsedDocument>, SignatureContractKitError> {
        let inputs = RustYamlDocumentInput::parse_many(catalog_name, bytes, usage, cancellation)?;
        let mut documents = Vec::with_capacity(inputs.len());
        for (location, input) in inputs {
            cancellation.checkpoint()?;
            documents.push(RustYamlParsedDocument {
                document: input.into_document(&location, cancellation)?,
                location,
            });
        }
        Ok(documents)
    }

    pub(in crate::languages::rust::parser::yaml) fn into_inventory(
        self,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let mut inventory = SignatureInventory::default();
        let inventory_scope = location.inventory_scope();
        let document_context = RustYamlDocumentContext::new(
            self.contract_version,
            self.root,
            self.files,
            self.extraction,
            cancellation,
        )?;

        for signature in self.signatures {
            cancellation.checkpoint()?;
            if signature.label.trim().is_empty() {
                return Err(SignatureContractKitError::parse_failed(
                    location,
                    "signature label must not be empty",
                ));
            }
            let group_id =
                crate::inventory::SignatureId::scoped(Some(&inventory_scope), &signature.label);
            let context = document_context.signature_context(&signature, cancellation)?;
            for entry in signature.entries {
                cancellation.checkpoint()?;
                inventory.insert(
                    entry.into_signature_entry(group_id.clone(), Some(&inventory_scope))?,
                )?;
            }
            inventory.set_group_context(group_id, context)?;
        }

        Ok(inventory)
    }

    pub(in crate::languages::rust::parser::yaml) fn signature_order(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustItemId>, SignatureContractKitError> {
        let mut order = Vec::with_capacity(self.signatures.len());
        for signature in &self.signatures {
            cancellation.checkpoint()?;
            order.push(
                signature
                    .entries
                    .first()
                    .map(|entry| entry.id().clone())
                    .ok_or_else(|| {
                        SignatureContractKitError::conversion_failed(format!(
                            "signature {} has no structural entry",
                            signature.label
                        ))
                    })?,
            );
        }
        Ok(order)
    }

    pub(in crate::languages::rust::parser::yaml) fn signatures_by_id(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<BTreeMap<RustItemId, &RustYamlNamedSignature>, SignatureContractKitError> {
        let mut signatures = BTreeMap::new();
        for signature in &self.signatures {
            cancellation.checkpoint()?;
            let id = signature
                .entries
                .first()
                .map(RustParsedEntry::id)
                .ok_or_else(|| {
                    SignatureContractKitError::conversion_failed(format!(
                        "signature {} has no structural entry",
                        signature.label
                    ))
                })?;
            if signatures.insert(id.clone(), signature).is_some() {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "duplicate structural signature {} in document",
                    id.render()
                )));
            }
        }
        Ok(signatures)
    }

    pub(in crate::languages::rust::parser::yaml) fn render_eq(
        &self,
        other: &Self,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut files = std::collections::BTreeSet::new();
        for file in &self.files {
            cancellation.checkpoint()?;
            files.insert(file);
        }
        let mut other_files = std::collections::BTreeSet::new();
        for file in &other.files {
            cancellation.checkpoint()?;
            other_files.insert(file);
        }
        let signatures = self.signatures_by_id(cancellation)?;
        let other_signatures = other.signatures_by_id(cancellation)?;
        if self.contract_version != other.contract_version
            || self.root != other.root
            || self.extraction != other.extraction
            || files != other_files
            || signatures != other_signatures
        {
            return Ok(false);
        }

        let mut sketches = BTreeMap::new();
        for sketch in &self.sketches {
            cancellation.checkpoint()?;
            sketches.insert(sketch.id.as_str(), sketch);
        }
        let mut other_sketches = BTreeMap::new();
        for sketch in &other.sketches {
            cancellation.checkpoint()?;
            other_sketches.insert(sketch.id.as_str(), sketch);
        }
        Ok(sketches == other_sketches)
    }
}

#[derive(Serialize)]
struct RustYamlDocumentContext {
    pub(super) contract_version: u16,
    pub(super) root: String,
    pub(super) files: Vec<CatalogPath>,
    pub(super) extraction: Option<RustYamlExtraction>,
}

impl RustYamlDocumentContext {
    pub(super) fn new(
        contract_version: u16,
        root: String,
        mut files: Vec<CatalogPath>,
        extraction: Option<RustYamlExtraction>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        files.sort();
        cancellation.checkpoint()?;
        files.dedup();
        Ok(Self {
            contract_version,
            root,
            files,
            extraction,
        })
    }

    pub(super) fn signature_context(
        &self,
        signature: &RustYamlNamedSignature,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureGroupContext, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let extraction = serde_json::to_vec(&(
            self.contract_version,
            &self.files,
            &self.extraction,
            &signature.crate_id,
        ))
        .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))?;
        cancellation.checkpoint()?;
        let document_metadata = serde_json::to_vec(&(
            &self.root,
            &signature.sketch,
            signature.signature_type.as_str(),
        ))
        .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))?;
        cancellation.checkpoint()?;

        Ok(SignatureGroupContext::new(
            Some(extraction),
            Some(document_metadata),
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlDocumentInput {
    pub(super) contract_version: Option<u16>,
    pub(super) root: String,
    pub(super) files: Vec<RustYamlShorthandCatalogPath>,
    pub(super) extraction: Option<RustYamlExtractionInput>,
    pub(super) signatures: Vec<RustYamlRawEntry>,
    pub(super) sketches: Vec<RustYamlSketch>,
}

impl RustYamlDocumentInput {
    pub(super) fn parse_many(
        catalog_name: &CatalogPath,
        bytes: &[u8],
        usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<(RustYamlDocumentLocation, Self)>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let source = std::str::from_utf8(bytes).map_err(|error| {
            SignatureContractKitError::parse_failed(
                catalog_name,
                format!("input is not valid UTF-8: {error}"),
            )
        })?;
        let stream = RustYamlDocumentStream::inspect(catalog_name, source, usage, cancellation)?;
        let report = std::rc::Rc::new(std::cell::RefCell::new(None));
        let report_sink = std::rc::Rc::clone(&report);
        let options = serde_saphyr::options! {
            budget: usage.semantic_parser_budget(stream.raw_report()),
            alias_limits: usage.semantic_alias_limits(stream.raw_report()),
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
            strict_booleans: true,
        }
        .with_budget_report(move |value| {
            *report_sink.borrow_mut() = Some(value);
        });
        cancellation.checkpoint()?;
        let parsed = serde_saphyr::from_multiple_with_options::<Self>(source, options)
            .map_err(|source| Self::map_yaml_error(catalog_name, &stream, source, usage))?;
        cancellation.checkpoint()?;
        // `from_multiple_with_options` calls `LiveEvents::finish` before it
        // returns, so the callback has delivered the final raw-plus-replay
        // report before this cell is read.
        let report = report.borrow_mut().take().ok_or_else(|| {
            SignatureContractKitError::conversion_failed(
                "semantic YAML parser did not return its resource report",
            )
        })?;
        usage.record_replay_report(catalog_name, stream.raw_report(), &report)?;
        let mut documents = Vec::with_capacity(parsed.len());
        for (document_index, document) in parsed.into_iter().enumerate() {
            cancellation.checkpoint()?;
            documents.push((
                RustYamlDocumentLocation::new(catalog_name.clone(), document_index),
                document,
            ));
        }
        if documents.is_empty() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "YAML file contains no non-empty contract documents",
            ));
        }
        if documents.len() != stream.document_count() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "semantic YAML document count differs from the physical stream",
            ));
        }
        Ok(documents)
    }

    pub(super) fn map_yaml_error(
        catalog_name: &CatalogPath,
        stream: &RustYamlDocumentStream,
        source: serde_saphyr::Error,
        usage: &YamlUsage<'_>,
    ) -> SignatureContractKitError {
        if let Some(error) =
            usage.limit_for_semantic_parser_error(catalog_name, stream.raw_report(), &source)
        {
            return error.into();
        }
        let location = RustYamlDocumentLocation::new(
            catalog_name.clone(),
            stream.document_index_for_error(&source),
        );
        match source.without_snippet() {
            serde_saphyr::Error::DuplicateMappingKey {
                key,
                location: yaml_location,
            } => SignatureContractKitError::duplicate_yaml_key(
                &location,
                key.clone(),
                yaml_location.line(),
                yaml_location.column(),
            ),
            _ => SignatureContractKitError::parse_failed(&location, source.to_string()),
        }
    }

    pub(super) fn into_document(
        self,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlDocument, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let contract_version = self.contract_version.ok_or_else(|| {
            SignatureContractKitError::unsupported_contract_version(location, "missing")
        })?;
        if contract_version != 2 {
            return Err(SignatureContractKitError::unsupported_contract_version(
                location,
                contract_version,
            ));
        }
        let mut files = Vec::with_capacity(self.files.len());
        for file in self.files {
            cancellation.checkpoint()?;
            files.push(file.to_catalog_path());
        }
        let extraction = self
            .extraction
            .map(|extraction| {
                extraction.into_extraction(location.catalog_name(), &files, cancellation)
            })
            .transpose()
            .map_err(|source| {
                if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                    source
                } else {
                    SignatureContractKitError::parse_failed(location, source.to_string())
                }
            })?;
        if !self.signatures.is_empty() && extraction.is_none() {
            return Err(SignatureContractKitError::parse_failed(
                location,
                "signature-bearing v2 documents require extraction metadata",
            ));
        }
        let mut item_ids = RustItemIdAllocator::default();
        let mut signatures = Vec::with_capacity(self.signatures.len());
        if let Some(extraction) = extraction.as_ref() {
            let mut decoder = RustYamlDocumentDecoder {
                catalog_name: location.catalog_name(),
                extraction,
                item_ids: &mut item_ids,
                cancellation,
            };
            for signature in self.signatures {
                cancellation.checkpoint()?;
                signatures.push(decoder.decode_entry(signature).map_err(|source| {
                    if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                        source
                    } else {
                        SignatureContractKitError::parse_failed(location, source.to_string())
                    }
                })?);
            }
        }
        RustYamlDocumentValidator::new(location, &self.root, &files, &signatures, &self.sketches)
            .validate(cancellation)?;

        Ok(RustYamlDocument {
            contract_version,
            root: self.root,
            files,
            extraction,
            signatures,
            sketches: self.sketches,
        })
    }
}

struct RustYamlDocumentStream {
    pub(super) document_count: usize,
    pub(super) document_start_lines: Vec<u64>,
    pub(super) raw_report: serde_saphyr::budget::BudgetReport,
}

impl RustYamlDocumentStream {
    pub(super) fn inspect(
        catalog_name: &CatalogPath,
        source: &str,
        usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        use serde_saphyr::granit_parser::{Event, Parser};

        cancellation.checkpoint()?;
        let raw_report = usage.validate_source(catalog_name, source)?;

        let mut document_count = 0;
        let mut document_start_lines = Vec::new();
        let mut document_has_data = None;
        for next in Parser::new_from_str(source) {
            cancellation.checkpoint()?;
            let (event, span) = next.map_err(|error| {
                SignatureContractKitError::parse_failed(
                    RustYamlDocumentLocation::new(catalog_name.clone(), document_count),
                    error.to_string(),
                )
            })?;
            match event {
                Event::DocumentStart(..) => {
                    if document_has_data.replace(false).is_some() {
                        return Err(SignatureContractKitError::parse_failed(
                            RustYamlDocumentLocation::new(catalog_name.clone(), document_count),
                            "YAML parser started a document before ending the previous document",
                        ));
                    }
                    document_start_lines.push(u64::try_from(span.start.line()).unwrap_or(u64::MAX));
                }
                Event::Alias(_) | Event::SequenceStart(..) | Event::MappingStart(..) => {
                    if let Some(has_data) = document_has_data.as_mut() {
                        *has_data = true;
                    }
                }
                Event::Scalar(value, style, _, tag) => {
                    if let Some(has_data) = document_has_data.as_mut()
                        && !Self::is_null_scalar(value.as_ref(), style, tag.as_deref())
                    {
                        *has_data = true;
                    }
                }
                Event::DocumentEnd => {
                    let has_data = document_has_data.take().unwrap_or(false);
                    if !has_data {
                        return Err(SignatureContractKitError::parse_failed(
                            RustYamlDocumentLocation::new(catalog_name.clone(), document_count),
                            "document must not be empty or null",
                        ));
                    }
                    document_count += 1;
                }
                Event::Nothing
                | Event::StreamStart
                | Event::StreamEnd
                | Event::Comment(..)
                | Event::SequenceEnd
                | Event::MappingEnd => {}
            }
        }
        if document_has_data.is_some() {
            return Err(SignatureContractKitError::parse_failed(
                RustYamlDocumentLocation::new(catalog_name.clone(), document_count),
                "YAML parser did not terminate the final document",
            ));
        }
        if document_count == 0 {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "contract stream must contain at least one document",
            ));
        }
        let raw_report = raw_report.ok_or_else(|| {
            SignatureContractKitError::conversion_failed(
                "YAML resource preflight did not return a report for a valid stream",
            )
        })?;
        Ok(Self {
            document_count,
            document_start_lines,
            raw_report,
        })
    }

    pub(super) fn document_count(&self) -> usize {
        self.document_count
    }

    pub(super) fn raw_report(&self) -> &serde_saphyr::budget::BudgetReport {
        &self.raw_report
    }

    pub(super) fn document_index_for_error(&self, error: &serde_saphyr::Error) -> usize {
        let Some(line) = error.location().map(|location| location.line()) else {
            return 0;
        };
        self.document_start_lines
            .partition_point(|start| *start <= line)
            .saturating_sub(1)
    }

    pub(super) fn is_null_scalar(
        value: &str,
        style: serde_saphyr::granit_parser::ScalarStyle,
        tag: Option<&serde_saphyr::granit_parser::Tag>,
    ) -> bool {
        if style != serde_saphyr::granit_parser::ScalarStyle::Plain {
            return false;
        }

        if let Some(tag) = tag {
            return tag.core_suffix() == Some("null");
        }

        value.is_empty() || value == "~" || value.eq_ignore_ascii_case("null")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(in crate::languages::rust::parser::yaml) enum RustYamlExtraction {
    RustSyntaxV2 {
        profile: RustYamlExtractionProfile,
        crates: Vec<RustYamlCrate>,
    },
    RustCompilerV1 {
        profile: RustYamlExtractionProfile,
        crates: Vec<RustYamlCrate>,
        compiler: RustYamlCompilerContext,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::languages::rust::parser::yaml) enum RustYamlExtractionProfile {
    RustApiV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlCrate {
    id: RustCrateId,
    root: CatalogPath,
    kind: RustCrateKind,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum RustYamlExtractionInput {
    RustSyntaxV2 {
        profile: RustYamlExtractionProfile,
        crates: Vec<RustYamlCrateInput>,
    },
    RustCompilerV1 {
        profile: RustYamlExtractionProfile,
        crates: Vec<RustYamlCrateInput>,
        compiler: RustYamlCompilerContext,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlCrateInput {
    pub(super) id: String,
    pub(super) root: RustYamlShorthandCatalogPath,
    pub(super) kind: RustCrateKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlCompilerContext {
    artifact_schema_version: u16,
    extractor_version: String,
    compiler_version: String,
    rustdoc_format_version: u32,
    target_triple: String,
    features: Vec<String>,
    cfg_values: Vec<String>,
    package: String,
    target: String,
    macro_expansion: bool,
    name_resolution: bool,
}

impl RustYamlExtractionInput {
    pub(super) fn into_extraction(
        self,
        catalog_name: &CatalogPath,
        files: &[CatalogPath],
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlExtraction, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (profile, crate_inputs, compiler) = match self {
            Self::RustSyntaxV2 { profile, crates } => (profile, crates, None),
            Self::RustCompilerV1 {
                profile,
                crates,
                compiler,
            } => (profile, crates, Some(compiler)),
        };
        let mut roots = Vec::with_capacity(crate_inputs.len());
        for value in crate_inputs {
            cancellation.checkpoint()?;
            roots.push(RustCrateRoot {
                id: value.id,
                root: value.root.to_catalog_path(),
                kind: value.kind,
            });
        }

        let extraction = RustExtraction::from_roots(files.iter().cloned(), roots, cancellation)
            .map_err(|source| {
                if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                    source
                } else {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                }
            })?;
        let crates = RustYamlExtraction::crates_from_rust_extraction(&extraction, cancellation)?;
        match compiler {
            None => Ok(RustYamlExtraction::RustSyntaxV2 { profile, crates }),
            Some(compiler) => {
                let compiler = compiler.validated(catalog_name, cancellation)?;
                if crates.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "rust_compiler_v1 requires exactly one selected crate target",
                    ));
                }
                Ok(RustYamlExtraction::RustCompilerV1 {
                    profile,
                    crates,
                    compiler,
                })
            }
        }
    }
}

impl RustYamlExtraction {
    pub(in crate::languages::rust::parser::yaml) fn from_rust_extraction(
        extraction: &RustExtraction,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self::RustSyntaxV2 {
            profile: RustYamlExtractionProfile::RustApiV1,
            crates: Self::crates_from_rust_extraction(extraction, cancellation)?,
        })
    }

    pub(in crate::languages::rust::parser::yaml) fn from_compiler_context(
        context: &RustCompilerExtractionContext,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let crate_metadata = context.crate_metadata();
        Ok(Self::RustCompilerV1 {
            profile: RustYamlExtractionProfile::RustApiV1,
            crates: vec![RustYamlCrate {
                id: context.canonical_crate_id().clone(),
                root: crate_metadata.root.clone(),
                kind: crate_metadata.kind,
            }],
            compiler: RustYamlCompilerContext::from_compiler_context(context, cancellation)?,
        })
    }

    pub(in crate::languages::rust::parser::yaml) fn is_syntax(&self) -> bool {
        matches!(self, Self::RustSyntaxV2 { .. })
    }

    pub(in crate::languages::rust::parser::yaml) fn is_compiler(&self) -> bool {
        matches!(self, Self::RustCompilerV1 { .. })
    }

    pub(in crate::languages::rust::parser::yaml) fn validate_compiler_context(
        &self,
        context: &RustCompilerExtractionContext,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let expected = Self::from_compiler_context(context, cancellation)?;
        cancellation.checkpoint()?;
        if self == &expected {
            return Ok(());
        }
        Err(SignatureContractKitError::parse_failed(
            location.catalog_name(),
            format!(
                "compiler extraction metadata in document {} does not match the supplied compiler artifact",
                location.document_index()
            ),
        ))
    }

    pub(in crate::languages::rust::parser::yaml) fn crates(&self) -> &[RustYamlCrate] {
        match self {
            Self::RustSyntaxV2 { crates, .. } | Self::RustCompilerV1 { crates, .. } => crates,
        }
    }

    pub(super) fn crates_from_rust_extraction(
        extraction: &RustExtraction,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustYamlCrate>, SignatureContractKitError> {
        let mut crates = Vec::with_capacity(extraction.crates().len());
        for value in extraction.crates() {
            cancellation.checkpoint()?;
            crates.push(RustYamlCrate {
                id: value.id().clone(),
                root: value.root().clone(),
                kind: value.kind(),
            });
        }
        Ok(crates)
    }

    pub(in crate::languages::rust::parser::yaml) fn to_rust_extraction(
        &self,
        files: &[CatalogPath],
        cancellation: &CancellationProbe,
    ) -> Result<RustExtraction, SignatureContractKitError> {
        let mut roots = Vec::with_capacity(self.crates().len());
        for value in self.crates() {
            cancellation.checkpoint()?;
            roots.push(RustCrateRoot {
                id: value.id.as_str().to_owned(),
                root: value.root.clone(),
                kind: value.kind,
            });
        }

        RustExtraction::from_roots(files.iter().cloned(), roots, cancellation)
    }

    pub(super) fn signature_crate_id(
        &self,
        explicit: Option<&str>,
        signature_label: &str,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustCrateId, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match explicit {
            Some(explicit) => {
                for candidate in self.crates() {
                    cancellation.checkpoint()?;
                    if candidate.id.as_str() == explicit {
                        return Ok(candidate.id.clone());
                    }
                }
                Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    format!("signature {signature_label} references unknown crate_id {explicit}"),
                ))
            }
            None if self.crates().len() == 1 => Ok(self.crates()[0].id.clone()),
            None => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "signature {signature_label} requires crate_id because extraction declares multiple crates"
                ),
            )),
        }
    }
}

impl RustYamlCompilerContext {
    pub(super) fn from_compiler_context(
        context: &RustCompilerExtractionContext,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let crate_metadata = context.crate_metadata();
        let mut features = Vec::with_capacity(context.features().len());
        for feature in context.features() {
            cancellation.checkpoint()?;
            features.push(feature.clone());
        }
        let mut cfg_values = Vec::with_capacity(context.cfg_values().len());
        for value in context.cfg_values() {
            cancellation.checkpoint()?;
            cfg_values.push(value.clone());
        }
        Ok(Self {
            artifact_schema_version: context.artifact_schema_version(),
            extractor_version: context.extractor_version().to_owned(),
            compiler_version: context.compiler_version().to_owned(),
            rustdoc_format_version: context.rustdoc_format_version(),
            target_triple: context.target_triple().to_owned(),
            features,
            cfg_values,
            package: crate_metadata.package.clone(),
            target: crate_metadata.target.clone(),
            macro_expansion: true,
            name_resolution: true,
        })
    }

    pub(super) fn validated(
        mut self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if self.artifact_schema_version != RUST_COMPILER_ARTIFACT_SCHEMA_VERSION {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "unsupported compiler artifact schema {}; expected {}",
                    self.artifact_schema_version, RUST_COMPILER_ARTIFACT_SCHEMA_VERSION
                ),
            ));
        }
        if self.rustdoc_format_version != rustdoc_types::FORMAT_VERSION {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "unsupported rustdoc JSON format {}; expected {}",
                    self.rustdoc_format_version,
                    rustdoc_types::FORMAT_VERSION
                ),
            ));
        }
        for (field, value) in [
            ("extractor_version", self.extractor_version.as_str()),
            ("compiler_version", self.compiler_version.as_str()),
            ("target_triple", self.target_triple.as_str()),
            ("package", self.package.as_str()),
            ("target", self.target.as_str()),
        ] {
            cancellation.checkpoint()?;
            Self::validate_text(catalog_name, field, value, cancellation)?;
        }
        self.features =
            Self::normalized_values(catalog_name, "features", self.features, cancellation)?;
        self.cfg_values =
            Self::normalized_values(catalog_name, "cfg_values", self.cfg_values, cancellation)?;
        if !self.macro_expansion || !self.name_resolution {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "rust_compiler_v1 requires macro_expansion and name_resolution to be true",
            ));
        }
        Ok(self)
    }

    pub(super) fn normalized_values(
        catalog_name: &CatalogPath,
        field: &'static str,
        mut values: Vec<String>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<String>, SignatureContractKitError> {
        for value in &values {
            cancellation.checkpoint()?;
            Self::validate_text(catalog_name, field, value, cancellation)?;
        }
        cancellation.checkpoint()?;
        values.sort();
        cancellation.checkpoint()?;
        values.dedup();
        Ok(values)
    }

    pub(super) fn validate_text(
        catalog_name: &CatalogPath,
        field: &'static str,
        value: &str,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut has_control = false;
        for (index, character) in value.chars().enumerate() {
            if index % 1024 == 0 {
                cancellation.checkpoint()?;
            }
            if character.is_control() {
                has_control = true;
                break;
            }
        }
        if value.is_empty() || value.trim() != value || has_control {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "compiler extraction field {field} must be nonempty and contain no surrounding whitespace or control characters"
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlNamedSignature {
    pub(in crate::languages::rust::parser::yaml) label: String,
    pub(in crate::languages::rust::parser::yaml) crate_id: RustCrateId,
    pub(in crate::languages::rust::parser::yaml) file: CatalogPath,
    pub(in crate::languages::rust::parser::yaml) signature_type: RustYamlSignatureType,
    pub(in crate::languages::rust::parser::yaml) sketch: Option<String>,
    pub(in crate::languages::rust::parser::yaml) entries: Vec<RustParsedEntry>,
}

#[derive(Clone, Eq, PartialEq)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlSketch {
    pub(in crate::languages::rust::parser::yaml) id: String,
    pub(in crate::languages::rust::parser::yaml) file: CatalogPath,
    pub(in crate::languages::rust::parser::yaml) signature: String,
    pub(in crate::languages::rust::parser::yaml) signature_type: String,
    pub(in crate::languages::rust::parser::yaml) matching: RustYamlSketchMatching,
    pub(in crate::languages::rust::parser::yaml) code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlSketchMatching {
    pub(in crate::languages::rust::parser::yaml) normalization: String,
    pub(in crate::languages::rust::parser::yaml) occurrence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlSketchBody {
    pub(super) file: RustYamlShorthandCatalogPath,
    pub(super) signature: String,
    pub(super) signature_type: String,
    pub(super) matching: RustYamlSketchMatching,
    pub(super) code: String,
}

struct RustYamlSketchEnvelopeValue(RustYamlSketchBody);

impl<'de> Deserialize<'de> for RustYamlSketchEnvelopeValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EnvelopeValueVisitor;

        impl<'de> de::Visitor<'de> for EnvelopeValueVisitor {
            type Value = RustYamlSketchEnvelopeValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a nested v2 sketch mapping")
            }

            fn visit_map<M>(self, mapping: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                RustYamlSketchBody::deserialize(de::value::MapAccessDeserializer::new(mapping))
                    .map(RustYamlSketchEnvelopeValue)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Err(E::custom(
                    "flattened sketch fields are not supported in contract_version 2",
                ))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_unit()
            }
        }

        deserializer.deserialize_any(EnvelopeValueVisitor)
    }
}

impl<'de> Deserialize<'de> for RustYamlSketch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SketchVisitor;

        impl<'de> de::Visitor<'de> for SketchVisitor {
            type Value = RustYamlSketch;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sketch mapping")
            }

            fn visit_map<M>(self, mut mapping: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let id = mapping
                    .next_key::<String>()?
                    .ok_or_else(|| de::Error::custom("sketch entry is missing its identifier"))?;
                let RustYamlSketchEnvelopeValue(body) = mapping.next_value()?;
                if let Some(candidate) = mapping.next_key::<String>()? {
                    return Err(de::Error::custom(format!(
                        "sketch entry has more than one identifier key, including {candidate}"
                    )));
                }
                Ok(RustYamlSketch {
                    id,
                    file: body.file.to_catalog_path(),
                    signature: body.signature,
                    signature_type: body.signature_type,
                    matching: body.matching,
                    code: body.code,
                })
            }
        }

        deserializer.deserialize_map(SketchVisitor)
    }
}

impl Serialize for RustYamlSketch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        #[derive(Serialize)]
        struct RustYamlSketchBodyRef<'a> {
            file: &'a CatalogPath,
            signature: &'a str,
            signature_type: &'a str,
            matching: &'a RustYamlSketchMatching,
            code: &'a str,
        }

        let body = RustYamlSketchBodyRef {
            file: &self.file,
            signature: &self.signature,
            signature_type: &self.signature_type,
            matching: &self.matching,
            code: &self.code,
        };
        let mut mapping = serializer.serialize_map(Some(1))?;
        mapping.serialize_entry(&self.id, &body)?;
        mapping.end()
    }
}

struct RustYamlDocumentValidator<'a> {
    pub(super) location: &'a RustYamlDocumentLocation,
    pub(super) root: &'a str,
    pub(super) files: &'a [CatalogPath],
    pub(super) signatures: &'a [RustYamlNamedSignature],
    pub(super) sketches: &'a [RustYamlSketch],
}

impl<'a> RustYamlDocumentValidator<'a> {
    pub(super) fn new(
        location: &'a RustYamlDocumentLocation,
        root: &'a str,
        files: &'a [CatalogPath],
        signatures: &'a [RustYamlNamedSignature],
        sketches: &'a [RustYamlSketch],
    ) -> Self {
        Self {
            location,
            root,
            files,
            signatures,
            sketches,
        }
    }

    pub(super) fn validate(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        if self.root.trim().is_empty() {
            return self.fail("root must not be empty".to_owned());
        }
        let mut files = std::collections::BTreeSet::new();
        for file in self.files {
            cancellation.checkpoint()?;
            if !file.has_extension("rs") {
                return self.fail(format!(
                    "listed source file {file} must have a .rs extension"
                ));
            }
            files.insert(file);
        }

        let mut labels = std::collections::BTreeSet::new();
        let mut references = BTreeMap::<&str, usize>::new();
        let mut sketches = BTreeMap::new();
        for sketch in self.sketches {
            cancellation.checkpoint()?;
            sketches.insert(sketch.id.as_str(), sketch);
        }
        if sketches.len() != self.sketches.len() {
            return self.fail("duplicate sketch identifier in document".to_owned());
        }

        for signature in self.signatures {
            cancellation.checkpoint()?;
            if !labels.insert(signature.label.as_str()) {
                return self.fail(format!("duplicate signature label {}", signature.label));
            }
            if !files.contains(&signature.file) {
                return self.fail(format!(
                    "signature {} references unlisted source file {}",
                    signature.label, signature.file
                ));
            }
            if let Some(sketch_id) = signature.sketch.as_deref() {
                let sketch = sketches.get(sketch_id).ok_or_else(|| {
                    SignatureContractKitError::parse_failed(
                        self.location,
                        format!(
                            "signature {} links missing sketch {sketch_id}",
                            signature.label
                        ),
                    )
                })?;
                if sketch.file != signature.file {
                    return self.fail(format!(
                        "sketch {sketch_id} file {} does not match linked signature file {}",
                        sketch.file, signature.file
                    ));
                }
                if sketch.signature != signature.label {
                    return self.fail(format!(
                        "sketch {sketch_id} signature {} does not match linked signature {}",
                        sketch.signature, signature.label
                    ));
                }
                if sketch.signature_type != signature.signature_type.as_str() {
                    return self.fail(format!(
                        "sketch {sketch_id} signature_type {} does not match linked signature type {}",
                        sketch.signature_type,
                        signature.signature_type.as_str()
                    ));
                }
                *references.entry(sketch_id).or_default() += 1;
            }
        }

        for sketch in self.sketches {
            cancellation.checkpoint()?;
            if sketch.id.trim().is_empty() {
                return self.fail("sketch id must not be empty".to_owned());
            }
            if !files.contains(&sketch.file) {
                return self.fail(format!(
                    "sketch {} references unlisted source file {}",
                    sketch.id, sketch.file
                ));
            }
            match references
                .get(sketch.id.as_str())
                .copied()
                .unwrap_or_default()
            {
                1 => {}
                0 => return self.fail(format!("orphan sketch {}", sketch.id)),
                count => {
                    return self.fail(format!(
                        "sketch {} is referenced by {count} signatures",
                        sketch.id
                    ));
                }
            }
        }

        Ok(())
    }

    pub(super) fn fail<T>(&self, message: String) -> Result<T, SignatureContractKitError> {
        Err(SignatureContractKitError::parse_failed(
            self.location,
            message,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::RustYamlDocumentStream;
    use crate::files::CatalogPath;

    #[test]
    fn physical_stream_inspection_enforces_the_default_yaml_depth_budget() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let source = format!("value: {}0{}\n", "[".repeat(140), "]".repeat(140));
        let limits = crate::limits::YamlLimits::default();
        let mut usage = limits.usage();
        let cancellation = crate::work::CancellationProbe::new();
        let result =
            RustYamlDocumentStream::inspect(&catalog_name, &source, &mut usage, &cancellation);
        let Err(error) = result else {
            panic!("deep YAML must be rejected before the raw event pass");
        };
        let rendered = error.to_string().to_lowercase();

        assert!(
            rendered.contains("budget") || rendered.contains("depth"),
            "{error}"
        );
    }
}
