use super::document::{RustContractDocument, RustContractDocuments};
use crate::api::{ResolveSketchesRequest, ResolveSketchesResponse};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemKind};
use quote::ToTokens;
use std::collections::BTreeMap;

pub(in crate::languages::rust::parser) struct RustSketchResolver {
    request: ResolveSketchesRequest,
}

impl RustSketchResolver {
    pub(in crate::languages::rust::parser) fn new(request: ResolveSketchesRequest) -> Self {
        Self { request }
    }

    pub(in crate::languages::rust::parser) fn resolve(
        self,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        let ResolveSketchesRequest {
            source_files,
            contract_files,
        } = self.request;
        let sources = source_files
            .into_entries()
            .filter(|(path, _)| path.has_extension("rs"))
            .map(|(path, bytes)| RustSketchSource::new(path, bytes))
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let documents = RustContractDocuments::parse(contract_files)?;

        let mut seeds = Vec::new();
        for RustContractDocument {
            catalog_name,
            document,
        } in documents.documents
        {
            for signature in document.signatures {
                let Some(sketch_id) = signature.sketch else {
                    continue;
                };
                let entry = signature.entries.first().ok_or_else(|| {
                    SignatureContractKitError::conversion_failed(format!(
                        "signature {} has no structural Rust item",
                        signature.label
                    ))
                })?;
                let source = sources.get(&signature.file).ok_or_else(|| {
                    SignatureContractKitError::parse_failed(
                        &signature.file,
                        "linked sketch source file is missing from source catalog",
                    )
                })?;
                seeds.push(crate::api::ResolvedSketchSeed {
                    contract_file: catalog_name.clone(),
                    sketch_id,
                    signature_type: signature.signature_type.as_str().to_owned(),
                    file: signature.file,
                    code: source.item_text(entry.id())?,
                });
            }
        }
        seeds.sort_by(|left, right| {
            left.contract_file
                .cmp(&right.contract_file)
                .then_with(|| left.sketch_id.cmp(&right.sketch_id))
        });
        Ok(ResolveSketchesResponse { seeds })
    }
}

struct RustSketchSource {
    path: CatalogPath,
    source: String,
    syntax: syn::File,
    line_starts: Vec<usize>,
}

impl RustSketchSource {
    fn new(
        path: CatalogPath,
        bytes: Vec<u8>,
    ) -> Result<(CatalogPath, Self), SignatureContractKitError> {
        let source = String::from_utf8(bytes)
            .map_err(|error| SignatureContractKitError::parse_failed(&path, error.to_string()))?;
        let syntax = syn::parse_file(&source)
            .map_err(|error| SignatureContractKitError::parse_failed(&path, error.to_string()))?;
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Ok((
            path.clone(),
            Self {
                path,
                source,
                syntax,
                line_starts,
            },
        ))
    }

    fn item_text(&self, id: &RustItemId) -> Result<String, SignatureContractKitError> {
        let item = RustSketchItemFinder::new(id)
            .find(&self.syntax.items)
            .ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    &self.path,
                    format!("linked Rust item {} was not found", id.name()),
                )
            })?;
        use syn::spanned::Spanned;
        let span = item.span();
        let start = self.byte_offset(span.start())?;
        let end = self.byte_offset(span.end())?;
        self.source
            .get(start..end)
            .map(str::to_owned)
            .ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    &self.path,
                    format!("source span for {} is outside the file", id.name()),
                )
            })
    }

    fn byte_offset(
        &self,
        location: proc_macro2::LineColumn,
    ) -> Result<usize, SignatureContractKitError> {
        let line_index = location.line.checked_sub(1).ok_or_else(|| {
            SignatureContractKitError::parse_failed(&self.path, "source span has invalid line")
        })?;

        let line_start = self.line_starts.get(line_index).copied().ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                &self.path,
                "source span has invalid line or column",
            )
        })?;
        let line_end = self
            .line_starts
            .get(line_index + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line = self.source.get(line_start..line_end).ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                &self.path,
                "source span has invalid line or column",
            )
        })?;
        let byte_column = line
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(line.len()))
            .nth(location.column)
            .ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    &self.path,
                    "source span has invalid line or column",
                )
            })?;

        line_start.checked_add(byte_column).ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                &self.path,
                "source span has invalid line or column",
            )
        })
    }
}

struct RustSketchItemFinder<'a> {
    id: &'a RustItemId,
}

impl<'a> RustSketchItemFinder<'a> {
    fn new(id: &'a RustItemId) -> Self {
        Self { id }
    }

    fn find<'b>(&self, items: &'b [syn::Item]) -> Option<&'b syn::Item> {
        self.find_in(self.id.module_path(), items)
    }

    fn find_in<'b>(&self, module_path: &[String], items: &'b [syn::Item]) -> Option<&'b syn::Item> {
        if let Some((module, remainder)) = module_path.split_first() {
            let nested = items.iter().find_map(|item| match item {
                syn::Item::Mod(value) if value.ident == module => {
                    value.content.as_ref().map(|(_, items)| items.as_slice())
                }
                _ => None,
            })?;
            return self.find_in(remainder, nested);
        }
        items.iter().find(|item| self.matches(item))
    }

    fn matches(&self, item: &syn::Item) -> bool {
        match (self.id.kind(), item) {
            (RustItemKind::Function, syn::Item::Fn(value)) => value.sig.ident == self.id.name(),
            (RustItemKind::Struct, syn::Item::Struct(value)) => value.ident == self.id.name(),
            (RustItemKind::Enum, syn::Item::Enum(value)) => value.ident == self.id.name(),
            (RustItemKind::Trait, syn::Item::Trait(value)) => value.ident == self.id.name(),
            (RustItemKind::Union, syn::Item::Union(value)) => value.ident == self.id.name(),
            (RustItemKind::Static, syn::Item::Static(value)) => value.ident == self.id.name(),
            (RustItemKind::Macro, syn::Item::Macro(value)) => self.matches_macro(value),
            (RustItemKind::TypeAlias, syn::Item::Type(value)) => value.ident == self.id.name(),
            (RustItemKind::Implementation, _) => false,
            _ => false,
        }
    }

    fn matches_macro(&self, item: &syn::ItemMacro) -> bool {
        match &item.ident {
            Some(identifier) => identifier == self.id.name(),
            None => item.mac.path.to_token_stream().to_string() == self.id.name(),
        }
    }
}
