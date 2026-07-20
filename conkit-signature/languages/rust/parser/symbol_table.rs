use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use quote::ToTokens as _;
use syn::spanned::Spanned as _;

use crate::files::CatalogPath;
use crate::languages::rust::parser::inventory_collector::RustItemContext;
use crate::languages::rust::parser::item_converter::RustImportBinding;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath, RustSourceGraph};
use crate::languages::rust::types::impl_type::RustImplementationOwner;
use crate::limits::{DiagnosticEvidenceUsage, DiagnosticLimits, LimitExceeded};
use crate::work::CancellationProbe;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustOwnerResolutionSite {
    file: CatalogPath,
    module: RustModuleId,
    start: usize,
    end: usize,
    requested: String,
}

impl RustOwnerResolutionSite {
    fn from_implementation(
        context: &RustItemContext<'_>,
        item: &syn::ItemImpl,
    ) -> Result<Self, RustSymbolTableError> {
        let requested = item.self_ty.to_token_stream().to_string();
        let span = context.source_span(item.self_ty.span()).map_err(|source| {
            RustSymbolTableError::SourceEvidence {
                file: context.file().clone(),
                module: context.module_id().clone(),
                requested: requested.clone(),
                message: source.to_string(),
            }
        })?;
        let range = span.byte_range();
        Ok(Self {
            file: span.file().clone(),
            module: context.module_id().clone(),
            start: range.start,
            end: range.end,
            requested,
        })
    }
}

impl fmt::Display for RustOwnerResolutionSite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} in {} ({}) at bytes {}..{}",
            self.requested, self.file, self.module, self.start, self.end
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RustSymbolTableError {
    #[error("Rust symbol-table construction or resolution was canceled")]
    OperationCanceled,
    #[error(transparent)]
    LimitExceeded(#[from] LimitExceeded),
    #[error(
        "cannot register implementation owner candidate {id}: module {module} is outside the source graph"
    )]
    UnknownDeclarationModule { id: String, module: RustModuleId },
    #[error(
        "cannot register import in {file} ({module}) at bytes {start}..{end}: module is outside the source graph"
    )]
    UnknownImportModule {
        file: CatalogPath,
        module: RustModuleId,
        start: usize,
        end: usize,
    },
    #[error(
        "cannot retain source evidence for implementation owner {requested:?} in {file} ({module}): {message}"
    )]
    SourceEvidence {
        file: CatalogPath,
        module: RustModuleId,
        requested: String,
        message: String,
    },
    #[error("unsupported implementation owner {site}: {reason}")]
    UnsupportedOwner {
        site: Box<RustOwnerResolutionSite>,
        reason: String,
    },
    #[error(
        "cannot resolve implementation owner {site}; lexical candidates: {suggestions:?}; external-prelude and compiler-resolved paths require compiler-backed extraction"
    )]
    UnresolvedOwner {
        site: Box<RustOwnerResolutionSite>,
        suggestions: Vec<String>,
    },
    #[error("implementation owner {site} is ambiguous; candidates: {candidates:?}")]
    AmbiguousOwner {
        site: Box<RustOwnerResolutionSite>,
        candidates: Vec<String>,
    },
    #[error(
        "implementation owner {site} reaches ambiguous import {local_name:?}; bindings: {bindings:?}"
    )]
    AmbiguousImport {
        site: Box<RustOwnerResolutionSite>,
        local_name: String,
        bindings: Vec<String>,
    },
    #[error("implementation owner {site} contains an import cycle: {bindings:?}")]
    ImportCycle {
        site: Box<RustOwnerResolutionSite>,
        bindings: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RustResolvedSymbol {
    Module(RustModuleId),
    Declaration(RustItemId),
}

struct RustResolutionEvidence<'limits> {
    usage: DiagnosticEvidenceUsage<'limits>,
    values: BTreeSet<String>,
}

impl<'limits> RustResolutionEvidence<'limits> {
    fn new(limits: &'limits DiagnosticLimits) -> Self {
        Self {
            usage: limits.evidence_usage(),
            values: BTreeSet::new(),
        }
    }

    fn insert(&mut self, value: String) -> Result<(), RustSymbolTableError> {
        if self.values.contains(&value) {
            return Ok(());
        }
        self.usage.record_text(&value)?;
        self.values.insert(value);
        Ok(())
    }

    fn into_values(self) -> Vec<String> {
        self.values.into_iter().collect()
    }
}

pub(super) struct RustSymbolTable {
    modules: BTreeSet<RustModuleId>,
    declarations: BTreeMap<RustModuleId, BTreeMap<String, Vec<RustItemId>>>,
    imports: BTreeMap<RustModuleId, BTreeMap<String, Vec<RustImportBinding>>>,
    diagnostic_limits: DiagnosticLimits,
    cancellation: CancellationProbe,
}

impl RustSymbolTable {
    pub(super) fn new(
        graph: &RustSourceGraph,
        diagnostic_limits: &DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, RustSymbolTableError> {
        let mut modules = BTreeSet::new();
        for module in graph.modules() {
            cancellation
                .checkpoint()
                .map_err(|_| RustSymbolTableError::OperationCanceled)?;
            modules.insert(module.id().clone());
        }
        Ok(Self {
            modules,
            declarations: BTreeMap::new(),
            imports: BTreeMap::new(),
            diagnostic_limits: diagnostic_limits.clone(),
            cancellation: cancellation.clone(),
        })
    }

    fn checkpoint(&self) -> Result<(), RustSymbolTableError> {
        self.cancellation
            .checkpoint()
            .map_err(|_| RustSymbolTableError::OperationCanceled)
    }

    pub(super) fn register_declaration(
        &mut self,
        id: RustItemId,
    ) -> Result<(), RustSymbolTableError> {
        self.checkpoint()?;
        if !RustImplementationOwner::supports(&id) {
            return Ok(());
        }
        if !self.modules.contains(id.module_id()) {
            return Err(RustSymbolTableError::UnknownDeclarationModule {
                id: id.diagnostic_path(),
                module: id.module_id().clone(),
            });
        }

        let name = id.name().to_owned();
        let candidates = self
            .declarations
            .entry(id.module_id().clone())
            .or_default()
            .entry(name)
            .or_default();
        candidates.push(id);
        candidates.sort();
        self.checkpoint()?;
        Ok(())
    }

    pub(super) fn register_import(
        &mut self,
        binding: RustImportBinding,
    ) -> Result<(), RustSymbolTableError> {
        self.checkpoint()?;
        if !self.modules.contains(binding.declared_in()) {
            let range = binding.span().byte_range();
            return Err(RustSymbolTableError::UnknownImportModule {
                file: binding.span().file().clone(),
                module: binding.declared_in().clone(),
                start: range.start,
                end: range.end,
            });
        }
        if binding.local_name() == "_" {
            return Ok(());
        }

        self.imports
            .entry(binding.declared_in().clone())
            .or_default()
            .entry(binding.local_name().to_owned())
            .or_default()
            .push(binding);
        Ok(())
    }

    pub(super) fn resolve_implementation_owner(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemImpl,
    ) -> Result<RustImplementationOwner, RustSymbolTableError> {
        self.checkpoint()?;
        let site = RustOwnerResolutionSite::from_implementation(context, item)?;
        let syn::Type::Path(owner_path) = item.self_ty.as_ref() else {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site),
                reason: "syntax-mode owner resolution requires an ordinary type path".to_owned(),
            });
        };
        if owner_path.qself.is_some() {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site),
                reason: "qualified-self owner paths require compiler-backed resolution".to_owned(),
            });
        }
        if owner_path.path.leading_colon.is_some() {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site),
                reason: "leading `::` resolves through the external prelude and requires compiler-backed resolution"
                    .to_owned(),
            });
        }

        self.validate_owner_application(item, &owner_path.path, &site)?;
        let segments = RustModulePath::semantic_path_segments(&owner_path.path);
        let mut active_imports = BTreeSet::new();
        let resolved = self.resolve_path(
            context.module_id(),
            false,
            &segments,
            &site,
            &mut active_imports,
        )?;
        let RustResolvedSymbol::Declaration(owner_id) = resolved else {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site),
                reason: "implementation owner path resolves to a module, not a type declaration"
                    .to_owned(),
            });
        };

        RustImplementationOwner::new(owner_id, site.requested.clone()).map_err(|source| {
            RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site),
                reason: source.to_string(),
            }
        })
    }

    fn resolve_path(
        &self,
        from: &RustModuleId,
        leading_colon: bool,
        segments: &[String],
        site: &RustOwnerResolutionSite,
        active_imports: &mut BTreeSet<(RustModuleId, String)>,
    ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
        self.checkpoint()?;
        if leading_colon {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site.clone()),
                reason: "leading `::` resolves through the external prelude and requires compiler-backed resolution"
                    .to_owned(),
            });
        }
        let Some(first) = segments.first().map(String::as_str) else {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site.clone()),
                reason: "implementation owner path cannot be empty".to_owned(),
            });
        };

        let (mut module, mut consumed, allows_super) = match first {
            "crate" => (from.crate_root(), 1, false),
            "self" => (from.clone(), 1, true),
            "super" => (from.clone(), 0, true),
            _ => (from.clone(), 0, false),
        };
        while allows_super
            && segments
                .get(consumed)
                .is_some_and(|segment| segment == "super")
        {
            self.checkpoint()?;
            module = module
                .parent()
                .ok_or_else(|| RustSymbolTableError::UnsupportedOwner {
                    site: Box::new(site.clone()),
                    reason: "implementation owner path traverses above the crate root".to_owned(),
                })?;
            consumed += 1;
        }

        if consumed == segments.len() {
            return Ok(RustResolvedSymbol::Module(module));
        }
        if let Some(keyword) = segments[consumed..]
            .iter()
            .find(|segment| matches!(segment.as_str(), "crate" | "self" | "super"))
        {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site.clone()),
                reason: format!(
                    "path keyword {keyword:?} is not valid at this position in an implementation owner"
                ),
            });
        }

        self.resolve_segments(&module, &segments[consumed..], site, active_imports)
    }

    fn resolve_segments(
        &self,
        module: &RustModuleId,
        segments: &[String],
        site: &RustOwnerResolutionSite,
        active_imports: &mut BTreeSet<(RustModuleId, String)>,
    ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
        self.checkpoint()?;
        let Some(first) = segments.first() else {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site.clone()),
                reason: "implementation owner path cannot be empty".to_owned(),
            });
        };
        let mut resolved = self.resolve_name(module, first, site, active_imports)?;
        for segment in &segments[1..] {
            self.checkpoint()?;
            let RustResolvedSymbol::Module(next_module) = resolved else {
                return Err(RustSymbolTableError::UnsupportedOwner {
                    site: Box::new(site.clone()),
                    reason: format!(
                        "path segment {segment:?} follows a type declaration; associated owner paths require compiler-backed resolution"
                    ),
                });
            };
            resolved = self.resolve_name(&next_module, segment, site, active_imports)?;
        }
        Ok(resolved)
    }

    fn resolve_name(
        &self,
        module: &RustModuleId,
        name: &str,
        site: &RustOwnerResolutionSite,
        active_imports: &mut BTreeSet<(RustModuleId, String)>,
    ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
        self.checkpoint()?;
        if let Some(candidates) = self
            .declarations
            .get(module)
            .and_then(|declarations| declarations.get(name))
        {
            return match candidates.as_slice() {
                [candidate] => Ok(RustResolvedSymbol::Declaration(candidate.clone())),
                _ => Err(RustSymbolTableError::AmbiguousOwner {
                    site: Box::new(site.clone()),
                    candidates: self.diagnostic_candidates(candidates.iter())?,
                }),
            };
        }

        if let Some(bindings) = self
            .imports
            .get(module)
            .and_then(|imports| imports.get(name))
        {
            let [binding] = bindings.as_slice() else {
                let mut evidence = RustResolutionEvidence::new(&self.diagnostic_limits);
                for binding in bindings {
                    self.checkpoint()?;
                    let range = binding.span().byte_range();
                    evidence.insert(format!(
                        "{} in {} at bytes {}..{}",
                        binding.render_path(),
                        binding.span().file(),
                        range.start,
                        range.end
                    ))?;
                }
                return Err(RustSymbolTableError::AmbiguousImport {
                    site: Box::new(site.clone()),
                    local_name: name.to_owned(),
                    bindings: evidence.into_values(),
                });
            };
            return self.resolve_import(binding, site, active_imports);
        }

        if let Some(child) = self.child_module(module, name) {
            return Ok(RustResolvedSymbol::Module(child));
        }

        Err(RustSymbolTableError::UnresolvedOwner {
            site: Box::new(site.clone()),
            suggestions: self.declaration_suggestions(module, name)?,
        })
    }

    fn resolve_import(
        &self,
        binding: &RustImportBinding,
        site: &RustOwnerResolutionSite,
        active_imports: &mut BTreeSet<(RustModuleId, String)>,
    ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
        self.checkpoint()?;
        let key = (
            binding.declared_in().clone(),
            binding.local_name().to_owned(),
        );
        if !active_imports.insert(key.clone()) {
            let mut evidence = RustResolutionEvidence::new(&self.diagnostic_limits);
            for (module, name) in active_imports.iter() {
                self.checkpoint()?;
                evidence.insert(format!("{module}::{name}"))?;
            }
            evidence.insert(format!("{}::{}", key.0, key.1))?;
            return Err(RustSymbolTableError::ImportCycle {
                site: Box::new(site.clone()),
                bindings: evidence.into_values(),
            });
        }

        let resolved = self.resolve_path(
            binding.declared_in(),
            binding.leading_colon(),
            binding.target_segments(),
            site,
            active_imports,
        );
        active_imports.remove(&key);
        resolved
    }

    fn child_module(&self, parent: &RustModuleId, name: &str) -> Option<RustModuleId> {
        let mut segments = parent.module_path().segments().to_vec();
        segments.push(name.to_owned());
        let path = RustModulePath::new(segments).ok()?;
        let child = RustModuleId::new(parent.crate_id().clone(), path);
        self.modules.contains(&child).then_some(child)
    }

    fn declaration_suggestions(
        &self,
        from: &RustModuleId,
        name: &str,
    ) -> Result<Vec<String>, RustSymbolTableError> {
        let mut evidence = RustResolutionEvidence::new(&self.diagnostic_limits);
        for (module, declarations) in &self.declarations {
            self.checkpoint()?;
            if module.crate_id() != from.crate_id() {
                continue;
            }
            let Some(candidates) = declarations.get(name) else {
                continue;
            };
            for candidate in candidates {
                self.checkpoint()?;
                evidence.insert(candidate.diagnostic_path())?;
            }
        }
        Ok(evidence.into_values())
    }

    fn diagnostic_candidates<'candidate>(
        &self,
        candidates: impl Iterator<Item = &'candidate RustItemId>,
    ) -> Result<Vec<String>, RustSymbolTableError> {
        let mut evidence = RustResolutionEvidence::new(&self.diagnostic_limits);
        for candidate in candidates {
            self.checkpoint()?;
            evidence.insert(candidate.diagnostic_path())?;
        }
        Ok(evidence.into_values())
    }

    fn validate_owner_application(
        &self,
        item: &syn::ItemImpl,
        path: &syn::Path,
        site: &RustOwnerResolutionSite,
    ) -> Result<(), RustSymbolTableError> {
        self.checkpoint()?;
        let Some(last) = path.segments.last() else {
            return Err(RustSymbolTableError::UnsupportedOwner {
                site: Box::new(site.clone()),
                reason: "implementation owner path cannot be empty".to_owned(),
            });
        };
        for segment in path
            .segments
            .iter()
            .take(path.segments.len().saturating_sub(1))
        {
            self.checkpoint()?;
            if !matches!(segment.arguments, syn::PathArguments::None) {
                return Err(RustSymbolTableError::UnsupportedOwner {
                    site: Box::new(site.clone()),
                    reason: format!(
                        "nonterminal owner path segment {} cannot have generic arguments in syntax mode",
                        segment.ident
                    ),
                });
            }
        }

        match &last.arguments {
            syn::PathArguments::None => Ok(()),
            syn::PathArguments::AngleBracketed(arguments) => {
                if self.identity_application(&item.generics, arguments)? {
                    Ok(())
                } else {
                    Err(RustSymbolTableError::UnsupportedOwner {
                        site: Box::new(site.clone()),
                        reason: "specialized implementation owner applications require compiler-backed resolution; syntax mode accepts only a bare owner or its declared generic parameters unchanged and in order"
                            .to_owned(),
                    })
                }
            }
            syn::PathArguments::Parenthesized(_) => Err(
                RustSymbolTableError::UnsupportedOwner {
                    site: Box::new(site.clone()),
                    reason: "specialized implementation owner applications require compiler-backed resolution; syntax mode accepts only a bare owner or its declared generic parameters unchanged and in order"
                        .to_owned(),
                },
            ),
        }
    }

    fn identity_application(
        &self,
        generics: &syn::Generics,
        arguments: &syn::AngleBracketedGenericArguments,
    ) -> Result<bool, RustSymbolTableError> {
        if arguments.args.len() != generics.params.len() {
            return Ok(false);
        }
        for (parameter, argument) in generics.params.iter().zip(&arguments.args) {
            self.checkpoint()?;
            if !self.generic_argument_matches(parameter, argument) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn generic_argument_matches(
        &self,
        parameter: &syn::GenericParam,
        argument: &syn::GenericArgument,
    ) -> bool {
        match (parameter, argument) {
            (syn::GenericParam::Type(parameter), syn::GenericArgument::Type(value)) => {
                let name = RustModulePath::semantic_ident(&parameter.ident);
                self.type_path_name(value).as_deref() == Some(name.as_str())
            }
            (syn::GenericParam::Lifetime(parameter), syn::GenericArgument::Lifetime(value)) => {
                value.ident == parameter.lifetime.ident
            }
            (syn::GenericParam::Const(parameter), syn::GenericArgument::Const(value)) => {
                let name = RustModulePath::semantic_ident(&parameter.ident);
                self.const_path_name(value).as_deref() == Some(name.as_str())
            }
            (syn::GenericParam::Const(parameter), syn::GenericArgument::Type(value)) => {
                let name = RustModulePath::semantic_ident(&parameter.ident);
                self.type_path_name(value).as_deref() == Some(name.as_str())
            }
            _ => false,
        }
    }

    fn type_path_name(&self, value: &syn::Type) -> Option<String> {
        let syn::Type::Path(value) = value else {
            return None;
        };
        if value.qself.is_some() {
            return None;
        }
        self.path_name(&value.path)
    }

    fn const_path_name(&self, value: &syn::Expr) -> Option<String> {
        match value {
            syn::Expr::Path(value) if value.qself.is_none() => self.path_name(&value.path),
            syn::Expr::Block(value)
                if value.attrs.is_empty()
                    && matches!(value.block.stmts.as_slice(), [syn::Stmt::Expr(_, None)]) =>
            {
                let [syn::Stmt::Expr(value, None)] = value.block.stmts.as_slice() else {
                    return None;
                };
                self.const_path_name(value)
            }
            _ => None,
        }
    }

    fn path_name(&self, path: &syn::Path) -> Option<String> {
        if path.leading_colon.is_some() || path.segments.len() != 1 {
            return None;
        }
        let segment = path.segments.first()?;
        matches!(segment.arguments, syn::PathArguments::None)
            .then(|| RustModulePath::semantic_ident(&segment.ident))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use quote::ToTokens as _;
    use syn::spanned::Spanned as _;

    use super::{
        RustOwnerResolutionSite, RustResolvedSymbol, RustSymbolTable, RustSymbolTableError,
    };
    use crate::api::{RustCrateKind, RustCrateRoot};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::languages::rust::parser::inventory_collector::RustItemContext;
    use crate::languages::rust::parser::item_converter::RustImportBinding;
    use crate::languages::rust::parser::signature_id::RustItemId;
    use crate::languages::rust::parser::source_graph::{
        RustCapabilityDiagnostics, RustCrateId, RustExtraction, RustModuleId, RustModulePath,
        RustSourceGraph,
    };
    use crate::languages::rust::source::RustSourceCatalog;
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::declaration::RustItemKind;

    struct SymbolFixture {
        table: RustSymbolTable,
        root: RustModuleId,
        models: RustModuleId,
        other: RustModuleId,
    }

    impl SymbolFixture {
        fn new() -> Self {
            let file = CatalogPath::new("lib.rs").expect("fixture source path");
            let files = BTreeSet::from([file.clone()]);
            let mut catalog = FileCatalog::new();
            catalog
                .insert(
                    file.clone(),
                    br#"
                        mod models { pub struct Widget; }
                        mod other { pub struct Widget; }
                    "#
                    .to_vec(),
                )
                .expect("unique fixture source");
            let cancellation = crate::work::CancellationProbe::new();
            let sources = RustSourceCatalog::parse_allowlist(&files, catalog, &cancellation)
                .expect("parsed fixture source");
            let extraction = RustExtraction::from_roots(
                files,
                [RustCrateRoot {
                    id: "sample".to_owned(),
                    root: file,
                    kind: RustCrateKind::Library,
                }],
                &cancellation,
            )
            .expect("fixture extraction");
            let limits = crate::limits::RustExtractionLimits::default();
            let mut usage = limits.usage();
            let diagnostic_limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&diagnostic_limits);
            let graph = RustSourceGraph::build(
                &extraction,
                &sources,
                &mut usage,
                &mut diagnostics,
                &cancellation,
            )
            .expect("fixture source graph");
            let root = Self::module_id(&[]);
            let models = Self::module_id(&["models"]);
            let other = Self::module_id(&["other"]);
            Self {
                table: RustSymbolTable::new(&graph, &diagnostic_limits, &cancellation)
                    .expect("fixture symbol table"),
                root,
                models,
                other,
            }
        }

        fn module_id(path: &[&str]) -> RustModuleId {
            Self::module_id_in("sample", path)
        }

        fn module_id_in(crate_id: &str, path: &[&str]) -> RustModuleId {
            RustModuleId::new(
                RustCrateId::new(crate_id, &crate::work::CancellationProbe::new())
                    .expect("fixture crate id"),
                RustModulePath::new(path.iter().map(|segment| (*segment).to_owned()).collect())
                    .expect("fixture module path"),
            )
        }

        fn owner_id(module: &RustModuleId, name: &str) -> RustItemId {
            RustItemId::new(module.clone(), RustItemKind::Struct, name)
        }

        fn site(module: &RustModuleId, requested: &str) -> RustOwnerResolutionSite {
            RustOwnerResolutionSite {
                file: CatalogPath::new("lib.rs").expect("fixture source path"),
                module: module.clone(),
                start: 10,
                end: 20,
                requested: requested.to_owned(),
            }
        }

        fn import_binding(
            declared_in: &RustModuleId,
            target_segments: &[&str],
            alias: &str,
        ) -> RustImportBinding {
            let file = CatalogPath::new("binding.rs").expect("binding source path");
            let mut catalog = FileCatalog::new();
            catalog
                .insert(
                    file.clone(),
                    b"use crate::models::Widget as FixtureAlias;".to_vec(),
                )
                .expect("unique binding source");
            let sources = RustSourceCatalog::parse_allowlist(
                &BTreeSet::from([file.clone()]),
                catalog,
                &crate::work::CancellationProbe::new(),
            )
            .expect("parsed binding source");
            let syntax = sources
                .shared_syntax(&file)
                .expect("binding syntax retained");
            let span = sources
                .source_span(&file, syntax.items[0].span())
                .expect("binding source span");
            RustImportBinding::new(
                declared_in.clone(),
                span,
                false,
                target_segments
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect(),
                Some(alias.to_owned()),
                RustAttributes::default(),
            )
            .expect("valid fixture import")
        }

        fn resolve(
            &self,
            from: &RustModuleId,
            segments: &[&str],
        ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
            self.table.resolve_path(
                from,
                false,
                &segments
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>(),
                &Self::site(from, &segments.join("::")),
                &mut BTreeSet::new(),
            )
        }

        fn resolve_syntax(
            &self,
            from: &RustModuleId,
            path: syn::Path,
        ) -> Result<RustResolvedSymbol, RustSymbolTableError> {
            let requested = path.to_token_stream().to_string();
            self.table.resolve_path(
                from,
                path.leading_colon.is_some(),
                &RustModulePath::semantic_path_segments(&path),
                &Self::site(from, &requested),
                &mut BTreeSet::new(),
            )
        }
    }

    #[test]
    fn same_module_owner_wins_without_a_global_name_fallback() {
        let mut fixture = SymbolFixture::new();
        let root_owner = SymbolFixture::owner_id(&fixture.root, "Widget");
        let other_owner = SymbolFixture::owner_id(&fixture.other, "Widget");
        fixture
            .table
            .register_declaration(root_owner.clone())
            .expect("root owner registration");
        fixture
            .table
            .register_declaration(other_owner)
            .expect("other owner registration");

        assert_eq!(
            fixture
                .resolve(&fixture.root, &["Widget"])
                .expect("same-module owner resolution"),
            RustResolvedSymbol::Declaration(root_owner)
        );

        let mut nonlocal = SymbolFixture::new();
        let nonlocal_owner = SymbolFixture::owner_id(&nonlocal.other, "Widget");
        nonlocal
            .table
            .register_declaration(nonlocal_owner.clone())
            .expect("nonlocal owner registration");
        let error = nonlocal
            .resolve(&nonlocal.root, &["Widget"])
            .expect_err("a globally unique nonlocal owner must not be selected");
        let RustSymbolTableError::UnresolvedOwner { suggestions, .. } = error else {
            panic!("unexpected nonlocal resolution error");
        };
        assert_eq!(suggestions, vec![nonlocal_owner.diagnostic_path()]);
    }

    #[test]
    fn raw_owner_paths_and_import_aliases_resolve_to_semantic_identifiers() {
        let mut fixture = SymbolFixture::new();
        let keyword_module = SymbolFixture::module_id(&["type"]);
        fixture.table.modules.insert(keyword_module.clone());
        let owner = SymbolFixture::owner_id(&keyword_module, "match");
        fixture
            .table
            .register_declaration(owner.clone())
            .expect("raw owner declaration");

        assert_eq!(
            fixture
                .resolve_syntax(&fixture.root, syn::parse_quote!(crate::r#type::r#match),)
                .expect("raw qualified owner"),
            RustResolvedSymbol::Declaration(owner.clone())
        );

        fixture
            .table
            .register_import(SymbolFixture::import_binding(
                &fixture.root,
                &["crate", "type", "match"],
                "async",
            ))
            .expect("raw alias binding");
        assert_eq!(
            fixture
                .resolve_syntax(&fixture.root, syn::parse_quote!(r#async))
                .expect("raw import alias"),
            RustResolvedSymbol::Declaration(owner)
        );
    }

    #[test]
    fn implementation_owner_resolution_unraws_the_parsed_owner_path() {
        let file = CatalogPath::new("lib.rs").expect("fixture source path");
        let files = BTreeSet::from([file.clone()]);
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                file.clone(),
                b"mod r#type { pub struct r#match; } impl crate::r#type::r#match {}".to_vec(),
            )
            .expect("unique fixture source");
        let cancellation = crate::work::CancellationProbe::new();
        let sources = RustSourceCatalog::parse_allowlist(&files, catalog, &cancellation)
            .expect("parsed raw-owner source");
        let extraction = RustExtraction::from_roots(
            files,
            [RustCrateRoot {
                id: "sample".to_owned(),
                root: file.clone(),
                kind: RustCrateKind::Library,
            }],
            &cancellation,
        )
        .expect("fixture extraction");
        let limits = crate::limits::RustExtractionLimits::default();
        let mut usage = limits.usage();
        let diagnostic_limits = crate::limits::DiagnosticLimits::default();
        let mut diagnostics = RustCapabilityDiagnostics::new(&diagnostic_limits);
        let graph = RustSourceGraph::build(
            &extraction,
            &sources,
            &mut usage,
            &mut diagnostics,
            &cancellation,
        )
        .expect("raw-owner source graph");
        let root = SymbolFixture::module_id(&[]);
        let owner_id = SymbolFixture::owner_id(&SymbolFixture::module_id(&["type"]), "match");
        let mut table = RustSymbolTable::new(&graph, &diagnostic_limits, &cancellation)
            .expect("raw-owner symbol table");
        table
            .register_declaration(owner_id.clone())
            .expect("raw owner registration");
        let syntax = sources.shared_syntax(&file).expect("fixture syntax");
        let implementation = syntax
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Impl(item) => Some(item),
                _ => None,
            })
            .expect("fixture implementation");
        let context = RustItemContext::new(&sources, file, root);

        assert_eq!(
            table
                .resolve_implementation_owner(&context, implementation)
                .expect("raw implementation owner")
                .id(),
            &owner_id
        );
    }

    #[test]
    fn unresolved_suggestions_are_sorted_readable_and_current_crate_only() {
        let mut fixture = SymbolFixture::new();
        let alpha = SymbolFixture::module_id(&["alpha"]);
        let zeta = SymbolFixture::module_id(&["zeta"]);
        let foreign = SymbolFixture::module_id_in("foreign", &["api"]);
        fixture.table.modules.insert(alpha.clone());
        fixture.table.modules.insert(zeta.clone());
        fixture.table.modules.insert(foreign.clone());

        let alpha_owner = SymbolFixture::owner_id(&alpha, "Widget");
        let zeta_owner = SymbolFixture::owner_id(&zeta, "Widget");
        let foreign_owner = SymbolFixture::owner_id(&foreign, "Widget");
        for owner in [&zeta_owner, &foreign_owner, &alpha_owner] {
            fixture
                .table
                .register_declaration(owner.clone())
                .expect("suggestion candidate");
        }

        let error = fixture
            .resolve(&fixture.root, &["Widget"])
            .expect_err("nonlexical owner remains unresolved");
        let rendered = error.to_string();
        let RustSymbolTableError::UnresolvedOwner { site, suggestions } = error else {
            panic!("unexpected suggestion error");
        };

        assert_eq!(
            suggestions,
            vec![alpha_owner.diagnostic_path(), zeta_owner.diagnostic_path()]
        );
        assert!(rendered.contains(&site.to_string()), "{rendered}");
        assert!(!rendered.contains("rust:v2:"), "{rendered}");
        assert!(!rendered.contains("foreign::api::Widget"), "{rendered}");
    }

    #[test]
    fn ambiguous_owner_evidence_uses_readable_sorted_paths_and_the_site() {
        let mut fixture = SymbolFixture::new();
        let structure = RustItemId::new(fixture.root.clone(), RustItemKind::Struct, "Widget");
        let enumeration = RustItemId::new(fixture.root.clone(), RustItemKind::Enum, "Widget");
        fixture
            .table
            .register_declaration(structure.clone())
            .expect("structure candidate");
        fixture
            .table
            .register_declaration(enumeration.clone())
            .expect("enumeration candidate");

        let error = fixture
            .resolve(&fixture.root, &["Widget"])
            .expect_err("same-module duplicate owner name must be ambiguous");
        let rendered = error.to_string();
        let RustSymbolTableError::AmbiguousOwner { site, candidates } = error else {
            panic!("unexpected ambiguity error");
        };
        let mut expected = vec![structure.diagnostic_path(), enumeration.diagnostic_path()];
        expected.sort();

        assert_eq!(candidates, expected);
        assert!(rendered.contains(&site.to_string()), "{rendered}");
        assert!(!rendered.contains("rust:v2:"), "{rendered}");
    }

    #[test]
    fn ambiguous_owner_evidence_is_bounded_before_candidates_are_retained() {
        let mut fixture = SymbolFixture::new();
        fixture.table.diagnostic_limits = crate::limits::DiagnosticLimits {
            count: 1,
            serialized_bytes: u64::MAX,
        };
        fixture
            .table
            .register_declaration(RustItemId::new(
                fixture.root.clone(),
                RustItemKind::Struct,
                "Widget",
            ))
            .expect("structure candidate");
        fixture
            .table
            .register_declaration(RustItemId::new(
                fixture.root.clone(),
                RustItemKind::Enum,
                "Widget",
            ))
            .expect("enumeration candidate");

        let error = fixture
            .resolve(&fixture.root, &["Widget"])
            .expect_err("second retained candidate must cross the evidence budget");

        assert!(
            matches!(&error, RustSymbolTableError::LimitExceeded(_)),
            "{error}"
        );
    }

    #[test]
    fn candidate_registration_errors_do_not_expose_wire_identity_encoding() {
        let mut fixture = SymbolFixture::new();
        let missing_module = SymbolFixture::module_id(&["missing"]);
        let candidate = SymbolFixture::owner_id(&missing_module, "Widget");

        let error = fixture
            .table
            .register_declaration(candidate.clone())
            .expect_err("module outside graph must fail registration");
        let rendered = error.to_string();

        assert!(
            rendered.contains(&candidate.diagnostic_path()),
            "{rendered}"
        );
        assert!(!rendered.contains("rust:v2:"), "{rendered}");
    }

    #[test]
    fn direct_and_module_import_aliases_resolve_lexically() {
        let mut fixture = SymbolFixture::new();
        let owner = SymbolFixture::owner_id(&fixture.models, "Widget");
        fixture
            .table
            .register_declaration(owner.clone())
            .expect("model owner registration");
        fixture
            .table
            .register_import(SymbolFixture::import_binding(
                &fixture.root,
                &["crate", "models", "Widget"],
                "LocalWidget",
            ))
            .expect("direct alias registration");
        fixture
            .table
            .register_import(SymbolFixture::import_binding(
                &fixture.root,
                &["crate", "models"],
                "model",
            ))
            .expect("module alias registration");

        assert_eq!(
            fixture
                .resolve(&fixture.root, &["LocalWidget"])
                .expect("direct alias resolution"),
            RustResolvedSymbol::Declaration(owner.clone())
        );
        assert_eq!(
            fixture
                .resolve(&fixture.root, &["model", "Widget"])
                .expect("module alias resolution"),
            RustResolvedSymbol::Declaration(owner)
        );
    }

    #[test]
    fn import_cycles_fail_with_deterministic_binding_evidence() {
        let mut fixture = SymbolFixture::new();
        fixture
            .table
            .register_import(SymbolFixture::import_binding(
                &fixture.root,
                &["Second"],
                "First",
            ))
            .expect("first cyclic import");
        fixture
            .table
            .register_import(SymbolFixture::import_binding(
                &fixture.root,
                &["First"],
                "Second",
            ))
            .expect("second cyclic import");

        let error = fixture
            .resolve(&fixture.root, &["First"])
            .expect_err("cyclic aliases must fail");
        let RustSymbolTableError::ImportCycle { bindings, .. } = error else {
            panic!("unexpected cyclic import error");
        };
        assert_eq!(
            bindings,
            vec!["sample::First".to_owned(), "sample::Second".to_owned()]
        );
    }

    #[test]
    fn crate_self_and_repeated_super_paths_share_exact_module_identity() {
        let mut fixture = SymbolFixture::new();
        let outer = SymbolFixture::module_id(&["outer"]);
        let inner = SymbolFixture::module_id(&["outer", "inner"]);
        let leaf = SymbolFixture::module_id(&["outer", "inner", "leaf"]);
        fixture.table.modules.insert(outer.clone());
        fixture.table.modules.insert(inner);
        fixture.table.modules.insert(leaf.clone());
        let owner = SymbolFixture::owner_id(&outer, "Widget");
        fixture
            .table
            .register_declaration(owner.clone())
            .expect("outer owner registration");

        for path in [
            &["crate", "outer", "Widget"][..],
            &["super", "super", "Widget"][..],
            &["self", "super", "super", "Widget"][..],
        ] {
            assert_eq!(
                fixture
                    .resolve(&leaf, path)
                    .expect("qualified lexical owner resolution"),
                RustResolvedSymbol::Declaration(owner.clone())
            );
        }
    }

    #[test]
    fn specialized_owner_applications_fail_while_identity_applications_remain_supported() {
        let fixture = SymbolFixture::new();
        let site = SymbolFixture::site(&fixture.root, "Widget<T>");
        let identity: syn::ItemImpl = syn::parse_quote!(
            impl<T> Widget<T> {}
        );
        let specialized: syn::ItemImpl = syn::parse_quote!(impl Widget<u8> {});
        let syn::Type::Path(identity_path) = identity.self_ty.as_ref() else {
            panic!("identity fixture must use a type path");
        };
        let syn::Type::Path(specialized_path) = specialized.self_ty.as_ref() else {
            panic!("specialized fixture must use a type path");
        };

        fixture
            .table
            .validate_owner_application(&identity, &identity_path.path, &site)
            .expect("identity generic application");
        let error = fixture
            .table
            .validate_owner_application(&specialized, &specialized_path.path, &site)
            .expect_err("specialized application must fail closed");
        assert!(error.to_string().contains("specialized"));
    }

    #[test]
    fn canceled_symbol_table_stops_before_mutating_registration_state() {
        let mut fixture = SymbolFixture::new();
        fixture.table.cancellation.cancel();

        let error = fixture
            .table
            .register_declaration(SymbolFixture::owner_id(&fixture.root, "Canceled"))
            .expect_err("canceled registration must stop");

        assert_eq!(error, RustSymbolTableError::OperationCanceled);
        assert!(fixture.table.declarations.is_empty());
    }
}
