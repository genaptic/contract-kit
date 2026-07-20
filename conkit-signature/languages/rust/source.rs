use std::collections::BTreeMap;
use std::sync::Arc;

use syn::spanned::Spanned;

use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::parser::source_graph::RustSourceGraphError;
use crate::limits::RustExtractionLimits;
use crate::work::CancellationProbe;

/// Parsed physical-source cache for exactly one allowlist union.
pub(crate) struct RustSourceCatalog {
    sources: BTreeMap<CatalogPath, RustParsedSource>,
    pending: BTreeMap<CatalogPath, Vec<u8>>,
}

impl RustSourceCatalog {
    /// Decodes and parses only the exact physical-file allowlist.
    pub(crate) fn parse_allowlist(
        files: &std::collections::BTreeSet<CatalogPath>,
        catalog: FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        for path in files {
            cancellation.checkpoint()?;
            if catalog.get(path).is_none() {
                return Err(RustSourceGraphError::MissingParticipant { path: path.clone() }.into());
            }
        }

        let mut sources = BTreeMap::new();
        for (path, bytes) in catalog.into_entries() {
            cancellation.checkpoint()?;
            if !files.contains(&path) {
                continue;
            }

            let source = RustParsedSource::parse(&path, bytes, cancellation)?;
            sources.insert(path, source);
        }

        Ok(Self {
            sources,
            pending: BTreeMap::new(),
        })
    }

    /// Retains raw catalog entries for graph-owned, parse-on-required-edge loading.
    pub(crate) fn deferred(
        files: &std::collections::BTreeSet<CatalogPath>,
        catalog: FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut pending = BTreeMap::new();
        for (path, bytes) in catalog.into_entries() {
            cancellation.checkpoint()?;
            if files.contains(&path) {
                pending.insert(path, bytes);
            }
        }
        Ok(Self {
            sources: BTreeMap::new(),
            pending,
        })
    }

    pub(super) fn shared_syntax(&self, path: &CatalogPath) -> Option<Arc<syn::File>> {
        self.sources
            .get(path)
            .map(|source| Arc::clone(&source.syntax))
    }

    pub(super) fn load_syntax(
        &mut self,
        path: &CatalogPath,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Arc<syn::File>, SignatureContractKitError> {
        if let Some(syntax) = self.shared_syntax(path) {
            return Ok(syntax);
        }
        cancellation.checkpoint()?;
        let bytes = self.pending.remove(path).ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                path: path.clone(),
            })
        })?;
        limits.validate_source_count(self.sources.len().saturating_add(1))?;
        limits.validate_source_file(path, bytes.len())?;
        let source = RustParsedSource::parse(path, bytes, cancellation)?;
        let syntax = Arc::clone(&source.syntax);
        self.sources.insert(path.clone(), source);
        Ok(syntax)
    }

    pub(crate) fn source_span(
        &self,
        path: &CatalogPath,
        span: proc_macro2::Span,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        self.sources
            .get(path)
            .ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                    path: path.clone(),
                })
            })?
            .source_span(path, span)
    }

    pub(crate) fn source_span_from_byte_range(
        &self,
        path: &CatalogPath,
        start: usize,
        end: usize,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        self.sources
            .get(path)
            .ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                    path: path.clone(),
                })
            })?
            .source_span_from_byte_range(path, start, end)
    }

    pub(crate) fn source_text(
        &self,
        span: &RustSourceSpan,
    ) -> Result<&str, SignatureContractKitError> {
        self.sources
            .get(span.file())
            .ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                    path: span.file().clone(),
                })
            })?
            .source_text(span)
    }

    /// Resolves a validated rustdoc module-header range to one complete parsed
    /// module declaration in the same physical source.
    pub(super) fn module_for_compiler_header(
        &self,
        header: &RustSourceSpan,
        cancellation: &CancellationProbe,
    ) -> Result<(syn::ItemMod, RustSourceSpan), SignatureContractKitError> {
        self.sources
            .get(header.file())
            .ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                    path: header.file().clone(),
                })
            })?
            .module_for_compiler_header(header, cancellation)
    }

    pub(super) fn syntax_items(
        &self,
        path: &CatalogPath,
        index_path: &[usize],
    ) -> Option<&[syn::Item]> {
        let source = self.sources.get(path)?;
        let mut items = source.syntax.items.as_slice();
        for index in index_path {
            let syn::Item::Mod(module) = items.get(*index)? else {
                return None;
            };
            items = module.content.as_ref()?.1.as_slice();
        }
        Some(items)
    }
}

/// Physical source location retained for extraction and diagnostics, never canonical semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustSourceSpan {
    file: CatalogPath,
    start: usize,
    end: usize,
}

impl RustSourceSpan {
    pub(crate) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(crate) fn byte_range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

struct RustParsedSource {
    source: String,
    syntax: Arc<syn::File>,
    line_starts: Vec<usize>,
}

impl RustParsedSource {
    fn parse(
        path: &CatalogPath,
        bytes: Vec<u8>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let source = String::from_utf8(bytes).map_err(|source| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidUtf8 {
                path: path.clone(),
                message: source.to_string(),
            })
        })?;
        let syntax = syn::parse_file(&source).map(Arc::new).map_err(|source| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidRustSyntax {
                path: path.clone(),
                message: source.to_string(),
            })
        })?;
        cancellation.checkpoint()?;
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if index % 4096 == 0 {
                cancellation.checkpoint()?;
            }
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }

        Ok(Self {
            source,
            syntax,
            line_starts,
        })
    }

    fn source_span(
        &self,
        path: &CatalogPath,
        span: proc_macro2::Span,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        let start = self.byte_offset(path, span.start())?;
        let end = self.byte_offset(path, span.end())?;
        if start > end {
            return Err(RustSourceGraphError::InvalidSourceSpan {
                path: path.clone(),
                message: format!("start byte {start} exceeds end byte {end}"),
            }
            .into());
        }

        Ok(RustSourceSpan {
            file: path.clone(),
            start,
            end,
        })
    }

    fn source_span_from_byte_range(
        &self,
        path: &CatalogPath,
        start: usize,
        end: usize,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        let span = RustSourceSpan {
            file: path.clone(),
            start,
            end,
        };
        self.source_text(&span)?;
        Ok(span)
    }

    fn source_text(&self, span: &RustSourceSpan) -> Result<&str, SignatureContractKitError> {
        if span.start > span.end {
            return Err(RustSourceGraphError::InvalidSourceSpan {
                path: span.file().clone(),
                message: format!("start byte {} exceeds end byte {}", span.start, span.end),
            }
            .into());
        }

        self.source.get(span.byte_range()).ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                path: span.file().clone(),
                message: format!(
                    "byte range {}..{} is outside a {}-byte source",
                    span.start,
                    span.end,
                    self.source.len()
                ),
            })
        })
    }

    fn module_for_compiler_header(
        &self,
        header: &RustSourceSpan,
        cancellation: &CancellationProbe,
    ) -> Result<(syn::ItemMod, RustSourceSpan), SignatureContractKitError> {
        cancellation.checkpoint()?;
        self.source_text(header)?;

        let mut pending = self.syntax.items.iter().rev().collect::<Vec<_>>();
        let mut matched = None;
        while let Some(item) = pending.pop() {
            cancellation.checkpoint()?;
            let syn::Item::Mod(module) = item else {
                continue;
            };
            if let Some((_, nested)) = &module.content {
                pending.extend(nested.iter().rev());
            }
            let module_span = self.source_span(header.file(), module.span())?;
            let identifier_span = self.source_span(header.file(), module.ident.span())?;
            if module_span.start <= header.start
                && header.end <= module_span.end
                && header.start <= identifier_span.start
                && identifier_span.end <= header.end
            {
                if matched.is_some() {
                    return Err(SignatureContractKitError::conversion_failed(format!(
                        "compiler module header {}..{} ambiguously identifies multiple declarations",
                        header.start, header.end,
                    )));
                }
                matched = Some((module.clone(), module_span));
            }
        }

        cancellation.checkpoint()?;
        matched.ok_or_else(|| {
            SignatureContractKitError::conversion_failed(format!(
                "compiler module header {}..{} does not identify a parsed module declaration",
                header.start, header.end,
            ))
        })
    }

    fn byte_offset(
        &self,
        path: &CatalogPath,
        location: proc_macro2::LineColumn,
    ) -> Result<usize, SignatureContractKitError> {
        let line_index = location.line.checked_sub(1).ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                path: path.clone(),
                message: "line numbers are one-based".to_owned(),
            })
        })?;
        let line_start = self.line_starts.get(line_index).copied().ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                path: path.clone(),
                message: format!("line {} is outside the source", location.line),
            })
        })?;
        let line_end = self
            .line_starts
            .get(line_index + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line = self.source.get(line_start..line_end).ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                path: path.clone(),
                message: format!("line {} has an invalid byte range", location.line),
            })
        })?;
        let byte_column = line
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(line.len()))
            .nth(location.column)
            .ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                    path: path.clone(),
                    message: format!(
                        "column {} is outside line {}",
                        location.column, location.line
                    ),
                })
            })?;

        line_start.checked_add(byte_column).ok_or_else(|| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidSourceSpan {
                path: path.clone(),
                message: "source span byte offset overflowed".to_owned(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RustSourceCatalog, RustSourceSpan};
    use crate::api::{RustCrateKind, RustCrateRoot};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::languages::rust::parser::source_graph::RustExtraction;
    use crate::work::CancellationProbe;
    use proc_macro2::LineColumn;
    use std::collections::BTreeSet;
    use syn::spanned::Spanned;

    struct SourceFixture {
        path: CatalogPath,
        source: String,
    }

    impl SourceFixture {
        fn new(source: &str) -> Self {
            Self {
                path: CatalogPath::new("lib.rs").expect("valid fixture path"),
                source: source.to_owned(),
            }
        }

        fn parse(self) -> (CatalogPath, String, RustSourceCatalog) {
            let cancellation = CancellationProbe::new();
            let mut catalog = FileCatalog::new();
            catalog
                .insert(self.path.clone(), self.source.as_bytes().to_vec())
                .expect("unique fixture path");
            let extraction = RustExtraction::from_roots(
                BTreeSet::from([self.path.clone()]),
                [RustCrateRoot {
                    id: "sample".to_owned(),
                    root: self.path.clone(),
                    kind: RustCrateKind::Library,
                }],
                &cancellation,
            )
            .expect("valid extraction");
            let parsed =
                RustSourceCatalog::parse_allowlist(extraction.files(), catalog, &cancellation)
                    .expect("valid Rust source");

            (self.path, self.source, parsed)
        }
    }

    #[test]
    fn overlapping_document_projections_reuse_one_parsed_physical_source_owner() {
        let path = crate::files::CatalogPath::new("lib.rs").expect("source path");
        let mut catalog = crate::files::FileCatalog::new();
        catalog
            .insert(path.clone(), b"pub fn answer() {}\n".to_vec())
            .expect("source insert");
        let allowlist = std::collections::BTreeSet::from([path.clone()]);
        let parsed = super::RustSourceCatalog::parse_allowlist(
            &allowlist,
            catalog,
            &crate::work::CancellationProbe::new(),
        )
        .expect("one cached physical source parse");

        let first_projection = parsed
            .shared_syntax(&path)
            .expect("first document projection syntax");
        let second_projection = parsed
            .shared_syntax(&path)
            .expect("second document projection syntax");

        assert!(
            std::sync::Arc::ptr_eq(&first_projection, &second_projection),
            "overlapping documents must borrow the one cached syn tree"
        );
    }

    #[test]
    fn source_spans_preserve_exact_unicode_item_text() {
        let (path, source, catalog) = SourceFixture::new(
            "const PREFIX: &str = \"🦀\"; pub fn target() -> &'static str { \"λ\" }",
        )
        .parse();
        let syntax = catalog.shared_syntax(&path).expect("parsed syntax");
        let item = syntax.items.last().expect("target item");

        let span = catalog
            .source_span(&path, item.span())
            .expect("valid item span");

        assert_eq!(
            catalog.source_text(&span).expect("exact source text"),
            "pub fn target() -> &'static str { \"λ\" }"
        );
        assert_eq!(span.file(), &path);
        assert_eq!(
            span.byte_range().start,
            source.find("pub fn target").expect("target byte offset")
        );
        assert_eq!(span.byte_range().end, source.len());
    }

    #[test]
    fn source_parsing_observes_cancellation_before_line_indexing() {
        let path = CatalogPath::new("lib.rs").expect("path");
        let mut catalog = FileCatalog::new();
        catalog
            .insert(path.clone(), b"pub fn answer() {}\n".to_vec())
            .expect("source");
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let result =
            RustSourceCatalog::parse_allowlist(&BTreeSet::from([path]), catalog, &cancellation);
        let Err(error) = result else {
            panic!("canceled source parsing must stop");
        };

        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn allowlist_selection_observes_cancellation_while_skipping_unlisted_entries() {
        let path = CatalogPath::new("unlisted.rs").expect("path");
        let mut catalog = FileCatalog::new();
        catalog
            .insert(path, b"pub fn ignored() {}\n".to_vec())
            .expect("source");
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let result = RustSourceCatalog::parse_allowlist(&BTreeSet::new(), catalog, &cancellation);
        let Err(error) = result else {
            panic!("canceled allowlist selection must stop while skipping entries");
        };

        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn deferred_source_selection_observes_cancellation_before_scanning_catalog_entries() {
        let path = CatalogPath::new("lib.rs").expect("path");
        let mut catalog = FileCatalog::new();
        catalog
            .insert(path.clone(), b"pub fn answer() {}\n".to_vec())
            .expect("source");
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let result = RustSourceCatalog::deferred(&BTreeSet::from([path]), catalog, &cancellation);
        let Err(error) = result else {
            panic!("canceled deferred source selection must stop");
        };

        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn line_columns_count_unicode_scalars_before_converting_to_bytes() {
        let (path, source, catalog) =
            SourceFixture::new("const CRAB: &str = \"🦀\"; pub fn target() {}").parse();
        let parsed = catalog.sources.get(&path).expect("parsed source");
        let prefix = source
            .split_once("pub fn target")
            .expect("target delimiter")
            .0;

        let offset = parsed
            .byte_offset(
                &path,
                LineColumn {
                    line: 1,
                    column: prefix.chars().count(),
                },
            )
            .expect("Unicode-aware byte offset");

        assert_eq!(offset, prefix.len());
    }

    #[test]
    fn multiline_source_spans_use_retained_line_starts() {
        let (path, _, catalog) = SourceFixture::new(
            "const BEFORE: usize = 1;\n\npub fn target(\n    value: &str,\n) -> &str {\n    value\n}\n\nconst AFTER: usize = 2;",
        )
        .parse();
        let syntax = catalog.shared_syntax(&path).expect("parsed syntax");
        let item = syntax.items.get(1).expect("target item");
        let span = catalog
            .source_span(&path, item.span())
            .expect("valid multiline span");

        assert_eq!(
            catalog.source_text(&span).expect("multiline source text"),
            "pub fn target(\n    value: &str,\n) -> &str {\n    value\n}"
        );
    }

    #[test]
    fn invalid_line_columns_and_byte_ranges_fail_without_panicking() {
        let (path, source, catalog) = SourceFixture::new("pub fn target() {}").parse();
        let parsed = catalog.sources.get(&path).expect("parsed source");

        for location in [
            LineColumn { line: 0, column: 0 },
            LineColumn { line: 2, column: 0 },
            LineColumn {
                line: 1,
                column: source.chars().count() + 1,
            },
        ] {
            let error = parsed
                .byte_offset(&path, location)
                .expect_err("invalid location must fail");
            assert!(error.to_string().contains("source span"));
        }

        let reversed = RustSourceSpan {
            file: path.clone(),
            start: source.len(),
            end: 0,
        };
        let out_of_bounds = RustSourceSpan {
            file: path,
            start: 0,
            end: source.len() + 1,
        };

        assert!(catalog.source_text(&reversed).is_err());
        assert!(catalog.source_text(&out_of_bounds).is_err());
    }
}
