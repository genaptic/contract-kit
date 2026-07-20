use super::super::input::{RustYamlDocument, RustYamlDocumentLocation, RustYamlSketch};
use super::output::RustYamlRenderedSignature;
use super::proposal::RustYamlGeneratedDocument;
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::limits::{
    GeneratedOutputMeter, ReturnedOutputBuffer, ScratchText, ScratchWriter, YamlUsage,
};
use crate::work::CancellationProbe;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) struct RustYamlLosslessEditor {
    catalog_name: CatalogPath,
    original: Arc<[u8]>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::{RustYamlDocumentLocation, RustYamlLineEnding};
    use crate::files::CatalogPath;
    use crate::limits::OutputLimits;

    #[test]
    fn line_ending_scan_stops_when_generation_is_canceled() {
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();
        let source = "contract_version: 2\r\n";
        let location =
            RustYamlDocumentLocation::new(CatalogPath::new("main.yml").expect("catalog path"), 0);

        let error = RustYamlLineEnding::from_document_source(
            source,
            &(0..source.len()),
            &location,
            &cancellation,
        )
        .expect_err("canceled line-ending scan must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn line_ending_scan_is_bounded_to_one_physical_document() {
        let source = "first: value\n---\r\nsecond: value\r\n---\rthird: value\r";
        let location =
            RustYamlDocumentLocation::new(CatalogPath::new("main.yml").expect("catalog path"), 0);
        let cancellation = crate::work::CancellationProbe::new();
        let second_start = source.find("---\r\n").expect("second document");
        let third_start = source.find("---\rthird").expect("third document");

        assert_eq!(
            RustYamlLineEnding::from_document_source(
                source,
                &(0..second_start),
                &location,
                &cancellation,
            )
            .expect("LF document"),
            RustYamlLineEnding::Lf
        );
        assert_eq!(
            RustYamlLineEnding::from_document_source(
                source,
                &(second_start..third_start),
                &location,
                &cancellation,
            )
            .expect("CRLF document"),
            RustYamlLineEnding::CrLf
        );
        assert_eq!(
            RustYamlLineEnding::from_document_source(
                source,
                &(third_start..source.len()),
                &location,
                &cancellation,
            )
            .expect("CR document"),
            RustYamlLineEnding::Cr
        );
        assert_eq!(
            RustYamlLineEnding::from_document_source(
                "contract_version: 2",
                &(0.."contract_version: 2".len()),
                &location,
                &cancellation,
            )
            .expect("document without a physical break"),
            RustYamlLineEnding::Lf
        );
    }

    #[test]
    fn one_line_document_does_not_inherit_its_separator_line_ending() {
        let source = "{contract_version: 2}\r\n---\r\n{contract_version: 2}\r\n";
        let yaml = yaml_edit::YamlFile::from_str(source).expect("multi-document YAML");
        let document = yaml.documents().next().expect("first YAML document");
        let physical_range = document.byte_range();
        let bounds = physical_range.start as usize..physical_range.end as usize;
        let location =
            RustYamlDocumentLocation::new(CatalogPath::new("main.yml").expect("catalog path"), 0);

        assert_eq!(
            RustYamlLineEnding::from_document_source(
                source,
                &bounds,
                &location,
                &crate::work::CancellationProbe::new(),
            )
            .expect("one-line document line-ending policy"),
            RustYamlLineEnding::Lf,
        );
    }

    #[test]
    fn generated_line_ending_conversion_preserves_isolated_carriage_returns() {
        let path = CatalogPath::new("main.yml").expect("catalog path");
        let cancellation = crate::work::CancellationProbe::new();
        let limits = OutputLimits::default();
        let mut output = limits.meter(&cancellation);
        let mut buffer = output.returned_buffer(&path);

        let rendering =
            RustYamlLineEnding::CrLf.write_generated("first\nsecond\r\nthird\rfourth", &mut buffer);
        let rendered = output
            .commit_returned_buffer(buffer, rendering)
            .expect("generated line endings");

        assert_eq!(rendered, b"first\r\nsecond\r\nthird\rfourth");
    }
}

#[derive(Serialize)]
struct RustYamlSignatureSection<'a> {
    signatures: &'a [BTreeMap<String, RustYamlRenderedSignature>],
}

#[derive(Serialize)]
struct RustYamlFlowSignatureSection<'a> {
    signatures: serde_saphyr::FlowSeq<&'a [BTreeMap<String, RustYamlRenderedSignature>]>,
}

#[derive(Serialize)]
struct RustYamlSketchSection<'a> {
    sketches: &'a [RustYamlSketch],
}

#[derive(Serialize)]
struct RustYamlFlowSketchSection<'a> {
    sketches: serde_saphyr::FlowSeq<&'a [RustYamlSketch]>,
}

struct RustYamlSectionPreview<'meter, 'limits> {
    source: ScratchText<'meter, 'limits>,
    document: yaml_edit::Document,
}

struct RustYamlDocumentEdits<'meter, 'limits> {
    bounds: std::ops::Range<usize>,
    line_ending: RustYamlLineEnding,
    signature_preview: Option<RustYamlSectionPreview<'meter, 'limits>>,
    sketch_preview: Option<RustYamlSectionPreview<'meter, 'limits>>,
    edits: Vec<RustYamlTextEdit<'meter, 'limits>>,
}

struct RustYamlEditContext<'editor, 'document, 'meter, 'limits, 'operation> {
    editor: &'editor RustYamlLosslessEditor,
    current: &'document yaml_edit::Document,
    proposal: &'document RustYamlGeneratedDocument,
    original_document: &'document RustYamlDocument,
    original_signature_order: &'document [RustItemId],
    document_end: usize,
    output: &'meter GeneratedOutputMeter<'limits>,
    cancellation: &'operation CancellationProbe,
}

struct RustYamlTextEdit<'meter, 'limits> {
    range: std::ops::Range<usize>,
    replacement: RustYamlReplacement<'meter, 'limits>,
}

enum RustYamlReplacement<'meter, 'limits> {
    Deletion,
    SignaturePreview(std::ops::Range<usize>),
    SketchPreview(std::ops::Range<usize>),
    Generated(ScratchText<'meter, 'limits>),
}

#[derive(Clone, Copy)]
enum RustYamlPreviewKind {
    Signature,
    Sketch,
}

impl RustYamlPreviewKind {
    fn replacement<'meter, 'limits>(
        self,
        range: std::ops::Range<usize>,
    ) -> RustYamlReplacement<'meter, 'limits> {
        match self {
            Self::Signature => RustYamlReplacement::SignaturePreview(range),
            Self::Sketch => RustYamlReplacement::SketchPreview(range),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RustYamlLineEnding {
    Lf,
    CrLf,
    Cr,
}

impl RustYamlDocumentEdits<'_, '_> {
    fn validate(
        &mut self,
        source: &str,
        location: &RustYamlDocumentLocation,
    ) -> Result<(), SignatureContractKitError> {
        self.edits.sort_by(|left, right| {
            left.range
                .start
                .cmp(&right.range.start)
                .then_with(|| left.range.end.cmp(&right.range.end))
        });
        let mut previous_end = self.bounds.start;
        for edit in &self.edits {
            if edit.range.start < self.bounds.start
                || edit.range.start > edit.range.end
                || edit.range.start < previous_end
                || edit.range.end > self.bounds.end
                || !source.is_char_boundary(edit.range.start)
                || !source.is_char_boundary(edit.range.end)
            {
                return Err(SignatureContractKitError::unsupported_lossless_edit(
                    location,
                    "CST-derived YAML edits overlap or lie outside their retained document",
                ));
            }
            match &edit.replacement {
                RustYamlReplacement::SignaturePreview(range) => {
                    let Some(preview) = &self.signature_preview else {
                        return Err(SignatureContractKitError::unsupported_lossless_edit(
                            location,
                            "signature edit has no retained generated preview",
                        ));
                    };
                    if preview.source.as_str().get(range.clone()).is_none() {
                        return Err(SignatureContractKitError::unsupported_lossless_edit(
                            location,
                            "signature replacement lies outside its generated preview",
                        ));
                    }
                }
                RustYamlReplacement::SketchPreview(range) => {
                    let Some(preview) = &self.sketch_preview else {
                        return Err(SignatureContractKitError::unsupported_lossless_edit(
                            location,
                            "sketch edit has no retained generated preview",
                        ));
                    };
                    if preview.source.as_str().get(range.clone()).is_none() {
                        return Err(SignatureContractKitError::unsupported_lossless_edit(
                            location,
                            "sketch replacement lies outside its generated preview",
                        ));
                    }
                }
                RustYamlReplacement::Deletion | RustYamlReplacement::Generated(_) => {}
            }
            previous_end = edit.range.end;
        }
        Ok(())
    }

    fn write_into(
        self,
        source: &str,
        source_cursor: &mut usize,
        output: &mut ReturnedOutputBuffer,
    ) -> std::fmt::Result {
        use std::fmt::Write as _;

        let Self {
            line_ending,
            signature_preview,
            sketch_preview,
            edits,
            ..
        } = self;
        for edit in edits {
            output.checkpoint_format()?;
            let Some(prefix) = source.get(*source_cursor..edit.range.start) else {
                return Err(std::fmt::Error);
            };
            output.write_str(prefix)?;
            match edit.replacement {
                RustYamlReplacement::Deletion => {}
                RustYamlReplacement::SignaturePreview(range) => {
                    let Some(replacement) = signature_preview
                        .as_ref()
                        .and_then(|preview| preview.source.as_str().get(range))
                    else {
                        return Err(std::fmt::Error);
                    };
                    line_ending.write_generated(replacement, output)?;
                }
                RustYamlReplacement::SketchPreview(range) => {
                    let Some(replacement) = sketch_preview
                        .as_ref()
                        .and_then(|preview| preview.source.as_str().get(range))
                    else {
                        return Err(std::fmt::Error);
                    };
                    line_ending.write_generated(replacement, output)?;
                }
                RustYamlReplacement::Generated(replacement) => {
                    line_ending.write_generated(replacement.as_str(), output)?;
                }
            }
            *source_cursor = edit.range.end;
        }
        Ok(())
    }
}

impl RustYamlLosslessEditor {
    pub(super) fn new(
        catalog_name: CatalogPath,
        original_bytes: Arc<[u8]>,
    ) -> Result<Self, SignatureContractKitError> {
        std::str::from_utf8(&original_bytes).map_err(|source| {
            SignatureContractKitError::parse_failed(&catalog_name, source.to_string())
        })?;
        Ok(Self {
            catalog_name,
            original: original_bytes,
        })
    }

    fn source(&self) -> Result<&str, SignatureContractKitError> {
        std::str::from_utf8(&self.original).map_err(|source| {
            SignatureContractKitError::parse_failed(&self.catalog_name, source.to_string())
        })
    }

    pub(super) fn apply(
        self,
        proposed: Vec<RustYamlGeneratedDocument>,
        output: &mut GeneratedOutputMeter<'_>,
        yaml_usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<u8>, SignatureContractKitError> {
        use std::str::FromStr;

        cancellation.checkpoint()?;
        let source = self.source()?;
        let editable = yaml_edit::YamlFile::from_str(source).map_err(|source| {
            SignatureContractKitError::unsupported_lossless_edit(
                &self.catalog_name,
                source.to_string(),
            )
        })?;
        cancellation.checkpoint()?;
        let mut documents = Vec::new();
        for document in editable.documents() {
            cancellation.checkpoint()?;
            documents.push(document);
        }
        if documents.len() != proposed.len() {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                &self.catalog_name,
                "semantic documents do not align with the retained physical YAML stream",
            ));
        }
        for (index, proposal) in proposed.iter().enumerate() {
            cancellation.checkpoint()?;
            if proposal.location.document_index() != index {
                return Err(SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    "semantic document indexes do not align with the physical YAML stream",
                ));
            }
        }

        let mut rendered = output.returned_buffer(&self.catalog_name);
        let rendering = {
            let mut source_cursor = 0;
            let mut rendering = Ok(());
            for (document_index, proposal) in proposed.iter().enumerate() {
                cancellation.checkpoint()?;
                if proposal.unchanged(cancellation)? {
                    continue;
                }
                let current = documents.get(document_index).ok_or_else(|| {
                    SignatureContractKitError::unsupported_lossless_edit(
                        &proposal.location,
                        "document is unavailable in the retained physical YAML stream",
                    )
                })?;
                let document_end = documents
                    .get(document_index + 1)
                    .map_or(source.len(), |document| {
                        document.byte_range().start as usize
                    });
                let mut edits =
                    self.document_edits(current, proposal, document_end, output, cancellation)?;
                edits.validate(source, &proposal.location)?;
                if let Err(error) = edits.write_into(source, &mut source_cursor, &mut rendered) {
                    rendering = Err(error);
                    break;
                }
            }
            if rendering.is_ok() {
                use std::fmt::Write as _;
                let Some(remainder) = source.get(source_cursor..) else {
                    return Err(SignatureContractKitError::unsupported_lossless_edit(
                        &self.catalog_name,
                        "edited YAML cursor lies outside the retained source",
                    ));
                };
                rendering = rendered.write_str(remainder);
            }
            rendering
        };
        let rendered = output.commit_returned_buffer(rendered, rendering)?;
        cancellation.checkpoint()?;
        let reparsed =
            RustYamlDocument::parse_many(&self.catalog_name, &rendered, yaml_usage, cancellation)
                .map_err(|source| {
                if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                    source
                } else {
                    SignatureContractKitError::unsupported_lossless_edit(
                        &self.catalog_name,
                        format!("edited YAML failed semantic reparse: {source}"),
                    )
                }
            })?;
        if reparsed.len() != proposed.len() {
            return Err(SignatureContractKitError::yaml_semantic_mismatch(
                &self.catalog_name,
            ));
        }
        for (actual, expected) in reparsed.iter().zip(&proposed) {
            cancellation.checkpoint()?;
            if !actual
                .document
                .render_eq(&expected.proposed_document, cancellation)?
            {
                return Err(SignatureContractKitError::yaml_semantic_mismatch(
                    &expected.location,
                ));
            }
        }
        Ok(rendered)
    }

    fn document_edits<'meter, 'limits>(
        &self,
        current: &yaml_edit::Document,
        proposal: &RustYamlGeneratedDocument,
        document_end: usize,
        output: &'meter GeneratedOutputMeter<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlDocumentEdits<'meter, 'limits>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (original_document, original_signature_order) = proposal.existing_origin()?;
        let physical_range = current.byte_range();
        let bounds = physical_range.start as usize..document_end;
        let line_ending_bounds = physical_range.start as usize..physical_range.end as usize;
        let line_ending = RustYamlLineEnding::from_document_source(
            self.source()?,
            &line_ending_bounds,
            &proposal.location,
            cancellation,
        )?;
        let signatures_changed = !proposal.signatures_unchanged();
        let sketches_changed = !proposal.sketches_unchanged();
        let signature_preview =
            if !signatures_changed || !proposal.signature_preview_required(cancellation)? {
                None
            } else {
                Some(self.proposed_signature_preview(current, proposal, output, cancellation)?)
            };
        let sketch_preview = if !sketches_changed || !proposal.sketch_preview_required() {
            None
        } else {
            Some(self.proposed_sketch_preview(current, proposal, output, cancellation)?)
        };
        let context = RustYamlEditContext {
            editor: self,
            current,
            proposal,
            original_document,
            original_signature_order,
            document_end,
            output,
            cancellation,
        };
        let mut edits = Vec::new();
        if signatures_changed {
            if let Some(preview) = &signature_preview {
                context.collect_signature_edits(preview, &mut edits)?;
            } else {
                self.collect_signature_removal_edits(current, proposal, &mut edits, cancellation)?;
            }
        }
        if sketches_changed {
            if let Some(preview) = &sketch_preview {
                context.collect_sketch_edits(preview, &mut edits)?;
            } else {
                self.collect_sketch_removal_edits(current, proposal, &mut edits, cancellation)?;
            }
        }
        Ok(RustYamlDocumentEdits {
            bounds,
            line_ending,
            signature_preview,
            sketch_preview,
            edits,
        })
    }

    fn proposed_signature_preview<'meter, 'limits>(
        &self,
        current: &yaml_edit::Document,
        proposal: &RustYamlGeneratedDocument,
        output: &'meter GeneratedOutputMeter<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlSectionPreview<'meter, 'limits>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let signatures = current.get_sequence("signatures").ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable signatures sequence",
            )
        })?;
        if signatures.is_flow_style() {
            let flow = output.serialize_yaml_scratch(
                proposal.location.catalog_name(),
                &RustYamlFlowSignatureSection {
                    signatures: serde_saphyr::FlowSeq(proposal.signatures.as_slice()),
                },
            )?;
            if serde_saphyr::from_slice::<serde_json::Value>(flow.as_str().as_bytes()).is_ok() {
                return self.parse_section_preview(flow, &proposal.location, cancellation);
            }
            drop(flow);
        }
        self.parse_section_preview(
            output.serialize_yaml_scratch(
                proposal.location.catalog_name(),
                &RustYamlSignatureSection {
                    signatures: proposal.signatures.as_slice(),
                },
            )?,
            &proposal.location,
            cancellation,
        )
    }

    fn proposed_sketch_preview<'meter, 'limits>(
        &self,
        current: &yaml_edit::Document,
        proposal: &RustYamlGeneratedDocument,
        output: &'meter GeneratedOutputMeter<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlSectionPreview<'meter, 'limits>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let sketches = self.retained_sketches(current).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable sketches sequence",
            )
        })?;
        if sketches.is_flow_style() {
            self.parse_section_preview(
                output.serialize_yaml_scratch(
                    proposal.location.catalog_name(),
                    &RustYamlFlowSketchSection {
                        sketches: serde_saphyr::FlowSeq(
                            proposal.proposed_document.sketches.as_slice(),
                        ),
                    },
                )?,
                &proposal.location,
                cancellation,
            )
        } else {
            self.parse_section_preview(
                output.serialize_yaml_scratch(
                    proposal.location.catalog_name(),
                    &RustYamlSketchSection {
                        sketches: proposal.proposed_document.sketches.as_slice(),
                    },
                )?,
                &proposal.location,
                cancellation,
            )
        }
    }

    fn retained_sketches(&self, document: &yaml_edit::Document) -> Option<yaml_edit::Sequence> {
        if let Some(sketches) = document.get_sequence("sketches") {
            return Some(sketches);
        }

        // yaml-edit 0.2.x can attach a valid indentationless top-level sequence
        // to the final entry of the preceding sequence. Accept only the CST
        // shape whose source position proves that `sketches` starts at column 1.
        let signatures = document.get_sequence("signatures")?;
        let final_signature_node = signatures.last()?;
        self.indentless_sketches(&final_signature_node)
    }

    fn indentless_sketches(&self, signature: &yaml_edit::YamlNode) -> Option<yaml_edit::Sequence> {
        let sketches = signature.as_mapping()?.get_sequence("sketches")?;
        let source = self.source().ok()?;
        (sketches.start_position(source).column == 1).then_some(sketches)
    }

    fn parse_section_preview<'meter, 'limits>(
        &self,
        source: ScratchText<'meter, 'limits>,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlSectionPreview<'meter, 'limits>, SignatureContractKitError> {
        use std::str::FromStr;

        let document = yaml_edit::Document::from_str(source.as_str()).map_err(|error| {
            SignatureContractKitError::unsupported_lossless_edit(location, error.to_string())
        })?;
        cancellation.checkpoint()?;
        Ok(RustYamlSectionPreview { source, document })
    }

    fn collect_signature_removal_edits<'meter, 'limits>(
        &self,
        current: &yaml_edit::Document,
        proposal: &RustYamlGeneratedDocument,
        edits: &mut Vec<RustYamlTextEdit<'meter, 'limits>>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (_, original_signature_order) = proposal.existing_origin()?;
        let current_signatures = current.get_sequence("signatures").ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable signatures sequence",
            )
        })?;
        if current_signatures.len() != original_signature_order.len() {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "signature sequence does not align with its typed semantic document",
            ));
        }
        let retained = proposal
            .proposed_signature_order
            .iter()
            .collect::<BTreeSet<_>>();
        for (index, id) in original_signature_order.iter().enumerate() {
            cancellation.checkpoint()?;
            if retained.contains(id) {
                continue;
            }
            let node = current_signatures.get(index).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("signature node {index} is unavailable"),
                )
            })?;
            edits.push(self.remove_sequence_entry_edit(
                &node,
                &proposal.location,
                "signature",
                index,
                cancellation,
            )?);
        }
        Ok(())
    }
}

impl<'editor, 'document, 'meter, 'limits, 'operation>
    RustYamlEditContext<'editor, 'document, 'meter, 'limits, 'operation>
{
    fn collect_signature_edits(
        &self,
        proposed: &RustYamlSectionPreview<'meter, 'limits>,
        edits: &mut Vec<RustYamlTextEdit<'meter, 'limits>>,
    ) -> Result<(), SignatureContractKitError> {
        let current = self.current;
        let proposal = self.proposal;
        let original_document = self.original_document;
        let original_signature_order = self.original_signature_order;
        let cancellation = self.cancellation;
        cancellation.checkpoint()?;
        let initial_edit_count = edits.len();
        let current_signatures = current.get_sequence("signatures").ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable signatures sequence",
            )
        })?;
        let proposed_signature_nodes =
            proposed
                .document
                .get_sequence("signatures")
                .ok_or_else(|| {
                    SignatureContractKitError::unsupported_lossless_edit(
                        &proposal.location,
                        "generated document has no signatures sequence",
                    )
                })?;
        let original_signatures = original_document.signatures_by_id(cancellation)?;
        let proposed_signatures = proposal.proposed_document.signatures_by_id(cancellation)?;
        if current_signatures.len() != original_signature_order.len()
            || proposed_signature_nodes.len() != proposal.proposed_signature_order.len()
        {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "signature sequence does not align with its typed semantic document",
            ));
        }

        if current_signatures.is_flow_style() != proposed_signature_nodes.is_flow_style() {
            edits.push(self.replace_sequence_edit(
                &current_signatures,
                &proposed_signature_nodes,
                proposed.source.as_str(),
                RustYamlPreviewKind::Signature,
            )?);
            return Ok(());
        }

        if proposal.proposed_signature_order.is_empty() {
            if !original_signature_order.is_empty() {
                edits.push(self.replace_sequence_edit(
                    &current_signatures,
                    &proposed_signature_nodes,
                    proposed.source.as_str(),
                    RustYamlPreviewKind::Signature,
                )?);
            }
            return Ok(());
        }
        if original_signature_order.is_empty() {
            edits.push(self.replace_sequence_edit(
                &current_signatures,
                &proposed_signature_nodes,
                proposed.source.as_str(),
                RustYamlPreviewKind::Signature,
            )?);
            return Ok(());
        }

        let mut proposed_nodes = BTreeMap::new();
        for (index, id) in proposal.proposed_signature_order.iter().enumerate() {
            cancellation.checkpoint()?;
            let node = proposed_signature_nodes.get(index).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("generated signature node {index} is unavailable"),
                )
            })?;
            if proposed_nodes.insert(id.clone(), node).is_some() {
                return Err(SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("generated signature {} is duplicated", id.render()),
                ));
            }
        }

        let mut retained_indices = Vec::new();
        for (index, id) in original_signature_order.iter().enumerate() {
            cancellation.checkpoint()?;
            let current_node = current_signatures.get(index).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("signature node {index} is unavailable"),
                )
            })?;
            if proposed_signatures.contains_key(id) {
                retained_indices.push(index);
            } else {
                edits.push(self.editor.remove_sequence_entry_edit(
                    &current_node,
                    &proposal.location,
                    "signature",
                    index,
                    cancellation,
                )?);
            }
        }

        for (index, id) in original_signature_order.iter().enumerate() {
            cancellation.checkpoint()?;
            if !proposed_signatures.contains_key(id) {
                continue;
            }
            let original = original_signatures.get(id).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("original signature {} has no typed entry", id.render()),
                )
            })?;
            let replacement = proposed_signatures.get(id).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("retained signature {} has no proposed entry", id.render()),
                )
            })?;
            if original != replacement {
                let node = proposed_nodes.get(id).ok_or_else(|| {
                    SignatureContractKitError::unsupported_lossless_edit(
                        &proposal.location,
                        format!("changed signature {} has no generated node", id.render()),
                    )
                })?;
                let current_node = current_signatures.get(index).ok_or_else(|| {
                    SignatureContractKitError::unsupported_lossless_edit(
                        &proposal.location,
                        format!("signature node {index} is unavailable"),
                    )
                })?;
                edits.push(self.replace_sequence_value_edit(
                    &current_node,
                    node,
                    proposed.source.as_str(),
                    "signature",
                    index,
                )?);
            }
        }

        let Some(reference_index) = retained_indices.last().copied() else {
            edits.truncate(initial_edit_count);
            edits.push(self.replace_sequence_edit(
                &current_signatures,
                &proposed_signature_nodes,
                proposed.source.as_str(),
                RustYamlPreviewKind::Signature,
            )?);
            return Ok(());
        };
        let reference = current_signatures.get(reference_index).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                format!("retained signature node {reference_index} is unavailable"),
            )
        })?;
        let insertion = self.editor.sequence_entry_range(
            &reference,
            &proposal.location,
            "signature",
            reference_index,
        )?;
        let mut additions = self
            .output
            .scratch_writer(proposal.location.catalog_name())?;
        let mut additions_rendering = Ok(());
        for id in &proposal.proposed_signature_order {
            cancellation.checkpoint()?;
            if original_signatures.contains_key(id) {
                continue;
            }
            let node = proposed_nodes.get(id).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("added signature {} has no generated node", id.render()),
                )
            })?;
            additions_rendering = self.write_rebased_sequence_entry(
                &reference,
                node,
                proposed.source.as_str(),
                "signature",
                &mut additions,
            )?;
            if additions_rendering.is_err() {
                break;
            }
        }
        let additions = additions.finish_text(additions_rendering)?;
        if !additions.as_str().is_empty() {
            edits.push(RustYamlTextEdit {
                range: insertion.end..insertion.end,
                replacement: RustYamlReplacement::Generated(additions),
            });
        }
        Ok(())
    }

    fn write_rebased_sequence_entry(
        &self,
        current: &yaml_edit::YamlNode,
        proposed: &yaml_edit::YamlNode,
        proposed_source: &str,
        kind: &str,
        output: &mut ScratchWriter<'_, '_>,
    ) -> Result<std::io::Result<()>, SignatureContractKitError> {
        let location = &self.proposal.location;
        let current_range = self
            .editor
            .sequence_entry_range(current, location, kind, 0)?;
        let proposed_range = self
            .editor
            .sequence_entry_range(proposed, location, kind, 0)?;
        let replacement = proposed_source.get(proposed_range.clone()).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("generated {kind} entry lies outside its CST preview"),
            )
        })?;
        let current_column = self.editor.source_column(
            self.editor.source()?,
            current_range.start,
            location,
            self.cancellation,
        )?;
        let proposed_column = self.editor.source_column(
            proposed_source,
            proposed_range.start,
            location,
            self.cancellation,
        )?;
        self.editor.validate_reindent_yaml_node(
            replacement,
            current_column,
            proposed_column,
            true,
            location,
        )?;
        Ok(self.editor.write_reindented_yaml_node(
            replacement,
            current_column,
            proposed_column,
            true,
            output,
        ))
    }

    fn replace_sequence_value_edit(
        &self,
        current: &yaml_edit::YamlNode,
        proposed: &yaml_edit::YamlNode,
        proposed_source: &str,
        kind: &str,
        index: usize,
    ) -> Result<RustYamlTextEdit<'meter, 'limits>, SignatureContractKitError> {
        use yaml_edit::AsYaml as _;

        let location = &self.proposal.location;
        let current_node = current.as_node().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("{kind} node {index} has no concrete syntax node"),
            )
        })?;
        let proposed_node = proposed.as_node().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("generated {kind} node {index} has no concrete syntax node"),
            )
        })?;
        let current_value = current_node
            .children()
            .find(|child| child.kind() == yaml_edit::SyntaxKind::MAPPING_ENTRY)
            .unwrap_or_else(|| current_node.clone());
        let proposed_value = proposed_node
            .children()
            .find(|child| child.kind() == yaml_edit::SyntaxKind::MAPPING_ENTRY)
            .unwrap_or_else(|| proposed_node.clone());
        let current_range = yaml_edit::advanced::syntax_node_range(&current_value);
        let proposed_range = yaml_edit::advanced::syntax_node_range(&proposed_value);
        let current_start: usize = current_range.start().into();
        let current_end: usize = current_range.end().into();
        let proposed_start: usize = proposed_range.start().into();
        let proposed_end: usize = proposed_range.end().into();
        let replacement = proposed_source
            .get(proposed_start..proposed_end)
            .ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    location,
                    format!("generated {kind} node {index} lies outside its CST preview"),
                )
            })?;
        let current_column = self.editor.source_column(
            self.editor.source()?,
            current_start,
            location,
            self.cancellation,
        )?;
        let proposed_column = self.editor.source_column(
            proposed_source,
            proposed_start,
            location,
            self.cancellation,
        )?;
        let replacement = if current_column == proposed_column {
            RustYamlReplacement::SignaturePreview(proposed_start..proposed_end)
        } else {
            RustYamlReplacement::Generated(self.editor.reindent_yaml_node(
                replacement,
                current_column,
                proposed_column,
                false,
                location,
                self.output,
            )?)
        };
        Ok(RustYamlTextEdit {
            range: current_start..current_end,
            replacement,
        })
    }

    fn replace_sequence_edit(
        &self,
        current: &yaml_edit::Sequence,
        proposed: &yaml_edit::Sequence,
        proposed_source: &str,
        preview_kind: RustYamlPreviewKind,
    ) -> Result<RustYamlTextEdit<'meter, 'limits>, SignatureContractKitError> {
        use yaml_edit::AsYaml as _;

        let location = &self.proposal.location;
        let current_range = current.byte_range();
        let proposed_range = proposed.byte_range();
        let proposed_start = proposed_range.start as usize;
        let proposed_end = proposed_range.end as usize;
        let replacement = proposed_source
            .get(proposed_start..proposed_end)
            .ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    location,
                    "generated sequence lies outside its CST preview",
                )
            })?;
        let current_start = current_range.start as usize;
        let proposed_column = self.editor.source_column(
            proposed_source,
            proposed_start,
            location,
            self.cancellation,
        )?;
        let current_column = self.editor.source_column(
            self.editor.source()?,
            current_start,
            location,
            self.cancellation,
        )?;
        if current.is_flow_style() && proposed.is_flow_style() {
            return Ok(RustYamlTextEdit {
                range: current_start..current_range.end as usize,
                replacement: preview_kind.replacement(proposed_start..proposed_end),
            });
        }
        let sequence_node = current.as_node().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "retained sequence has no concrete syntax node",
            )
        })?;
        let value_node = sequence_node.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "retained sequence has no owning YAML value",
            )
        })?;
        let field_node = value_node.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "retained sequence has no owning YAML field",
            )
        })?;
        let mapping_node = field_node.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "retained sequence field has no owning YAML mapping",
            )
        })?;
        let indentless_sketches_start = current
            .last()
            .map(|signature| {
                self.editor
                    .indentless_sketches_entry_start(&signature, location)
            })
            .transpose()?
            .flatten();
        let target_column = if current.is_flow_style() {
            let field_start =
                usize::from(yaml_edit::advanced::syntax_node_range(&field_node).start());
            self.editor.source_column(
                self.editor.source()?,
                field_start,
                location,
                self.cancellation,
            )? + 2
        } else {
            current_column
        };
        let mut found_field = false;
        let next_field_start = mapping_node.children().find_map(|child| {
            if child == field_node {
                found_field = true;
                return None;
            }
            if found_field && child.kind() == yaml_edit::SyntaxKind::MAPPING_ENTRY {
                let range = yaml_edit::advanced::syntax_node_range(&child);
                return Some(usize::from(range.start()));
            }
            None
        });
        let end = next_field_start
            .into_iter()
            .chain(indentless_sketches_start)
            .min()
            .unwrap_or(self.document_end);
        let value_start = usize::from(yaml_edit::advanced::syntax_node_range(&value_node).start());
        if end < value_start {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                location,
                "retained sequence boundary precedes its concrete YAML value",
            ));
        }
        let mut generated = self.output.scratch_writer(location.catalog_name())?;
        let rendering = if proposed.is_empty() {
            std::io::Write::write_all(&mut generated, b" ")
                .and_then(|()| std::io::Write::write_all(&mut generated, replacement.as_bytes()))
                .and_then(|()| std::io::Write::write_all(&mut generated, b"\n"))
        } else {
            self.editor.validate_reindent_yaml_node(
                replacement,
                target_column,
                proposed_column,
                true,
                location,
            )?;
            std::io::Write::write_all(&mut generated, b"\n")
                .and_then(|()| {
                    self.editor.write_reindented_yaml_node(
                        replacement,
                        target_column,
                        proposed_column,
                        true,
                        &mut generated,
                    )
                })
                .and_then(|()| {
                    if replacement.ends_with('\n') {
                        Ok(())
                    } else {
                        std::io::Write::write_all(&mut generated, b"\n")
                    }
                })
        };
        Ok(RustYamlTextEdit {
            range: value_start..end,
            replacement: RustYamlReplacement::Generated(generated.finish_text(rendering)?),
        })
    }
}

impl RustYamlLosslessEditor {
    fn remove_sequence_entry_edit<'meter, 'limits>(
        &self,
        node: &yaml_edit::YamlNode,
        location: &RustYamlDocumentLocation,
        kind: &str,
        index: usize,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlTextEdit<'meter, 'limits>, SignatureContractKitError> {
        let mut range = self.sequence_entry_range(node, location, kind, index)?;
        let source = self.source()?;
        let column = self.source_column(source, range.start, location, cancellation)?;
        let line_start = range.start.checked_sub(column).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "YAML sequence column exceeds its source offset",
            )
        })?;
        if source
            .get(line_start..range.start)
            .is_some_and(|prefix| prefix.bytes().all(|byte| byte == b' ' || byte == b'\t'))
        {
            range.start = line_start;
        }
        Ok(RustYamlTextEdit {
            range,
            replacement: RustYamlReplacement::Deletion,
        })
    }

    fn sequence_entry_range(
        &self,
        node: &yaml_edit::YamlNode,
        location: &RustYamlDocumentLocation,
        kind: &str,
        index: usize,
    ) -> Result<std::ops::Range<usize>, SignatureContractKitError> {
        use yaml_edit::AsYaml as _;

        let indentless_sketches_start = self.indentless_sketches_entry_start(node, location)?;
        let node = node.as_node().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("{kind} node {index} has no concrete syntax node"),
            )
        })?;
        let entry = node.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("{kind} node {index} has no sequence entry"),
            )
        })?;
        if entry.kind() != yaml_edit::SyntaxKind::SEQUENCE_ENTRY {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                location,
                format!("{kind} node {index} is not owned by a sequence entry"),
            ));
        }
        let range = yaml_edit::advanced::syntax_node_range(&entry);
        let start = range.start().into();
        let mut end = range.end().into();
        if let Some(sketches_start) = indentless_sketches_start {
            if sketches_start < start || sketches_start > end {
                return Err(SignatureContractKitError::unsupported_lossless_edit(
                    location,
                    "indentless sketches lie outside their malformed signature CST entry",
                ));
            }
            end = sketches_start;
        }
        Ok(start..end)
    }

    fn indentless_sketches_entry_start(
        &self,
        signature: &yaml_edit::YamlNode,
        location: &RustYamlDocumentLocation,
    ) -> Result<Option<usize>, SignatureContractKitError> {
        use yaml_edit::AsYaml as _;

        let Some(sketches) = self.indentless_sketches(signature) else {
            return Ok(None);
        };
        let node = sketches.as_node().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "indentless sketches have no concrete syntax node",
            )
        })?;
        let value = node.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "indentless sketches have no owning YAML value",
            )
        })?;
        let entry = value.parent().ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "indentless sketches have no owning YAML field",
            )
        })?;
        if entry.kind() != yaml_edit::SyntaxKind::MAPPING_ENTRY {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                location,
                "indentless sketches are not owned by a concrete YAML field",
            ));
        }
        Ok(Some(
            yaml_edit::advanced::syntax_node_range(&entry)
                .start()
                .into(),
        ))
    }

    fn source_column(
        &self,
        source: &str,
        offset: usize,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<usize, SignatureContractKitError> {
        let prefix = source.as_bytes().get(..offset).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "CST node offset lies outside its YAML source",
            )
        })?;
        let mut consumed = 0_usize;
        for chunk in prefix.rchunks(64 * 1024) {
            cancellation.checkpoint()?;
            if let Some(index) = chunk.iter().rposition(|byte| matches!(byte, b'\n' | b'\r')) {
                return Ok(consumed.saturating_add(chunk.len() - index - 1));
            }
            consumed = consumed.saturating_add(chunk.len());
        }
        cancellation.checkpoint()?;
        Ok(prefix.len())
    }

    fn validate_reindent_yaml_node(
        &self,
        source: &str,
        current_column: usize,
        proposed_column: usize,
        adjust_first_line: bool,
        location: &RustYamlDocumentLocation,
    ) -> Result<(), SignatureContractKitError> {
        if current_column >= proposed_column {
            return Ok(());
        }
        let remove = proposed_column - current_column;
        for (index, line) in source.split_inclusive('\n').enumerate() {
            let adjust = adjust_first_line || index > 0;
            if adjust {
                if line.get(remove..).is_none() {
                    return Err(SignatureContractKitError::unsupported_lossless_edit(
                        location,
                        "generated YAML indentation cannot be rebased to the retained sequence",
                    ));
                }
                if !line.as_bytes()[..remove].iter().all(|byte| *byte == b' ') {
                    return Err(SignatureContractKitError::unsupported_lossless_edit(
                        location,
                        "generated YAML indentation cannot be rebased to the retained sequence",
                    ));
                }
            }
        }
        Ok(())
    }

    fn write_reindented_yaml_node(
        &self,
        source: &str,
        current_column: usize,
        proposed_column: usize,
        adjust_first_line: bool,
        output: &mut ScratchWriter<'_, '_>,
    ) -> std::io::Result<()> {
        const SPACES: [u8; 64] = [b' '; 64];
        for (index, line) in source.split_inclusive('\n').enumerate() {
            let adjust = adjust_first_line || index > 0;
            if adjust && current_column >= proposed_column {
                let mut remaining = current_column - proposed_column;
                while remaining > 0 {
                    let length = remaining.min(SPACES.len());
                    std::io::Write::write_all(output, &SPACES[..length])?;
                    remaining -= length;
                }
                std::io::Write::write_all(output, line.as_bytes())?;
            } else if adjust {
                let remove = proposed_column - current_column;
                let trimmed = line.get(remove..).ok_or_else(|| {
                    std::io::Error::other("generated YAML indentation changed after validation")
                })?;
                std::io::Write::write_all(output, trimmed.as_bytes())?;
            } else {
                std::io::Write::write_all(output, line.as_bytes())?;
            }
        }
        Ok(())
    }

    fn reindent_yaml_node<'meter, 'limits>(
        &self,
        source: &str,
        current_column: usize,
        proposed_column: usize,
        adjust_first_line: bool,
        location: &RustYamlDocumentLocation,
        output: &'meter GeneratedOutputMeter<'limits>,
    ) -> Result<ScratchText<'meter, 'limits>, SignatureContractKitError> {
        self.validate_reindent_yaml_node(
            source,
            current_column,
            proposed_column,
            adjust_first_line,
            location,
        )?;
        let mut writer = output.scratch_writer(location.catalog_name())?;
        let rendering = self.write_reindented_yaml_node(
            source,
            current_column,
            proposed_column,
            adjust_first_line,
            &mut writer,
        );
        writer.finish_text(rendering)
    }

    fn collect_sketch_removal_edits<'meter, 'limits>(
        &self,
        current: &yaml_edit::Document,
        proposal: &RustYamlGeneratedDocument,
        edits: &mut Vec<RustYamlTextEdit<'meter, 'limits>>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (original_document, _) = proposal.existing_origin()?;
        let current_sketches = self.retained_sketches(current).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable sketches sequence",
            )
        })?;
        if current_sketches.len() != original_document.sketches.len() {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "sketch sequence does not align with its typed semantic document",
            ));
        }
        let retained = proposal
            .proposed_document
            .sketches
            .iter()
            .map(|sketch| sketch.id.as_str())
            .collect::<BTreeSet<_>>();
        for (index, sketch) in original_document.sketches.iter().enumerate() {
            cancellation.checkpoint()?;
            if retained.contains(sketch.id.as_str()) {
                continue;
            }
            let node = current_sketches.get(index).ok_or_else(|| {
                SignatureContractKitError::unsupported_lossless_edit(
                    &proposal.location,
                    format!("sketch node {index} is unavailable"),
                )
            })?;
            edits.push(self.remove_sequence_entry_edit(
                &node,
                &proposal.location,
                "sketch",
                index,
                cancellation,
            )?);
        }
        Ok(())
    }
}

impl<'editor, 'document, 'meter, 'limits, 'operation>
    RustYamlEditContext<'editor, 'document, 'meter, 'limits, 'operation>
{
    fn collect_sketch_edits(
        &self,
        proposed: &RustYamlSectionPreview<'meter, 'limits>,
        edits: &mut Vec<RustYamlTextEdit<'meter, 'limits>>,
    ) -> Result<(), SignatureContractKitError> {
        let current = self.current;
        let proposal = self.proposal;
        let original_document = self.original_document;
        let cancellation = self.cancellation;
        cancellation.checkpoint()?;
        if original_document.sketches == proposal.proposed_document.sketches {
            return Ok(());
        }

        let current_sketches = self.editor.retained_sketches(current).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "contract document has no editable sketches sequence",
            )
        })?;
        if current_sketches.len() != original_document.sketches.len() {
            return Err(SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "sketch sequence does not align with its typed semantic document",
            ));
        }
        let proposed_sketches = proposed.document.get_sequence("sketches").ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                &proposal.location,
                "generated document has no sketches sequence",
            )
        })?;
        if proposal.proposed_document.sketches.is_empty() {
            edits.push(self.replace_sequence_edit(
                &current_sketches,
                &proposed_sketches,
                proposed.source.as_str(),
                RustYamlPreviewKind::Sketch,
            )?);
            return Ok(());
        }

        self.editor
            .collect_sketch_removal_edits(current, proposal, edits, cancellation)
    }
}

impl RustYamlLineEnding {
    fn from_document_source(
        source: &str,
        bounds: &std::ops::Range<usize>,
        location: &RustYamlDocumentLocation,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let document = source.get(bounds.clone()).ok_or_else(|| {
            SignatureContractKitError::unsupported_lossless_edit(
                location,
                "physical YAML document range lies outside the retained source",
            )
        })?;
        let bytes = document.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if index % (64 * 1024) == 0 {
                cancellation.checkpoint()?;
            }
            match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => return Ok(Self::CrLf),
                b'\r' => return Ok(Self::Cr),
                b'\n' => return Ok(Self::Lf),
                _ => index += 1,
            }
        }
        cancellation.checkpoint()?;
        Ok(Self::Lf)
    }

    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::Cr => b"\r",
        }
    }

    fn write_generated(self, value: &str, output: &mut ReturnedOutputBuffer) -> std::fmt::Result {
        const CHECKPOINT_BYTES: usize = 64 * 1024;

        let bytes = value.as_bytes();
        let mut segment_start = 0;
        let mut index = 0;
        let mut scanned_since_checkpoint = 0_usize;
        output.checkpoint_format()?;
        while index < bytes.len() {
            let (consumed, line_break) =
                if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                    (2, true)
                } else if bytes[index] == b'\n' {
                    (1, true)
                } else {
                    (1, false)
                };
            if scanned_since_checkpoint.saturating_add(consumed) > CHECKPOINT_BYTES {
                output.checkpoint_format()?;
                scanned_since_checkpoint = 0;
            }
            scanned_since_checkpoint = scanned_since_checkpoint.saturating_add(consumed);
            if !line_break {
                index += 1;
                continue;
            }

            std::io::Write::write_all(output, &bytes[segment_start..index])
                .map_err(|_| std::fmt::Error)?;
            std::io::Write::write_all(output, self.as_bytes()).map_err(|_| std::fmt::Error)?;
            index += consumed;
            segment_start = index;
        }
        std::io::Write::write_all(output, &bytes[segment_start..]).map_err(|_| std::fmt::Error)
    }
}
