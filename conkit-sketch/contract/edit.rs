mod scalar;

use self::scalar::{SketchCodeNode, SketchLineEnding};
use super::document::{
    SketchContractDocument, SketchContractDocuments, SketchContractFile, SketchSemanticDocument,
};
use crate::api::{GenerateResponse, SketchGenerationCounts};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::generate::SketchRefreshSeeds;
use crate::limits::{GeneratedBytes, ScratchText, SketchLimits};
use crate::work::CancellationProbe;
use std::collections::BTreeSet;
use std::str::FromStr;
use yaml_edit::AsYaml;

impl SketchContractDocuments {
    pub(crate) fn refresh(
        self,
        seeds: SketchRefreshSeeds,
        mut counts: SketchGenerationCounts,
        limits: &SketchLimits,
        yaml_budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        let expected_refresh_count = seeds.len();
        let Self { files, passthrough } = self;
        let mut generated = GeneratedBytes::new(&limits.output, cancellation);
        let mut output = FileCatalog::new();

        for (path, bytes) in passthrough.into_entries() {
            cancellation.checkpoint()?;
            generated.record(&path, bytes.len())?;
            output.insert(path, bytes)?;
        }

        for (path, file) in files {
            cancellation.checkpoint()?;
            let bytes = if seeds.targets_file(&path) {
                file.refresh(&seeds, &mut counts, yaml_budget, cancellation, &generated)?
            } else {
                file.original_bytes
            };
            generated.record(&path, bytes.len())?;
            output.insert(path, bytes)?;
        }

        if counts.refreshed_sketch_count != expected_refresh_count {
            return Err(SketchContractKitError::conversion_failed(format!(
                "refreshed {} sketches but expected {expected_refresh_count}",
                counts.refreshed_sketch_count
            )));
        }

        Ok(GenerateResponse {
            contract_files: output,
            counts,
        })
    }
}

impl SketchContractFile {
    fn refresh(
        self,
        seeds: &SketchRefreshSeeds,
        counts: &mut SketchGenerationCounts,
        yaml_budget: &mut crate::limits::YamlBudget<'_>,
        cancellation: &CancellationProbe,
        output: &GeneratedBytes<'_>,
    ) -> Result<Vec<u8>, SketchContractKitError> {
        let Self {
            catalog_name,
            original_bytes,
            documents,
        } = self;
        let mut changed_documents = BTreeSet::new();

        let rendered = {
            let mut edits = Vec::new();
            for (document_offset, document) in documents.iter().enumerate() {
                cancellation.checkpoint_at(document_offset)?;
                for (sketch_index, sketch) in document.semantic.sketches.iter().enumerate() {
                    cancellation.checkpoint_at(sketch_index)?;
                    if let Some(code) = seeds.code_for(&sketch.id) {
                        let changed = code != sketch.code;
                        counts.record_refreshed(changed);
                        if changed {
                            changed_documents.insert(document.index);
                            edits.push(SketchCodeEdit {
                                document_index: document.index,
                                sketch_index,
                                sketch_id: &sketch.id,
                                current: code,
                            });
                        }
                    }
                }
            }

            if edits.is_empty() {
                return Ok(original_bytes);
            }

            let source = std::str::from_utf8(&original_bytes).map_err(|error| {
                SketchContractKitError::parse_failed(
                    &catalog_name,
                    format!("input is not valid UTF-8: {error}"),
                )
            })?;
            SketchYamlEditor::new(&catalog_name, source).apply(&edits, output, cancellation)
        }?;

        cancellation.checkpoint()?;
        let reparsed = match Self::parse(catalog_name.clone(), rendered, yaml_budget, cancellation)
        {
            Ok(reparsed) => reparsed,
            Err(error) if error.limit_exceeded().is_some() || error.is_operation_cancelled() => {
                return Err(error);
            }
            Err(error) => {
                return Err(SketchContractKitError::unsupported_lossless_edit(
                    &catalog_name,
                    format!("edited YAML failed semantic reparse: {error}"),
                ));
            }
        };
        let mut semantics_match = reparsed.documents.len() == documents.len();
        if semantics_match {
            for (index, (current, previous)) in
                reparsed.documents.iter().zip(&documents).enumerate()
            {
                cancellation.checkpoint_at(index)?;
                if !current.matches_refresh(previous, seeds, cancellation)? {
                    semantics_match = false;
                    break;
                }
            }
        }
        if !semantics_match {
            return Err(SketchContractKitError::yaml_semantic_mismatch(catalog_name));
        }

        for _ in changed_documents {
            counts.record_changed_document();
        }

        Ok(reparsed.original_bytes)
    }
}

impl SketchContractDocument {
    fn matches_refresh(
        &self,
        previous: &Self,
        seeds: &SketchRefreshSeeds,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        if self.index != previous.index {
            return Ok(false);
        }
        self.semantic
            .matches_refresh(&previous.semantic, seeds, cancellation)
    }
}

impl SketchSemanticDocument {
    fn matches_refresh(
        &self,
        previous: &Self,
        seeds: &SketchRefreshSeeds,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        if self.root != previous.root
            || self.files != previous.files
            || self.signatures != previous.signatures
            || self.sketches.len() != previous.sketches.len()
        {
            return Ok(false);
        }

        for (index, (current, previous)) in self.sketches.iter().zip(&previous.sketches).enumerate()
        {
            cancellation.checkpoint_at(index)?;
            if !current.matches_refresh(previous, seeds.code_for(&previous.id)) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

struct SketchYamlEditor<'a> {
    catalog_name: &'a CatalogPath,
    source: &'a str,
}

struct ScalarEditor<'edit, 'seed> {
    range: std::ops::Range<usize>,
    destination_mapping_column: usize,
    flow_context: bool,
    original: SketchCodeNode,
    line_ending: SketchLineEnding,
    edit: &'edit SketchCodeEdit<'seed>,
}

struct VerifiedEditSet<'edit, 'seed> {
    targets: Vec<ScalarEditor<'edit, 'seed>>,
}

impl<'edit, 'seed> ScalarEditor<'edit, 'seed> {
    fn render<'meter, 'limits>(
        &self,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        let suffix = editor.source.get(self.range.end..).ok_or_else(|| {
            SketchContractKitError::unsupported_lossless_edit(
                editor.catalog_name,
                format!(
                    "sketch {} code range lies outside its YAML source",
                    self.edit.sketch_id
                ),
            )
        })?;
        self.render_scalar(suffix, editor, output, cancellation)
    }
}

impl<'a> SketchYamlEditor<'a> {
    fn new(catalog_name: &'a CatalogPath, source: &'a str) -> Self {
        Self {
            catalog_name,
            source,
        }
    }

    fn apply(
        &self,
        edits: &[SketchCodeEdit],
        output: &GeneratedBytes<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<u8>, SketchContractKitError> {
        cancellation.checkpoint()?;
        let syntax = yaml_edit::YamlFile::from_str(self.source).map_err(|error| {
            SketchContractKitError::parse_failed(self.catalog_name, error.to_string())
        })?;
        cancellation.checkpoint()?;
        let mut documents = Vec::new();
        for (index, document) in syntax.documents().enumerate() {
            cancellation.checkpoint_at(index)?;
            documents.push(document);
        }
        let targets = self.targets(edits, &documents, cancellation)?;
        VerifiedEditSet::new(targets, &documents, self, cancellation)?.render(
            self,
            output,
            cancellation,
        )
    }

    fn targets<'edit, 'seed>(
        &self,
        edits: &'edit [SketchCodeEdit<'seed>],
        documents: &[yaml_edit::Document],
        cancellation: &CancellationProbe,
    ) -> Result<Vec<ScalarEditor<'edit, 'seed>>, SketchContractKitError> {
        let mut targets = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let document = documents.get(edit.document_index).ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("missing concrete document {}", edit.document_index),
                )
            })?;
            let sketches = self.sketches(document, self.source).ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!(
                        "document {} sketches is not a sequence",
                        edit.document_index
                    ),
                )
            })?;
            let sketch = sketches.get(edit.sketch_index).ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!(
                        "document {} has no sketch at index {}",
                        edit.document_index, edit.sketch_index
                    ),
                )
            })?;
            let outer_mapping = sketch.as_mapping().ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("sketch {} is not a concrete mapping", edit.sketch_id),
                )
            })?;
            let body = outer_mapping
                .find_entry_by_key(edit.sketch_id)
                .and_then(|entry| entry.value_node())
                .and_then(|value| value.as_mapping().cloned())
                .ok_or_else(|| {
                    SketchContractKitError::unsupported_lossless_edit(
                        self.catalog_name,
                        format!("sketch {} has no concrete body mapping", edit.sketch_id),
                    )
                })?;
            let entry = body.find_entry_by_key("code").ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("sketch {} has no concrete code entry", edit.sketch_id),
                )
            })?;
            let value = entry.value_node().ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("sketch {} code has no concrete value", edit.sketch_id),
                )
            })?;
            if SketchCodeNode::edit_path_has_anchor(&value, cancellation)? {
                return Err(SketchContractKitError::anchored_sketch_code_mutation(
                    self.catalog_name,
                    edit.document_index,
                    edit.sketch_id,
                ));
            }
            if value.as_alias().is_some() {
                return Err(SketchContractKitError::aliased_sketch_code_mutation(
                    self.catalog_name,
                    edit.document_index,
                    edit.sketch_id,
                ));
            }
            let code = SketchCodeNode::from_yaml(value).ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("sketch {} code is not a scalar", edit.sketch_id),
                )
            })?;
            let range = code.editable_range(&body, self, cancellation)?;
            targets.push(ScalarEditor {
                range,
                destination_mapping_column: self.source_column(
                    self.source,
                    usize::try_from(body.byte_range().start).unwrap_or(usize::MAX),
                    cancellation,
                )?,
                flow_context: body.is_flow_style(),
                line_ending: SketchLineEnding::for_target(&code, document, cancellation)?,
                original: code,
                edit,
            });
        }
        Ok(targets)
    }

    fn sketches(
        &self,
        document: &yaml_edit::Document,
        source: &str,
    ) -> Option<yaml_edit::Sequence> {
        if let Some(sketches) = document.get_sequence("sketches") {
            return Some(sketches);
        }

        self.misnested_sketches(document, source)
    }

    fn misnested_sketches(
        &self,
        document: &yaml_edit::Document,
        source: &str,
    ) -> Option<yaml_edit::Sequence> {
        if document.get_sequence("sketches").is_some() {
            return None;
        }

        let signatures = document.get_sequence("signatures")?;
        let final_signature = signatures.last()?.as_mapping()?.clone();
        let sketches = final_signature.get_sequence("sketches")?;

        (sketches.start_position(source).column == 1).then_some(sketches)
    }

    fn mapping_entry_range(
        &self,
        mapping: &yaml_edit::Mapping,
        key: &str,
    ) -> Result<Option<std::ops::Range<usize>>, SketchContractKitError> {
        let Some(index) = mapping.find_entry_index_by_key(key) else {
            return Ok(None);
        };
        let node = mapping
            .as_node()
            .and_then(|mapping| mapping.children_with_tokens().nth(index))
            .and_then(|element| element.into_node())
            .ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    self.catalog_name,
                    format!("{key} has no concrete YAML mapping entry"),
                )
            })?;
        let range = node.text_range();
        Ok(Some(usize::from(range.start())..usize::from(range.end())))
    }

    fn source_column(
        &self,
        source: &str,
        offset: usize,
        cancellation: &CancellationProbe,
    ) -> Result<usize, SketchContractKitError> {
        let prefix = source.as_bytes().get(..offset).ok_or_else(|| {
            SketchContractKitError::unsupported_lossless_edit(
                self.catalog_name,
                "concrete YAML offset lies outside its source",
            )
        })?;
        let mut consumed = 0_usize;
        for chunk in prefix.rchunks(64 * 1024) {
            cancellation.checkpoint_at(consumed)?;
            if let Some(index) = chunk.iter().rposition(|byte| matches!(byte, b'\n' | b'\r')) {
                return Ok(consumed.saturating_add(chunk.len().saturating_sub(index + 1)));
            }
            consumed = consumed.saturating_add(chunk.len());
        }
        Ok(prefix.len())
    }
}

impl<'edit, 'seed> VerifiedEditSet<'edit, 'seed> {
    fn new(
        mut targets: Vec<ScalarEditor<'edit, 'seed>>,
        documents: &[yaml_edit::Document],
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        if targets.is_empty() {
            return Err(SketchContractKitError::unsupported_lossless_edit(
                editor.catalog_name,
                "verified sketch scalar edit set must not be empty",
            ));
        }
        targets.sort_by_key(|target| target.range.start);

        let mut cursor = 0;
        for (index, target) in targets.iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            if target.range.start < cursor
                || target.range.start >= target.range.end
                || target.range.end > editor.source.len()
                || !editor.source.is_char_boundary(target.range.start)
                || !editor.source.is_char_boundary(target.range.end)
            {
                return Err(SketchContractKitError::unsupported_lossless_edit(
                    editor.catalog_name,
                    "concrete sketch scalar edit ranges overlap or are invalid",
                ));
            }
            cursor = target.range.end;
        }

        for (document_index, document) in documents.iter().enumerate() {
            cancellation.checkpoint_at(document_index)?;
            let Some(mapping) = document.as_mapping() else {
                continue;
            };
            let extraction = editor.mapping_entry_range(&mapping, "extraction")?;
            let signatures = editor.mapping_entry_range(&mapping, "signatures")?;
            let (signature_leading, signature_trailing) = match (
                signatures,
                editor.misnested_sketches(document, editor.source),
            ) {
                (None, _) => (None, None),
                (Some(signatures), None) => (Some(signatures), None),
                (Some(signatures), Some(sketches)) => {
                    cancellation.checkpoint()?;
                    let sketches = sketches.byte_range();
                    let sketches = sketches.start as usize..sketches.end as usize;
                    if sketches.start < signatures.start || sketches.end > signatures.end {
                        return Err(SketchContractKitError::unsupported_lossless_edit(
                            editor.catalog_name,
                            "misnested sketches lie outside the concrete signatures section",
                        ));
                    }
                    (
                        (signatures.start < sketches.start)
                            .then_some(signatures.start..sketches.start),
                        (sketches.end < signatures.end).then_some(sketches.end..signatures.end),
                    )
                }
            };

            for (range_index, range) in [extraction, signature_leading, signature_trailing]
                .into_iter()
                .flatten()
                .enumerate()
            {
                cancellation.checkpoint_at(range_index)?;
                if range.start >= range.end
                    || range.end > editor.source.len()
                    || !editor.source.is_char_boundary(range.start)
                    || !editor.source.is_char_boundary(range.end)
                {
                    return Err(SketchContractKitError::unsupported_lossless_edit(
                        editor.catalog_name,
                        "signature-owned concrete YAML range lies outside its source",
                    ));
                }
                cancellation.checkpoint()?;
                let target_index =
                    targets.partition_point(|target| target.range.end <= range.start);
                cancellation.checkpoint_at(target_index)?;
                if targets.get(target_index).is_some_and(|target| {
                    target.range.start < range.end && range.start < target.range.end
                }) {
                    return Err(SketchContractKitError::unsupported_lossless_edit(
                        editor.catalog_name,
                        "concrete sketch scalar edit overlaps signature-owned YAML sections",
                    ));
                }
            }
        }

        Ok(Self { targets })
    }

    fn render(
        self,
        editor: &SketchYamlEditor<'_>,
        output: &GeneratedBytes<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<u8>, SketchContractKitError> {
        let mut rendered = output.returned_buffer(editor.catalog_name);
        let mut cursor = 0;
        for (index, target) in self.targets.into_iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let complement = editor
                .source
                .as_bytes()
                .get(cursor..target.range.start)
                .ok_or_else(|| {
                    SketchContractKitError::unsupported_lossless_edit(
                        editor.catalog_name,
                        "verified sketch scalar complement lies outside its YAML source",
                    )
                })?;
            if let Err(source) = std::io::Write::write_all(&mut rendered, complement) {
                return rendered.finish(Err(source));
            }

            {
                let replacement = target.render(editor, output, cancellation)?;
                if let Err(source) =
                    std::io::Write::write_all(&mut rendered, replacement.as_str().as_bytes())
                {
                    return rendered.finish(Err(source));
                }
            }
            cursor = target.range.end;
        }
        let trailing = editor.source.as_bytes().get(cursor..).ok_or_else(|| {
            SketchContractKitError::unsupported_lossless_edit(
                editor.catalog_name,
                "verified sketch scalar trailing complement lies outside its YAML source",
            )
        })?;
        let result = std::io::Write::write_all(&mut rendered, trailing);
        rendered.finish(result)
    }
}

struct SketchCodeEdit<'source> {
    document_index: usize,
    sketch_index: usize,
    sketch_id: &'source str,
    current: &'source str,
}

#[cfg(test)]
mod tests {
    use super::super::document::SketchContractFile;
    use super::{SketchCodeEdit, SketchYamlEditor, VerifiedEditSet};
    use crate::contract::tests::ContractYaml;
    use crate::files::CatalogPath;
    use crate::limits::{GeneratedBytes, SketchLimits};
    use crate::work::CancellationProbe;
    use std::str::FromStr;

    #[test]
    fn verified_edit_set_rejects_invalid_overlapping_and_cancelled_ranges() {
        let source = "# multibyte: é\nsketches:\n  - body:\n      code: stale\n";
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, source);
        let syntax = yaml_edit::YamlFile::from_str(source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "current",
        }];
        let cancellation = CancellationProbe::new();
        let multibyte_middle = source.find('é').expect("multibyte character") + 1;

        for (name, range) in [
            ("empty", 0..0),
            ("out of bounds", source.len()..source.len() + 1),
            ("non-UTF-8 boundary", multibyte_middle..multibyte_middle + 1),
        ] {
            let mut targets = editor
                .targets(&edits, &documents, &cancellation)
                .expect("scalar edit target");
            targets[0].range = range;
            let Err(error) = VerifiedEditSet::new(targets, &documents, &editor, &cancellation)
            else {
                panic!("{name} edit range must be rejected");
            };
            assert!(error.to_string().contains("ranges overlap or are invalid"));
        }

        let overlapping = [
            SketchCodeEdit {
                document_index: 0,
                sketch_index: 0,
                sketch_id: "body",
                current: "first",
            },
            SketchCodeEdit {
                document_index: 0,
                sketch_index: 0,
                sketch_id: "body",
                current: "second",
            },
        ];
        let limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: 0,
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let error = editor
            .apply(&overlapping, &output, &cancellation)
            .expect_err("overlapping ranges must fail before scratch output is opened");
        assert!(error.limit_exceeded().is_none());
        assert!(error.to_string().contains("ranges overlap or are invalid"));

        let targets = editor
            .targets(&edits, &documents, &cancellation)
            .expect("cancellable scalar edit target");
        cancellation.cancel();
        let Err(error) = VerifiedEditSet::new(targets, &documents, &editor, &cancellation) else {
            panic!("cancelled edit-set verification must stop");
        };
        assert!(error.is_operation_cancelled());
    }

    #[test]
    fn verified_edit_set_protects_extraction_and_signature_bytes() {
        let source = ContractYaml::linked("answer", "body", "function", "stale");
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, &source);
        let syntax = yaml_edit::YamlFile::from_str(&source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let mapping = documents[0].as_mapping().expect("root mapping");
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "current",
        }];
        let cancellation = CancellationProbe::new();

        VerifiedEditSet::new(
            editor
                .targets(&edits, &documents, &cancellation)
                .expect("valid scalar edit target"),
            &documents,
            &editor,
            &cancellation,
        )
        .expect("ordinary sketch target is outside protected ranges");

        for key in ["extraction", "signatures"] {
            let mut targets = editor
                .targets(&edits, &documents, &cancellation)
                .expect("scalar edit target");
            targets[0].range = editor
                .mapping_entry_range(&mapping, key)
                .expect("concrete section range")
                .expect("present section");
            let Err(error) = VerifiedEditSet::new(targets, &documents, &editor, &cancellation)
            else {
                panic!("{key} overlap must be rejected");
            };
            assert!(error.to_string().contains("signature-owned YAML sections"));
        }
    }

    #[test]
    fn verified_edit_set_finds_a_later_document_protected_overlap() {
        let first = ContractYaml::linked("first", "first_body", "function", "stale");
        let second = ContractYaml::linked("second", "second_body", "function", "stale");
        let source = format!("---\n{first}---\n{second}");
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, &source);
        let syntax = yaml_edit::YamlFile::from_str(&source).expect("two concrete documents");
        let documents = syntax.documents().collect::<Vec<_>>();
        let edits = [
            SketchCodeEdit {
                document_index: 1,
                sketch_index: 0,
                sketch_id: "second_body",
                current: "second current",
            },
            SketchCodeEdit {
                document_index: 0,
                sketch_index: 0,
                sketch_id: "first_body",
                current: "first current",
            },
        ];
        let cancellation = CancellationProbe::new();

        VerifiedEditSet::new(
            editor
                .targets(&edits, &documents, &cancellation)
                .expect("reverse-ordered targets"),
            &documents,
            &editor,
            &cancellation,
        )
        .expect("sorted disjoint targets remain outside protected ranges");

        let second_mapping = documents[1].as_mapping().expect("second root mapping");
        let protected = editor
            .mapping_entry_range(&second_mapping, "signatures")
            .expect("concrete second signatures range")
            .expect("second signatures section");
        let mut targets = editor
            .targets(&edits, &documents, &cancellation)
            .expect("reverse-ordered targets");
        let first_end = targets
            .iter()
            .find(|target| target.edit.document_index == 0)
            .expect("first target")
            .range
            .end;
        assert!(first_end <= protected.start);
        let second_target = targets
            .iter_mut()
            .find(|target| target.edit.document_index == 1)
            .expect("second target");
        second_target.range = protected;

        let Err(error) = VerifiedEditSet::new(targets, &documents, &editor, &cancellation) else {
            panic!("the later candidate overlap must be rejected");
        };
        assert!(error.to_string().contains("signature-owned YAML sections"));
    }

    #[test]
    fn misnested_signature_protection_excludes_only_the_sketch_sequence() {
        let source = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }\nsignatures:\n- answer:\n    file: lib.rs\n    signature_type: function\n    sketch: answer_body\nsketches:\n- answer_body:\n    file: lib.rs\n    signature: answer\n    signature_type: function\n    matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n    code: old code\n";
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let editor = SketchYamlEditor::new(&path, source);
        let syntax = yaml_edit::YamlFile::from_str(source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "answer_body",
            current: "replacement body",
        }];

        VerifiedEditSet::new(
            editor
                .targets(&edits, &documents, &cancellation)
                .expect("misnested sketch target"),
            &documents,
            &editor,
            &cancellation,
        )
        .expect("the exact misnested sketch sequence is editable");

        let mapping = documents[0].as_mapping().expect("root mapping");
        let signatures = editor
            .mapping_entry_range(&mapping, "signatures")
            .expect("concrete signatures range")
            .expect("signatures section");
        let mut targets = editor
            .targets(&edits, &documents, &cancellation)
            .expect("misnested sketch target");
        targets[0].range = signatures.start..signatures.start + 1;
        let Err(error) = VerifiedEditSet::new(targets, &documents, &editor, &cancellation) else {
            panic!("the signature-owned prefix must remain protected");
        };
        assert!(error.to_string().contains("signature-owned YAML sections"));
    }

    #[test]
    fn crlf_anchor_edit_preserves_all_other_semantics_across_documents() {
        let source = r#"---
# first physical document; all presentation choices are intentional
contract_version: !!int 2
root: >-
  ../src
files: [&source "lib.rs"]
extraction: {mode: rust_syntax_v2, profile: 'rust_api_v1', crates: [{id: "main", root: *source, kind: library}]}
signatures:
  # comment adjacent to the signature node
  - answer_function:
      crate_id: main
      file: *source
      signature_type: 'function'
      name: "answer"
      visibility: public
      parameters: []
      return_type: u8
      sketch: answer_body
sketches:
  - answer_body:
      file: *source
      signature: answer_function
      signature_type: function
      matching: {normalization: exact_lines_v1, occurrence: at_least_one}
      code: |-
        stale sketch body
# after-targeted-sketch
...
---
# second physical document must remain byte-exact when the first changes
contract_version: 2
root: '../src'
files: [other.rs]
extraction: {mode: rust_syntax_v2, profile: rust_api_v1, crates: [{id: other, root: other.rs, kind: library}]}
signatures:
  - other_function:
      crate_id: other
      file: other.rs
      signature_type: function
      name: other
      visibility: public
      parameters: []
sketches: []
...
"#
        .replace('\n', "\r\n");
        let path = CatalogPath::new("main.yml").expect("path");
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let mut previous_budget = limits.yaml_budget();
        let previous = SketchContractFile::parse(
            path.clone(),
            source.as_bytes().to_vec(),
            &mut previous_budget,
            &cancellation,
        )
        .expect("previous semantic document");
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let rendered = SketchYamlEditor::new(&path, &source)
            .apply(
                &[SketchCodeEdit {
                    document_index: 0,
                    sketch_index: 0,
                    sketch_id: "answer_body",
                    current: "pub fn answer() -> u8 { 42 }",
                }],
                &output,
                &cancellation,
            )
            .expect("lossless edit");
        let mut current_budget = limits.yaml_budget();
        let current = SketchContractFile::parse(path, rendered, &mut current_budget, &cancellation)
            .expect("current semantic document");
        let mut expected = previous.documents.clone();
        expected[0].semantic.sketches[0].code = "pub fn answer() -> u8 { 42 }".to_owned();

        assert_eq!(current.documents, expected);
    }

    #[test]
    fn verified_edit_render_copies_every_complement_byte_exactly() {
        let source = "# leading multibyte comment: é\nsketches:\n  - body:\n      code: stale\n# trailing presentation comment\n";
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, source);
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "first\nsecond",
        }];
        let syntax = yaml_edit::YamlFile::from_str(source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let range = editor
            .targets(&edits, &documents, &CancellationProbe::new())
            .expect("scalar edit target")[0]
            .range
            .clone();
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let rendered = editor
            .apply(&edits, &output, &cancellation)
            .expect("verified edit rendering");
        let removed = range.end - range.start;
        let replacement_length = rendered
            .len()
            .checked_add(removed)
            .and_then(|length| length.checked_sub(source.len()))
            .expect("replacement length");
        let replacement_end = range
            .start
            .checked_add(replacement_length)
            .expect("replacement end");

        assert_eq!(&rendered[..range.start], &source.as_bytes()[..range.start]);
        assert_eq!(
            &rendered[replacement_end..],
            &source.as_bytes()[range.end..]
        );
    }

    #[test]
    fn changed_targets_release_each_replacement_before_rendering_the_next() {
        let mut source = "sketches:\n".to_owned();
        let ids = (0..12)
            .map(|index| format!("body_{index}"))
            .collect::<Vec<_>>();
        for id in &ids {
            source.push_str(&format!("  - {id}:\n      code: stale\n"));
        }
        let codes = ids
            .iter()
            .map(|id| format!("{}-{id}", "x".repeat(512)))
            .collect::<Vec<_>>();
        let edits = ids
            .iter()
            .zip(&codes)
            .enumerate()
            .map(|(sketch_index, (id, code))| SketchCodeEdit {
                document_index: 0,
                sketch_index,
                sketch_id: id,
                current: code,
            })
            .collect::<Vec<_>>();
        let mut limits = SketchLimits::default();
        limits.output.scratch_bytes = 4 * 1024;
        let cancellation = CancellationProbe::new();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let path = CatalogPath::new("main.yml").expect("path");
        let rendered = SketchYamlEditor::new(&path, &source)
            .apply(&edits, &output, &cancellation)
            .expect("one-at-a-time replacement peak fits the scratch budget");
        let rendered = std::str::from_utf8(&rendered).expect("rendered UTF-8");

        assert!(
            codes.iter().map(String::len).sum::<usize>()
                > usize::try_from(limits.output.scratch_bytes).expect("scratch limit")
        );
        for code in &codes {
            assert!(rendered.contains(code));
        }
    }

    #[test]
    fn one_edit_pass_preserves_each_changed_documents_local_line_endings_in_either_order() {
        let lf_document = "---\nsketches:\n- lf_body:\n    code: stale\n";
        let crlf_document = "---\r\nsketches:\r\n- crlf_body:\r\n    code: stale\r\n";
        let source = format!("{lf_document}{crlf_document}");
        let path = CatalogPath::new("main.yml").expect("path");
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let forward = [
            SketchCodeEdit {
                document_index: 0,
                sketch_index: 0,
                sketch_id: "lf_body",
                current: "lf first\nlf second",
            },
            SketchCodeEdit {
                document_index: 1,
                sketch_index: 0,
                sketch_id: "crlf_body",
                current: "crlf first\ncrlf second",
            },
        ];
        let reverse = [
            SketchCodeEdit {
                document_index: 1,
                sketch_index: 0,
                sketch_id: "crlf_body",
                current: "crlf first\ncrlf second",
            },
            SketchCodeEdit {
                document_index: 0,
                sketch_index: 0,
                sketch_id: "lf_body",
                current: "lf first\nlf second",
            },
        ];

        let forward_output = GeneratedBytes::new(&limits.output, &cancellation);
        let forward_rendered = SketchYamlEditor::new(&path, &source)
            .apply(&forward, &forward_output, &cancellation)
            .expect("forward-order multi-document edit");
        let reverse_output = GeneratedBytes::new(&limits.output, &cancellation);
        let reverse_rendered = SketchYamlEditor::new(&path, &source)
            .apply(&reverse, &reverse_output, &cancellation)
            .expect("reverse-order multi-document edit");
        assert_eq!(reverse_rendered, forward_rendered);

        let rendered = std::str::from_utf8(&forward_rendered).expect("rendered UTF-8");
        let (lf, crlf) = rendered
            .split_once("---\r\n")
            .expect("CRLF second document marker");
        assert!(!lf.contains('\r'));
        assert!(lf.contains("lf first\n"));
        assert!(lf.contains("lf second\n"));
        for (index, byte) in crlf.as_bytes().iter().enumerate() {
            if *byte == b'\n' {
                assert!(index > 0 && crlf.as_bytes()[index - 1] == b'\r');
            }
        }
        assert!(crlf.contains("crlf first\r\n"));
        assert!(crlf.contains("crlf second\r\n"));
    }

    #[test]
    fn changed_multiline_scalar_preserves_cr_only_document_breaks() {
        let source = "sketches:\r- body:\r    code: stale\r";
        let path = CatalogPath::new("main.yml").expect("path");
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let rendered = SketchYamlEditor::new(&path, source)
            .apply(
                &[SketchCodeEdit {
                    document_index: 0,
                    sketch_index: 0,
                    sketch_id: "body",
                    current: "first\nsecond",
                }],
                &output,
                &cancellation,
            )
            .expect("CR-only scalar edit");
        let rendered = std::str::from_utf8(&rendered).expect("rendered UTF-8");

        assert!(!rendered.contains('\n'));
        assert!(
            rendered.contains("code: |-\r      first\r      second"),
            "{rendered:?}",
        );
        yaml_edit::YamlFile::from_str(rendered).expect("rendered CR-only YAML reparses");
    }
}
