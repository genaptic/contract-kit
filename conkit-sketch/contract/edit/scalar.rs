use super::{ScalarEditor, SketchYamlEditor};
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::limits::{GeneratedBytes, ScratchText};
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};
use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};
use std::str::FromStr;
use yaml_edit::AsYaml;

pub(super) struct SketchCodeNode {
    scalar: yaml_edit::Scalar,
    tag: Option<String>,
}

impl SketchCodeNode {
    pub(super) fn from_yaml(value: yaml_edit::YamlNode) -> Option<Self> {
        match value {
            yaml_edit::YamlNode::Scalar(scalar) => Some(Self { scalar, tag: None }),
            yaml_edit::YamlNode::TaggedNode(tagged) => {
                let tag = tagged.tag()?;
                Some(Self {
                    scalar: tagged.value()?,
                    tag: Some(tag),
                })
            }
            yaml_edit::YamlNode::Mapping(_)
            | yaml_edit::YamlNode::Sequence(_)
            | yaml_edit::YamlNode::Alias(_) => None,
        }
    }

    pub(super) fn edit_path_has_anchor(
        value: &yaml_edit::YamlNode,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        cancellation.checkpoint()?;
        let Some(node) = value.as_node() else {
            return Ok(false);
        };

        for (index, ancestor) in node.ancestors().enumerate() {
            cancellation.checkpoint_at(index)?;
            let kind = ancestor.kind();
            let property_wrapper = matches!(
                kind,
                yaml_edit::SyntaxKind::VALUE
                    | yaml_edit::SyntaxKind::SEQUENCE_ENTRY
                    | yaml_edit::SyntaxKind::TAGGED_NODE
                    | yaml_edit::SyntaxKind::DOCUMENT
            );
            if property_wrapper {
                for (token_index, token) in ancestor
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .enumerate()
                {
                    cancellation.checkpoint_at(token_index)?;
                    for _ in token.text().as_bytes().chunks(64 * 1024) {
                        cancellation.checkpoint()?;
                    }
                    if token.kind() == yaml_edit::SyntaxKind::ANCHOR {
                        return Ok(true);
                    }
                }
            }
            if kind == yaml_edit::SyntaxKind::DOCUMENT {
                break;
            }
        }

        Ok(false)
    }

    pub(super) fn editable_range(
        &self,
        body: &yaml_edit::Mapping,
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<std::ops::Range<usize>, SketchContractKitError> {
        cancellation.checkpoint()?;
        let range = self.scalar.byte_range();
        let start = range.start as usize;
        let mut end = range.end as usize;
        if matches!(self.scalar.value().as_bytes().first(), Some(b'|' | b'>')) {
            let body_column = editor.source_column(
                editor.source,
                usize::try_from(body.byte_range().start).unwrap_or(usize::MAX),
                cancellation,
            )?;
            let node = self.scalar.as_node().ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    editor.catalog_name,
                    "block scalar has no concrete syntax node",
                )
            })?;
            let mut past_header = false;
            for (index, element) in node.descendants_with_tokens().enumerate() {
                cancellation.checkpoint_at(index)?;
                let Some(token) = element.into_token() else {
                    continue;
                };
                if !past_header {
                    past_header = token.kind() == yaml_edit::SyntaxKind::NEWLINE;
                    continue;
                }
                if token.kind() != yaml_edit::SyntaxKind::COMMENT {
                    continue;
                }
                let token_start = usize::from(token.text_range().start());
                if yaml_edit::byte_offset_to_line_column(editor.source, token_start).column
                    <= body_column
                {
                    end = end.min(token_start);
                    break;
                }
            }
        }

        Ok(start..end)
    }
}

struct ScalarCandidate<'meter, 'limits> {
    source: ScratchText<'meter, 'limits>,
    node: yaml_edit::YamlNode,
    source_mapping_column: usize,
}

impl<'edit, 'seed> ScalarEditor<'edit, 'seed> {
    pub(super) fn render_scalar<'meter, 'limits>(
        &self,
        retained_suffix: &str,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        cancellation.checkpoint()?;
        let presentation =
            self.inspect_presentation(&self.original.scalar, editor.source, editor, cancellation)?;
        let single_quoted_control = matches!(
            &presentation,
            ScalarPresentation::Inline(InlineScalarStyle::SingleQuoted)
        ) && self.edit.current.chars().any(|character| {
            character.is_control() || matches!(character, '\u{FEFF}' | '\u{2028}' | '\u{2029}')
        });
        let has_header_comment = matches!(
            &presentation,
            ScalarPresentation::Block {
                details: BlockPresentation {
                    has_header_comment: true,
                    ..
                },
                ..
            }
        );

        let encoding = if single_quoted_control {
            ScalarEncoding::SafeFallback(self.safe_fallback_candidate(
                editor,
                output,
                cancellation,
            )?)
        } else {
            let serialized = match &presentation {
                ScalarPresentation::Inline(InlineScalarStyle::Plain) => {
                    self.serialize_codec(self.edit.current, false, editor, output, cancellation)
                }
                ScalarPresentation::Inline(InlineScalarStyle::SingleQuoted) => self
                    .serialize_codec(
                        serde_saphyr::SingleQuoted(self.edit.current),
                        false,
                        editor,
                        output,
                        cancellation,
                    ),
                ScalarPresentation::Inline(InlineScalarStyle::DoubleQuoted) => self
                    .serialize_codec(
                        serde_saphyr::DoubleQuoted(self.edit.current),
                        false,
                        editor,
                        output,
                        cancellation,
                    ),
                ScalarPresentation::Block {
                    style: BlockScalarStyle::Literal,
                    ..
                } => self.serialize_codec(
                    serde_saphyr::LitStr(self.edit.current),
                    false,
                    editor,
                    output,
                    cancellation,
                ),
                ScalarPresentation::Block {
                    style: BlockScalarStyle::Folded,
                    ..
                } => {
                    // Explicit folded wrappers emit a physical line for the terminal
                    // `split` sentinel but leave chomping selection to the caller. Remove
                    // one semantic line ending before restoring the required indicator so
                    // `>+` preserves exactly the requested number of trailing line breaks.
                    let folded = self
                        .edit
                        .current
                        .strip_suffix("\r\n")
                        .or_else(|| self.edit.current.strip_suffix('\n'))
                        .unwrap_or(self.edit.current);
                    self.serialize_codec(
                        serde_saphyr::FoldStr(folded),
                        true,
                        editor,
                        output,
                        cancellation,
                    )
                }
            }?;
            cancellation.checkpoint()?;
            let document = yaml_edit::Document::from_str(serialized.as_str())
                .map_err(|error| self.unsupported(editor, error.to_string()))?;
            let replacement = self
                .codec_code_node(&document)
                .and_then(|value| value.as_scalar().cloned())
                .ok_or_else(|| {
                    self.unsupported(
                        editor,
                        "semantic serializer did not produce the typed code scalar",
                    )
                })?;
            let scalar_source = self.render_presented_scalar(
                &presentation,
                &replacement,
                serialized.as_str(),
                editor,
                output,
                cancellation,
            )?;
            let range = replacement.byte_range();
            let replaced = self.replace_encoded_scalar(
                serialized.as_str(),
                range.start as usize..range.end as usize,
                scalar_source.as_str(),
                editor,
                output,
                cancellation,
            )?;
            drop(scalar_source);
            drop(serialized);
            let serialized = self.line_ending.convert(
                replaced.as_str(),
                editor.catalog_name,
                output,
                cancellation,
            )?;
            drop(replaced);

            ScalarEncoding::Preferred(self.preferred_candidate(serialized, editor, cancellation)?)
        };

        match encoding {
            ScalarEncoding::Preferred(candidate) => {
                let rendered = self.render_candidate(
                    candidate,
                    retained_suffix,
                    editor,
                    output,
                    cancellation,
                )?;
                let preferred_is_exact = match self.final_candidate_is_exact(
                    rendered.as_str(),
                    retained_suffix,
                    editor,
                    cancellation,
                ) {
                    Ok(exact) => exact,
                    Err(error)
                        if error.is_operation_cancelled() || error.limit_exceeded().is_some() =>
                    {
                        return Err(error);
                    }
                    Err(_) => false,
                };
                if preferred_is_exact {
                    return Ok(rendered);
                }
                drop(rendered);
                if has_header_comment {
                    return Err(self.unsupported(
                        editor,
                        "safe scalar fallback cannot preserve the block-header comment",
                    ));
                }
                let fallback = self.safe_fallback_candidate(editor, output, cancellation)?;
                let rendered =
                    self.render_candidate(fallback, retained_suffix, editor, output, cancellation)?;
                if !self.final_candidate_is_exact(
                    rendered.as_str(),
                    retained_suffix,
                    editor,
                    cancellation,
                )? {
                    return Err(self.unsupported(
                        editor,
                        "fallback scalar does not preserve the requested value",
                    ));
                }
                Ok(rendered)
            }
            ScalarEncoding::SafeFallback(candidate) => {
                let rendered = self.render_candidate(
                    candidate,
                    retained_suffix,
                    editor,
                    output,
                    cancellation,
                )?;
                if !self.final_candidate_is_exact(
                    rendered.as_str(),
                    retained_suffix,
                    editor,
                    cancellation,
                )? {
                    return Err(self.unsupported(
                        editor,
                        "fallback scalar does not preserve the requested value",
                    ));
                }
                Ok(rendered)
            }
        }
    }

    fn preferred_candidate<'meter, 'limits>(
        &self,
        source: ScratchText<'meter, 'limits>,
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<ScalarCandidate<'meter, 'limits>, SketchContractKitError> {
        cancellation.checkpoint()?;
        let document = yaml_edit::Document::from_str(source.as_str())
            .map_err(|error| self.unsupported(editor, error.to_string()))?;
        let node = self.codec_code_node(&document).ok_or_else(|| {
            self.unsupported(
                editor,
                "semantic serializer did not produce the typed code node",
            )
        })?;
        SketchCodeNode::from_yaml(node.clone()).ok_or_else(|| {
            self.unsupported(editor, "semantic serializer did not produce scalar code")
        })?;
        drop(document);
        self.candidate_from_node(node, source, editor, cancellation)
    }

    fn safe_fallback_candidate<'meter, 'limits>(
        &self,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScalarCandidate<'meter, 'limits>, SketchContractKitError> {
        let serialized = self.serialize_codec(
            serde_saphyr::DoubleQuoted(self.edit.current),
            false,
            editor,
            output,
            cancellation,
        )?;
        let converted = self.line_ending.convert(
            serialized.as_str(),
            editor.catalog_name,
            output,
            cancellation,
        )?;
        drop(serialized);
        cancellation.checkpoint()?;
        let document = yaml_edit::Document::from_str(converted.as_str())
            .map_err(|error| self.unsupported(editor, error.to_string()))?;
        let scalar = self
            .codec_code_node(&document)
            .and_then(|value| value.as_scalar().cloned())
            .ok_or_else(|| {
                self.unsupported(editor, "fallback serializer did not produce scalar code")
            })?;
        drop(document);

        if self.original.tag.is_some() {
            let range = scalar.byte_range();
            let scalar_source = converted
                .as_str()
                .get(range.start as usize..range.end as usize)
                .ok_or_else(|| {
                    self.unsupported(
                        editor,
                        "fallback serializer returned an invalid scalar range",
                    )
                })?;
            let tagged = self.replace_encoded_scalar(
                converted.as_str(),
                range.start as usize..range.end as usize,
                scalar_source,
                editor,
                output,
                cancellation,
            )?;
            drop(converted);
            cancellation.checkpoint()?;
            let document = yaml_edit::Document::from_str(tagged.as_str())
                .map_err(|error| self.unsupported(editor, error.to_string()))?;
            let node = self.codec_code_node(&document).ok_or_else(|| {
                self.unsupported(editor, "fallback serializer did not preserve tagged code")
            })?;
            SketchCodeNode::from_yaml(node.clone()).ok_or_else(|| {
                self.unsupported(
                    editor,
                    "fallback serializer did not produce tagged scalar code",
                )
            })?;
            drop(document);
            self.candidate_from_node(node, tagged, editor, cancellation)
        } else {
            self.candidate_from_node(
                yaml_edit::YamlNode::Scalar(scalar),
                converted,
                editor,
                cancellation,
            )
        }
    }

    fn candidate_from_node<'meter, 'limits>(
        &self,
        node: yaml_edit::YamlNode,
        source: ScratchText<'meter, 'limits>,
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<ScalarCandidate<'meter, 'limits>, SketchContractKitError> {
        let syntax = node.as_node().ok_or_else(|| {
            self.unsupported(editor, "serialized code has no concrete syntax node")
        })?;
        let entry = syntax
            .ancestors()
            .find(|ancestor| ancestor.kind() == yaml_edit::SyntaxKind::MAPPING_ENTRY)
            .ok_or_else(|| {
                self.unsupported(editor, "serialized code has no owning mapping entry")
            })?;
        let entry_start = usize::from(entry.text_range().start());
        if entry_start > source.as_str().len() {
            return Err(self.unsupported(
                editor,
                "serialized code mapping lies outside its YAML source",
            ));
        }
        let source_mapping_column =
            editor.source_column(source.as_str(), entry_start, cancellation)?;

        Ok(ScalarCandidate {
            source,
            node,
            source_mapping_column,
        })
    }

    fn render_candidate<'meter, 'limits>(
        &self,
        candidate: ScalarCandidate<'meter, 'limits>,
        retained_suffix: &str,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        let ScalarCandidate {
            source,
            node,
            source_mapping_column,
        } = candidate;
        let scalar = match node {
            yaml_edit::YamlNode::Scalar(scalar) => scalar,
            yaml_edit::YamlNode::TaggedNode(tagged) => tagged.value().ok_or_else(|| {
                SketchContractKitError::unsupported_lossless_edit(
                    editor.catalog_name,
                    "tagged sketch code has no scalar value",
                )
            })?,
            yaml_edit::YamlNode::Mapping(_)
            | yaml_edit::YamlNode::Sequence(_)
            | yaml_edit::YamlNode::Alias(_) => {
                return Err(SketchContractKitError::unsupported_lossless_edit(
                    editor.catalog_name,
                    "sketch code replacement is not a scalar",
                ));
            }
        };
        drop(source);
        let mut rendered = if matches!(scalar.value().as_bytes().first(), Some(b'|' | b'>')) {
            self.render_rebased_block_scalar(
                &scalar,
                source_mapping_column,
                editor,
                output,
                cancellation,
            )?
        } else {
            self.serialize_concrete_scalar(&scalar, editor, output, cancellation)?
        };
        self.remove_duplicated_boundary_line_ending(&mut rendered, retained_suffix, editor)?;
        Ok(rendered)
    }

    fn final_candidate_is_exact(
        &self,
        source: &str,
        retained_suffix: &str,
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        cancellation.checkpoint()?;
        const WRAPPER_PREFIX: &str = "sketches:\n- sketch:\n";
        const WRAPPER_KEY: &str = "code: ";

        let boundary = SketchLineEnding::leading_length(retained_suffix)
            .and_then(|length| retained_suffix.get(..length))
            .unwrap_or_default();
        let mapping_column = if self.flow_context {
            4
        } else {
            self.destination_mapping_column
        };
        let mut document_count = 0_usize;
        let mut document_end_count = 0_usize;
        let mut mapping_count = 0_usize;
        let mut mapping_end_count = 0_usize;
        let mut sequence_count = 0_usize;
        let mut sequence_end_count = 0_usize;
        let mut scalar_count = 0_usize;
        let wrapper = WRAPPER_PREFIX
            .chars()
            .chain(std::iter::repeat_n(' ', mapping_column))
            .chain(WRAPPER_KEY.chars())
            .chain(source.chars())
            .chain(boundary.chars());
        for (index, event) in Parser::new_from_iter(wrapper).enumerate() {
            cancellation.checkpoint_at(index)?;
            let (event, _) = event.map_err(|error| {
                if error.to_string().contains("unknown anchor") {
                    return self.unsupported(editor, "rendered scalar is an alias");
                }
                self.unsupported(
                    editor,
                    format!("rendered scalar failed concrete-syntax validation: {error}"),
                )
            })?;
            if event.anchor_id().is_some() {
                return Err(self.unsupported(editor, "rendered scalar contains an anchor"));
            }
            match event {
                Event::DocumentStart(_, _) => {
                    document_count = document_count.saturating_add(1);
                    if document_count > 1 {
                        return Err(self.unsupported(
                            editor,
                            "rendered scalar contains multiple YAML documents",
                        ));
                    }
                }
                Event::Alias(_) => {
                    return Err(self.unsupported(editor, "rendered scalar is an alias"));
                }
                Event::MappingStart(_, _, _) => {
                    mapping_count = mapping_count.saturating_add(1);
                    if mapping_count > 3 {
                        return Err(self.unsupported(editor, "rendered scalar is a YAML container"));
                    }
                }
                Event::SequenceStart(_, _, _) => {
                    sequence_count = sequence_count.saturating_add(1);
                    if sequence_count > 1 {
                        return Err(self.unsupported(editor, "rendered scalar is a YAML container"));
                    }
                }
                Event::Scalar(value, style, _, _) => {
                    scalar_count = scalar_count.saturating_add(1);
                    match scalar_count {
                        1 if value.as_ref() == "sketches" => {}
                        2 if value.as_ref() == "sketch" => {}
                        3 if value.as_ref() == "code" => {}
                        4 => {
                            if self.flow_context
                                && !self.flow_scalar_is_closed(
                                    value.as_ref(),
                                    style,
                                    cancellation,
                                )?
                            {
                                return Err(self.unsupported(
                                    editor,
                                    "rendered scalar is not closed in its destination flow context",
                                ));
                            }
                        }
                        1..=3 => {
                            return Err(self.unsupported(
                                editor,
                                "rendered scalar escaped its destination wrapper",
                            ));
                        }
                        _ => {
                            return Err(self.unsupported(
                                editor,
                                "rendered scalar document contains multiple YAML values",
                            ));
                        }
                    }
                }
                Event::DocumentEnd => {
                    document_end_count = document_end_count.saturating_add(1);
                }
                Event::MappingEnd => {
                    mapping_end_count = mapping_end_count.saturating_add(1);
                }
                Event::SequenceEnd => {
                    sequence_end_count = sequence_end_count.saturating_add(1);
                }
                Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::Comment(_, _) => {}
            }
        }
        if document_count != 1
            || document_end_count != 1
            || mapping_count != 3
            || mapping_end_count != 3
            || sequence_count != 1
            || sequence_end_count != 1
            || scalar_count != 4
        {
            return Err(self.unsupported(
                editor,
                "rendered scalar did not preserve its destination wrapper",
            ));
        }

        cancellation.checkpoint()?;
        let prefix = std::io::Cursor::new(WRAPPER_PREFIX.as_bytes());
        let indentation = std::io::Read::take(
            std::io::repeat(b' '),
            u64::try_from(mapping_column).unwrap_or(u64::MAX),
        );
        let key = std::io::Cursor::new(WRAPPER_KEY.as_bytes());
        let scalar = std::io::Cursor::new(source.as_bytes());
        let boundary = std::io::Cursor::new(boundary.as_bytes());
        let reader = std::io::Read::chain(prefix, indentation);
        let reader = std::io::Read::chain(reader, key);
        let reader = std::io::Read::chain(reader, scalar);
        let reader = std::io::Read::chain(reader, boundary);
        let semantic = serde_saphyr::from_reader::<_, ScalarCodecDocument<String>>(reader)
            .map_err(|error| {
                self.unsupported(
                    editor,
                    format!("rendered scalar failed exact semantic validation: {error}"),
                )
            })?;
        cancellation.checkpoint()?;
        let [entry] = semantic.sketches;
        Ok(entry.sketch.code == self.edit.current)
    }

    fn flow_scalar_is_closed(
        &self,
        value: &str,
        style: ScalarStyle,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        match style {
            ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted => return Ok(true),
            ScalarStyle::Literal | ScalarStyle::Folded => return Ok(false),
            ScalarStyle::Plain => {}
        }
        let bytes = value.as_bytes();
        for (index, byte) in bytes.iter().copied().enumerate() {
            cancellation.checkpoint_at(index)?;
            if matches!(byte, b'[' | b']' | b'{' | b'}' | b',') {
                return Ok(false);
            }
            if byte == b':'
                && bytes.get(index.saturating_add(1)).is_none_or(|next| {
                    next.is_ascii_whitespace() || matches!(*next, b'[' | b']' | b'{' | b'}' | b',')
                })
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn render_rebased_block_scalar<'meter, 'limits>(
        &self,
        scalar: &yaml_edit::Scalar,
        source_mapping_column: usize,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        let indentation_delta = isize::try_from(self.destination_mapping_column)
            .unwrap_or(isize::MAX)
            .saturating_sub(isize::try_from(source_mapping_column).unwrap_or(isize::MAX));
        let node = scalar.as_node().ok_or_else(|| {
            SketchContractKitError::unsupported_lossless_edit(
                editor.catalog_name,
                "serialized block scalar has no concrete syntax node",
            )
        })?;
        let mut rendered = output.scratch_writer(editor.catalog_name)?;
        let mut rendering = Ok(());
        let mut past_header = false;
        let mut at_line_start = false;

        for (index, element) in node.descendants_with_tokens().enumerate() {
            cancellation.checkpoint_at(index)?;
            let Some(token) = element.into_token() else {
                continue;
            };
            match token.kind() {
                yaml_edit::SyntaxKind::NEWLINE => {
                    rendering = rendering
                        .and_then(|()| std::fmt::Write::write_str(&mut rendered, token.text()));
                    past_header = true;
                    at_line_start = true;
                }
                yaml_edit::SyntaxKind::INDENT if past_header && at_line_start => {
                    let current = isize::try_from(token.text().len()).unwrap_or(isize::MAX);
                    let adjusted = current.saturating_add(indentation_delta);
                    if adjusted <= 0 || !token.text().as_bytes().iter().all(|byte| *byte == b' ') {
                        return Err(SketchContractKitError::unsupported_lossless_edit(
                            editor.catalog_name,
                            "serialized block scalar indentation cannot be rebased",
                        ));
                    }
                    for indentation_index in 0..usize::try_from(adjusted).unwrap_or(usize::MAX) {
                        cancellation.checkpoint_at(indentation_index)?;
                        rendering = rendering
                            .and_then(|()| std::fmt::Write::write_char(&mut rendered, ' '));
                    }
                    at_line_start = false;
                }
                _ => {
                    rendering = rendering
                        .and_then(|()| std::fmt::Write::write_str(&mut rendered, token.text()));
                    at_line_start = false;
                }
            }
        }

        rendered.finish_text(rendering)
    }

    fn remove_duplicated_boundary_line_ending(
        &self,
        rendered: &mut ScratchText<'_, '_>,
        retained_suffix: &str,
        editor: &SketchYamlEditor<'_>,
    ) -> Result<(), SketchContractKitError> {
        let Some(generated_length) = SketchLineEnding::trailing_length(rendered.as_str()) else {
            return Ok(());
        };
        if SketchLineEnding::leading_length(retained_suffix).is_none() {
            return Ok(());
        }
        if !rendered.truncate_tail(generated_length) {
            return Err(SketchContractKitError::unsupported_lossless_edit(
                editor.catalog_name,
                "generated scalar line ending could not be removed at its source boundary",
            ));
        }
        Ok(())
    }

    fn replace_encoded_scalar<'meter, 'limits>(
        &self,
        serialized: &str,
        range: std::ops::Range<usize>,
        scalar: &str,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        cancellation.checkpoint()?;
        if range.start > range.end
            || range.end > serialized.len()
            || !serialized.is_char_boundary(range.start)
            || !serialized.is_char_boundary(range.end)
        {
            return Err(
                self.unsupported(editor, "serializer returned an invalid scalar byte range")
            );
        }

        let mut replaced = output.scratch_writer(editor.catalog_name)?;
        let rendering = std::fmt::Write::write_str(&mut replaced, &serialized[..range.start])
            .and_then(|()| match self.original.tag.as_deref() {
                Some(tag) => std::fmt::Write::write_str(&mut replaced, tag)
                    .and_then(|()| std::fmt::Write::write_str(&mut replaced, " ")),
                None => Ok(()),
            })
            .and_then(|()| std::fmt::Write::write_str(&mut replaced, scalar))
            .and_then(|()| std::fmt::Write::write_str(&mut replaced, &serialized[range.end..]));
        replaced.finish_text(rendering)
    }

    fn serialize_concrete_scalar<'meter, 'limits>(
        &self,
        scalar: &yaml_edit::Scalar,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        cancellation.checkpoint()?;
        let mut rendered = output.scratch_writer(editor.catalog_name)?;
        let rendering = std::fmt::Write::write_fmt(&mut rendered, format_args!("{scalar}"));
        rendered.finish_text(rendering)
    }

    fn unsupported(
        &self,
        editor: &SketchYamlEditor<'_>,
        message: impl Into<String>,
    ) -> SketchContractKitError {
        SketchContractKitError::unsupported_lossless_edit(
            editor.catalog_name,
            format!("sketch {} code: {}", self.edit.sketch_id, message.into()),
        )
    }
}

#[derive(Deserialize, Serialize)]
struct ScalarCodecDocument<T> {
    sketches: [ScalarCodecEntry<T>; 1],
}

#[derive(Deserialize, Serialize)]
struct ScalarCodecEntry<T> {
    sketch: ScalarCodecBody<T>,
}

#[derive(Deserialize, Serialize)]
struct ScalarCodecBody<T> {
    code: T,
}

impl ScalarEditor<'_, '_> {
    fn serialize_codec<'meter, 'limits, T>(
        &self,
        value: T,
        force_folded: bool,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError>
    where
        T: Serialize,
    {
        cancellation.checkpoint()?;
        let document = ScalarCodecDocument {
            sketches: [ScalarCodecEntry {
                sketch: ScalarCodecBody { code: value },
            }],
        };
        let options = if self.flow_context {
            serde_saphyr::ser_options! {
                compact_list_indent: false,
                prefer_block_scalars: false,
                min_fold_chars: 0,
            }
        } else if force_folded {
            serde_saphyr::ser_options! {
                compact_list_indent: false,
                min_fold_chars: 0,
            }
        } else {
            serde_saphyr::ser_options! {
                compact_list_indent: false,
            }
        };

        let mut rendered = output.scratch_writer(editor.catalog_name)?;
        let rendering = serde_saphyr::to_io_writer_with_options(&mut rendered, &document, options);
        rendered.finish_text(rendering)
    }

    fn codec_code_node(&self, document: &yaml_edit::Document) -> Option<yaml_edit::YamlNode> {
        document
            .get_sequence("sketches")?
            .get(0)?
            .as_mapping()?
            .find_entry_by_key("sketch")?
            .value_node()?
            .as_mapping()?
            .find_entry_by_key("code")?
            .value_node()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockScalarStyle {
    Literal,
    Folded,
}

enum ScalarPresentation<'source> {
    Inline(InlineScalarStyle),
    Block {
        style: BlockScalarStyle,
        details: BlockPresentation<'source>,
    },
}

enum ScalarEncoding<T> {
    Preferred(T),
    SafeFallback(T),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SketchBlockChomping {
    Strip,
    Clip,
    Keep,
}

impl SketchBlockChomping {
    fn for_value(
        value: &str,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let bytes = value.as_bytes();
        let mut end = bytes.len();
        let mut line_endings = 0;
        while end > 0 && bytes[end - 1] == b'\n' {
            cancellation.checkpoint_at(bytes.len().saturating_sub(end))?;
            end -= 1;
            if end > 0 && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            line_endings += 1;
        }

        Ok(match line_endings {
            0 => Self::Strip,
            1 => Self::Clip,
            _ => Self::Keep,
        })
    }

    fn indicator(self) -> Option<char> {
        match self {
            Self::Strip => Some('-'),
            Self::Clip => None,
            Self::Keep => Some('+'),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SketchBlockIndicatorOrder {
    IndentationFirst,
    ChompingFirst,
}

struct BlockPresentation<'source> {
    indentation: Option<u8>,
    chomping: SketchBlockChomping,
    order: SketchBlockIndicatorOrder,
    header_suffix: &'source str,
    has_header_comment: bool,
    line_ending: SketchLineEnding,
}

impl<'source> BlockPresentation<'source> {
    fn default_for_scan(header_suffix: &'source str) -> Self {
        Self {
            indentation: None,
            chomping: SketchBlockChomping::Clip,
            order: SketchBlockIndicatorOrder::IndentationFirst,
            header_suffix,
            has_header_comment: false,
            line_ending: SketchLineEnding::Lf,
        }
    }
}

impl<'edit, 'seed> ScalarEditor<'edit, 'seed> {
    fn inspect_presentation<'source>(
        &self,
        scalar: &yaml_edit::Scalar,
        source: &'source str,
        editor: &SketchYamlEditor<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<ScalarPresentation<'source>, SketchContractKitError> {
        let node = scalar
            .as_node()
            .ok_or_else(|| self.unsupported(editor, "scalar has no concrete syntax node"))?;
        let raw = scalar.value();
        let mut style = match raw.as_bytes().first() {
            Some(b'\'') => ScalarPresentation::Inline(InlineScalarStyle::SingleQuoted),
            Some(b'"') => ScalarPresentation::Inline(InlineScalarStyle::DoubleQuoted),
            Some(b'|') => ScalarPresentation::Block {
                style: BlockScalarStyle::Literal,
                details: BlockPresentation::default_for_scan(&source[..0]),
            },
            Some(b'>') => ScalarPresentation::Block {
                style: BlockScalarStyle::Folded,
                details: BlockPresentation::default_for_scan(&source[..0]),
            },
            _ => ScalarPresentation::Inline(InlineScalarStyle::Plain),
        };
        let mut header = matches!(&style, ScalarPresentation::Block { .. });
        let mut indicator_chars = Vec::new();
        let mut header_suffix_start = None;
        let mut header_suffix_end = None;
        let mut has_header_comment = false;
        let mut line_ending = None;

        for (token_index, element) in node.descendants_with_tokens().enumerate() {
            cancellation.checkpoint_at(token_index)?;
            let Some(token) = element.into_token() else {
                continue;
            };
            for _ in token.text().as_bytes().chunks(64 * 1024) {
                cancellation.checkpoint()?;
            }
            match token.kind() {
                yaml_edit::SyntaxKind::PIPE => {
                    style = ScalarPresentation::Block {
                        style: BlockScalarStyle::Literal,
                        details: BlockPresentation::default_for_scan(&source[..0]),
                    };
                    header = true;
                }
                yaml_edit::SyntaxKind::GREATER => {
                    style = ScalarPresentation::Block {
                        style: BlockScalarStyle::Folded,
                        details: BlockPresentation::default_for_scan(&source[..0]),
                    };
                    header = true;
                }
                yaml_edit::SyntaxKind::NEWLINE if header => {
                    if let Some(detected) = SketchLineEnding::from_token(token.text()) {
                        line_ending = Some(detected);
                        break;
                    }
                }
                yaml_edit::SyntaxKind::INT
                | yaml_edit::SyntaxKind::STRING
                | yaml_edit::SyntaxKind::PLUS
                | yaml_edit::SyntaxKind::DASH
                    if header =>
                {
                    for (character_index, character) in token.text().chars().enumerate() {
                        cancellation.checkpoint_at(character_index)?;
                        if matches!(character, '1'..='9' | '+' | '-') {
                            indicator_chars.push(character);
                        }
                    }
                }
                yaml_edit::SyntaxKind::COMMENT if header => {
                    has_header_comment = true;
                    let range = token.text_range();
                    header_suffix_start.get_or_insert(usize::from(range.start()));
                    header_suffix_end = Some(usize::from(range.end()));
                }
                yaml_edit::SyntaxKind::WHITESPACE if header => {
                    let range = token.text_range();
                    header_suffix_start.get_or_insert(usize::from(range.start()));
                    header_suffix_end = Some(usize::from(range.end()));
                }
                _ => {}
            }
        }

        let header_suffix = match (header_suffix_start, header_suffix_end) {
            (Some(start), Some(end)) => source.get(start..end).ok_or_else(|| {
                self.unsupported(
                    editor,
                    "block scalar header suffix lies outside its YAML source",
                )
            })?,
            (None, None) => &source[..0],
            (Some(_), None) | (None, Some(_)) => {
                return Err(
                    self.unsupported(editor, "block scalar header suffix range is incomplete")
                );
            }
        };
        match style {
            ScalarPresentation::Block { style, .. } => {
                let indentation_position = indicator_chars
                    .iter()
                    .position(|character| character.is_ascii_digit());
                let chomping_position = indicator_chars
                    .iter()
                    .position(|character| matches!(character, '+' | '-'));
                let indentation = indentation_position
                    .and_then(|position| indicator_chars[position].to_digit(10))
                    .and_then(|value| u8::try_from(value).ok());
                let chomping = chomping_position
                    .map(|position| indicator_chars[position])
                    .map_or(SketchBlockChomping::Clip, |character| {
                        if character == '+' {
                            SketchBlockChomping::Keep
                        } else {
                            SketchBlockChomping::Strip
                        }
                    });
                let order = match (indentation_position, chomping_position) {
                    (Some(indentation), Some(chomping)) if chomping < indentation => {
                        SketchBlockIndicatorOrder::ChompingFirst
                    }
                    _ => SketchBlockIndicatorOrder::IndentationFirst,
                };
                Ok(ScalarPresentation::Block {
                    style,
                    details: BlockPresentation {
                        indentation,
                        chomping,
                        order,
                        header_suffix,
                        has_header_comment,
                        line_ending: line_ending.unwrap_or(SketchLineEnding::Lf),
                    },
                })
            }
            inline @ ScalarPresentation::Inline(_) => Ok(inline),
        }
    }

    fn render_presented_scalar<'meter, 'limits>(
        &self,
        original: &ScalarPresentation<'_>,
        scalar: &yaml_edit::Scalar,
        source: &str,
        editor: &SketchYamlEditor<'_>,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        let generated = self.inspect_presentation(scalar, source, editor, cancellation)?;
        let ScalarPresentation::Block {
            style: generated_style,
            details: generated_block,
        } = &generated
        else {
            let replacement_source =
                self.serialize_concrete_scalar(scalar, editor, output, cancellation)?;
            let converted = self.line_ending.convert(
                replacement_source.as_str(),
                editor.catalog_name,
                output,
                cancellation,
            )?;
            drop(replacement_source);
            return Ok(converted);
        };
        let matching_original = match original {
            ScalarPresentation::Block { style, details } if style == generated_style => {
                Some(details)
            }
            ScalarPresentation::Inline(_) | ScalarPresentation::Block { .. } => None,
        };
        let preferred_block = matching_original.unwrap_or(generated_block);
        let indentation = preferred_block.indentation.or(generated_block.indentation);
        let generated_base = generated_block.indentation.unwrap_or(2) as isize;
        let desired_base = indentation.unwrap_or(generated_base as u8) as isize;
        let indentation_delta = desired_base - generated_base;
        let line_ending = if matching_original.is_some() {
            preferred_block.line_ending
        } else {
            self.line_ending
        };
        let chomping = {
            let required = SketchBlockChomping::for_value(self.edit.current, cancellation)?;
            if preferred_block.chomping == required {
                preferred_block.chomping
            } else {
                required
            }
        };
        let marker = match generated_style {
            BlockScalarStyle::Literal => '|',
            BlockScalarStyle::Folded => '>',
        };
        let mut rendered = output.scratch_writer(editor.catalog_name)?;
        let mut rendering = std::fmt::Write::write_char(&mut rendered, marker);
        match preferred_block.order {
            SketchBlockIndicatorOrder::IndentationFirst => {
                if let Some(indentation) = indentation {
                    rendering = rendering.and_then(|()| {
                        std::fmt::Write::write_char(&mut rendered, char::from(b'0' + indentation))
                    });
                }
                if let Some(indicator) = chomping.indicator() {
                    rendering = rendering
                        .and_then(|()| std::fmt::Write::write_char(&mut rendered, indicator));
                }
            }
            SketchBlockIndicatorOrder::ChompingFirst => {
                if let Some(indicator) = chomping.indicator() {
                    rendering = rendering
                        .and_then(|()| std::fmt::Write::write_char(&mut rendered, indicator));
                }
                if let Some(indentation) = indentation {
                    rendering = rendering.and_then(|()| {
                        std::fmt::Write::write_char(&mut rendered, char::from(b'0' + indentation))
                    });
                }
            }
        }
        rendering = rendering
            .and_then(|()| std::fmt::Write::write_str(&mut rendered, preferred_block.header_suffix))
            .and_then(|()| std::fmt::Write::write_str(&mut rendered, line_ending.as_str()));

        let node = scalar.as_node().ok_or_else(|| {
            self.unsupported(editor, "serialized block scalar has no syntax node")
        })?;
        let mut past_header = false;
        let mut at_line_start = true;
        for (token_index, element) in node.descendants_with_tokens().enumerate() {
            cancellation.checkpoint_at(token_index)?;
            let Some(token) = element.into_token() else {
                continue;
            };
            if !past_header {
                if token.kind() == yaml_edit::SyntaxKind::NEWLINE {
                    past_header = true;
                }
                continue;
            }
            match token.kind() {
                yaml_edit::SyntaxKind::NEWLINE => {
                    rendering = rendering.and_then(|()| {
                        std::fmt::Write::write_str(&mut rendered, line_ending.as_str())
                    });
                    at_line_start = true;
                }
                yaml_edit::SyntaxKind::INDENT if at_line_start && indentation_delta != 0 => {
                    let mut current = 0_isize;
                    for (index, _) in token.text().chars().enumerate() {
                        cancellation.checkpoint_at(index)?;
                        current = current.saturating_add(1);
                    }
                    let adjusted = current.saturating_add(indentation_delta);
                    if adjusted <= 0 {
                        return Err(self
                            .unsupported(editor, "block scalar indentation cannot be preserved"));
                    }
                    for index in 0..adjusted as usize {
                        cancellation.checkpoint_at(index)?;
                        rendering = rendering
                            .and_then(|()| std::fmt::Write::write_char(&mut rendered, ' '));
                    }
                    at_line_start = false;
                }
                _ => {
                    rendering = rendering
                        .and_then(|()| std::fmt::Write::write_str(&mut rendered, token.text()));
                    at_line_start = false;
                }
            }
        }
        rendered.finish_text(rendering)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SketchLineEnding {
    Lf,
    CrLf,
    Cr,
}

impl SketchLineEnding {
    pub(super) fn for_target(
        code: &SketchCodeNode,
        document: &yaml_edit::Document,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        if let Some(line_ending) = Self::first_in_cst(&code.scalar, cancellation)? {
            return Ok(line_ending);
        }
        Ok(Self::first_in_cst(document, cancellation)?.unwrap_or(Self::Lf))
    }

    fn first_in_cst(
        value: &impl yaml_edit::AsYaml,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SketchContractKitError> {
        let Some(node) = value.as_node() else {
            return Ok(None);
        };
        for (index, element) in node.descendants_with_tokens().enumerate() {
            cancellation.checkpoint_at(index)?;
            let Some(token) = element.into_token() else {
                continue;
            };
            if token.kind() == yaml_edit::SyntaxKind::NEWLINE
                && let Some(line_ending) = Self::from_token(token.text())
            {
                return Ok(Some(line_ending));
            }
        }
        Ok(None)
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "\r\n" => Some(Self::CrLf),
            "\r" => Some(Self::Cr),
            "\n" => Some(Self::Lf),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }

    fn leading_length(value: &str) -> Option<usize> {
        if value.starts_with("\r\n") {
            Some(2)
        } else if value.starts_with(['\n', '\r']) {
            Some(1)
        } else {
            None
        }
    }

    fn trailing_length(value: &str) -> Option<usize> {
        if value.ends_with("\r\n") {
            Some(2)
        } else if value.ends_with(['\n', '\r']) {
            Some(1)
        } else {
            None
        }
    }

    fn convert<'meter, 'limits>(
        self,
        value: &str,
        catalog_name: &CatalogPath,
        output: &'meter GeneratedBytes<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError> {
        let bytes = value.as_bytes();
        let mut rendered = output.scratch_writer(catalog_name)?;
        let mut segment_start = 0;
        let mut index = 0;
        let mut rendering = Ok(());
        while index < bytes.len() {
            cancellation.checkpoint_at(index)?;
            let consumed =
                if bytes[index] == b'\r' && bytes.get(index.saturating_add(1)) == Some(&b'\n') {
                    2
                } else if bytes[index] == b'\n' {
                    1
                } else {
                    index += 1;
                    continue;
                };
            rendering = rendering
                .and_then(|()| {
                    std::io::Write::write_all(&mut rendered, &bytes[segment_start..index])
                        .map_err(|_| std::fmt::Error)
                })
                .and_then(|()| {
                    std::io::Write::write_all(&mut rendered, self.as_str().as_bytes())
                        .map_err(|_| std::fmt::Error)
                });
            index = index.saturating_add(consumed);
            segment_start = index;
        }
        rendering = rendering.and_then(|()| {
            std::io::Write::write_all(&mut rendered, &bytes[segment_start..])
                .map_err(|_| std::fmt::Error)
        });
        rendered.finish_text(rendering)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::document::SketchContractFile;
    use super::super::{ScalarEditor, SketchCodeEdit, SketchYamlEditor};
    use super::{SketchCodeNode, SketchLineEnding};
    use crate::contract::tests::ContractYaml;
    use crate::files::CatalogPath;
    use crate::limits::{GeneratedBytes, SketchLimits};
    use crate::work::CancellationProbe;
    use std::str::FromStr;

    #[test]
    fn edit_path_anchor_scan_covers_property_orders_and_stays_local() {
        let cases = [
            (
                "code scalar",
                "sketches:\n  - body:\n      code: &code old\n",
                true,
            ),
            (
                "sketch body",
                "sketches:\n  - body: &body\n      code: old\n",
                true,
            ),
            (
                "sketch entry",
                "sketches:\n  - &entry\n    body:\n      code: old\n",
                true,
            ),
            (
                "sketch sequence",
                "sketches: &sketches\n  - body:\n      code: old\n",
                true,
            ),
            (
                "document root",
                "--- &document {sketches: [{body: {code: old}}]}\n",
                true,
            ),
            (
                "tag then anchor",
                "sketches:\n  - body:\n      code: !!str &code old\n",
                true,
            ),
            (
                "anchor then tag",
                "sketches:\n  - body:\n      code: &code !!str old\n",
                true,
            ),
            (
                "anchored sibling value",
                "sketches:\n  - body:\n      sibling: &sibling old\n      code: old\n",
                false,
            ),
            (
                "anchored sibling key",
                "sketches:\n  - body:\n      ? &key sibling\n      : old\n      code: old\n",
                false,
            ),
            (
                "anchored files",
                "files: &files [lib.rs]\nsketches:\n  - body:\n      code: old\n",
                false,
            ),
            (
                "anchored extraction",
                "extraction: &extraction { mode: syntax }\nsketches:\n  - body:\n      code: old\n",
                false,
            ),
            (
                "another anchored sketch",
                "sketches:\n  - body:\n      code: old\n  - other: &other\n      code: old\n",
                false,
            ),
            (
                "signature-owned anchor",
                "signatures:\n  - answer:\n      opaque: &opaque value\nsketches:\n  - body:\n      code: old\n",
                false,
            ),
        ];
        let cancellation = CancellationProbe::new();

        for (name, source, expected) in cases {
            let document = yaml_edit::Document::from_str(source)
                .unwrap_or_else(|error| panic!("{name} YAML failed to parse: {error}"));
            let root = document
                .as_mapping()
                .unwrap_or_else(|| panic!("{name} document root mapping"));
            let sketches_value = root
                .find_entry_by_key("sketches")
                .and_then(|entry| entry.value_node())
                .unwrap_or_else(|| panic!("{name} sketches value"));
            let sketch_entry = sketches_value
                .as_sequence()
                .and_then(|sequence| sequence.first())
                .unwrap_or_else(|| panic!("{name} sketch entry"));
            let sketch_body = sketch_entry
                .as_mapping()
                .and_then(|mapping| mapping.find_entry_by_key("body"))
                .and_then(|entry| entry.value_node())
                .and_then(|value| value.as_mapping().cloned())
                .unwrap_or_else(|| panic!("{name} sketch body"));
            let code = sketch_body
                .find_entry_by_key("code")
                .and_then(|entry| entry.value_node())
                .unwrap_or_else(|| panic!("{name} code value"));

            assert_eq!(
                SketchCodeNode::edit_path_has_anchor(&code, &cancellation)
                    .unwrap_or_else(|error| panic!("{name} anchor scan failed: {error}")),
                expected,
                "{name}",
            );
        }
    }

    #[test]
    fn scalar_editor_preserves_closed_presentations_and_validates_fallbacks() {
        let cases = [
            ("plain", "old", "new", "code: new"),
            ("single quoted", "'old'", "new", "code: 'new'"),
            ("double quoted", "\"old\"", "new", "code: \"new\""),
            ("tagged", "!!str 'old'", "new", "code: !!str 'new'"),
            (
                "literal indentation",
                "|2-\n        old",
                "new",
                "code: |2-",
            ),
            (
                "literal indicator order",
                "|-2\n        old",
                "new",
                "code: |-2",
            ),
            (
                "literal header comment",
                "|2- # keep\n        old",
                "new",
                "code: |2- # keep",
            ),
            (
                "literal keep",
                "|+\n        old\n        ",
                "new\n\n",
                "code: |+",
            ),
            ("folded strip", ">-\n        old", "new", "code: >-"),
            ("folded clip", ">\n        old", "new\n", "code: >\n"),
            (
                "folded keep",
                ">+\n        old\n        ",
                "new\n\n",
                "code: >+",
            ),
            (
                "plain fallback",
                "old",
                "value: # code",
                "code: \"value: # code\"",
            ),
            (
                "tagged fallback",
                "!!str old",
                "value: # code",
                "code: !!str \"value: # code\"",
            ),
        ];

        for (name, scalar, current, expected) in cases {
            let source = ContractYaml::linked("answer", "body", "function", "old").replacen(
                "code: 'old'",
                &format!("code: {scalar}"),
                1,
            );
            let path = CatalogPath::new("main.yml").expect("path");
            let limits = SketchLimits::default();
            let cancellation = CancellationProbe::new();
            let output = GeneratedBytes::new(&limits.output, &cancellation);
            let rendered = SketchYamlEditor::new(&path, &source)
                .apply(
                    &[SketchCodeEdit {
                        document_index: 0,
                        sketch_index: 0,
                        sketch_id: "body",
                        current,
                    }],
                    &output,
                    &cancellation,
                )
                .unwrap_or_else(|error| panic!("{name} edit failed: {error}"));
            let rendered_text = std::str::from_utf8(&rendered).expect("rendered UTF-8");
            assert!(
                rendered_text.contains(expected),
                "{name}: expected {expected:?} in {rendered_text:?}",
            );

            let mut budget = limits.yaml_budget();
            let parsed = SketchContractFile::parse(path, rendered, &mut budget, &cancellation)
                .unwrap_or_else(|error| panic!("{name} semantic reparse failed: {error}"));
            assert_eq!(
                parsed.documents[0].semantic.sketches[0].code, current,
                "{name}"
            );
        }
    }

    #[test]
    fn final_scalar_certification_rejects_non_scalar_ambiguous_and_wrong_values() {
        let source = "code: old";
        let document = yaml_edit::Document::from_str(source).expect("scalar YAML");
        let original = document
            .as_mapping()
            .and_then(|mapping| mapping.find_entry_by_key("code"))
            .and_then(|entry| entry.value_node())
            .and_then(SketchCodeNode::from_yaml)
            .expect("original scalar");
        let edit = SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "expected",
        };
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, source);
        let cancellation = CancellationProbe::new();
        let mut scalar_editor = ScalarEditor {
            range: 0..0,
            destination_mapping_column: 6,
            flow_context: false,
            original,
            line_ending: SketchLineEnding::Lf,
            edit: &edit,
        };

        assert!(
            scalar_editor
                .final_candidate_is_exact("expected", "", &editor, &cancellation)
                .expect("exact plain scalar")
        );
        assert!(
            scalar_editor
                .final_candidate_is_exact("!!str 'expected'", "", &editor, &cancellation)
                .expect("exact tagged scalar")
        );
        assert!(
            !scalar_editor
                .final_candidate_is_exact("other", "", &editor, &cancellation)
                .expect("well-formed wrong scalar")
        );

        for (name, candidate, expected_error) in [
            ("malformed", "\"unterminated", "concrete-syntax validation"),
            ("anchor", "&value expected", "contains an anchor"),
            ("alias", "*value", "is an alias"),
            ("sequence", "[expected]", "is a YAML container"),
            ("mapping", "{value: expected}", "is a YAML container"),
            (
                "multiple documents",
                "expected\n---\nother\n",
                "multiple YAML documents",
            ),
        ] {
            let Err(error) =
                scalar_editor.final_candidate_is_exact(candidate, "", &editor, &cancellation)
            else {
                panic!("{name} candidate must be rejected");
            };
            assert!(
                error.to_string().contains(expected_error),
                "{name}: {error}"
            );
        }

        scalar_editor.flow_context = true;
        let Err(error) = scalar_editor.final_candidate_is_exact(
            "expected, trailing",
            "",
            &editor,
            &cancellation,
        ) else {
            panic!("flow-delimiter ambiguity must be rejected");
        };
        assert!(error.to_string().contains("destination flow context"));
    }

    #[test]
    fn block_header_certification_borrows_only_the_retained_boundary_line_ending() {
        let source = "code: old";
        let document = yaml_edit::Document::from_str(source).expect("scalar YAML");
        let original = document
            .as_mapping()
            .and_then(|mapping| mapping.find_entry_by_key("code"))
            .and_then(|entry| entry.value_node())
            .and_then(SketchCodeNode::from_yaml)
            .expect("original scalar");
        let edit = SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "",
        };
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, source);
        let cancellation = CancellationProbe::new();
        let scalar_editor = ScalarEditor {
            range: 0..0,
            destination_mapping_column: 6,
            flow_context: false,
            original,
            line_ending: SketchLineEnding::Lf,
            edit: &edit,
        };

        for (name, retained_suffix) in [
            ("LF", "\nretained: value"),
            ("CRLF", "\r\nretained: value"),
            ("CR", "\rretained: value"),
        ] {
            assert!(
                scalar_editor
                    .final_candidate_is_exact(">-", retained_suffix, &editor, &cancellation,)
                    .unwrap_or_else(|error| panic!("{name} boundary failed: {error}")),
                "{name} borrowed boundary must preserve the empty block scalar",
            );
        }
    }

    #[test]
    fn flow_scalar_certification_is_independent_of_absolute_destination_column() {
        let source = "sketches: [{ body: { code: stale } }]\n";
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, source);
        let syntax = yaml_edit::YamlFile::from_str(source).expect("flow-style YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "current",
        }];
        let cancellation = CancellationProbe::new();
        let mut target = editor
            .targets(&edits, &documents, &cancellation)
            .expect("flow scalar target")
            .pop()
            .expect("one target");
        assert!(target.flow_context);
        target.destination_mapping_column = usize::MAX;

        assert!(
            target
                .final_candidate_is_exact("\"current\"", "", &editor, &cancellation)
                .expect("fixed-size flow semantic wrapper")
        );
    }

    #[test]
    fn cancelled_large_block_scalar_stops_every_lossless_inner_scan() {
        let source = format!("code: |-\n{}", "  let answer = 42;\n".repeat(16_384));
        let document = yaml_edit::Document::from_str(&source).expect("large block scalar YAML");
        let mapping = document.as_mapping().expect("code mapping");
        let entry = mapping.find_entry_by_key("code").expect("code entry");
        let value = entry.value_node().expect("code value");
        let code = SketchCodeNode::from_yaml(value.clone()).expect("scalar code");
        let path = CatalogPath::new("main.yml").expect("path");
        let editor = SketchYamlEditor::new(&path, &source);
        let edit = SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "large_body",
            current: "new\n\n",
        };
        let cancellation = CancellationProbe::new();
        let limits = SketchLimits::default();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let scalar_editor = ScalarEditor {
            range: 0..0,
            destination_mapping_column: 0,
            flow_context: false,
            line_ending: SketchLineEnding::Lf,
            original: code,
            edit: &edit,
        };
        let presentation = scalar_editor
            .inspect_presentation(
                &scalar_editor.original.scalar,
                &source,
                &editor,
                &cancellation,
            )
            .expect("active scalar presentation scan");
        cancellation.cancel();

        let Err(render_error) = scalar_editor.render_presented_scalar(
            &presentation,
            &scalar_editor.original.scalar,
            &source,
            &editor,
            &output,
            &cancellation,
        ) else {
            panic!("cancelled block-scalar rendering must stop");
        };
        assert!(render_error.is_operation_cancelled());

        let Err(presentation_error) = scalar_editor.inspect_presentation(
            &scalar_editor.original.scalar,
            &source,
            &editor,
            &cancellation,
        ) else {
            panic!("cancelled scalar presentation scan must stop");
        };
        assert!(presentation_error.is_operation_cancelled());

        let anchor_error = SketchCodeNode::edit_path_has_anchor(&value, &cancellation)
            .expect_err("cancelled anchor scan must stop");
        assert!(anchor_error.is_operation_cancelled());

        let Err(line_ending_error) =
            SketchLineEnding::for_target(&scalar_editor.original, &document, &cancellation)
        else {
            panic!("cancelled line-ending token scan must stop");
        };
        assert!(line_ending_error.is_operation_cancelled());

        let trailing_error =
            super::SketchBlockChomping::for_value(&"\n".repeat(16_384), &cancellation)
                .expect_err("cancelled trailing-line scan must stop");
        assert!(trailing_error.is_operation_cancelled());
    }

    #[test]
    fn cancellable_line_ending_conversion_preserves_isolated_carriage_returns() {
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let limits = SketchLimits::default();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let mixed = "first\rsecond\r\nthird\nfourth";

        assert_eq!(
            SketchLineEnding::Lf
                .convert(mixed, &path, &output, &cancellation)
                .expect("LF conversion")
                .as_str(),
            "first\rsecond\nthird\nfourth"
        );
        assert_eq!(
            SketchLineEnding::CrLf
                .convert(mixed, &path, &output, &cancellation)
                .expect("CRLF conversion")
                .as_str(),
            "first\rsecond\r\nthird\r\nfourth"
        );
        assert_eq!(
            SketchLineEnding::Cr
                .convert(mixed, &path, &output, &cancellation)
                .expect("CR conversion")
                .as_str(),
            "first\rsecond\rthird\rfourth"
        );
    }

    #[test]
    fn line_ending_selection_prefers_scalar_then_document_then_lf() {
        let cancellation = CancellationProbe::new();
        let detect = |source: &str| {
            let document = yaml_edit::Document::from_str(source).expect("line-ending YAML");
            let mapping = document.as_mapping().expect("code mapping");
            let code = mapping
                .find_entry_by_key("code")
                .and_then(|entry| entry.value_node())
                .and_then(SketchCodeNode::from_yaml)
                .expect("scalar code");
            SketchLineEnding::for_target(&code, &document, &cancellation)
                .expect("line-ending selection")
        };

        assert_eq!(
            detect("prefix: value\r\ncode: |-\n  first\n  second\n"),
            SketchLineEnding::Lf,
        );
        assert_eq!(
            detect("prefix: value\r\ncode: stale"),
            SketchLineEnding::CrLf,
        );
        assert_eq!(detect("code: stale"), SketchLineEnding::Lf);
        let isolated_content = detect("prefix: value\ncode: \"first\rsecond\"\n");
        assert_eq!(isolated_content, SketchLineEnding::Lf);
        let path = CatalogPath::new("main.yml").expect("path");
        let limits = SketchLimits::default();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        assert_eq!(
            isolated_content
                .convert("first\rsecond\nthird", &path, &output, &cancellation)
                .expect("selected LF conversion")
                .as_str(),
            "first\rsecond\nthird",
        );
        assert_eq!(
            detect("prefix: value\rcode: |-\r  first\r  second\r"),
            SketchLineEnding::Cr,
        );
    }

    #[test]
    fn scalar_boundary_keeps_the_retained_source_line_ending_without_hybrids() {
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let limits = SketchLimits::default();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let source = "code: old";
        let document = yaml_edit::Document::from_str(source).expect("scalar YAML");
        let code = document
            .as_mapping()
            .and_then(|mapping| mapping.find_entry_by_key("code"))
            .and_then(|entry| entry.value_node())
            .and_then(SketchCodeNode::from_yaml)
            .expect("scalar code");
        let edit = SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "new",
        };
        let scalar_editor = ScalarEditor {
            range: 0..0,
            destination_mapping_column: 0,
            flow_context: false,
            original: code,
            line_ending: SketchLineEnding::Lf,
            edit: &edit,
        };
        let editor = SketchYamlEditor::new(&path, source);

        for (generated, suffix, expected) in [
            ("value\r\n", "\nnext", "value"),
            ("value\n", "\r\nnext", "value"),
            ("value\r", "\nnext", "value"),
            ("value\n", "\rnext", "value"),
            ("value\r\n", "next", "value\r\n"),
        ] {
            let mut writer = output.scratch_writer(&path).expect("scratch writer");
            std::io::Write::write_all(&mut writer, generated.as_bytes()).expect("generated scalar");
            let mut rendered = writer
                .finish_text(Ok::<(), std::fmt::Error>(()))
                .expect("retained scalar");
            scalar_editor
                .remove_duplicated_boundary_line_ending(&mut rendered, suffix, &editor)
                .expect("boundary normalization");
            assert_eq!(rendered.as_str(), expected);
        }
    }

    #[test]
    fn one_live_scalar_transformation_chain_respects_the_combined_scratch_limit() {
        let source = "sketches:\n  - body:\n      code: stale\n";
        let code = "x".repeat(512);
        let mut limits = SketchLimits::default();
        limits.output.scratch_bytes = 1024;
        let cancellation = CancellationProbe::new();
        let output = GeneratedBytes::new(&limits.output, &cancellation);
        let path = CatalogPath::new("main.yml").expect("path");
        let error = SketchYamlEditor::new(&path, source)
            .apply(
                &[SketchCodeEdit {
                    document_index: 0,
                    sketch_index: 0,
                    sketch_id: "body",
                    current: &code,
                }],
                &output,
                &cancellation,
            )
            .expect_err("simultaneously live scalar transformations must share one budget");
        let limit = error.limit_exceeded().expect("typed scratch limit");

        assert_eq!(
            limit.resource,
            crate::limits::LimitResource::OutputScratchBytes
        );
        assert_eq!(limit.limit, 1024);
        assert_eq!(limit.observed_at_least, 1025);
        assert_eq!(limit.file.as_ref(), Some(&path));
    }

    #[test]
    fn final_scalar_render_releases_its_validated_serialized_predecessor() {
        let indentation = " ".repeat(256);
        let source = format!("sketches:\n- body:\n{indentation}code: |-\n{indentation}  old\n");
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current: "first\nsecond",
        }];
        let syntax = yaml_edit::YamlFile::from_str(&source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let editor = SketchYamlEditor::new(&path, &source);
        let targets = editor
            .targets(&edits, &documents, &cancellation)
            .expect("scalar edit target");
        let unrestricted = SketchLimits::default();
        let unrestricted_output = GeneratedBytes::new(&unrestricted.output, &cancellation);
        let replacement = targets[0]
            .render(&editor, &unrestricted_output, &cancellation)
            .expect("measure final scalar replacement");
        let exact_peak = u64::try_from(replacement.as_str().len()).expect("replacement length");
        drop(replacement);

        let exact_limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: exact_peak,
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let exact_output = GeneratedBytes::new(&exact_limits.output, &cancellation);
        editor
            .apply(&edits, &exact_output, &cancellation)
            .expect("final replacement alone exactly fits the live scratch budget");

        let crossing_limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: exact_peak.saturating_sub(1),
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let crossing_output = GeneratedBytes::new(&crossing_limits.output, &cancellation);
        let error = editor
            .apply(&edits, &crossing_output, &cancellation)
            .expect_err("one byte below the final replacement peak must fail");
        let limit = error.limit_exceeded().expect("typed scratch limit");
        assert_eq!(
            limit.resource,
            crate::limits::LimitResource::OutputScratchBytes
        );
        assert_eq!(limit.limit, exact_peak.saturating_sub(1));
        assert_eq!(limit.observed_at_least, exact_peak);
    }

    #[test]
    fn single_quoted_control_fallback_obeys_its_exact_live_scratch_peak() {
        let source = "sketches:\n  - body:\n      code: 'old'\n";
        let path = CatalogPath::new("main.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let current = "first\nsecond";
        let edits = [SketchCodeEdit {
            document_index: 0,
            sketch_index: 0,
            sketch_id: "body",
            current,
        }];
        let editor = SketchYamlEditor::new(&path, source);
        let syntax = yaml_edit::YamlFile::from_str(source).expect("concrete YAML");
        let documents = syntax.documents().collect::<Vec<_>>();
        let targets = editor
            .targets(&edits, &documents, &cancellation)
            .expect("scalar edit target");
        let unrestricted = SketchLimits::default();
        let unrestricted_output = GeneratedBytes::new(&unrestricted.output, &cancellation);
        let envelope = targets[0]
            .serialize_codec(
                serde_saphyr::DoubleQuoted(current),
                false,
                &editor,
                &unrestricted_output,
                &cancellation,
            )
            .expect("measure fallback envelope");
        let exact_peak = u64::try_from(envelope.as_str().len())
            .expect("fallback envelope length")
            .saturating_mul(2);
        drop(envelope);

        let exact_limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: exact_peak,
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let exact_output = GeneratedBytes::new(&exact_limits.output, &cancellation);
        let rendered = editor
            .apply(&edits, &exact_output, &cancellation)
            .expect("the fallback's serialize-and-convert peak fits exactly");
        let rendered = std::str::from_utf8(&rendered).expect("rendered UTF-8");
        assert!(rendered.contains("code: \"first\\nsecond\""));

        let crossing_limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: exact_peak.saturating_sub(1),
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let crossing_output = GeneratedBytes::new(&crossing_limits.output, &cancellation);
        let error = editor
            .apply(&edits, &crossing_output, &cancellation)
            .expect_err("one byte below the fallback live peak must fail");
        let limit = error.limit_exceeded().expect("typed scratch limit");
        assert_eq!(
            limit.resource,
            crate::limits::LimitResource::OutputScratchBytes
        );
        assert_eq!(limit.limit, exact_peak.saturating_sub(1));
        assert_eq!(limit.observed_at_least, exact_peak);
    }
}
