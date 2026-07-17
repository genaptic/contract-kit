use super::model::{SketchContracts, SketchMatchPolicy};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::limits::{RawYamlReport, SketchLimits};
use crate::work::CancellationProbe;
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};
use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SketchYamlDocumentInput {
    #[serde(default)]
    pub(super) contract_version: Option<u16>,
    pub(super) root: String,
    pub(super) files: Vec<String>,
    #[serde(default)]
    pub(super) extraction: Option<IgnoredAny>,
    pub(super) signatures: Vec<BTreeMap<String, SketchSignatureInput>>,
    pub(super) sketches: Vec<SketchYamlInput>,
}

#[derive(Deserialize)]
pub(super) struct SketchSignatureInput {
    file: String,
    signature_type: String,
    #[serde(default)]
    sketch: Option<String>,
    #[serde(flatten)]
    _signature_owned: BTreeMap<String, IgnoredAny>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SketchSemanticSignature {
    pub(super) label: String,
    pub(super) file: String,
    pub(super) signature_type: String,
    pub(super) sketch: Option<String>,
}

impl SketchSemanticSignature {
    pub(super) fn from_input(
        mut entry: BTreeMap<String, SketchSignatureInput>,
        location: &str,
    ) -> Result<Self, SketchContractKitError> {
        if entry.len() != 1 {
            return Err(SketchContractKitError::parse_failed(
                location,
                "signature entries must contain exactly one named signature",
            ));
        }
        let Some((label, input)) = entry.pop_first() else {
            return Err(SketchContractKitError::parse_failed(
                location,
                "signature entry is empty",
            ));
        };

        Ok(Self {
            label,
            file: input.file,
            signature_type: input.signature_type,
            sketch: input.sketch,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SketchYamlInput {
    pub(super) id: String,
    pub(super) file: String,
    pub(super) signature: String,
    pub(super) signature_type: String,
    pub(super) matching: SketchMatchPolicy,
    pub(super) code: String,
}

impl<'de> Deserialize<'de> for SketchYamlInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut entry = BTreeMap::<String, SketchYamlBodyInput>::deserialize(deserializer)?;
        if entry.len() != 1 {
            return Err(serde::de::Error::custom(
                "sketch entries must contain exactly one named sketch",
            ));
        }
        let Some((id, body)) = entry.pop_first() else {
            return Err(serde::de::Error::custom("sketch entry is empty"));
        };

        Ok(Self {
            id,
            file: body.file,
            signature: body.signature,
            signature_type: body.signature_type,
            matching: body.matching,
            code: body.code,
        })
    }
}

impl SketchYamlInput {
    pub(super) fn matches_refresh(&self, previous: &Self, refreshed_code: Option<&str>) -> bool {
        self.id == previous.id
            && self.file == previous.file
            && self.signature == previous.signature
            && self.signature_type == previous.signature_type
            && self.matching == previous.matching
            && self.code == refreshed_code.unwrap_or(previous.code.as_str())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SketchYamlBodyInput {
    file: String,
    signature: String,
    signature_type: String,
    matching: SketchMatchPolicy,
    code: String,
}

impl SketchContracts {
    pub(crate) fn from_catalog(
        catalog: FileCatalog,
        limits: &SketchLimits,
        yaml_budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let documents = SketchContractDocuments::from_catalog(catalog, yaml_budget, cancellation)?;
        documents.contracts(limits, cancellation)
    }
}

pub(crate) struct SketchContractDocuments {
    pub(super) files: BTreeMap<CatalogPath, SketchContractFile>,
    pub(super) passthrough: FileCatalog,
}

impl SketchContractDocuments {
    pub(crate) fn from_catalog(
        catalog: FileCatalog,
        yaml_budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let mut files = BTreeMap::new();
        let mut passthrough = FileCatalog::new();

        for (file_index, (catalog_name, bytes)) in catalog.into_entries().enumerate() {
            cancellation.checkpoint_at(file_index)?;
            if !catalog_name.as_str().contains('/')
                && (catalog_name.has_extension("yaml") || catalog_name.has_extension("yml"))
            {
                let file = SketchContractFile::parse(
                    catalog_name.clone(),
                    bytes,
                    yaml_budget,
                    cancellation,
                )?;
                files.insert(catalog_name, file);
            } else {
                passthrough.insert(catalog_name, bytes)?;
            }
        }

        Ok(Self { files, passthrough })
    }
}

const CONTRACT_VERSION: u16 = 2;

pub(super) struct SketchContractFile {
    pub(super) catalog_name: CatalogPath,
    pub(super) original_bytes: Vec<u8>,
    pub(super) documents: Vec<SketchContractDocument>,
}

impl SketchContractFile {
    pub(super) fn parse(
        catalog_name: CatalogPath,
        original_bytes: Vec<u8>,
        budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        cancellation.checkpoint()?;
        let source = std::str::from_utf8(&original_bytes).map_err(|error| {
            SketchContractKitError::parse_failed(
                &catalog_name,
                format!("input is not valid UTF-8: {error}"),
            )
        })?;
        cancellation.checkpoint()?;
        let stream = SketchDocumentStream::inspect(&catalog_name, source, budget, cancellation)?;

        let report = Rc::new(RefCell::new(None));
        let report_sink = Rc::clone(&report);
        let options = serde_saphyr::options! {
            budget: budget.semantic_parser_budget(stream.raw_report()),
            alias_limits: budget.semantic_alias_limits(stream.raw_report()),
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
            strict_booleans: true,
        }
        .with_budget_report(move |value| {
            *report_sink.borrow_mut() = Some(value);
        });
        cancellation.checkpoint()?;
        let inputs =
            serde_saphyr::from_multiple_with_options::<SketchYamlDocumentInput>(source, options)
                .map_err(|error| {
                    let document_index = stream.document_index_for_error(&error);
                    if let Some(limit) = budget.limit_for_semantic_parser_error(
                        &catalog_name,
                        stream.raw_report(),
                        &error,
                    ) {
                        return SketchContractKitError::from(limit);
                    }
                    match error.without_snippet() {
                        serde_saphyr::Error::DuplicateMappingKey { key, .. } => {
                            SketchContractKitError::duplicate_yaml_key(
                                &catalog_name,
                                document_index,
                                key.clone(),
                            )
                        }
                        _ => SketchContractKitError::parse_failed(
                            format!("{catalog_name} document {document_index}"),
                            error.to_string(),
                        ),
                    }
                })?;
        cancellation.checkpoint()?;
        // The all-content entry point calls `LiveEvents::finish` before it
        // returns, so the callback has delivered the final raw-plus-replay
        // report before this cell is read.
        let report = report.borrow_mut().take().ok_or_else(|| {
            SketchContractKitError::conversion_failed(
                "semantic YAML parser did not return its resource report",
            )
        })?;
        budget.record_replay_report(&catalog_name, stream.raw_report(), &report)?;
        if inputs.len() != stream.document_count() {
            return Err(SketchContractKitError::parse_failed(
                &catalog_name,
                format!(
                    "semantic parser returned {} documents for {} source documents",
                    inputs.len(),
                    stream.document_count()
                ),
            ));
        }

        let mut documents = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.into_iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            documents.push(SketchContractDocument::from_input(
                input,
                &catalog_name,
                index,
                cancellation,
            )?);
        }

        Ok(Self {
            catalog_name,
            original_bytes,
            documents,
        })
    }
}

pub(super) struct SketchDocumentStream {
    document_count: usize,
    document_start_lines: Vec<u64>,
    raw_report: RawYamlReport,
}

impl SketchDocumentStream {
    pub(super) fn inspect(
        catalog_name: &CatalogPath,
        source: &str,
        budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        cancellation.checkpoint()?;
        let mut meter = budget.raw_meter(catalog_name);
        let mut document_count = 0;
        let mut document_start_lines = Vec::new();
        let mut document_has_data = None;
        let mut metadata_error = None;

        for (event_index, next) in Parser::new_from_str(source).enumerate() {
            cancellation.checkpoint_at(event_index)?;
            let (event, span) = match next {
                Ok(item) => item,
                Err(error) => {
                    return Err(metadata_error.unwrap_or_else(|| {
                        SketchContractKitError::parse_failed(catalog_name, error.to_string())
                    }));
                }
            };
            meter.observe(&event)?;

            if metadata_error.is_some() {
                continue;
            }

            match event {
                Event::DocumentStart(..) => {
                    if document_has_data.replace(false).is_some() {
                        metadata_error = Some(SketchContractKitError::parse_failed(
                            catalog_name,
                            "YAML parser started a document before ending the previous document",
                        ));
                        continue;
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
                        metadata_error = Some(SketchContractKitError::parse_failed(
                            catalog_name,
                            format!("document {document_count} must not be empty or null"),
                        ));
                        continue;
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

        if metadata_error.is_none() && document_has_data.is_some() {
            metadata_error = Some(SketchContractKitError::parse_failed(
                catalog_name,
                "YAML parser did not terminate the final document",
            ));
        }
        if metadata_error.is_none() && document_count == 0 {
            metadata_error = Some(SketchContractKitError::parse_failed(
                catalog_name,
                "contract stream must contain at least one document",
            ));
        }
        let raw_report = meter.finish()?;
        if let Some(error) = metadata_error {
            return Err(error);
        }

        Ok(Self {
            document_count,
            document_start_lines,
            raw_report,
        })
    }

    fn document_count(&self) -> usize {
        self.document_count
    }

    fn raw_report(&self) -> &RawYamlReport {
        &self.raw_report
    }

    fn document_index_for_error(&self, error: &serde_saphyr::Error) -> usize {
        let Some(line) = error.location().map(|location| location.line()) else {
            return 0;
        };
        self.document_start_lines
            .partition_point(|start| *start <= line)
            .saturating_sub(1)
    }

    fn is_null_scalar(
        value: &str,
        style: ScalarStyle,
        tag: Option<&serde_saphyr::granit_parser::Tag>,
    ) -> bool {
        if style != ScalarStyle::Plain {
            return false;
        }

        if let Some(tag) = tag {
            return tag.core_suffix() == Some("null");
        }

        value.is_empty() || value == "~" || value.eq_ignore_ascii_case("null")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SketchContractDocument {
    pub(super) index: usize,
    pub(super) semantic: SketchSemanticDocument,
}

impl SketchContractDocument {
    pub(super) fn from_input(
        input: SketchYamlDocumentInput,
        catalog_name: &CatalogPath,
        index: usize,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        cancellation.checkpoint_at(index)?;
        let location = format!("{catalog_name} document {index}");
        if input.contract_version != Some(CONTRACT_VERSION) {
            return Err(SketchContractKitError::unsupported_contract_version(
                location,
                input.contract_version,
            ));
        }
        if !input.signatures.is_empty() && input.extraction.is_none() {
            return Err(SketchContractKitError::parse_failed(
                location,
                "signature-bearing contract document requires extraction",
            ));
        }

        let mut signatures = Vec::with_capacity(input.signatures.len());
        for (signature_index, entry) in input.signatures.into_iter().enumerate() {
            cancellation.checkpoint_at(signature_index)?;
            signatures.push(SketchSemanticSignature::from_input(entry, &location)?);
        }

        Ok(Self {
            index,
            semantic: SketchSemanticDocument {
                root: input.root,
                files: input.files,
                signatures,
                sketches: input.sketches,
            },
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SketchSemanticDocument {
    pub(super) root: String,
    pub(super) files: Vec<String>,
    pub(super) signatures: Vec<SketchSemanticSignature>,
    pub(super) sketches: Vec<SketchYamlInput>,
}

#[cfg(test)]
mod tests {
    use super::super::model::{SketchMatchPolicy, SketchNormalization, SketchOccurrence};
    use super::{
        SketchContractDocument, SketchContractFile, SketchDocumentStream, SketchYamlDocumentInput,
    };
    use crate::contract::tests::{ContractYaml, SketchContracts, TestCatalog};
    use crate::files::CatalogPath;
    use crate::limits::SketchLimits;
    use crate::work::CancellationProbe;

    #[test]
    fn combined_signature_and_nested_sketch_yaml_is_accepted() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.YmL",
                r#"
contract_version: 2
root: ../src
files:
  - utils.rs
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: utils.rs, kind: library }] }
signatures:
  - parse_positive:
      file: utils.rs
      signature_type: function
      name: parse_positive
      visibility: public(crate)
      parameters:
        - input: "&str"
      sketch: parse_positive_body
sketches:
  - parse_positive_body:
      file: utils.rs
      signature: parse_positive
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: |
        fn parse_positive(input: &str) -> i32 {
            input.parse().unwrap()
        }
"#,
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("parse");
        let entry = contracts.entries().first().expect("sketch entry");

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts.contract_document_count(), 1);
        assert_eq!(entry.id().as_str(), "parse_positive_body");
        assert_eq!(entry.contract_file().as_str(), "main.YmL");
        assert_eq!(entry.file().as_str(), "utils.rs");
        assert_eq!(entry.linked_signature().as_str(), "parse_positive");
        assert_eq!(entry.signature_type().as_str(), "function");
        assert_eq!(
            &entry.matching_policy(),
            &SketchMatchPolicy::new(
                SketchNormalization::ExactLinesV1,
                SketchOccurrence::AtLeastOne,
            )
        );
        assert_eq!(entry.normalization(), SketchNormalization::ExactLinesV1);
        assert_eq!(entry.occurrence(), SketchOccurrence::AtLeastOne);
        assert!(!entry.snippet().normalized().is_empty());
    }

    #[test]
    fn both_explicit_occurrence_policies_use_one_public_matching_model() {
        for (occurrence, expected) in [
            ("at_least_one", SketchOccurrence::AtLeastOne),
            ("exactly_one", SketchOccurrence::ExactlyOne),
        ] {
            let yaml = ContractYaml::linked("answer", "answer_body", "function", "fn answer() {}")
                .replace(
                    "occurrence: at_least_one",
                    &format!("occurrence: {occurrence}"),
                );
            let contracts = SketchContracts::from_catalog(
                TestCatalog::new()
                    .with_file("main.yml", &yaml)
                    .into_catalog(),
            )
            .expect("explicit matching policy");
            let entry = contracts.entries().first().expect("sketch entry");

            assert_eq!(entry.normalization(), SketchNormalization::ExactLinesV1);
            assert_eq!(entry.occurrence(), expected);
            assert_eq!(
                entry.matching_policy().normalization(),
                SketchNormalization::ExactLinesV1
            );
            assert_eq!(entry.matching_policy().occurrence(), expected);
            assert_eq!(
                serde_json::to_string(&entry.matching_policy())
                    .expect("serialized matching policy"),
                format!("{{\"normalization\":\"exact_lines_v1\",\"occurrence\":\"{occurrence}\"}}")
            );
        }
    }

    #[test]
    fn nested_yaml_entries_are_not_contract_documents() {
        let catalog = TestCatalog::new()
            .with_file(
                "nested/invalid.yml",
                "version: 1\nlanguage: rust\nthis is not the combined format\n",
            )
            .with_file(
                "main.yml",
                "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n",
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("parse root document");

        assert!(contracts.entries().is_empty());
        assert_eq!(contracts.contract_document_count(), 1);
    }

    #[test]
    fn malformed_raw_stream_does_not_commit_partial_yaml_usage() {
        let path = CatalogPath::new("main.yml").expect("path");
        let valid = "value: [one, two]\n";
        let report = serde_saphyr::budget::check_yaml_budget(
            valid,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("valid fixture report");
        let mut limits = SketchLimits::default();
        limits.yaml.documents = u64::try_from(report.documents).expect("documents");
        limits.yaml.depth = u64::try_from(report.max_depth).expect("depth");
        limits.yaml.nodes = u64::try_from(report.nodes).expect("nodes");
        limits.yaml.aliases = u64::try_from(report.aliases).expect("aliases");
        limits.yaml.alias_expansion_bytes =
            u64::try_from(report.total_scalar_bytes).expect("scalar bytes");
        let mut budget = limits.yaml_budget();
        let cancellation = CancellationProbe::new();

        let Err(malformed) =
            SketchDocumentStream::inspect(&path, "value: [one,\n", &mut budget, &cancellation)
        else {
            panic!("truncated sequence must fail");
        };
        assert!(malformed.limit_exceeded().is_none());

        SketchDocumentStream::inspect(&path, valid, &mut budget, &cancellation)
            .expect("the complete exact-boundary stream must retain the full budget");
    }

    #[test]
    fn raw_limits_and_document_metadata_keep_legacy_error_precedence() {
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let mut limited = SketchLimits::default();
        limited.yaml.documents = 1;
        let Err(limit) = SketchDocumentStream::inspect(
            &path,
            "---\n---\nvalue: one\n",
            &mut limited.yaml_budget(),
            &cancellation,
        ) else {
            panic!("the later raw document breach must precede the empty document error");
        };
        assert_eq!(
            limit.limit_exceeded().expect("typed raw limit").resource,
            crate::LimitResource::YamlDocumentCount
        );

        let Err(metadata) = SketchDocumentStream::inspect(
            &path,
            "---\n---\nvalue: [\n",
            &mut SketchLimits::default().yaml_budget(),
            &cancellation,
        ) else {
            panic!("the earlier empty document must precede the later syntax error");
        };
        assert!(metadata.limit_exceeded().is_none());
        assert!(
            metadata
                .to_string()
                .contains("document 0 must not be empty or null"),
            "{metadata}"
        );
    }

    #[test]
    fn cancellation_stops_raw_semantic_and_document_conversion() {
        let path = CatalogPath::new("main.yml").expect("path");
        let source = "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n";
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let mut stream_budget = limits.yaml_budget();
        let Err(stream_error) =
            SketchDocumentStream::inspect(&path, source, &mut stream_budget, &cancellation)
        else {
            panic!("cancelled event scan must fail");
        };
        assert!(stream_error.to_string().contains("cancelled"));

        let mut parse_budget = limits.yaml_budget();
        let Err(parse_error) = SketchContractFile::parse(
            path.clone(),
            source.as_bytes().to_vec(),
            &mut parse_budget,
            &cancellation,
        ) else {
            panic!("cancelled semantic parse must fail");
        };
        assert!(parse_error.to_string().contains("cancelled"));

        let Err(document_error) = SketchContractDocument::from_input(
            SketchYamlDocumentInput {
                contract_version: Some(2),
                root: "../src".to_owned(),
                files: Vec::new(),
                extraction: None,
                signatures: Vec::new(),
                sketches: Vec::new(),
            },
            &path,
            0,
            &cancellation,
        ) else {
            panic!("cancelled semantic document conversion must fail");
        };
        assert!(document_error.to_string().contains("cancelled"));
    }
}
