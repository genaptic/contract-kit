use super::artifact::{CompilerSourcePath, CompilerSourceProvenance, RustCompilerArtifactFailure};
use super::index::RustdocIndex;
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::parser::RustParsedEntry;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::source::{RustSourceCatalog, RustSourceSpan};
use crate::languages::rust::types::declaration::RustDeclaration;
use crate::limits::RustExtractionLimits;
use crate::work::CancellationProbe;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) struct CompilerSourceIndex<'source, 'limits> {
    sources: &'source FileCatalog,
    limits: &'limits RustExtractionLimits,
    requested_endpoints: BTreeMap<CatalogPath, Vec<u64>>,
    files: BTreeMap<CatalogPath, CompilerSourceFileIndex>,
}

impl<'source, 'limits> CompilerSourceIndex<'source, 'limits> {
    pub(super) fn new(
        sources: &'source FileCatalog,
        mappings: &[CompilerSourcePath],
        limits: &'limits RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut requested_endpoints = BTreeMap::<CatalogPath, Vec<u64>>::new();
        for mapping in mappings {
            cancellation.checkpoint()?;
            let CompilerSourceProvenance::Exact {
                file,
                byte_start,
                byte_end,
            } = &mapping.provenance
            else {
                continue;
            };
            let endpoints = requested_endpoints.entry(file.clone()).or_default();
            endpoints.push(*byte_start);
            endpoints.push(*byte_end);
        }
        Ok(Self {
            sources,
            limits,
            requested_endpoints,
            files: BTreeMap::new(),
        })
    }

    pub(super) fn file(
        &mut self,
        file: &CatalogPath,
        item_id: u32,
        cancellation: &CancellationProbe,
    ) -> Result<&CompilerSourceFileIndex, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if let Some((owned_file, endpoints)) = self.requested_endpoints.remove_entry(file) {
            let source_bytes = self.sources.get(file).ok_or_else(|| {
                RustCompilerArtifactFailure::source_map(
                    Some(item_id),
                    format!("exact provenance file {file} is absent from the source catalog"),
                )
            })?;
            let index = CompilerSourceFileIndex::new(
                file,
                item_id,
                source_bytes,
                endpoints,
                self.limits,
                cancellation,
            )?;
            self.files.insert(owned_file, index);
        }
        self.files.get(file).ok_or_else(|| {
            SignatureContractKitError::conversion_failed(format!(
                "source index for {file} was not retained"
            ))
        })
    }
}

#[derive(Debug)]
pub(super) struct CompilerSourceFileIndex {
    pub(super) endpoints: Vec<CompilerSourceEndpoint>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CompilerSourceEndpoint {
    offset: usize,
    coordinate: Option<(usize, usize)>,
    preceding_coordinate: Option<(usize, usize)>,
}

impl CompilerSourceFileIndex {
    pub(super) fn new(
        file: &CatalogPath,
        item_id: u32,
        source_bytes: &[u8],
        mut requested_endpoints: Vec<u64>,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        limits.validate_source_file(file, source_bytes.len())?;
        let source = std::str::from_utf8(source_bytes).map_err(|error| {
            RustCompilerArtifactFailure::source_map(
                Some(item_id),
                format!("exact provenance file {file} is not UTF-8: {error}"),
            )
        })?;
        cancellation.checkpoint()?;
        requested_endpoints.sort_unstable();
        cancellation.checkpoint()?;
        requested_endpoints.dedup();
        cancellation.checkpoint()?;
        let mut endpoints = Vec::with_capacity(requested_endpoints.len());
        for requested in requested_endpoints {
            cancellation.checkpoint()?;
            let Ok(offset) = usize::try_from(requested) else {
                continue;
            };
            if offset > source.len() || !source.is_char_boundary(offset) {
                continue;
            }
            endpoints.push(CompilerSourceEndpoint {
                offset,
                coordinate: None,
                preceding_coordinate: None,
            });
        }
        let mut line = 1;
        let mut column = 1;
        let mut preceding_coordinate = None;
        let mut next_endpoint = 0;
        for (offset, character) in source.char_indices() {
            cancellation.checkpoint()?;
            if endpoints
                .get(next_endpoint)
                .is_some_and(|endpoint| endpoint.offset == offset)
            {
                let endpoint = &mut endpoints[next_endpoint];
                endpoint.coordinate = Some((line, column));
                endpoint.preceding_coordinate = preceding_coordinate;
                next_endpoint += 1;
            }
            preceding_coordinate = Some((line, column));
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        if endpoints
            .get(next_endpoint)
            .is_some_and(|endpoint| endpoint.offset == source.len())
        {
            let endpoint = &mut endpoints[next_endpoint];
            endpoint.preceding_coordinate = preceding_coordinate;
        }
        Ok(Self { endpoints })
    }

    pub(super) fn span_coordinates(
        &self,
        start: usize,
        end: usize,
    ) -> Option<((usize, usize), (usize, usize))> {
        if start >= end {
            return None;
        }
        Some((
            self.endpoint(start)?.coordinate?,
            self.endpoint(end)?.preceding_coordinate?,
        ))
    }

    fn endpoint(&self, offset: usize) -> Option<&CompilerSourceEndpoint> {
        self.endpoints
            .binary_search_by_key(&offset, |endpoint| endpoint.offset)
            .ok()
            .and_then(|index| self.endpoints.get(index))
    }
}

impl CompilerSourceProvenance {
    pub(super) fn file(&self) -> &CatalogPath {
        match self {
            Self::Exact { file, .. } => file,
            Self::CompilerGenerated { crate_root } => crate_root,
        }
    }

    pub(super) fn validate_item_span(
        &self,
        item_id: u32,
        span: Option<&rustdoc_types::Span>,
        source_index: &mut CompilerSourceIndex<'_, '_>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        match (self, span) {
            (
                Self::Exact {
                    file,
                    byte_start,
                    byte_end,
                },
                Some(span),
            ) => {
                if !span.filename.ends_with(Path::new(file.as_str())) {
                    return Err(RustCompilerArtifactFailure::source_map(
                        Some(item_id),
                        format!(
                            "exact provenance file {file} contradicts rustdoc span file {:?}",
                            span.filename
                        ),
                    ));
                }
                let source = source_index.file(file, item_id, cancellation)?;
                let start = usize::try_from(*byte_start).map_err(|_| {
                    RustCompilerArtifactFailure::source_map(
                        Some(item_id),
                        format!("start byte {byte_start} cannot be represented on this host"),
                    )
                })?;
                let end = usize::try_from(*byte_end).map_err(|_| {
                    RustCompilerArtifactFailure::source_map(
                        Some(item_id),
                        format!("end byte {byte_end} cannot be represented on this host"),
                    )
                })?;
                let Some((begin, inclusive_end)) = source.span_coordinates(start, end) else {
                    return Err(RustCompilerArtifactFailure::source_map(
                        Some(item_id),
                        format!(
                            "exact byte range {byte_start}..{byte_end} is not a nonempty UTF-8-aligned range in {file}"
                        ),
                    ));
                };
                if begin != span.begin || inclusive_end != span.end {
                    return Err(RustCompilerArtifactFailure::source_map(
                        Some(item_id),
                        format!(
                            "exact byte range resolves to {begin:?}..={inclusive_end:?}, but rustdoc records {:?}..={:?}",
                            span.begin, span.end
                        ),
                    ));
                }
                Ok(())
            }
            (Self::Exact { .. }, None) => Err(RustCompilerArtifactFailure::source_map(
                Some(item_id),
                "exact provenance requires a rustdoc source span",
            )),
            (Self::CompilerGenerated { .. }, Some(_)) => {
                Err(RustCompilerArtifactFailure::source_map(
                    Some(item_id),
                    "compiler-generated provenance contradicts the rustdoc source span",
                ))
            }
            (Self::CompilerGenerated { .. }, None) => Ok(()),
        }
    }
}

pub(super) struct RustdocProvenanceResolver<'index, 'sources, 'operation, 'limits> {
    pub(super) index: &'index RustdocIndex,
    pub(super) sources: &'sources mut RustSourceCatalog,
    pub(super) limits: &'limits RustExtractionLimits,
    pub(super) cancellation: &'operation CancellationProbe,
}

impl RustdocProvenanceResolver<'_, '_, '_, '_> {
    pub(super) fn exact_module_source(
        mut self,
        item: &rustdoc_types::Item,
    ) -> Result<(syn::ItemMod, RustSourceSpan), SignatureContractKitError> {
        let index = self.index;
        let provenance = &index
            .source_map
            .get(&item.id.0)
            .ok_or_else(|| {
                RustCompilerArtifactFailure::source_map(
                    Some(item.id.0),
                    "public module has no logical source provenance",
                )
            })?
            .provenance;
        let CompilerSourceProvenance::Exact {
            file,
            byte_start,
            byte_end,
        } = provenance
        else {
            self.cancellation.checkpoint()?;
            return Err(index.unsupported_item(
                item,
                "rustdoc does not retain inline, out-of-line, or #[path] module shape and this public module has no exact source provenance",
            ));
        };
        let header_span = self.exact_span(item.id.0, file, *byte_start, *byte_end)?;
        self.sources
            .module_for_compiler_header(&header_span, self.cancellation)
            .map_err(|error| {
                if error.is_operation_canceled() {
                    error
                } else {
                    index.unsupported_item(
                        item,
                        format!(
                            "exact source span does not identify one complete module declaration: {error}"
                        ),
                    )
                }
            })
    }

    pub(super) fn parsed_declaration(
        mut self,
        item_id: u32,
        id: RustItemId,
        declaration: RustDeclaration,
        provenance: &CompilerSourceProvenance,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        match provenance {
            CompilerSourceProvenance::Exact {
                file,
                byte_start,
                byte_end,
            } => Ok(RustParsedEntry::from_source(
                id,
                declaration,
                self.exact_span(item_id, file, *byte_start, *byte_end)?,
            )),
            CompilerSourceProvenance::CompilerGenerated { crate_root } => {
                self.cancellation.checkpoint()?;
                Ok(RustParsedEntry::from_compiler_generated(
                    id,
                    declaration,
                    crate_root.clone(),
                ))
            }
        }
    }

    fn exact_span(
        &mut self,
        item_id: u32,
        file: &CatalogPath,
        byte_start: u64,
        byte_end: u64,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        let start = usize::try_from(byte_start).map_err(|_| {
            RustCompilerArtifactFailure::source_map(
                Some(item_id),
                "source start byte does not fit this platform",
            )
        })?;
        let end = usize::try_from(byte_end).map_err(|_| {
            RustCompilerArtifactFailure::source_map(
                Some(item_id),
                "source end byte does not fit this platform",
            )
        })?;
        self.sources
            .load_syntax(file, self.limits, self.cancellation)?;
        self.sources
            .source_span_from_byte_range(file, start, end)
            .map_err(|error| {
                RustCompilerArtifactFailure::source_map(Some(item_id), error.to_string())
            })
    }
}
