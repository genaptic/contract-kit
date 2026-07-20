use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use proc_macro2::{TokenStream, TokenTree};
use serde::Serialize;
use syn::ext::IdentExt as _;
use syn::parse::Parser;

use crate::api::{RustCrateKind, RustCrateRoot};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::source::{RustSourceCatalog, RustSourceSpan};
use crate::languages::rust::types::declaration::RustDeclarationCapability;
use crate::limits::{
    DiagnosticEvidenceUsage, DiagnosticLimits, RustExtractionLimits, RustExtractionUsage,
};
use crate::work::CancellationProbe;

/// Stable caller-supplied identity for one Rust crate extraction root.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub(crate) struct RustCrateId(String);

impl RustCrateId {
    pub(crate) fn new(
        value: impl Into<String>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let value = value.into();
        if value.is_empty() {
            return Err(RustSourceGraphError::InvalidCrateId {
                value,
                reason: "crate id must not be empty".to_owned(),
            }
            .into());
        }
        for character in value.chars() {
            cancellation.checkpoint()?;
            let reason = if character.is_whitespace() {
                Some("crate id must not contain whitespace")
            } else if matches!(character, ':' | '=') {
                Some("crate id must not contain `:` or `=` identity delimiters")
            } else if character.is_control() {
                Some("crate id must not contain control characters")
            } else {
                None
            };
            if let Some(reason) = reason {
                return Err(RustSourceGraphError::InvalidCrateId {
                    value,
                    reason: reason.to_owned(),
                }
                .into());
            }
        }

        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RustCrateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One explicit crate identity, root file, and target role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustCrate {
    id: RustCrateId,
    root: CatalogPath,
    kind: RustCrateKind,
}

impl RustCrate {
    fn new(
        id: impl Into<String>,
        root: CatalogPath,
        kind: RustCrateKind,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if !root.has_extension("rs") {
            return Err(RustSourceGraphError::NonRustSource { path: root }.into());
        }

        Ok(Self {
            id: RustCrateId::new(id, cancellation)?,
            root,
            kind,
        })
    }

    pub(crate) fn id(&self) -> &RustCrateId {
        &self.id
    }

    pub(crate) fn root(&self) -> &CatalogPath {
        &self.root
    }

    pub(crate) fn kind(&self) -> RustCrateKind {
        self.kind
    }
}

/// Exact syntax-extraction participation and crate-root context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustExtraction {
    files: BTreeSet<CatalogPath>,
    crates: Vec<RustCrate>,
}

impl RustExtraction {
    pub(crate) fn from_roots(
        files: impl IntoIterator<Item = CatalogPath>,
        roots: impl IntoIterator<Item = RustCrateRoot>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut canonical_files = BTreeSet::new();
        for path in files {
            cancellation.checkpoint()?;
            canonical_files.insert(path);
        }

        let mut crates = Vec::new();
        for root in roots {
            cancellation.checkpoint()?;
            crates.push(RustCrate::new(root.id, root.root, root.kind, cancellation)?);
        }

        if crates.is_empty() {
            return Err(RustSourceGraphError::MissingCrateRoot.into());
        }
        for path in &canonical_files {
            cancellation.checkpoint()?;
            if !path.has_extension("rs") {
                return Err(RustSourceGraphError::NonRustSource { path: path.clone() }.into());
            }
        }

        let mut crates_by_id = BTreeMap::new();
        for crate_root in crates {
            cancellation.checkpoint()?;
            let crate_id = crate_root.id.clone();
            if crates_by_id.insert(crate_id.clone(), crate_root).is_some() {
                return Err(RustSourceGraphError::DuplicateCrateId { id: crate_id }.into());
            }
        }
        let mut crates = Vec::with_capacity(crates_by_id.len());
        for crate_root in crates_by_id.into_values() {
            cancellation.checkpoint()?;
            if !canonical_files.contains(crate_root.root()) {
                return Err(RustSourceGraphError::UnlistedCrateRoot {
                    crate_id: crate_root.id.clone(),
                    root: crate_root.root.clone(),
                }
                .into());
            }
            crates.push(crate_root);
        }

        Ok(Self {
            files: canonical_files,
            crates,
        })
    }

    pub(crate) fn files(&self) -> &BTreeSet<CatalogPath> {
        &self.files
    }

    pub(crate) fn crates(&self) -> &[RustCrate] {
        &self.crates
    }

    pub(crate) fn contains_file(&self, path: &CatalogPath) -> bool {
        self.files.contains(path)
    }
}

/// Canonical logical module path below one crate root.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub(crate) struct RustModulePath(Vec<String>);

impl RustModulePath {
    pub(crate) fn new(segments: Vec<String>) -> Result<Self, SignatureContractKitError> {
        let value = segments.join("::");
        for segment in &segments {
            Self::validate_declaration_segment(segment, &value)?;
        }

        Ok(Self(segments))
    }

    pub(crate) fn canonical_declaration_segment(
        value: String,
    ) -> Result<String, SignatureContractKitError> {
        Self::validate_declaration_segment(&value, &value)?;
        Ok(value)
    }

    pub(crate) fn semantic_ident(ident: &syn::Ident) -> String {
        ident.unraw().to_string()
    }

    pub(crate) fn source_ident(value: &str) -> String {
        if syn::parse_str::<syn::Ident>(value).is_ok() {
            return value.to_owned();
        }

        let raw = format!("r#{value}");
        syn::parse_str::<syn::Ident>(&raw).map_or_else(|_| value.to_owned(), |_| raw)
    }

    pub(crate) fn semantic_path_segments(path: &syn::Path) -> Vec<String> {
        path.segments
            .iter()
            .map(|segment| Self::semantic_ident(&segment.ident))
            .collect()
    }

    fn validate_declaration_segment(
        segment: &str,
        identity: &str,
    ) -> Result<(), SignatureContractKitError> {
        let invalid = |reason| {
            SignatureContractKitError::from(RustSourceGraphError::InvalidModuleIdentity {
                value: identity.to_owned(),
                reason,
            })
        };
        if segment.is_empty() {
            return Err(invalid("module path segments must not be empty".to_owned()));
        }
        if segment.trim() != segment {
            return Err(invalid(
                "module path segments must not have surrounding whitespace".to_owned(),
            ));
        }
        if segment.starts_with("r#") {
            return Err(invalid(format!(
                "module segment {segment:?} is not canonical; omit the raw-identifier prefix"
            )));
        }
        if matches!(segment, "_" | "crate" | "self" | "Self" | "super") {
            return Err(invalid(format!(
                "module segment {segment:?} is reserved and cannot name a declaration"
            )));
        }
        let parsed = syn::parse_str::<syn::Ident>(segment)
            .or_else(|_| syn::parse_str::<syn::Ident>(&format!("r#{segment}")))
            .map_err(|source| invalid(source.to_string()))?;
        if Self::semantic_ident(&parsed).as_str() != segment {
            return Err(invalid(format!(
                "module segment {segment:?} is not canonical"
            )));
        }
        Ok(())
    }

    fn child(&self, name: String) -> Self {
        let mut segments = self.0.clone();
        segments.push(name);
        Self(segments)
    }

    fn parent(&self) -> Option<Self> {
        let mut segments = self.0.clone();
        segments.pop()?;
        Some(Self(segments))
    }

    fn is_prefix_of(&self, other: &Self) -> bool {
        other.0.starts_with(&self.0)
    }

    pub(crate) fn segments(&self) -> &[String] {
        &self.0
    }
}

/// Canonical crate-plus-module identity used by the syntax graph.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct RustModuleId {
    crate_id: RustCrateId,
    module_path: RustModulePath,
}

impl RustModuleId {
    pub(crate) fn new(crate_id: RustCrateId, module_path: RustModulePath) -> Self {
        Self {
            crate_id,
            module_path,
        }
    }

    fn root(crate_id: RustCrateId) -> Self {
        Self::new(crate_id, RustModulePath::default())
    }

    fn child(&self, name: String) -> Self {
        Self {
            crate_id: self.crate_id.clone(),
            module_path: self.module_path.child(name),
        }
    }

    pub(crate) fn crate_root(&self) -> Self {
        Self::root(self.crate_id.clone())
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        self.module_path
            .parent()
            .map(|module_path| Self::new(self.crate_id.clone(), module_path))
    }

    pub(crate) fn is_ancestor_of(&self, other: &Self) -> bool {
        self.crate_id == other.crate_id && self.module_path.is_prefix_of(&other.module_path)
    }

    pub(crate) fn is_strict_ancestor_of(&self, other: &Self) -> bool {
        self != other && self.is_ancestor_of(other)
    }

    pub(crate) fn crate_id(&self) -> &RustCrateId {
        &self.crate_id
    }

    pub(crate) fn module_path(&self) -> &RustModulePath {
        &self.module_path
    }
}

/// Logical modules whose source spans are required by a linked-sketch projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustRequiredModules {
    modules: BTreeSet<RustModuleId>,
}

impl RustRequiredModules {
    pub(crate) fn new(
        modules: BTreeSet<RustModuleId>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(Self { modules })
    }

    fn traverses(&self, module: &RustModuleId) -> bool {
        self.modules
            .range(module.clone()..)
            .next()
            .is_some_and(|required| module.is_ancestor_of(required))
    }
}

impl fmt::Display for RustModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.crate_id.as_str())?;
        for segment in self.module_path.segments() {
            write!(formatter, "::{segment}")?;
        }
        Ok(())
    }
}

/// One logical module projection over a parsed physical source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustModule {
    id: RustModuleId,
    file: CatalogPath,
    syntax_index_path: Vec<usize>,
}

impl RustModule {
    pub(crate) fn id(&self) -> &RustModuleId {
        &self.id
    }

    pub(crate) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(crate) fn items<'source>(
        &self,
        sources: &'source RustSourceCatalog,
    ) -> Result<&'source [syn::Item], SignatureContractKitError> {
        sources
            .syntax_items(&self.file, &self.syntax_index_path)
            .ok_or_else(|| {
                SignatureContractKitError::from(
                    RustSourceGraphError::InvalidModuleSourceProjection {
                        module: self.id.clone(),
                        file: self.file.clone(),
                    },
                )
            })
    }
}

/// Honest syntax-mode warning for semantics the source graph retains but cannot evaluate.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RustAssociatedMacroContainer {
    Trait,
    Implementation,
}

impl fmt::Display for RustAssociatedMacroContainer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trait => formatter.write_str("trait"),
            Self::Implementation => formatter.write_str("implementation"),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RustCapabilityDiagnostic {
    ConditionalModule {
        module: RustModuleId,
    },
    DeclarationSemantics {
        id: RustItemId,
        file: CatalogPath,
        reason: RustDeclarationCapability,
        start: usize,
        end: usize,
    },
    PrivateImportSemantics {
        module: RustModuleId,
        file: CatalogPath,
        start: usize,
        end: usize,
    },
    AssociatedMacro {
        module: RustModuleId,
        file: CatalogPath,
        container: RustAssociatedMacroContainer,
        start: usize,
        end: usize,
    },
}

impl RustCapabilityDiagnostic {
    pub(crate) fn warning_message(&self) -> String {
        format!("rust_syntax_v2 capability warning: {self}")
    }

    pub(crate) fn declaration(
        id: RustItemId,
        reason: RustDeclarationCapability,
        span: RustSourceSpan,
    ) -> Self {
        let file = span.file().clone();
        let range = span.byte_range();
        Self::DeclarationSemantics {
            id,
            file,
            reason,
            start: range.start,
            end: range.end,
        }
    }

    pub(crate) fn associated_macro(
        module: RustModuleId,
        container: RustAssociatedMacroContainer,
        span: RustSourceSpan,
    ) -> Self {
        let file = span.file().clone();
        let range = span.byte_range();
        Self::AssociatedMacro {
            module,
            file,
            container,
            start: range.start,
            end: range.end,
        }
    }

    pub(crate) fn private_import(module: RustModuleId, span: RustSourceSpan) -> Self {
        let file = span.file().clone();
        let range = span.byte_range();
        Self::PrivateImportSemantics {
            module,
            file,
            start: range.start,
            end: range.end,
        }
    }
}

impl fmt::Display for RustCapabilityDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConditionalModule { module } => write!(
                formatter,
                "cfg/cfg_attr on module {module} is retained as syntax but not evaluated"
            ),
            Self::DeclarationSemantics {
                id,
                file,
                reason,
                start,
                end,
            } => write!(
                formatter,
                "Rust declaration {} in {file} ({}) at bytes {start}..{end} retains {reason} that rust_syntax_v2 cannot evaluate",
                id.render(),
                id.module_id(),
            ),
            Self::PrivateImportSemantics {
                module,
                file,
                start,
                end,
            } => write!(
                formatter,
                "private import in {file} ({module}) at bytes {start}..{end} retains semantics that syntax extraction cannot evaluate"
            ),
            Self::AssociatedMacro {
                module,
                file,
                container,
                start,
                end,
            } => write!(
                formatter,
                "macro in {container} item in {file} ({module}) at bytes {start}..{end} is retained as a syntax-mode capability warning"
            ),
        }
    }
}

/// Operation-owned, incrementally deduplicated syntax-capability evidence.
pub(crate) struct RustCapabilityDiagnostics<'limits> {
    usage: DiagnosticEvidenceUsage<'limits>,
    values: BTreeSet<RustCapabilityDiagnostic>,
}

impl<'limits> RustCapabilityDiagnostics<'limits> {
    pub(crate) fn new(limits: &'limits DiagnosticLimits) -> Self {
        Self {
            usage: limits.evidence_usage(),
            values: BTreeSet::new(),
        }
    }

    pub(crate) fn insert(
        &mut self,
        diagnostic: RustCapabilityDiagnostic,
    ) -> Result<bool, SignatureContractKitError> {
        if self.values.contains(&diagnostic) {
            return Ok(false);
        }
        let message = diagnostic.warning_message();
        self.usage.record_text(&message)?;
        Ok(self.values.insert(diagnostic))
    }

    pub(crate) fn into_values(
        self,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustCapabilityDiagnostic>, SignatureContractKitError> {
        let mut diagnostics = Vec::with_capacity(self.values.len());
        for diagnostic in self.values {
            cancellation.checkpoint()?;
            diagnostics.push(diagnostic);
        }
        Ok(diagnostics)
    }

    pub(crate) fn into_warning_messages(
        self,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<String>, SignatureContractKitError> {
        let mut messages = Vec::with_capacity(self.values.len());
        for diagnostic in self.values {
            cancellation.checkpoint()?;
            messages.push(diagnostic.warning_message());
        }
        Ok(messages)
    }
}

/// Allowlist-bounded logical Rust module graph for one extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustSourceGraph {
    modules: Vec<RustModule>,
}

impl RustSourceGraph {
    pub(crate) fn build(
        extraction: &RustExtraction,
        sources: &RustSourceCatalog,
        usage: &mut RustExtractionUsage<'_>,
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let graph = RustSourceGraphBuilder::new(
            extraction,
            sources,
            usage,
            diagnostics,
            cancellation,
            RustGraphScope::Complete,
        )
        .build()?;
        Ok(graph)
    }

    pub(crate) fn build_required(
        extraction: &RustExtraction,
        sources: &mut RustSourceCatalog,
        limits: &RustExtractionLimits,
        usage: &mut RustExtractionUsage<'_>,
        required: RustRequiredModules,
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let graph = RustSourceGraphBuilder::new_required(
            extraction,
            sources,
            limits,
            usage,
            diagnostics,
            cancellation,
            RustGraphScope::Required(required),
        )
        .build()?;
        Ok(graph)
    }

    pub(crate) fn modules(&self) -> &[RustModule] {
        &self.modules
    }
}

enum RustGraphScope {
    Complete,
    Required(RustRequiredModules),
}

impl RustGraphScope {
    fn traverses(&self, module: &RustModuleId) -> bool {
        match self {
            Self::Complete => true,
            Self::Required(required) => required.traverses(module),
        }
    }

    fn requires_every_allowlisted_file(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

enum RustGraphSources<'source> {
    Eager(&'source RustSourceCatalog),
    Deferred {
        catalog: &'source mut RustSourceCatalog,
        limits: &'source RustExtractionLimits,
    },
}

impl RustGraphSources<'_> {
    fn syntax(
        &mut self,
        path: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Arc<syn::File>, SignatureContractKitError> {
        match self {
            Self::Eager(catalog) => catalog.shared_syntax(path).ok_or_else(|| {
                SignatureContractKitError::from(RustSourceGraphError::MissingParticipant {
                    path: path.clone(),
                })
            }),
            Self::Deferred { catalog, limits } => catalog.load_syntax(path, limits, cancellation),
        }
    }
}

struct RustSourceGraphBuilder<'source, 'usage, 'limits, 'diagnostics, 'diagnostic_limits> {
    extraction: &'source RustExtraction,
    sources: RustGraphSources<'source>,
    usage: &'usage mut RustExtractionUsage<'limits>,
    diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
    cancellation: &'source CancellationProbe,
    scope: RustGraphScope,
    modules: BTreeMap<RustModuleId, RustModule>,
    claimed_files: BTreeSet<CatalogPath>,
    active_files: Vec<CatalogPath>,
    conditional_modules: BTreeSet<RustModuleId>,
}

impl<'source, 'usage, 'limits, 'diagnostics, 'diagnostic_limits>
    RustSourceGraphBuilder<'source, 'usage, 'limits, 'diagnostics, 'diagnostic_limits>
{
    fn new(
        extraction: &'source RustExtraction,
        sources: &'source RustSourceCatalog,
        usage: &'usage mut RustExtractionUsage<'limits>,
        diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
        cancellation: &'source CancellationProbe,
        scope: RustGraphScope,
    ) -> Self {
        Self {
            extraction,
            sources: RustGraphSources::Eager(sources),
            usage,
            diagnostics,
            cancellation,
            scope,
            modules: BTreeMap::new(),
            claimed_files: BTreeSet::new(),
            active_files: Vec::new(),
            conditional_modules: BTreeSet::new(),
        }
    }

    fn new_required(
        extraction: &'source RustExtraction,
        sources: &'source mut RustSourceCatalog,
        limits: &'source RustExtractionLimits,
        usage: &'usage mut RustExtractionUsage<'limits>,
        diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
        cancellation: &'source CancellationProbe,
        scope: RustGraphScope,
    ) -> Self {
        Self {
            extraction,
            sources: RustGraphSources::Deferred {
                catalog: sources,
                limits,
            },
            usage,
            diagnostics,
            cancellation,
            scope,
            modules: BTreeMap::new(),
            claimed_files: BTreeSet::new(),
            active_files: Vec::new(),
            conditional_modules: BTreeSet::new(),
        }
    }

    fn build(mut self) -> Result<RustSourceGraph, SignatureContractKitError> {
        let crate_roots = self.extraction.crates().to_vec();
        for crate_root in &crate_roots {
            self.cancellation.checkpoint()?;
            let context = RustModuleContext::root(crate_root);
            if !self.scope.traverses(&context.id) {
                continue;
            }
            let syntax = self.sources.syntax(crate_root.root(), self.cancellation)?;
            self.collect_module(context, &syntax.items, true)?;
        }

        if self.scope.requires_every_allowlisted_file()
            && let Some(path) = self
                .extraction
                .files()
                .difference(&self.claimed_files)
                .next()
        {
            return Err(RustSourceGraphError::DisconnectedSource { path: path.clone() }.into());
        }

        let mut modules = Vec::with_capacity(self.modules.len());
        for module in self.modules.into_values() {
            self.cancellation.checkpoint()?;
            modules.push(module);
        }
        Ok(RustSourceGraph { modules })
    }

    fn collect_module(
        &mut self,
        context: RustModuleContext,
        items: &[syn::Item],
        enters_physical_file: bool,
    ) -> Result<(), SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        if enters_physical_file {
            if let Some(cycle_start) = self
                .active_files
                .iter()
                .position(|file| file == &context.file)
            {
                let mut chain =
                    Vec::with_capacity(self.active_files.len().saturating_sub(cycle_start) + 1);
                for file in &self.active_files[cycle_start..] {
                    self.cancellation.checkpoint()?;
                    chain.push(file.to_string());
                }
                chain.push(context.file.to_string());
                return Err(RustSourceGraphError::ModuleCycle {
                    module: context.id,
                    chain: chain.join(" -> "),
                }
                .into());
            }
            self.active_files.push(context.file.clone());
        }
        if let Some(existing) = self.modules.get(&context.id) {
            if self.conditional_modules.contains(&context.id) {
                return Err(RustSourceGraphError::ConditionalDuplicateLogicalModule {
                    module: context.id,
                    existing: existing.file.clone(),
                    incoming: context.file,
                }
                .into());
            }
            return Err(RustSourceGraphError::DuplicateLogicalModule {
                module: context.id,
                existing: existing.file.clone(),
                incoming: context.file,
            }
            .into());
        }

        self.claimed_files.insert(context.file.clone());

        for (item_index, item) in items.iter().enumerate() {
            self.cancellation.checkpoint()?;
            self.record_encountered_item(item, &context.file)?;
            let syn::Item::Mod(module) = item else {
                continue;
            };
            let name = RustModulePath::semantic_ident(&module.ident);
            let child_id = context.id.child(name.clone());
            if !self.scope.traverses(&child_id) {
                continue;
            }
            if self.is_conditional(module)? {
                self.conditional_modules.insert(child_id.clone());
                self.diagnostics
                    .insert(RustCapabilityDiagnostic::ConditionalModule {
                        module: child_id.clone(),
                    })?;
            }

            let explicit_path = context.path_attribute(&child_id, module, self.cancellation)?;
            if let Some((_, inline_items)) = &module.content {
                let child = match explicit_path {
                    Some(path) => context.path_directed_inline_child(name, &path, item_index),
                    None => context.inline_child(name, item_index),
                };
                self.collect_module(child, inline_items, false)?;
                continue;
            }

            let target = match explicit_path {
                Some(path) => {
                    if !self.extraction.contains_file(&path) {
                        return Err(RustSourceGraphError::UnlistedModuleTarget {
                            module: child_id,
                            declared_in: context.file.clone(),
                            candidates: path.to_string(),
                        }
                        .into());
                    }
                    path
                }
                None => self.conventional_target(&context, &child_id, &name)?,
            };
            let syntax = self.sources.syntax(&target, self.cancellation)?;
            let child = context.file_child(name, target);
            self.collect_module(child, &syntax.items, true)?;
        }

        if enters_physical_file {
            self.active_files.pop();
        }
        self.modules.insert(
            context.id.clone(),
            RustModule {
                id: context.id,
                file: context.file,
                syntax_index_path: context.syntax_index_path,
            },
        );
        Ok(())
    }

    fn record_encountered_item(
        &mut self,
        item: &syn::Item,
        file: &CatalogPath,
    ) -> Result<(), SignatureContractKitError> {
        self.usage.record_item(Some(file))?;
        let associated_items = match item {
            syn::Item::Trait(item) => item.items.len(),
            syn::Item::Impl(item) => item.items.len(),
            syn::Item::ForeignMod(item) => item.items.len(),
            _ => 0,
        };
        self.usage.record_items(associated_items, Some(file))?;
        Ok(())
    }

    fn conventional_target(
        &self,
        context: &RustModuleContext,
        child: &RustModuleId,
        name: &str,
    ) -> Result<CatalogPath, RustSourceGraphError> {
        let candidates = context.conventional_targets(name)?;
        let selected = candidates
            .iter()
            .filter(|path| self.extraction.contains_file(path))
            .cloned()
            .collect::<Vec<_>>();

        match selected.as_slice() {
            [path] => Ok(path.clone()),
            [] => Err(RustSourceGraphError::UnlistedModuleTarget {
                module: child.clone(),
                declared_in: context.file.clone(),
                candidates: candidates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" or "),
            }),
            _ => Err(RustSourceGraphError::AmbiguousModuleTarget {
                module: child.clone(),
                declared_in: context.file.clone(),
                candidates: selected
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" and "),
            }),
        }
    }

    fn is_conditional(&self, module: &syn::ItemMod) -> Result<bool, SignatureContractKitError> {
        for attribute in &module.attrs {
            self.cancellation.checkpoint()?;
            if attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr") {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Clone)]
struct RustModuleContext {
    id: RustModuleId,
    file: CatalogPath,
    syntax_index_path: Vec<usize>,
    // Physical base for conventional `mod child;` targets.
    module_directory: Vec<String>,
    // Physical source/inline base for direct `#[path]` attributes.
    path_attribute_directory: Vec<String>,
}

impl RustModuleContext {
    fn root(crate_root: &RustCrate) -> Self {
        let mut physical_directory = crate_root
            .root()
            .as_str()
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        physical_directory.pop();
        Self {
            id: RustModuleId::root(crate_root.id().clone()),
            file: crate_root.root().clone(),
            syntax_index_path: Vec::new(),
            module_directory: physical_directory.clone(),
            path_attribute_directory: physical_directory,
        }
    }

    fn inline_child(&self, name: String, item_index: usize) -> Self {
        let mut module_directory = self.module_directory.clone();
        module_directory.push(name.clone());
        let mut syntax_index_path = self.syntax_index_path.clone();
        syntax_index_path.push(item_index);
        Self {
            id: self.id.child(name),
            file: self.file.clone(),
            syntax_index_path,
            path_attribute_directory: module_directory.clone(),
            module_directory,
        }
    }

    fn path_directed_inline_child(
        &self,
        name: String,
        path: &CatalogPath,
        item_index: usize,
    ) -> Self {
        let module_directory = path
            .as_str()
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut syntax_index_path = self.syntax_index_path.clone();
        syntax_index_path.push(item_index);
        Self {
            id: self.id.child(name),
            file: self.file.clone(),
            syntax_index_path,
            module_directory: module_directory.clone(),
            path_attribute_directory: module_directory,
        }
    }

    fn file_child(&self, name: String, file: CatalogPath) -> Self {
        let mut physical_directory = file
            .as_str()
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let physical_file = physical_directory.pop().unwrap_or_default();
        let mut module_directory = physical_directory.clone();
        if physical_file != "mod.rs" {
            let stem = physical_file.strip_suffix(".rs").unwrap_or(&physical_file);
            module_directory.push(stem.to_owned());
        }
        Self {
            id: self.id.child(name),
            file,
            syntax_index_path: Vec::new(),
            module_directory,
            path_attribute_directory: physical_directory,
        }
    }

    fn conventional_targets(&self, name: &str) -> Result<[CatalogPath; 2], RustSourceGraphError> {
        let mut flat = self.module_directory.clone();
        flat.push(format!("{name}.rs"));
        let mut nested = self.module_directory.clone();
        nested.push(name.to_owned());
        nested.push("mod.rs".to_owned());

        Ok([
            CatalogPath::new(flat.join("/")).map_err(|source| {
                RustSourceGraphError::InvalidModulePath {
                    module: self.id.child(name.to_owned()),
                    message: source.to_string(),
                }
            })?,
            CatalogPath::new(nested.join("/")).map_err(|source| {
                RustSourceGraphError::InvalidModulePath {
                    module: self.id.child(name.to_owned()),
                    message: source.to_string(),
                }
            })?,
        ])
    }

    fn path_attribute(
        &self,
        child: &RustModuleId,
        module: &syn::ItemMod,
        cancellation: &CancellationProbe,
    ) -> Result<Option<CatalogPath>, SignatureContractKitError> {
        for attribute in module
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("cfg_attr"))
        {
            cancellation.checkpoint()?;
            let syn::Meta::List(arguments) = &attribute.meta else {
                return Err(RustSourceGraphError::InvalidPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                    message: "#[cfg_attr] must use parenthesized arguments".to_owned(),
                }
                .into());
            };
            if self.cfg_attr_selects_path(child, &arguments.tokens, cancellation)? {
                return Err(RustSourceGraphError::ConditionalPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                }
                .into());
            }
        }

        let mut value = None;
        for attribute in module
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("path"))
        {
            cancellation.checkpoint()?;
            if value.is_some() {
                return Err(RustSourceGraphError::InvalidPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                    message: "duplicate #[path] attributes are ambiguous".to_owned(),
                }
                .into());
            }
            let syn::Meta::NameValue(name_value) = &attribute.meta else {
                return Err(RustSourceGraphError::InvalidPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                    message: "#[path] must be a string name-value attribute".to_owned(),
                }
                .into());
            };
            let syn::Expr::Lit(expression) = &name_value.value else {
                return Err(RustSourceGraphError::InvalidPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                    message: "#[path] must contain a string literal".to_owned(),
                }
                .into());
            };
            let syn::Lit::Str(path) = &expression.lit else {
                return Err(RustSourceGraphError::InvalidPathAttribute {
                    module: child.clone(),
                    declared_in: self.file.clone(),
                    message: "#[path] must contain a string literal".to_owned(),
                }
                .into());
            };
            value = Some(self.path_attribute_target(child, &path.value(), cancellation)?);
        }
        Ok(value)
    }

    fn path_attribute_target(
        &self,
        child: &RustModuleId,
        value: &str,
        cancellation: &CancellationProbe,
    ) -> Result<CatalogPath, SignatureContractKitError> {
        let mut path = self.path_attribute_directory.clone();
        for component in value.split('/') {
            cancellation.checkpoint()?;
            match component {
                "" => {
                    return Err(RustSourceGraphError::InvalidPathAttribute {
                        module: child.clone(),
                        declared_in: self.file.clone(),
                        message: format!("path {value:?} must not contain empty components"),
                    }
                    .into());
                }
                "." => {}
                ".." => {
                    if path.pop().is_none() {
                        return Err(RustSourceGraphError::InvalidPathAttribute {
                            module: child.clone(),
                            declared_in: self.file.clone(),
                            message: format!("path {value:?} escapes above the catalog root"),
                        }
                        .into());
                    }
                }
                component => path.push(component.to_owned()),
            }
        }
        CatalogPath::new(path.join("/")).map_err(|source| {
            RustSourceGraphError::InvalidPathAttribute {
                module: child.clone(),
                declared_in: self.file.clone(),
                message: source.to_string(),
            }
            .into()
        })
    }

    fn cfg_attr_selects_path(
        &self,
        child: &RustModuleId,
        tokens: &TokenStream,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SignatureContractKitError> {
        let mut pending = vec![tokens.clone()];
        let mut pending_index = 0;
        while pending_index < pending.len() {
            cancellation.checkpoint()?;
            let tokens = pending[pending_index].clone();
            pending_index += 1;

            let mut attribute_tokens = TokenStream::new();
            let mut after_predicate = false;
            for token in tokens {
                cancellation.checkpoint()?;
                if !after_predicate
                    && matches!(&token, TokenTree::Punct(punctuation) if punctuation.as_char() == ',')
                {
                    after_predicate = true;
                    continue;
                }
                if after_predicate {
                    attribute_tokens.extend([token]);
                }
            }
            if !after_predicate || attribute_tokens.is_empty() {
                continue;
            }

            let attributes =
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
                    .parse2(attribute_tokens)
                    .map_err(|source| {
                        SignatureContractKitError::from(
                            RustSourceGraphError::InvalidPathAttribute {
                                module: child.clone(),
                                declared_in: self.file.clone(),
                                message: format!(
                                    "cannot inspect #[cfg_attr] for a conditional #[path]: {source}"
                                ),
                            },
                        )
                    })?;
            for attribute in attributes {
                cancellation.checkpoint()?;
                if attribute.path().is_ident("path") {
                    return Ok(true);
                }
                if attribute.path().is_ident("cfg_attr") {
                    let syn::Meta::List(arguments) = attribute else {
                        return Err(RustSourceGraphError::InvalidPathAttribute {
                            module: child.clone(),
                            declared_in: self.file.clone(),
                            message: "nested #[cfg_attr] must use parenthesized arguments"
                                .to_owned(),
                        }
                        .into());
                    };
                    pending.push(arguments.tokens);
                }
            }
        }
        Ok(false)
    }
}

/// Typed extraction and graph-construction failures retained by the public error wrapper.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RustSourceGraphError {
    #[error("invalid Rust crate id {value:?}: {reason}")]
    InvalidCrateId { value: String, reason: String },
    #[error("Rust extraction must declare at least one crate root")]
    MissingCrateRoot,
    #[error("duplicate crate id {id}")]
    DuplicateCrateId { id: RustCrateId },
    #[error(
        "crate root {root} must appear in files (the exact source allowlist) for crate {crate_id}"
    )]
    UnlistedCrateRoot {
        crate_id: RustCrateId,
        root: CatalogPath,
    },
    #[error("Rust source {path} must have an .rs extension")]
    NonRustSource { path: CatalogPath },
    #[error("invalid Rust module identity {value:?}: {reason}")]
    InvalidModuleIdentity { value: String, reason: String },
    #[error("allowlisted Rust source {path} is missing from the participant catalog")]
    MissingParticipant { path: CatalogPath },
    #[error("allowlisted Rust source {path} is not valid UTF-8: {message}")]
    InvalidUtf8 { path: CatalogPath, message: String },
    #[error("failed to parse allowlisted Rust source {path}: {message}")]
    InvalidRustSyntax { path: CatalogPath, message: String },
    #[error("invalid source span in {path}: {message}")]
    InvalidSourceSpan { path: CatalogPath, message: String },
    #[error("logical module {module} cannot be projected into parsed source {file}")]
    InvalidModuleSourceProjection {
        module: RustModuleId,
        file: CatalogPath,
    },
    #[error("module {module} has an invalid logical path: {message}")]
    InvalidModulePath {
        module: RustModuleId,
        message: String,
    },
    #[error("invalid #[path] for module {module} declared in {declared_in}: {message}")]
    InvalidPathAttribute {
        module: RustModuleId,
        declared_in: CatalogPath,
        message: String,
    },
    #[error(
        "module {module} declared in {declared_in} targets {candidates}, which is unlisted from the exact source allowlist"
    )]
    UnlistedModuleTarget {
        module: RustModuleId,
        declared_in: CatalogPath,
        candidates: String,
    },
    #[error(
        "module {module} declared in {declared_in} is ambiguous between allowlisted files {candidates}"
    )]
    AmbiguousModuleTarget {
        module: RustModuleId,
        declared_in: CatalogPath,
        candidates: String,
    },
    #[error(
        "#[cfg_attr] for module {module} declared in {declared_in} may select #[path] that rust_syntax_v2 cannot evaluate"
    )]
    ConditionalPathAttribute {
        module: RustModuleId,
        declared_in: CatalogPath,
    },
    #[error("module graph cycle reaches {module}: {chain}")]
    ModuleCycle { module: RustModuleId, chain: String },
    #[error("logical module {module} is declared more than once from {existing} and {incoming}")]
    DuplicateLogicalModule {
        module: RustModuleId,
        existing: CatalogPath,
        incoming: CatalogPath,
    },
    #[error(
        "logical module {module} is declared conditionally more than once from {existing} and {incoming}; rust_syntax_v2 cannot evaluate which alternative exists, so use compiler-backed extraction"
    )]
    ConditionalDuplicateLogicalModule {
        module: RustModuleId,
        existing: CatalogPath,
        incoming: CatalogPath,
    },
    #[error(
        "allowlisted Rust source {path} is disconnected; declare it as a crate root or link it with mod"
    )]
    DisconnectedSource { path: CatalogPath },
}

#[cfg(test)]
mod tests {
    use super::{
        RustCapabilityDiagnostic, RustCapabilityDiagnostics, RustCrateId, RustExtraction,
        RustGraphScope, RustModuleId, RustModulePath, RustRequiredModules, RustSourceCatalog,
        RustSourceGraph, RustSourceGraphBuilder,
    };
    use crate::api::{RustCrateKind, RustCrateRoot};
    use crate::error::SignatureContractKitError;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::limits::RustExtractionLimits;
    use std::collections::BTreeSet;

    #[test]
    fn required_modules_use_ordered_prefix_lookup_for_logarithmic_traversal() {
        let cancellation = crate::work::CancellationProbe::new();
        let crate_id = RustCrateId::new("sample", &cancellation).expect("crate id");
        let nested = RustModuleId::new(
            crate_id.clone(),
            RustModulePath::new(vec!["outer".to_owned(), "inner".to_owned()])
                .expect("nested module"),
        );
        let required =
            RustRequiredModules::new([nested.clone()].into_iter().collect(), &cancellation)
                .expect("required-module index");

        assert!(required.traverses(&RustModuleId::root(crate_id.clone())));
        assert!(required.traverses(&RustModuleId::new(
            crate_id.clone(),
            RustModulePath::new(vec!["outer".to_owned()]).expect("outer module"),
        )));
        assert!(required.traverses(&nested));
        assert!(!required.traverses(&RustModuleId::new(
            crate_id,
            RustModulePath::new(vec!["unrelated".to_owned()]).expect("unrelated module"),
        )));
    }

    #[test]
    fn required_module_indexing_observes_operation_cancellation() {
        let cancellation = crate::work::CancellationProbe::new();
        let crate_id = RustCrateId::new("sample", &cancellation).expect("crate id");
        let module = RustModuleId::new(
            crate_id,
            RustModulePath::new(vec!["nested".to_owned()]).expect("nested module"),
        );
        cancellation.cancel();

        let error = RustRequiredModules::new([module].into_iter().collect(), &cancellation)
            .expect_err("canceled required-module indexing must stop");

        assert!(error.is_operation_canceled());
    }

    struct GraphFixture {
        catalog: FileCatalog,
        allowlist: BTreeSet<CatalogPath>,
        crates: Vec<RustCrateRoot>,
    }

    impl GraphFixture {
        fn empty() -> Self {
            Self {
                catalog: FileCatalog::new(),
                allowlist: BTreeSet::new(),
                crates: Vec::new(),
            }
        }

        fn single_crate(crate_id: &str, root: &str, kind: RustCrateKind, source: &str) -> Self {
            Self::empty()
                .allowlisted_source(root, source)
                .crate_root(crate_id, root, kind)
        }

        fn allowlisted_source(mut self, path: &str, source: &str) -> Self {
            let path = CatalogPath::new(path).expect("valid fixture path");
            self.catalog
                .insert(path.clone(), source.as_bytes().to_vec())
                .expect("unique fixture path");
            self.allowlist.insert(path);
            self
        }

        fn catalog_only_source(mut self, path: &str, source: &str) -> Self {
            self.catalog
                .insert(
                    CatalogPath::new(path).expect("valid fixture path"),
                    source.as_bytes().to_vec(),
                )
                .expect("unique fixture path");
            self
        }

        fn allow_missing(mut self, path: &str) -> Self {
            self.allowlist
                .insert(CatalogPath::new(path).expect("valid fixture path"));
            self
        }

        fn crate_root(mut self, crate_id: &str, root: &str, kind: RustCrateKind) -> Self {
            self.crates.push(RustCrateRoot {
                id: crate_id.to_owned(),
                root: CatalogPath::new(root).expect("valid crate root"),
                kind,
            });
            self
        }

        fn build(self) -> Result<RustSourceGraph, SignatureContractKitError> {
            self.build_with_sources().map(|(graph, _)| graph)
        }

        fn build_with_sources(
            self,
        ) -> Result<(RustSourceGraph, RustSourceCatalog), SignatureContractKitError> {
            let limits = RustExtractionLimits::default();
            self.build_with_limits_and_cancellation(&limits, &crate::work::CancellationProbe::new())
                .map(|(graph, sources, _)| (graph, sources))
        }

        fn build_with_limits(
            self,
            limits: &RustExtractionLimits,
        ) -> Result<RustSourceGraph, SignatureContractKitError> {
            self.build_with_limits_and_cancellation(limits, &crate::work::CancellationProbe::new())
                .map(|(graph, _, _)| graph)
        }

        fn build_with_cancellation(
            self,
            cancellation: &crate::work::CancellationProbe,
        ) -> Result<(RustSourceGraph, RustSourceCatalog), SignatureContractKitError> {
            let limits = RustExtractionLimits::default();
            self.build_with_limits_and_cancellation(&limits, cancellation)
                .map(|(graph, sources, _)| (graph, sources))
        }

        fn build_snapshot(self) -> Result<GraphSnapshot, SignatureContractKitError> {
            let limits = RustExtractionLimits::default();
            self.build_with_limits_and_cancellation(&limits, &crate::work::CancellationProbe::new())
                .map(|(graph, _, diagnostics)| GraphSnapshot::new(&graph, diagnostics))
        }

        fn build_with_limits_and_cancellation(
            self,
            limits: &RustExtractionLimits,
            cancellation: &crate::work::CancellationProbe,
        ) -> Result<(RustSourceGraph, RustSourceCatalog, Vec<String>), SignatureContractKitError>
        {
            let extraction = RustExtraction::from_roots(self.allowlist, self.crates, cancellation)?;
            let sources =
                RustSourceCatalog::parse_allowlist(extraction.files(), self.catalog, cancellation)?;
            let mut usage = limits.usage();
            let diagnostic_limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&diagnostic_limits);
            let graph = RustSourceGraph::build(
                &extraction,
                &sources,
                &mut usage,
                &mut diagnostics,
                cancellation,
            )?;
            let diagnostics = diagnostics.into_warning_messages(cancellation)?;
            Ok((graph, sources, diagnostics))
        }

        fn expect_graph_error(self) -> SignatureContractKitError {
            let cancellation = crate::work::CancellationProbe::new();
            let extraction = RustExtraction::from_roots(self.allowlist, self.crates, &cancellation)
                .expect("valid fixture extraction");
            let sources =
                RustSourceCatalog::parse_allowlist(extraction.files(), self.catalog, &cancellation)
                    .expect("valid fixture sources");
            let limits = RustExtractionLimits::default();
            let mut usage = limits.usage();
            let diagnostic_limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&diagnostic_limits);
            RustSourceGraphBuilder::new(
                &extraction,
                &sources,
                &mut usage,
                &mut diagnostics,
                &cancellation,
                RustGraphScope::Complete,
            )
            .build()
            .expect_err("fixture must produce a source-graph error")
        }
    }

    #[test]
    fn capability_evidence_deduplicates_before_charging_operation_limits() {
        let limits = crate::limits::DiagnosticLimits {
            count: 1,
            serialized_bytes: u64::MAX,
        };
        let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
        let diagnostic = RustCapabilityDiagnostic::ConditionalModule {
            module: RustModuleId::new(
                RustCrateId::new("sample", &crate::work::CancellationProbe::new())
                    .expect("crate id"),
                RustModulePath::default(),
            ),
        };

        assert!(
            diagnostics
                .insert(diagnostic.clone())
                .expect("first diagnostic")
        );
        assert!(
            !diagnostics
                .insert(diagnostic)
                .expect("duplicate diagnostic")
        );
        let error = diagnostics
            .insert(RustCapabilityDiagnostic::ConditionalModule {
                module: RustModuleId::new(
                    RustCrateId::new("second", &crate::work::CancellationProbe::new())
                        .expect("crate id"),
                    RustModulePath::default(),
                ),
            })
            .expect_err("second unique diagnostic must cross the limit");
        assert_eq!(
            error.to_string(),
            "resource limit DiagnosticCount exceeded: limit 1, observed at least 2"
        );
    }

    #[test]
    fn source_graph_observes_cancellation_before_traversing_a_crate_root() {
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let result = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "pub struct NeverVisited;",
        )
        .build_with_cancellation(&cancellation);
        let Err(error) = result else {
            panic!("canceled traversal must stop");
        };

        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn source_graph_accounts_top_level_and_associated_items_at_the_traversal_boundary() {
        let source = r#"
            pub trait Service {
                type Error;
                const READY: bool;
                fn execute(&self);
            }
            pub struct Worker;
            impl Service for Worker {
                type Error = ();
                const READY: bool = true;
                fn execute(&self) {}
            }
            unsafe extern "C" {
                fn native_execute();
                static READY: bool;
                type NativeState;
            }
        "#;
        GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, source)
            .build_with_limits(&RustExtractionLimits {
                items: 13,
                ..RustExtractionLimits::default()
            })
            .expect("all thirteen encountered items fit the exact boundary");

        let error = GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, source)
            .build_with_limits(&RustExtractionLimits {
                items: 12,
                ..RustExtractionLimits::default()
            })
            .expect_err("the thirteenth encountered item must stop graph traversal");
        let limit = error.limit_exceeded().expect("typed item budget error");
        assert_eq!(limit.resource, crate::limits::LimitResource::RustItemCount);
        assert_eq!(limit.limit, 12);
        assert_eq!(limit.observed_at_least, 13);
        assert_eq!(limit.file.as_ref().map(CatalogPath::as_str), Some("lib.rs"));
    }

    #[test]
    fn source_graph_counts_raw_impl_blocks_before_inventory_descriptor_merge() {
        let source = r#"
            pub struct Worker;
            impl Worker { pub fn first(&self) {} }
            impl Worker { pub fn second(&self) {} }
        "#;
        GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, source)
            .build_with_limits(&RustExtractionLimits {
                items: 5,
                ..RustExtractionLimits::default()
            })
            .expect("struct, two raw impls, and two methods fit exactly");

        let error = GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, source)
            .build_with_limits(&RustExtractionLimits {
                items: 4,
                ..RustExtractionLimits::default()
            })
            .expect_err("the second impl method must stop graph traversal before merge");
        let limit = error.limit_exceeded().expect("typed item budget error");
        assert_eq!(limit.resource, crate::limits::LimitResource::RustItemCount);
        assert_eq!(limit.observed_at_least, 5);
    }

    #[test]
    fn crate_ids_reject_identity_and_cli_delimiters_or_whitespace() {
        for value in ["alpha:beta", "alpha=beta", "alpha beta"] {
            let error = RustCrateId::new(value, &crate::work::CancellationProbe::new())
                .expect_err("crate identity delimiters must fail closed");
            let rendered = error.to_string();

            assert!(rendered.contains(value), "{rendered}");
            assert!(
                rendered.contains("delimiter") || rendered.contains("whitespace"),
                "{rendered}"
            );
        }
        let control = RustCrateId::new("alpha\tbeta", &crate::work::CancellationProbe::new())
            .expect_err("control characters must fail closed")
            .to_string();
        assert!(
            control.contains("control") || control.contains("whitespace"),
            "{control}"
        );
        let control = RustCrateId::new("alpha\u{7}beta", &crate::work::CancellationProbe::new())
            .expect_err("non-whitespace control characters must fail closed")
            .to_string();
        assert!(control.contains("control"), "{control}");

        assert_eq!(
            RustCrateId::new("alpha-beta_2", &crate::work::CancellationProbe::new(),)
                .expect("delimiter-safe crate id")
                .as_str(),
            "alpha-beta_2"
        );
    }

    #[test]
    fn crate_identity_and_extraction_construction_observe_cancellation() {
        let canceled = crate::work::CancellationProbe::new();
        canceled.cancel();
        let error = RustCrateId::new("sample", &canceled)
            .expect_err("crate identity validation must stop when canceled");
        assert!(error.to_string().contains("canceled"), "{error}");

        let root = CatalogPath::new("lib.rs").expect("crate root path");
        let error = RustExtraction::from_roots(
            [root.clone()],
            [RustCrateRoot {
                id: "sample".to_owned(),
                root,
                kind: RustCrateKind::Library,
            }],
            &canceled,
        )
        .expect_err("extraction construction must stop when canceled");
        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn extraction_from_roots_canonicalizes_and_enforces_the_complete_root_contract() {
        let cancellation = crate::work::CancellationProbe::new();
        let a = CatalogPath::new("a.rs").expect("first source path");
        let b = CatalogPath::new("b.rs").expect("second source path");
        let extraction = RustExtraction::from_roots(
            [b.clone(), a.clone(), b.clone()],
            [
                RustCrateRoot {
                    id: "b".to_owned(),
                    root: b.clone(),
                    kind: RustCrateKind::Binary,
                },
                RustCrateRoot {
                    id: "a".to_owned(),
                    root: a.clone(),
                    kind: RustCrateKind::Library,
                },
            ],
            &cancellation,
        )
        .expect("canonical extraction");

        assert_eq!(extraction.files().iter().collect::<Vec<_>>(), [&a, &b]);
        assert_eq!(
            extraction
                .crates()
                .iter()
                .map(|crate_root| crate_root.id().as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );

        let missing = RustExtraction::from_roots([a.clone()], std::iter::empty(), &cancellation)
            .expect_err("an extraction requires one crate root");
        assert!(missing.to_string().contains("at least one crate root"));

        let duplicate = RustExtraction::from_roots(
            [a.clone(), b.clone()],
            [
                RustCrateRoot {
                    id: "duplicate".to_owned(),
                    root: a.clone(),
                    kind: RustCrateKind::Library,
                },
                RustCrateRoot {
                    id: "duplicate".to_owned(),
                    root: b.clone(),
                    kind: RustCrateKind::Binary,
                },
            ],
            &cancellation,
        )
        .expect_err("crate IDs must be unique");
        assert!(
            duplicate
                .to_string()
                .contains("duplicate crate id duplicate")
        );

        let whitespace = RustExtraction::from_roots(
            [a.clone()],
            [RustCrateRoot {
                id: " sample ".to_owned(),
                root: a.clone(),
                kind: RustCrateKind::Library,
            }],
            &cancellation,
        )
        .expect_err("crate IDs must be exact");
        assert!(whitespace.to_string().contains("whitespace"));

        let unlisted = RustExtraction::from_roots(
            [a],
            [RustCrateRoot {
                id: "sample".to_owned(),
                root: b,
                kind: RustCrateKind::Library,
            }],
            &cancellation,
        )
        .expect_err("crate roots must belong to the exact allowlist");
        assert!(unlisted.to_string().contains("exact source allowlist"));
    }

    #[test]
    fn yaml_module_segments_are_semantic_and_reject_raw_or_reserved_spellings() {
        assert_eq!(
            RustModulePath::canonical_declaration_segment("type".to_owned())
                .expect("canonical keyword declaration segment"),
            "type"
        );
        assert_eq!(
            RustModulePath::new(vec!["type".to_owned(), "async".to_owned()])
                .expect("canonical keyword module segments")
                .segments(),
            &["type".to_owned(), "async".to_owned()]
        );

        for segment in ["r#type", "_", "crate", "self", "Self", "super"] {
            let error = RustModulePath::new(vec![segment.to_owned()])
                .expect_err("raw or reserved module identity must fail closed");
            let rendered = error.to_string();

            assert!(rendered.contains(segment), "{rendered}");
        }
    }

    #[test]
    fn canonical_semantic_identifiers_render_as_valid_source_spellings() {
        assert_eq!(RustModulePath::source_ident("Widget"), "Widget");
        assert_eq!(RustModulePath::source_ident("type"), "r#type");
        assert_eq!(RustModulePath::source_ident("async"), "r#async");
        assert_eq!(
            RustModulePath::source_ident("crate"),
            "crate",
            "path-control words are not declaration identifiers"
        );
    }

    #[test]
    fn raw_module_declarations_use_one_canonical_semantic_identity() {
        let graph = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "mod r#type { mod r#async { pub struct Value; } }",
        )
        .build()
        .expect("raw module graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                ("sample::type".to_owned(), "lib.rs".to_owned()),
                ("sample::type::async".to_owned(), "lib.rs".to_owned(),),
            ]
        );
    }

    struct GraphSnapshot {
        modules: Vec<(String, String)>,
        diagnostics: Vec<String>,
    }

    impl GraphSnapshot {
        fn from_graph(graph: &RustSourceGraph) -> Self {
            Self::new(graph, Vec::new())
        }

        fn new(graph: &RustSourceGraph, diagnostics: Vec<String>) -> Self {
            Self {
                modules: graph
                    .modules()
                    .iter()
                    .map(|module| (module.id().to_string(), module.file().to_string()))
                    .collect(),
                diagnostics,
            }
        }
    }

    struct ModuleItemsSnapshot {
        modules: Vec<(String, Vec<String>, Vec<usize>)>,
    }

    impl ModuleItemsSnapshot {
        fn from_graph(
            graph: &RustSourceGraph,
            sources: &RustSourceCatalog,
        ) -> Result<Self, SignatureContractKitError> {
            let mut modules = Vec::new();
            for module in graph.modules() {
                let names = module
                    .items(sources)?
                    .iter()
                    .filter_map(|item| match item {
                        syn::Item::Const(value) => Some(value.ident.to_string()),
                        syn::Item::Enum(value) => Some(value.ident.to_string()),
                        syn::Item::Fn(value) => Some(value.sig.ident.to_string()),
                        syn::Item::Mod(value) => Some(value.ident.to_string()),
                        syn::Item::Static(value) => Some(value.ident.to_string()),
                        syn::Item::Struct(value) => Some(value.ident.to_string()),
                        syn::Item::Trait(value) => Some(value.ident.to_string()),
                        syn::Item::Type(value) => Some(value.ident.to_string()),
                        syn::Item::Union(value) => Some(value.ident.to_string()),
                        syn::Item::ExternCrate(_)
                        | syn::Item::ForeignMod(_)
                        | syn::Item::Impl(_)
                        | syn::Item::Macro(_)
                        | syn::Item::TraitAlias(_)
                        | syn::Item::Use(_)
                        | syn::Item::Verbatim(_) => None,
                        _ => None,
                    })
                    .collect();
                modules.push((
                    module.id().to_string(),
                    names,
                    module.syntax_index_path.clone(),
                ));
            }
            Ok(Self { modules })
        }
    }

    #[test]
    fn library_and_binary_roots_have_explicit_crate_identity() {
        let library = GraphFixture::single_crate(
            "library_api",
            "lib.rs",
            RustCrateKind::Library,
            "pub fn library() {}",
        )
        .build()
        .expect("library graph");
        let binary =
            GraphFixture::single_crate("tool", "main.rs", RustCrateKind::Binary, "fn main() {}")
                .build()
                .expect("binary graph");

        assert_eq!(
            GraphSnapshot::from_graph(&library).modules,
            [("library_api".to_owned(), "lib.rs".to_owned())]
        );
        assert_eq!(
            GraphSnapshot::from_graph(&binary).modules,
            [("tool".to_owned(), "main.rs".to_owned())]
        );
    }

    #[test]
    fn conventional_module_resolution_accepts_foo_rs_or_foo_mod_rs() {
        let flat =
            GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, "mod foo;")
                .allowlisted_source("foo.rs", "pub struct Flat;")
                .build()
                .expect("foo.rs graph");
        let directory =
            GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, "mod foo;")
                .allowlisted_source("foo/mod.rs", "pub struct Directory;")
                .build()
                .expect("foo/mod.rs graph");

        assert_eq!(
            GraphSnapshot::from_graph(&flat).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                ("sample::foo".to_owned(), "foo.rs".to_owned()),
            ]
        );
        assert_eq!(
            GraphSnapshot::from_graph(&directory).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                ("sample::foo".to_owned(), "foo/mod.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn inline_and_nested_out_of_line_modules_share_one_logical_graph() {
        let graph = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "mod inline { pub mod nested {} } mod network;",
        )
        .allowlisted_source("network.rs", "mod client;")
        .allowlisted_source("network/client.rs", "pub struct Client;")
        .build()
        .expect("nested module graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                ("sample::inline".to_owned(), "lib.rs".to_owned()),
                ("sample::inline::nested".to_owned(), "lib.rs".to_owned()),
                ("sample::network".to_owned(), "network.rs".to_owned()),
                (
                    "sample::network::client".to_owned(),
                    "network/client.rs".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn module_item_slices_follow_inline_indexes_and_reset_for_physical_files() {
        let (graph, sources) = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "pub struct Root; mod inline { pub struct Inline; mod nested { pub struct Nested; } } mod external;",
        )
        .allowlisted_source(
            "external.rs",
            "pub struct External; mod nested { pub struct ExternalNested; }",
        )
        .build_with_sources()
        .expect("module source projection");

        assert_eq!(
            ModuleItemsSnapshot::from_graph(&graph, &sources)
                .expect("module item slices")
                .modules,
            [
                (
                    "sample".to_owned(),
                    vec![
                        "Root".to_owned(),
                        "inline".to_owned(),
                        "external".to_owned()
                    ],
                    vec![],
                ),
                (
                    "sample::external".to_owned(),
                    vec!["External".to_owned(), "nested".to_owned()],
                    vec![],
                ),
                (
                    "sample::external::nested".to_owned(),
                    vec!["ExternalNested".to_owned()],
                    vec![1],
                ),
                (
                    "sample::inline".to_owned(),
                    vec!["Inline".to_owned(), "nested".to_owned()],
                    vec![1],
                ),
                (
                    "sample::inline::nested".to_owned(),
                    vec!["Nested".to_owned()],
                    vec![1, 1],
                ),
            ]
        );
    }

    #[test]
    fn path_directed_physical_modules_start_at_the_target_syntax_root() {
        let (graph, sources) = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[path = \"custom/location.rs\"] mod transport;",
        )
        .allowlisted_source(
            "custom/location.rs",
            "pub struct Transport; mod detail { pub struct Detail; }",
        )
        .build_with_sources()
        .expect("path-directed module source projection");

        assert_eq!(
            ModuleItemsSnapshot::from_graph(&graph, &sources)
                .expect("path-directed item slices")
                .modules,
            [
                ("sample".to_owned(), vec!["transport".to_owned()], vec![]),
                (
                    "sample::transport".to_owned(),
                    vec!["Transport".to_owned(), "detail".to_owned()],
                    vec![],
                ),
                (
                    "sample::transport::detail".to_owned(),
                    vec!["Detail".to_owned()],
                    vec![1],
                ),
            ]
        );
    }

    #[test]
    fn path_directed_physical_module_derives_nested_children_from_its_target_file() {
        let graph = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[path = \"custom/location.rs\"] mod transport;",
        )
        .allowlisted_source("custom/location.rs", "mod detail;")
        .allowlisted_source("custom/location/detail.rs", "pub struct Detail;")
        .build()
        .expect("path-directed nested module graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                (
                    "sample::transport".to_owned(),
                    "custom/location.rs".to_owned(),
                ),
                (
                    "sample::transport::detail".to_owned(),
                    "custom/location/detail.rs".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn flat_and_mod_rs_modules_derive_the_same_nested_child_location() {
        let flat =
            GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, "mod foo;")
                .allowlisted_source("foo.rs", "mod detail;")
                .allowlisted_source("foo/detail.rs", "pub struct Detail;")
                .build()
                .expect("flat nested module graph");
        let directory =
            GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, "mod foo;")
                .allowlisted_source("foo/mod.rs", "mod detail;")
                .allowlisted_source("foo/detail.rs", "pub struct Detail;")
                .build()
                .expect("mod.rs nested module graph");

        let flat = GraphSnapshot::from_graph(&flat);
        let directory = GraphSnapshot::from_graph(&directory);
        assert!(
            flat.modules
                .contains(&("sample::foo::detail".to_owned(), "foo/detail.rs".to_owned(),))
        );
        assert!(
            directory
                .modules
                .contains(&("sample::foo::detail".to_owned(), "foo/detail.rs".to_owned(),))
        );
    }

    #[test]
    fn explicit_nonconventional_root_is_a_mod_rs_context_for_inline_paths() {
        let graph = GraphFixture::single_crate(
            "tool",
            "workspace/entrypoint.rs",
            RustCrateKind::Binary,
            "mod inline { #[path = \"payload.rs\"] mod payload; }",
        )
        .allowlisted_source("workspace/inline/payload.rs", "pub struct Payload;")
        .build()
        .expect("nonconventional explicit root graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("tool".to_owned(), "workspace/entrypoint.rs".to_owned(),),
                (
                    "tool::inline".to_owned(),
                    "workspace/entrypoint.rs".to_owned(),
                ),
                (
                    "tool::inline::payload".to_owned(),
                    "workspace/inline/payload.rs".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn nested_physical_lib_rs_is_not_misclassified_as_a_crate_root() {
        let graph = GraphFixture::single_crate(
            "sample",
            "entry.rs",
            RustCrateKind::Library,
            "#[path = \"nested/lib.rs\"] mod nested;",
        )
        .allowlisted_source("nested/lib.rs", "mod detail;")
        .allowlisted_source("nested/lib/detail.rs", "pub struct Detail;")
        .build()
        .expect("nested physical lib.rs graph");

        assert!(GraphSnapshot::from_graph(&graph).modules.contains(&(
            "sample::nested::detail".to_owned(),
            "nested/lib/detail.rs".to_owned(),
        )));
    }

    #[test]
    fn inline_path_directory_follows_the_rust_reference_thread_files_example() {
        let graph = GraphFixture::single_crate(
            "sample",
            "src/lib.rs",
            RustCrateKind::Library,
            "#[path = \"thread_files\"] mod thread { #[path = \"tls.rs\"] mod local_data; }",
        )
        .allowlisted_source("src/thread_files/tls.rs", "pub struct LocalData;")
        .build()
        .expect("Rust Reference inline path graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("sample".to_owned(), "src/lib.rs".to_owned()),
                ("sample::thread".to_owned(), "src/lib.rs".to_owned()),
                (
                    "sample::thread::local_data".to_owned(),
                    "src/thread_files/tls.rs".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn parent_relative_path_attribute_is_normalized_inside_the_catalog() {
        assert!(CatalogPath::new("../shared.rs").is_err());

        let graph = GraphFixture::single_crate(
            "sample",
            "src/lib.rs",
            RustCrateKind::Library,
            "#[path = \"nested/child.rs\"] mod child;",
        )
        .allowlisted_source(
            "src/nested/child.rs",
            "#[path = \"../shared.rs\"] mod shared;",
        )
        .allowlisted_source("src/shared.rs", "pub struct Shared;")
        .build()
        .expect("parent-relative path graph");

        assert!(GraphSnapshot::from_graph(&graph).modules.contains(&(
            "sample::child::shared".to_owned(),
            "src/shared.rs".to_owned(),
        )));
    }

    #[test]
    fn path_attribute_cannot_escape_above_the_catalog_root() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[path = \"../outside.rs\"] mod outside;",
        )
        .build()
        .expect_err("path above the catalog root must fail");
        let rendered = error.to_string();

        assert!(rendered.contains("#[path]"), "{rendered}");
        assert!(rendered.contains("catalog root"), "{rendered}");
        assert!(rendered.contains("../outside.rs"), "{rendered}");
    }

    #[test]
    fn cfg_attr_that_can_select_a_module_path_fails_closed() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[cfg_attr(target_os = \"linux\", path = \"linux.rs\")] mod os;",
        )
        .allowlisted_source("os.rs", "pub struct Os;")
        .expect_graph_error();
        let rendered = error.to_string();

        assert!(rendered.contains("cfg_attr"), "{rendered}");
        assert!(rendered.contains("path"), "{rendered}");
        assert!(rendered.contains("sample::os"), "{rendered}");
        assert!(rendered.contains("cannot evaluate"), "{rendered}");
    }

    #[test]
    fn nested_cfg_attr_that_can_select_a_module_path_fails_closed() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[cfg_attr(unix, cfg_attr(feature = \"tls\", path = \"tls.rs\"))] mod transport;",
        )
        .allowlisted_source("transport.rs", "pub struct Transport;")
        .build()
        .expect_err("nested conditional module path must fail closed");
        let rendered = error.to_string();

        assert!(rendered.contains("cfg_attr"), "{rendered}");
        assert!(rendered.contains("path"), "{rendered}");
        assert!(rendered.contains("sample::transport"), "{rendered}");
    }

    #[test]
    fn cfg_attr_without_a_path_keeps_the_edge_and_reports_capability() {
        let snapshot = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[cfg_attr(feature = \"docs\", doc = \"transport\")] mod transport;",
        )
        .allowlisted_source("transport.rs", "pub struct Transport;")
        .build_snapshot()
        .expect("non-path cfg_attr module graph");

        assert!(
            snapshot
                .modules
                .contains(&("sample::transport".to_owned(), "transport.rs".to_owned(),))
        );
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.diagnostics[0].contains("cfg_attr"));
    }

    #[test]
    fn path_attribute_selects_the_declared_allowlisted_file() {
        let graph = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[path = \"custom/location.rs\"] mod transport;",
        )
        .allowlisted_source("custom/location.rs", "pub struct Transport;")
        .build()
        .expect("path-directed module graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("sample".to_owned(), "lib.rs".to_owned()),
                (
                    "sample::transport".to_owned(),
                    "custom/location.rs".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn listed_but_missing_module_file_fails_with_edge_evidence() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "mod transport;",
        )
        .allow_missing("transport.rs")
        .build()
        .expect_err("listed missing source must fail");
        let rendered = error.to_string();

        assert!(rendered.contains("transport"), "{rendered}");
        assert!(rendered.contains("transport.rs"), "{rendered}");
        assert!(rendered.contains("missing"), "{rendered}");
    }

    #[test]
    fn simultaneous_foo_rs_and_foo_mod_rs_is_ambiguous() {
        let error =
            GraphFixture::single_crate("sample", "lib.rs", RustCrateKind::Library, "mod foo;")
                .allowlisted_source("foo.rs", "pub struct Flat;")
                .allowlisted_source("foo/mod.rs", "pub struct Directory;")
                .build()
                .expect_err("two conventional targets must be ambiguous");
        let rendered = error.to_string();

        assert!(rendered.contains("ambiguous"), "{rendered}");
        assert!(rendered.contains("foo.rs"), "{rendered}");
        assert!(rendered.contains("foo/mod.rs"), "{rendered}");
    }

    #[test]
    fn module_edge_cannot_escape_the_exact_allowlist() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "mod transport;",
        )
        .catalog_only_source("transport.rs", "pub struct Transport;")
        .build()
        .expect_err("unlisted module target must fail");
        let rendered = error.to_string();

        assert!(rendered.contains("transport.rs"), "{rendered}");
        assert!(
            rendered.contains("allowlist") || rendered.contains("unlisted"),
            "{rendered}"
        );
    }

    #[test]
    fn recursive_path_edges_report_a_cycle() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[path = \"a.rs\"] mod a;",
        )
        .allowlisted_source("a.rs", "#[path = \"lib.rs\"] mod root;")
        .build()
        .expect_err("recursive module edge must fail");
        let rendered = error.to_string();

        assert!(rendered.contains("cycle"), "{rendered}");
        assert!(rendered.contains("lib.rs"), "{rendered}");
        assert!(rendered.contains("a.rs"), "{rendered}");
    }

    #[test]
    fn disconnected_allowlisted_file_requires_an_explicit_root() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "pub struct Root;",
        )
        .allowlisted_source("orphan.rs", "pub struct Orphan;")
        .build()
        .expect_err("disconnected file must fail");
        let rendered = error.to_string();

        assert!(rendered.contains("orphan.rs"), "{rendered}");
        assert!(
            rendered.contains("disconnected") || rendered.contains("crate root"),
            "{rendered}"
        );
    }

    #[test]
    fn multiple_explicit_crates_and_roots_remain_distinct() {
        let graph = GraphFixture::empty()
            .allowlisted_source("alpha/lib.rs", "mod model;")
            .allowlisted_source("alpha/model.rs", "pub struct Model;")
            .allowlisted_source("beta/main.rs", "mod model; fn main() {}")
            .allowlisted_source("beta/model.rs", "pub struct Model;")
            .crate_root("alpha", "alpha/lib.rs", RustCrateKind::Library)
            .crate_root("beta", "beta/main.rs", RustCrateKind::Binary)
            .build()
            .expect("multi-crate graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("alpha".to_owned(), "alpha/lib.rs".to_owned()),
                ("alpha::model".to_owned(), "alpha/model.rs".to_owned()),
                ("beta".to_owned(), "beta/main.rs".to_owned()),
                ("beta::model".to_owned(), "beta/model.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn one_physical_file_can_have_distinct_cross_crate_logical_contexts() {
        let (graph, sources) = GraphFixture::empty()
            .allowlisted_source(
                "one.rs",
                "#[path = \"shared.rs\"] mod shared; pub use shared::Value;",
            )
            .allowlisted_source(
                "two.rs",
                "#[path = \"shared.rs\"] mod shared; pub use shared::Value;",
            )
            .allowlisted_source("shared.rs", "pub struct Value;")
            .crate_root("one", "one.rs", RustCrateKind::Library)
            .crate_root("two", "two.rs", RustCrateKind::Library)
            .build_with_sources()
            .expect("shared physical source graph");

        assert_eq!(
            GraphSnapshot::from_graph(&graph).modules,
            [
                ("one".to_owned(), "one.rs".to_owned()),
                ("one::shared".to_owned(), "shared.rs".to_owned()),
                ("two".to_owned(), "two.rs".to_owned()),
                ("two::shared".to_owned(), "shared.rs".to_owned()),
            ]
        );
        assert_eq!(
            ModuleItemsSnapshot::from_graph(&graph, &sources)
                .expect("shared physical source item slices")
                .modules,
            [
                ("one".to_owned(), vec!["shared".to_owned()], vec![]),
                ("one::shared".to_owned(), vec!["Value".to_owned()], vec![],),
                ("two".to_owned(), vec!["shared".to_owned()], vec![]),
                ("two::shared".to_owned(), vec!["Value".to_owned()], vec![],),
            ]
        );
    }

    #[test]
    fn graph_and_diagnostics_are_deterministic_across_catalog_insertion_order() {
        let forward = GraphFixture::empty()
            .allowlisted_source("lib.rs", "mod a; mod z;")
            .allowlisted_source("a.rs", "#[cfg(feature = \"fast\")] mod nested;")
            .allowlisted_source("a/nested.rs", "pub struct Nested;")
            .allowlisted_source("z.rs", "pub struct Z;")
            .crate_root("sample", "lib.rs", RustCrateKind::Library)
            .build_snapshot()
            .expect("forward graph");
        let reverse = GraphFixture::empty()
            .allowlisted_source("z.rs", "pub struct Z;")
            .allowlisted_source("a/nested.rs", "pub struct Nested;")
            .allowlisted_source("a.rs", "#[cfg(feature = \"fast\")] mod nested;")
            .allowlisted_source("lib.rs", "mod a; mod z;")
            .crate_root("sample", "lib.rs", RustCrateKind::Library)
            .build_snapshot()
            .expect("reverse graph");
        assert_eq!(forward.modules, reverse.modules);
        assert_eq!(forward.diagnostics, reverse.diagnostics);
    }

    #[test]
    fn conditional_module_edge_is_retained_with_a_capability_diagnostic() {
        let snapshot = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "#[cfg(feature = \"tls\")] mod transport;",
        )
        .allowlisted_source("transport.rs", "pub struct Transport;")
        .build_snapshot()
        .expect("conditional syntax graph");

        assert!(
            snapshot
                .modules
                .contains(&("sample::transport".to_owned(), "transport.rs".to_owned()))
        );
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.diagnostics[0].contains("cfg"));
        assert!(snapshot.diagnostics[0].contains("sample::transport"));
        assert!(snapshot.diagnostics[0].contains("not evaluated"));
    }

    #[test]
    fn repeated_cfg_split_module_declarations_fail_with_compiler_guidance() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            r#"
                #[cfg(feature = "client")]
                mod transport { pub struct Client; }
                #[cfg(not(feature = "client"))]
                mod transport { pub struct Server; }
            "#,
        )
        .expect_graph_error();
        let rendered = error.to_string();

        assert!(rendered.contains("sample::transport"), "{rendered}");
        assert!(rendered.contains("lib.rs"), "{rendered}");
        assert!(rendered.contains("rust_syntax_v2"), "{rendered}");
        assert!(rendered.contains("compiler-backed"), "{rendered}");
    }

    #[test]
    fn repeated_unconditional_modules_keep_the_ordinary_duplicate_error() {
        let error = GraphFixture::single_crate(
            "sample",
            "lib.rs",
            RustCrateKind::Library,
            "mod transport {} mod transport {}",
        )
        .expect_graph_error();

        assert_eq!(
            error.to_string(),
            "logical module sample::transport is declared more than once from lib.rs and lib.rs"
        );
    }
}
