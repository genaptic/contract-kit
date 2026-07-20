use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use syn::spanned::Spanned as _;

use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::item_converter::{
    RustConvertedDeclaration, RustItemConversion, RustItemConverter,
};
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemIdAllocator};
use crate::languages::rust::parser::source_graph::{
    RustAssociatedMacroContainer, RustCapabilityDiagnostic, RustCapabilityDiagnostics,
    RustModuleId, RustSourceGraph,
};
use crate::languages::rust::parser::symbol_table::RustSymbolTable;
use crate::languages::rust::parser::{RustParsedEntry, RustParsedProjection};
use crate::languages::rust::source::{RustSourceCatalog, RustSourceSpan};
use crate::languages::rust::types::declaration::{RustDeclaration, RustItemKind};
use crate::limits::{DiagnosticLimits, RustExtractionUsage};
use crate::work::CancellationProbe;

pub(super) struct RustInventoryCollector<
    'source,
    'usage,
    'limits,
    'diagnostics,
    'diagnostic_limits,
    'cancellation,
> {
    sources: &'source RustSourceCatalog,
    usage: &'usage mut RustExtractionUsage<'limits>,
    diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
    cancellation: &'cancellation CancellationProbe,
    allocator: RustItemIdAllocator,
    converter: RustItemConverter,
    symbol_table: RustSymbolTable,
    entries: Vec<RustParsedEntry>,
    implementation_sites: Vec<RustImplementationSite<'source>>,
}

impl<'source, 'usage, 'limits, 'diagnostics, 'diagnostic_limits, 'cancellation>
    RustInventoryCollector<
        'source,
        'usage,
        'limits,
        'diagnostics,
        'diagnostic_limits,
        'cancellation,
    >
{
    pub(super) fn collect(
        sources: &'source RustSourceCatalog,
        graph: RustSourceGraph,
        usage: &'usage mut RustExtractionUsage<'limits>,
        diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
        diagnostic_limits: &'diagnostic_limits DiagnosticLimits,
        cancellation: &'cancellation CancellationProbe,
    ) -> Result<RustParsedProjection, SignatureContractKitError> {
        let mut collector = Self::new(
            sources,
            &graph,
            usage,
            diagnostics,
            diagnostic_limits,
            cancellation,
        )?;
        collector.collect_declarations(&graph)?;
        collector.collect_implementations()?;
        collector.finish()
    }

    fn new(
        sources: &'source RustSourceCatalog,
        graph: &RustSourceGraph,
        usage: &'usage mut RustExtractionUsage<'limits>,
        diagnostics: &'diagnostics mut RustCapabilityDiagnostics<'diagnostic_limits>,
        diagnostic_limits: &'diagnostic_limits DiagnosticLimits,
        cancellation: &'cancellation CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            sources,
            usage,
            cancellation,
            diagnostics,
            allocator: RustItemIdAllocator::default(),
            converter: RustItemConverter::new(cancellation),
            symbol_table: RustSymbolTable::new(graph, diagnostic_limits, cancellation)?,
            entries: Vec::new(),
            implementation_sites: Vec::new(),
        })
    }

    fn collect_declarations(
        &mut self,
        graph: &RustSourceGraph,
    ) -> Result<(), SignatureContractKitError> {
        for module in graph.modules() {
            self.cancellation.checkpoint()?;
            let context =
                RustItemContext::new(self.sources, module.file().clone(), module.id().clone());
            for item in module.items(self.sources)? {
                self.cancellation.checkpoint()?;
                let source_span = context.source_span(item.span())?;
                if let syn::Item::Impl(implementation) = item {
                    self.implementation_sites.push(RustImplementationSite::new(
                        context.clone(),
                        implementation,
                        source_span,
                    ));
                    continue;
                }

                let converted = self.converter.convert_non_implementation_item(
                    &context,
                    item,
                    self.diagnostics,
                )?;
                self.record_conversion(&context, source_span, converted)?;
            }
        }
        Ok(())
    }

    fn record_conversion(
        &mut self,
        context: &RustItemContext<'_>,
        source_span: RustSourceSpan,
        converted: RustItemConversion,
    ) -> Result<(), SignatureContractKitError> {
        match converted {
            RustItemConversion::Declaration(declaration) => {
                self.record_declaration(context.module_id(), source_span, declaration)?;
            }
            RustItemConversion::PublicReexport {
                declaration,
                binding,
            } => {
                self.symbol_table.register_import(binding)?;
                self.record_declaration(context.module_id(), source_span, declaration)?;
            }
            RustItemConversion::PrivateImport(binding) => {
                if binding.requires_capability_warning() {
                    self.diagnostics
                        .insert(RustCapabilityDiagnostic::private_import(
                            binding.declared_in().clone(),
                            binding.span().clone(),
                        ))?;
                }
                self.symbol_table.register_import(binding)?;
            }
        }
        Ok(())
    }

    fn record_declaration(
        &mut self,
        module_id: &RustModuleId,
        source_span: RustSourceSpan,
        converted: RustConvertedDeclaration,
    ) -> Result<(), SignatureContractKitError> {
        let (semantic_name, declaration) = converted.into_parts();
        if !matches!(&declaration, RustDeclaration::Implementation(_)) {
            self.usage.record_signatures(1)?;
        }
        let kind = declaration.kind();
        let id = RustItemId::new(module_id.clone(), kind, semantic_name);
        self.symbol_table.register_declaration(id.clone())?;
        self.entries
            .push(RustParsedEntry::from_source(id, declaration, source_span));
        Ok(())
    }

    fn collect_implementations(&mut self) -> Result<(), SignatureContractKitError> {
        let mut grouped = BTreeMap::new();
        for site in std::mem::take(&mut self.implementation_sites) {
            self.cancellation.checkpoint()?;
            let owner = self
                .symbol_table
                .resolve_implementation_owner(site.context(), site.item())?;
            let converted = self.converter.convert_implementation(
                site.context(),
                site.item(),
                owner,
                self.diagnostics,
            )?;
            let (semantic_name, declaration) = converted.into_parts();
            let RustDeclaration::Implementation(implementation) = &declaration else {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation conversion produced a non-implementation declaration",
                ));
            };
            let owner_id = implementation.owner().id().clone();
            if semantic_name != owner_id.render() {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation semantic identity differs from its resolved owner",
                ));
            }
            let id = RustItemId::new(
                owner_id.module_id().clone(),
                RustItemKind::Implementation,
                owner_id.render(),
            );
            let parsed = RustParsedEntry::from_source(id, declaration, site.into_source_span());
            let key = parsed.implementation_descriptor()?;
            match grouped.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(parsed);
                }
                Entry::Occupied(mut entry) => {
                    entry
                        .get_mut()
                        .merge_same_implementation(parsed, self.cancellation)?;
                }
            }
        }

        for mut entry in grouped.into_values() {
            self.cancellation.checkpoint()?;
            entry.finalize_implementation(self.cancellation)?;
            self.usage.record_signatures(1)?;
            self.entries.push(entry);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<RustParsedProjection, SignatureContractKitError> {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            self.cancellation.checkpoint()?;
            entries.push(entry.allocate_id(&mut self.allocator)?);
        }
        for entry in &entries {
            self.cancellation.checkpoint()?;
            if let Some(reason) = entry.declaration().capability_reason() {
                self.diagnostics
                    .insert(RustCapabilityDiagnostic::declaration(
                        entry.id().clone(),
                        reason,
                        entry.source_span()?.clone(),
                    ))?;
            }
        }
        RustParsedProjection::new(entries, self.cancellation)
    }
}

struct RustImplementationSite<'source> {
    context: RustItemContext<'source>,
    item: &'source syn::ItemImpl,
    source_span: RustSourceSpan,
}

impl<'source> RustImplementationSite<'source> {
    fn new(
        context: RustItemContext<'source>,
        item: &'source syn::ItemImpl,
        source_span: RustSourceSpan,
    ) -> Self {
        Self {
            context,
            item,
            source_span,
        }
    }

    fn context(&self) -> &RustItemContext<'source> {
        &self.context
    }

    fn item(&self) -> &syn::ItemImpl {
        self.item
    }

    fn into_source_span(self) -> RustSourceSpan {
        self.source_span
    }
}

#[derive(Clone)]
pub(super) struct RustItemContext<'source> {
    sources: &'source RustSourceCatalog,
    file: CatalogPath,
    module_id: RustModuleId,
}

impl<'source> RustItemContext<'source> {
    pub(super) fn new(
        sources: &'source RustSourceCatalog,
        file: CatalogPath,
        module_id: RustModuleId,
    ) -> Self {
        Self {
            sources,
            file,
            module_id,
        }
    }

    pub(super) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(super) fn module_id(&self) -> &RustModuleId {
        &self.module_id
    }

    pub(super) fn source_span(
        &self,
        span: proc_macro2::Span,
    ) -> Result<RustSourceSpan, SignatureContractKitError> {
        self.sources.source_span(&self.file, span)
    }

    pub(super) fn unsupported_syntax<T>(
        &self,
        syntax_kind: impl Into<String>,
        span: proc_macro2::Span,
    ) -> Result<T, SignatureContractKitError> {
        Err(SignatureContractKitError::unsupported_rust_syntax(
            self.module_id.clone(),
            syntax_kind,
            self.source_span(span)?,
        ))
    }

    pub(super) fn associated_macro_diagnostic(
        &self,
        container: RustAssociatedMacroContainer,
        span: proc_macro2::Span,
    ) -> Result<RustCapabilityDiagnostic, SignatureContractKitError> {
        Ok(RustCapabilityDiagnostic::associated_macro(
            self.module_id.clone(),
            container,
            self.source_span(span)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::RustParsedProjection;
    use super::RustInventoryCollector;
    use crate::api::{RustCrateKind, RustCrateRoot};
    use crate::error::SignatureContractKitError;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::languages::rust::parser::source_graph::{
        RustCapabilityDiagnostic, RustCapabilityDiagnostics, RustExtraction, RustSourceGraph,
    };
    use crate::languages::rust::source::RustSourceCatalog;
    use crate::languages::rust::types::declaration::RustDeclaration;
    use crate::limits::RustExtractionLimits;
    use crate::work::CancellationProbe;

    struct InventoryFixture {
        sources: RustSourceCatalog,
        extraction: RustExtraction,
    }

    impl InventoryFixture {
        fn root(source: &str) -> Self {
            Self::files(&[("lib.rs", source)], &["lib.rs"], &[("sample", "lib.rs")])
        }

        fn files(entries: &[(&str, &str)], selected: &[&str], crates: &[(&str, &str)]) -> Self {
            let selected = selected
                .iter()
                .map(|path| CatalogPath::new(*path).expect("fixture selected path"))
                .collect::<BTreeSet<_>>();
            let mut catalog = FileCatalog::new();
            for (path, source) in entries {
                catalog
                    .insert(
                        CatalogPath::new(*path).expect("fixture catalog path"),
                        source.as_bytes().to_vec(),
                    )
                    .expect("unique fixture catalog path");
            }
            let cancellation = CancellationProbe::new();
            let extraction = RustExtraction::from_roots(
                selected.clone(),
                crates.iter().map(|(id, root)| RustCrateRoot {
                    id: (*id).to_owned(),
                    root: CatalogPath::new(*root).expect("fixture crate root"),
                    kind: RustCrateKind::Library,
                }),
                &cancellation,
            )
            .expect("fixture extraction");
            let sources = RustSourceCatalog::parse_allowlist(&selected, catalog, &cancellation)
                .expect("fixture sources");
            Self {
                sources,
                extraction,
            }
        }

        fn project(&self) -> Result<RustParsedProjection, SignatureContractKitError> {
            let limits = RustExtractionLimits::default();
            self.project_with_limits(&limits)
        }

        fn project_with_limits(
            &self,
            limits: &RustExtractionLimits,
        ) -> Result<RustParsedProjection, SignatureContractKitError> {
            self.collect_with_limits(limits)
                .map(|(projection, _)| projection)
        }

        fn project_with_diagnostics(
            &self,
        ) -> Result<(RustParsedProjection, Vec<RustCapabilityDiagnostic>), SignatureContractKitError>
        {
            self.collect_with_limits(&RustExtractionLimits::default())
        }

        fn collect_with_limits(
            &self,
            limits: &RustExtractionLimits,
        ) -> Result<(RustParsedProjection, Vec<RustCapabilityDiagnostic>), SignatureContractKitError>
        {
            let mut usage = limits.usage();
            let cancellation = CancellationProbe::new();
            let diagnostic_limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&diagnostic_limits);
            let graph = RustSourceGraph::build(
                &self.extraction,
                &self.sources,
                &mut usage,
                &mut diagnostics,
                &cancellation,
            )?;
            let projection = RustInventoryCollector::collect(
                &self.sources,
                graph,
                &mut usage,
                &mut diagnostics,
                &diagnostic_limits,
                &cancellation,
            )?;
            Ok((projection, diagnostics.into_values(&cancellation)?))
        }
    }

    #[test]
    fn every_modeled_top_level_family_enters_one_graph_projection() {
        let fixture = InventoryFixture::root(
            r#"
                pub const LIMIT: usize = 4;
                pub enum Choice { One }
                pub extern crate core as rust_core;
                pub fn execute() {}
                unsafe extern "C" { pub fn native_execute(); }
                pub struct Handler;
                impl Handler { pub fn execute(&self) {} }
                contract_item!();
                pub mod inline { pub fn nested() {} }
                pub static GLOBAL: usize = 4;
                pub trait Service { fn execute(&self); }
                pub trait ServiceAlias = Send + Sync;
                pub type ResultValue = Result<(), Error>;
                pub union Number { integer: u32, float: f32 }
                use crate::Handler as PrivateHandler;
                pub use crate::Handler as PublicHandler;
            "#,
        );

        let projection = fixture.project().expect("every pinned item is modeled");
        let kinds = projection
            .entries()
            .iter()
            .map(|entry| entry.id().kind().to_string())
            .collect::<Vec<_>>();

        for expected in [
            "constant",
            "enum",
            "extern_crate",
            "function",
            "foreign_module",
            "impl",
            "macro",
            "module",
            "static",
            "struct",
            "trait",
            "trait_alias",
            "type_alias",
            "union",
            "reexport",
        ] {
            assert!(
                kinds.iter().any(|actual| actual == expected),
                "missing {expected} from {kinds:?}"
            );
        }
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| kind.as_str() == "reexport")
                .count(),
            1,
            "private imports belong only to lexical resolution"
        );
    }

    #[test]
    fn lexical_owner_resolution_prefers_the_implementation_module() {
        let fixture = InventoryFixture::root(
            r#"
                mod alpha {
                    pub struct Widget;
                    impl Widget { pub fn new() -> Self { Self } }
                }
                mod beta { pub struct Widget; }
            "#,
        );

        let projection = fixture.project().expect("lexical owner resolution");
        let implementation = projection
            .entries()
            .iter()
            .find(|entry| matches!(entry.declaration(), RustDeclaration::Implementation(_)))
            .expect("implementation entry");

        assert_eq!(
            implementation.id().module_id().module_path().segments(),
            &["alpha".to_owned()]
        );
        assert_eq!(implementation.file().as_str(), "lib.rs");
        assert!(
            fixture
                .sources
                .source_text(implementation.source_span().expect("source origin"))
                .expect("implementation source")
                .trim_start()
                .starts_with("impl Widget")
        );
    }

    #[test]
    fn explicit_import_alias_resolves_without_a_global_name_fallback() {
        let fixture = InventoryFixture::root(
            r#"
                mod model { pub struct Widget; }
                mod adapter {
                    use crate::model::Widget as LocalWidget;
                    impl LocalWidget {}
                }
            "#,
        );

        let projection = fixture.project().expect("explicit alias resolution");
        let implementation = projection
            .entries()
            .iter()
            .find(|entry| matches!(entry.declaration(), RustDeclaration::Implementation(_)))
            .expect("implementation entry");

        assert_eq!(
            implementation.id().module_id().module_path().segments(),
            &["model".to_owned()]
        );
        assert_eq!(implementation.file().as_str(), "lib.rs");
    }

    #[test]
    fn unrelated_global_name_never_resolves_a_bare_owner() {
        let fixture = InventoryFixture::root(
            r#"
                mod model { pub struct Widget; }
                mod adapter { impl Widget {} }
            "#,
        );

        let error = match fixture.project() {
            Ok(_) => panic!("global bare-name fallback must remain deleted"),
            Err(error) => error,
        };
        let message = error.to_string();

        assert!(message.contains("Widget"), "{message}");
        assert!(message.contains("adapter"), "{message}");
        assert!(message.contains("model"), "{message}");
    }

    #[test]
    fn graph_and_associated_macro_capability_evidence_reaches_operation_collector() {
        let fixture = InventoryFixture::root(
            r#"
                #[cfg(feature = "optional")]
                mod conditional { pub fn enabled() {} }
                pub trait Service { generated_item!(); }
            "#,
        );

        let (_, diagnostics) = fixture
            .project_with_diagnostics()
            .expect("capability-bearing projection");
        let rendered = diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 3);
        assert!(
            rendered
                .iter()
                .any(|message| message.contains("not evaluated"))
        );
        assert!(
            rendered
                .iter()
                .any(|message| message.contains("conditional"))
        );
        assert!(
            rendered
                .iter()
                .any(|message| message.contains("trait item"))
        );
    }

    #[test]
    fn declaration_capability_evidence_preserves_the_conditional_reason() {
        let fixture = InventoryFixture::root(
            r#"
                #[cfg(feature = "durable")]
                pub fn conditional() {}
            "#,
        );

        let (_, diagnostics) = fixture
            .project_with_diagnostics()
            .expect("conditional declaration");
        let warnings = diagnostics
            .iter()
            .map(RustCapabilityDiagnostic::warning_message)
            .collect::<Vec<_>>();

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("cfg/cfg_attr"), "{}", warnings[0]);
        assert!(warnings[0].contains("rust_syntax_v2"), "{}", warnings[0]);
    }

    #[test]
    fn private_import_attributes_warn_without_duplicating_public_reexport_evidence() {
        let fixture = InventoryFixture::root(
            r#"
                mod model { pub struct Widget; }

                #[cfg(feature = "cfg-import")]
                use crate::model::Widget as CfgWidget;

                #[cfg_attr(feature = "cfg-attr-import", deprecated)]
                use crate::model::Widget as CfgAttrWidget;

                #[contract_semantics(owner = "host")]
                use crate::model::Widget as UnknownAttributeWidget;

                #[cfg(feature = "public-reexport")]
                pub use crate::model::Widget as PublicWidget;
            "#,
        );

        let (_, diagnostics) = fixture
            .project_with_diagnostics()
            .expect("attribute-bearing imports remain available to resolution");
        let private_imports = diagnostics
            .iter()
            .filter(|diagnostic| {
                matches!(
                    diagnostic,
                    RustCapabilityDiagnostic::PrivateImportSemantics { .. }
                )
            })
            .collect::<Vec<_>>();
        let declarations = diagnostics
            .iter()
            .filter(|diagnostic| {
                matches!(
                    diagnostic,
                    RustCapabilityDiagnostic::DeclarationSemantics { .. }
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(private_imports.len(), 3, "{private_imports:?}");
        assert_eq!(declarations.len(), 1, "{declarations:?}");
        assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
        for diagnostic in private_imports {
            let message = diagnostic.to_string();
            assert!(message.contains("private import"), "{message}");
            assert!(message.contains("lib.rs"), "{message}");
            assert!(message.contains("bytes"), "{message}");
        }
        assert!(
            declarations[0].to_string().contains("PublicWidget"),
            "{}",
            declarations[0]
        );
    }

    #[test]
    fn nested_semantic_uncertainty_emits_one_declaration_level_diagnostic() {
        let fixture = InventoryFixture::root(
            r#"
                pub fn execute(value: Wrapper<generated_type!()>) {}
            "#,
        );

        let (_, diagnostics) = fixture
            .project_with_diagnostics()
            .expect("capability-bearing declaration");

        assert_eq!(diagnostics.len(), 1);
        let message = diagnostics[0].to_string();
        assert!(message.contains("execute"), "{message}");
        assert!(message.contains("lib.rs"), "{message}");
        assert!(message.contains("bytes"), "{message}");
        assert!(message.contains("cannot evaluate"), "{message}");
    }

    #[test]
    fn repeatable_structural_ids_are_allocated_once_in_source_order() {
        let fixture = InventoryFixture::root(
            r#"
                contract_item!();
                contract_item!();
                extern "C" { fn first(); }
                extern "C" { fn second(); }
            "#,
        );

        let projection = fixture.project().expect("repeatable identities");
        let occurrences = projection
            .entries()
            .iter()
            .map(|entry| (entry.id().kind().to_string(), entry.id().render()))
            .collect::<Vec<_>>();

        for (kind, occurrence) in [
            ("macro", 1),
            ("macro", 2),
            ("foreign_module", 1),
            ("foreign_module", 2),
        ] {
            assert!(occurrences.iter().any(|(actual_kind, id)| {
                actual_kind == kind && id.ends_with(&format!(":occurrence:1:{occurrence}"))
            }));
        }
    }

    #[test]
    fn implementation_block_partition_and_item_order_are_nonsemantic() {
        let single = InventoryFixture::root(
            r#"
                pub struct Widget;
                impl Widget {
                    pub fn zeta(&self) {}
                    pub const ALPHA: bool = true;
                }
            "#,
        )
        .project()
        .expect("single implementation block");
        let partitioned_fixture = InventoryFixture::root(
            r#"
                pub struct Widget;
                impl Widget { pub fn zeta(&self) {} }
                impl Widget { pub const ALPHA: bool = true; }
            "#,
        );
        let partitioned = partitioned_fixture
            .project()
            .expect("partitioned implementation blocks");

        assert_eq!(single.entries(), partitioned.entries());
        let implementations = partitioned
            .entries()
            .iter()
            .filter(|entry| matches!(entry.declaration(), RustDeclaration::Implementation(_)))
            .collect::<Vec<_>>();
        assert_eq!(
            implementations.len(),
            1,
            "same-descriptor blocks must merge before identity allocation"
        );
        let locator = partitioned_fixture
            .sources
            .source_text(implementations[0].source_span().expect("source origin"))
            .expect("first implementation source");
        assert!(locator.contains("zeta"), "{locator}");
        assert!(!locator.contains("ALPHA"), "{locator}");
    }

    #[test]
    fn merged_implementation_consumes_one_signature_budget_after_partitioning() {
        let fixture = InventoryFixture::root(
            r#"
                pub struct Widget;
                impl Widget { pub fn zeta(&self) {} }
                impl Widget { pub const ALPHA: bool = true; }
            "#,
        );
        let exact = RustExtractionLimits {
            signatures: 2,
            ..RustExtractionLimits::default()
        };
        fixture
            .project_with_limits(&exact)
            .expect("one struct and one merged implementation fit the exact signature budget");

        let crossing = RustExtractionLimits {
            signatures: 1,
            ..RustExtractionLimits::default()
        };
        let error = match fixture.project_with_limits(&crossing) {
            Ok(_) => panic!("the merged implementation must consume one signature"),
            Err(error) => error,
        };
        let limit = error.limit_exceeded().expect("typed signature limit");
        assert_eq!(limit.resource, crate::limits::LimitResource::SignatureCount);
        assert_eq!(limit.limit, 1);
        assert_eq!(limit.observed_at_least, 2);
    }

    #[test]
    fn distinct_implementation_descriptors_receive_order_invariant_occurrences() {
        let forward = InventoryFixture::root(
            r#"
                pub struct Widget;
                pub trait Service {}
                impl Service for Widget {}
                impl Widget {}
            "#,
        )
        .project()
        .expect("forward implementation order");
        let reverse = InventoryFixture::root(
            r#"
                pub struct Widget;
                pub trait Service {}
                impl Widget {}
                impl Service for Widget {}
            "#,
        )
        .project()
        .expect("reverse implementation order");

        assert_eq!(forward.entries(), reverse.entries());
    }

    #[test]
    fn out_of_line_modules_keep_canonical_identity_and_physical_source_evidence() {
        let fixture = InventoryFixture::files(
            &[
                ("lib.rs", "mod transport;"),
                ("transport.rs", "pub fn send() {}"),
            ],
            &["lib.rs", "transport.rs"],
            &[("sample", "lib.rs")],
        );

        let projection = fixture.project().expect("out-of-line module projection");
        let send = projection
            .entries()
            .iter()
            .find(|entry| entry.id().name() == "send")
            .expect("send function");
        let module = projection
            .entries()
            .iter()
            .find(|entry| matches!(entry.declaration(), RustDeclaration::Module(_)))
            .expect("module declaration");

        assert_eq!(module.id().name(), "transport");
        assert!(module.id().module_id().module_path().segments().is_empty());
        assert_eq!(module.file().as_str(), "lib.rs");
        let RustDeclaration::Module(module_declaration) = module.declaration() else {
            panic!("module declaration");
        };
        assert!(!module_declaration.is_inline());
        assert_eq!(module_declaration.path_override(), None);
        assert_eq!(
            fixture
                .sources
                .source_text(module.source_span().expect("module source origin"))
                .expect("module source"),
            "mod transport;"
        );

        assert_eq!(
            send.id().module_id().module_path().segments(),
            &["transport".to_owned()]
        );
        assert_eq!(send.file().as_str(), "transport.rs");
        assert_eq!(
            fixture
                .sources
                .source_text(send.source_span().expect("source origin"))
                .expect("send source"),
            "pub fn send() {}"
        );
    }
}
