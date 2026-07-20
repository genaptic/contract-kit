//! Rustdoc source-provenance translation and batched coordinate resolution.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Component, Path, PathBuf};

use conkit_signature::{CatalogPath, CompilerSourcePath, CompilerSourceProvenance, FileCatalog};

use super::error::CompilerError;
use super::extractor::{RustdocSourceDocument, RustdocSourceSpan};
use super::limits::CompilerUsage;
use super::process::{CompilerOperation, CompilerSemanticResource};
use crate::platform::PortablePathRules;

pub(super) struct CompilerSourceTranslator<'operation> {
    usage: &'operation CompilerUsage,
    document: &'operation RustdocSourceDocument,
    current_directory: &'operation Path,
    canonical_root: PathBuf,
    source_files: &'operation FileCatalog,
    crate_root: &'operation CatalogPath,
}

impl<'operation> CompilerSourceTranslator<'operation> {
    pub(super) fn new(
        usage: &'operation CompilerUsage,
        document: &'operation RustdocSourceDocument,
        current_directory: &'operation Path,
        source_root: &Path,
        source_files: &'operation FileCatalog,
        crate_root: &'operation CatalogPath,
    ) -> Result<Self, CompilerError> {
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let canonical_root = fs_err::canonicalize(source_root).map_err(|source| {
            CompilerError::SourceRootUnavailable {
                path: source_root.to_path_buf(),
                source,
            }
        })?;
        Ok(Self {
            usage,
            document,
            current_directory,
            canonical_root,
            source_files,
            crate_root,
        })
    }

    pub(super) fn into_mappings(self) -> Result<Vec<CompilerSourcePath>, CompilerError> {
        self.usage.checkpoint(CompilerOperation::Rustdoc)?;
        let candidate_ceiling = self.usage.remaining_source_mappings().saturating_add(1);
        let mut source_indices = BTreeMap::new();
        let mut sources = Vec::new();
        let mut pending = Vec::new();
        for item in &self.document.index.items {
            self.usage.checkpoint(CompilerOperation::Rustdoc)?;
            if u64::try_from(pending.len()).unwrap_or(u64::MAX) >= candidate_ceiling {
                break;
            }
            if item.crate_id != 0 {
                continue;
            }
            let candidate = match item.span.as_ref() {
                Some(span) => match self.mapped_source(span) {
                    Some((file, bytes)) => {
                        let source_index = match source_indices.entry(file.clone()) {
                            Entry::Occupied(entry) => *entry.get(),
                            Entry::Vacant(entry) => {
                                let source_index = sources.len();
                                sources.push(SourceEndpointResolver::new(bytes));
                                entry.insert(source_index);
                                source_index
                            }
                        };
                        PendingSourceMapping::Exact {
                            item_id: item.id.0,
                            file,
                            span,
                            source_index,
                        }
                    }
                    None => {
                        continue;
                    }
                },
                None => PendingSourceMapping::Generated { item_id: item.id.0 },
            };
            if let PendingSourceMapping::Exact {
                span, source_index, ..
            } = &candidate
            {
                sources[*source_index].request(span);
            }
            pending.push(candidate);
        }
        let mut sources = sources.into_iter();
        let mut resolutions = Vec::new();
        let mut mappings = Vec::with_capacity(pending.len());
        for candidate in pending {
            self.usage.checkpoint(CompilerOperation::Rustdoc)?;
            let (rustdoc_item_id, provenance) = match candidate {
                PendingSourceMapping::Generated { item_id } => (
                    item_id,
                    CompilerSourceProvenance::CompilerGenerated {
                        crate_root: self.crate_root.clone(),
                    },
                ),
                PendingSourceMapping::Exact {
                    item_id,
                    file,
                    span,
                    source_index,
                } => {
                    if source_index == resolutions.len() {
                        resolutions.push(
                            sources
                                .next()
                                .expect("source indices follow first item occurrence")
                                .resolve(self.usage, &span.filename)?,
                        );
                    }
                    let (byte_start, byte_end) =
                        resolutions[source_index].byte_range(self.usage, span)?;
                    (
                        item_id,
                        CompilerSourceProvenance::Exact {
                            file,
                            byte_start: u64::try_from(byte_start).map_err(|_| {
                                Self::invalid_coordinate(
                                    span,
                                    "source start byte cannot be represented by the artifact schema",
                                )
                            })?,
                            byte_end: u64::try_from(byte_end).map_err(|_| {
                                Self::invalid_coordinate(
                                    span,
                                    "source end byte cannot be represented by the artifact schema",
                                )
                            })?,
                        },
                    )
                }
            };
            self.usage.account_semantic(
                CompilerOperation::Rustdoc,
                CompilerSemanticResource::SourceMappings,
                1,
            )?;
            mappings.push(CompilerSourcePath {
                rustdoc_item_id,
                provenance,
            });
        }
        Ok(mappings)
    }

    fn mapped_source(&self, span: &RustdocSourceSpan) -> Option<(CatalogPath, &'operation [u8])> {
        let candidate = if span.filename.is_absolute() {
            span.filename.clone()
        } else {
            self.current_directory.join(&span.filename)
        };
        let physical = fs_err::canonicalize(candidate).ok()?;
        let relative = physical.strip_prefix(&self.canonical_root).ok()?;
        let logical = self.logical_path(relative)?;
        self.source_files
            .get(&logical)
            .map(|bytes| (logical, bytes))
    }

    fn logical_path(&self, relative: &Path) -> Option<CatalogPath> {
        let mut components = Vec::new();
        for component in relative.components() {
            let Component::Normal(value) = component else {
                return None;
            };
            PortablePathRules::validate_component(value).ok()?;
            components.push(value.to_str()?.to_owned());
        }
        CatalogPath::new(components.join("/")).ok()
    }

    fn invalid_coordinate(span: &RustdocSourceSpan, message: &str) -> CompilerError {
        CompilerError::InvalidSourceSpan {
            path: span.filename.clone(),
            begin: span.begin,
            end: span.end,
            message: message.to_owned(),
        }
    }
}

enum PendingSourceMapping<'span> {
    Generated {
        item_id: u32,
    },
    Exact {
        item_id: u32,
        file: CatalogPath,
        span: &'span RustdocSourceSpan,
        source_index: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceCoordinate {
    line: usize,
    column: usize,
}

impl From<(usize, usize)> for SourceCoordinate {
    fn from((line, column): (usize, usize)) -> Self {
        Self { line, column }
    }
}

pub(super) struct SourceEndpointResolver<'source> {
    bytes: &'source [u8],
    requested: Vec<SourceCoordinate>,
}

impl<'source> SourceEndpointResolver<'source> {
    const CHECKPOINT_SCALARS: usize = 4 * 1024;

    pub(super) fn new(bytes: &'source [u8]) -> Self {
        Self {
            bytes,
            requested: Vec::new(),
        }
    }

    fn request(&mut self, span: &RustdocSourceSpan) {
        self.requested.extend([
            SourceCoordinate::from(span.begin),
            SourceCoordinate::from(span.end),
        ]);
    }

    fn resolve(
        mut self,
        usage: &CompilerUsage,
        path: &Path,
    ) -> Result<ResolvedSourceEndpoints, CompilerError> {
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let source = std::str::from_utf8(self.bytes).map_err(|source| {
            CompilerError::InvalidMappedSourceUtf8 {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        self.requested.sort_unstable();
        self.requested.dedup();
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let mut endpoints = Vec::with_capacity(self.requested.len());
        let mut request_index = 0;
        let mut line = 1;
        let mut column = 1;
        for (scalar_index, (byte_start, character)) in source.char_indices().enumerate() {
            if scalar_index % Self::CHECKPOINT_SCALARS == 0 {
                usage.checkpoint(CompilerOperation::Rustdoc)?;
            }
            let current = SourceCoordinate { line, column };
            while self
                .requested
                .get(request_index)
                .is_some_and(|requested| *requested < current)
            {
                request_index += 1;
            }
            if self.requested.get(request_index) == Some(&current) {
                endpoints.push((current, byte_start, byte_start + character.len_utf8()));
                request_index += 1;
            }
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        Ok(ResolvedSourceEndpoints {
            line_count: line,
            endpoints,
        })
    }
}

struct ResolvedSourceEndpoints {
    line_count: usize,
    endpoints: Vec<(SourceCoordinate, usize, usize)>,
}

impl ResolvedSourceEndpoints {
    fn byte_range(
        &self,
        usage: &CompilerUsage,
        span: &RustdocSourceSpan,
    ) -> Result<(usize, usize), CompilerError> {
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let start = Self::byte_offset(&self.endpoints, self.line_count, span, span.begin, false)?;
        let end = Self::byte_offset(&self.endpoints, self.line_count, span, span.end, true)?;
        if end <= start {
            return Err(CompilerSourceTranslator::invalid_coordinate(
                span,
                "the inclusive end precedes the beginning",
            ));
        }
        Ok((start, end))
    }

    fn byte_offset(
        endpoints: &[(SourceCoordinate, usize, usize)],
        line_count: usize,
        span: &RustdocSourceSpan,
        coordinate: (usize, usize),
        inclusive_end: bool,
    ) -> Result<usize, CompilerError> {
        let coordinate = SourceCoordinate::from(coordinate);
        if coordinate.line == 0 || coordinate.column == 0 {
            return Err(CompilerSourceTranslator::invalid_coordinate(
                span,
                "line and column are one-indexed",
            ));
        }
        let endpoint = endpoints
            .binary_search_by_key(&coordinate, |(coordinate, _, _)| *coordinate)
            .ok()
            .map(|index| endpoints[index]);
        let Some((_, byte_start, byte_end)) = endpoint else {
            let message = if coordinate.line > line_count {
                "line is outside the source file"
            } else {
                "column is outside the source line"
            };
            return Err(CompilerSourceTranslator::invalid_coordinate(span, message));
        };
        Ok(if inclusive_end { byte_end } else { byte_start })
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    fn source_endpoints_preserve_unicode_inclusive_and_crlf_coordinates() {
        let source = "aéz\r\nnext\n";
        let spans = [
            CompilerFixture::source_span("lib.rs", (2, 1), (2, 1)),
            CompilerFixture::source_span("lib.rs", (1, 5), (1, 5)),
            CompilerFixture::source_span("lib.rs", (1, 2), (1, 2)),
            CompilerFixture::source_span("lib.rs", (1, 4), (1, 4)),
        ];
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let mut resolver = SourceEndpointResolver::new(source.as_bytes());
        for span in &spans {
            resolver.request(span);
        }
        let resolution = resolver
            .resolve(&usage, Path::new("lib.rs"))
            .expect("one source pass");
        let ranges = spans
            .iter()
            .map(|span| {
                resolution
                    .byte_range(&usage, span)
                    .expect("valid scalar coordinate")
            })
            .collect::<Vec<_>>();

        assert_eq!(&source.as_bytes()[ranges[0].0..ranges[0].1], b"n");
        assert_eq!(&source.as_bytes()[ranges[1].0..ranges[1].1], b"\n");
        assert_eq!(&source.as_bytes()[ranges[2].0..ranges[2].1], "é".as_bytes());
        assert_eq!(&source.as_bytes()[ranges[3].0..ranges[3].1], b"\r");
    }

    #[test]
    fn source_endpoint_diagnostics_preserve_one_indexed_line_and_range_rules() {
        let cases = [
            ((0, 1), (1, 1), "line and column are one-indexed"),
            ((1, 0), (1, 1), "line and column are one-indexed"),
            ((3, 1), (3, 1), "line is outside the source file"),
            ((1, 4), (1, 4), "column is outside the source line"),
            ((2, 1), (2, 1), "column is outside the source line"),
            ((1, 2), (1, 1), "the inclusive end precedes the beginning"),
            (
                (1, usize::MAX),
                (1, usize::MAX),
                "column is outside the source line",
            ),
        ];
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        for (begin, end, expected) in cases {
            let span = CompilerFixture::source_span("lib.rs", begin, end);
            let mut resolver = SourceEndpointResolver::new(b"a\n");
            resolver.request(&span);
            let resolution = resolver
                .resolve(&usage, Path::new("lib.rs"))
                .expect("bounded source scan");
            let error = resolution
                .byte_range(&usage, &span)
                .expect_err("invalid coordinates fail closed");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
        }
    }

    #[test]
    fn source_endpoint_storage_is_proportional_to_unique_requests() {
        let source = "a".repeat(1024 * 1024) + "é";
        let span =
            CompilerFixture::source_span("lib.rs", (1, 1024 * 1024 + 1), (1, 1024 * 1024 + 1));
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let mut resolver = SourceEndpointResolver::new(source.as_bytes());
        resolver.request(&span);
        resolver.request(&span);
        assert_eq!(resolver.requested.len(), 4);

        let resolution = resolver
            .resolve(&usage, Path::new("lib.rs"))
            .expect("one long source scan");
        assert_eq!(resolution.endpoints.len(), 1);
        assert!(resolution.endpoints.capacity() < source.len());
        assert_eq!(resolution.endpoints[0].1, 1024 * 1024);
        assert_eq!(resolution.endpoints[0].2, 1024 * 1024 + "é".len());
    }

    #[test]
    fn source_endpoint_scan_observes_cancellation_before_long_line_work() {
        let application = ApplicationCancellation::new();
        let usage = CompilerUsage::new(
            CompilerLimits::default(),
            CompilerCancellation::from_flag(application.flag()),
        );
        application.request();
        let span = CompilerFixture::source_span("lib.rs", (1, 1), (1, 1));
        let source = vec![b'a'; 1024 * 1024];
        let mut resolver = SourceEndpointResolver::new(&source);
        resolver.request(&span);

        assert!(matches!(
            resolver.resolve(&usage, Path::new("lib.rs")),
            Err(CompilerError::CompilerExtractionCancelled)
        ));
    }

    #[test]
    fn rustdoc_document_builds_sorted_exact_and_generated_source_mappings() {
        let source = b"pub fn caf\xc3\xa9() {}\r\n";
        let fixture = SourceMappingFixture::new(&[("lib.rs", source)]);
        let document = CompilerFixture::source_document(
            false,
            [
                CompilerFixture::source_item(9, 0, Some(fixture.span("lib.rs", (1, 1), (1, 16)))),
                CompilerFixture::source_item(1, 0, None),
            ],
        );
        let mappings = fixture
            .translate(&document, CompilerLimits::default())
            .expect("source mappings");

        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.rustdoc_item_id)
                .collect::<Vec<_>>(),
            [0, 1, 9]
        );
        assert_eq!(
            mappings[2].provenance,
            CompilerSourceProvenance::Exact {
                file: CatalogPath::new("lib.rs").expect("logical source"),
                byte_start: 0,
                byte_end: 17,
            }
        );
    }

    #[test]
    fn source_mapping_omits_unallowlisted_and_external_items() {
        let fixture = SourceMappingFixture::new(&[("lib.rs", b"pub fn caf\xc3\xa9() {}\r\n")]);
        std::fs::write(fixture.root.path().join("private.rs"), b"fn private() {}\n")
            .expect("unallowlisted source");
        let document = CompilerFixture::source_document(
            false,
            [
                CompilerFixture::source_item(9, 0, Some(fixture.span("lib.rs", (1, 1), (1, 16)))),
                CompilerFixture::source_item(
                    10,
                    0,
                    Some(fixture.span("private.rs", (1, 1), (1, 15))),
                ),
                CompilerFixture::source_item(
                    99,
                    1,
                    Some(CompilerFixture::source_span(
                        "/external/lib.rs",
                        (1, 1),
                        (1, 1),
                    )),
                ),
            ],
        );
        let mappings = fixture
            .translate(&document, CompilerLimits::default())
            .expect("omitted mappings");

        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.rustdoc_item_id)
                .collect::<Vec<_>>(),
            [0, 9]
        );
    }

    #[test]
    fn source_mapping_preserves_invalid_span_before_limit_error_order() {
        let fixture = SourceMappingFixture::new(&[("lib.rs", b"a\n")]);
        let limits = CompilerLimits {
            source_mappings: 1,
            ..CompilerLimits::default()
        };
        for (column, invalid) in [(3, true), (1, false)] {
            let document = CompilerFixture::source_document(
                false,
                [CompilerFixture::source_item(
                    1,
                    0,
                    Some(fixture.span("lib.rs", (1, column), (1, column))),
                )],
            );
            let error = fixture
                .translate(&document, limits)
                .expect_err("the second retained mapping exceeds or is invalid");
            if invalid {
                assert!(matches!(error, CompilerError::InvalidSourceSpan { .. }));
            } else {
                assert!(matches!(
                    error,
                    CompilerError::CompilerSemanticLimit {
                        resource: CompilerSemanticResource::SourceMappings,
                        limit: 1,
                        observed_at_least: 2,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn source_mapping_reports_earlier_coordinate_before_later_invalid_utf8() {
        let fixture = SourceMappingFixture::new(&[("a.rs", b"a\n"), ("b.rs", &[0xff])]);
        let document = CompilerFixture::source_document(
            false,
            [
                CompilerFixture::source_item(1, 0, Some(fixture.span("a.rs", (1, 3), (1, 3)))),
                CompilerFixture::source_item(2, 0, Some(fixture.span("b.rs", (1, 1), (1, 1)))),
            ],
        );
        let error = fixture
            .translate(&document, CompilerLimits::default())
            .expect_err("earlier invalid coordinate must win");

        assert!(
            matches!(error, CompilerError::InvalidSourceSpan { path, .. } if path.ends_with("a.rs"))
        );
    }
}
