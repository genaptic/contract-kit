mod backend;
mod inventory_collector;
mod item_converter;
pub(in crate::languages::rust) mod signature_id;
pub(crate) mod source_graph;
mod symbol_table;
mod type_converter;
mod visibility_converter;
pub(in crate::languages::rust) mod yaml;

use crate::api::{
    ContractScope, GenerateResponse, GenerateTarget, ResolveSketchesRequest,
    ResolveSketchesResponse, RustExtractionInput,
};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{InventoryComparison, SignatureEntry, SignatureId, SignatureInventory};
use crate::languages::rust::parser::backend::{
    RustBackend, RustExtractionBackend, RustGenerationContext,
};
use crate::languages::rust::parser::inventory_collector::RustInventoryCollector;
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemIdAllocator};
use crate::languages::rust::parser::source_graph::{
    RustCapabilityDiagnostic, RustCapabilityDiagnostics, RustExtraction, RustRequiredModules,
    RustSourceGraph,
};
use crate::languages::rust::rustdoc::RustCompilerExtraction;
use crate::languages::rust::source::{RustSourceCatalog, RustSourceSpan};
use crate::languages::rust::types::base_type::RustImplementationContext;
use crate::languages::rust::types::declaration::RustDeclaration;
use crate::limits::{DiagnosticLimits, RustExtractionLimits, RustExtractionUsage, SignatureLimits};
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use symbol_table::RustSymbolTableError;

pub(crate) struct SignatureParser {
    limits: SignatureLimits,
}

impl SignatureParser {
    pub(crate) fn new(limits: SignatureLimits) -> Self {
        Self { limits }
    }

    pub(crate) fn limits(&self) -> &SignatureLimits {
        &self.limits
    }
}

pub(in crate::languages::rust) struct RustParsedFiles {
    sources: RustSourceCatalog,
}

impl RustParsedFiles {
    fn deferred(
        allowlist: &BTreeSet<CatalogPath>,
        catalog: FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            sources: RustSourceCatalog::deferred(allowlist, catalog, cancellation)?,
        })
    }

    fn parse_allowlist(
        allowlist: &BTreeSet<CatalogPath>,
        catalog: FileCatalog,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        limits.validate_source_count(allowlist.len())?;
        for path in allowlist {
            cancellation.checkpoint()?;
            let bytes = catalog.get(path).ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    path,
                    "listed source file is missing from source catalog",
                )
            })?;
            limits.validate_source_file(path, bytes.len())?;
        }

        Ok(Self {
            sources: RustSourceCatalog::parse_allowlist(allowlist, catalog, cancellation)?,
        })
    }

    fn project_for_extraction<'diagnostic_limits>(
        &self,
        extraction: &RustExtraction,
        usage: &mut RustExtractionUsage<'_>,
        diagnostics: &mut RustCapabilityDiagnostics<'diagnostic_limits>,
        diagnostic_limits: &'diagnostic_limits DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<RustParsedProjection, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let graph =
            RustSourceGraph::build(extraction, &self.sources, usage, diagnostics, cancellation)?;
        self.project_graph(graph, usage, diagnostics, diagnostic_limits, cancellation)
    }

    fn project_for_required_modules<'diagnostic_limits>(
        &mut self,
        extraction: &RustExtraction,
        required: RustRequiredModules,
        limits: &'diagnostic_limits SignatureLimits,
        usage: &mut RustExtractionUsage<'_>,
        diagnostics: &mut RustCapabilityDiagnostics<'diagnostic_limits>,
        cancellation: &CancellationProbe,
    ) -> Result<RustParsedProjection, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let graph = RustSourceGraph::build_required(
            extraction,
            &mut self.sources,
            &limits.rust,
            usage,
            required,
            diagnostics,
            cancellation,
        )?;
        self.project_graph(graph, usage, diagnostics, &limits.diagnostics, cancellation)
    }

    fn project_graph<'diagnostic_limits>(
        &self,
        graph: RustSourceGraph,
        usage: &mut RustExtractionUsage<'_>,
        diagnostics: &mut RustCapabilityDiagnostics<'diagnostic_limits>,
        diagnostic_limits: &'diagnostic_limits DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<RustParsedProjection, SignatureContractKitError> {
        RustInventoryCollector::collect(
            &self.sources,
            graph,
            usage,
            diagnostics,
            diagnostic_limits,
            cancellation,
        )
    }

    fn projected_source<'source>(
        &'source self,
        projection: &'source RustParsedProjection,
    ) -> RustProjectedSource<'source> {
        RustProjectedSource::new(&self.sources, projection)
    }
}

pub(in crate::languages::rust) struct RustProjectedSource<'source> {
    sources: &'source RustSourceCatalog,
    projection: &'source RustParsedProjection,
}

impl<'source> RustProjectedSource<'source> {
    pub(in crate::languages::rust) fn new(
        sources: &'source RustSourceCatalog,
        projection: &'source RustParsedProjection,
    ) -> Self {
        Self {
            sources,
            projection,
        }
    }

    pub(in crate::languages::rust) fn entry(
        &self,
        id: &RustItemId,
    ) -> Option<&'source RustParsedEntry> {
        self.projection.entry(id)
    }

    pub(in crate::languages::rust) fn source_text(
        &self,
        entry: &RustParsedEntry,
    ) -> Result<&'source str, SignatureContractKitError> {
        self.sources.source_text(entry.source_span()?)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::languages::rust) enum RustEntryOrigin {
    ExactSource(RustSourceSpan),
    CompilerGenerated { crate_root: CatalogPath },
    Contract(CatalogPath),
}

impl RustEntryOrigin {
    fn precedes(&self, other: &Self) -> Result<bool, SignatureContractKitError> {
        match (self, other) {
            (Self::Contract(_), _) | (_, Self::Contract(_)) => {
                Err(SignatureContractKitError::conversion_failed(
                    "contract provenance cannot participate in Rust extractor implementation merging",
                ))
            }
            (Self::ExactSource(left), Self::ExactSource(right)) => Ok((
                left.file(),
                left.byte_range().start,
                left.byte_range().end,
            ) < (
                right.file(),
                right.byte_range().start,
                right.byte_range().end,
            )),
            (Self::ExactSource(_), Self::CompilerGenerated { .. }) => Ok(true),
            (Self::CompilerGenerated { .. }, Self::ExactSource(_)) => Ok(false),
            (
                Self::CompilerGenerated { crate_root: left },
                Self::CompilerGenerated { crate_root: right },
            ) => Ok(left < right),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::languages::rust) struct RustParsedEntry {
    id: RustItemId,
    declaration: RustDeclaration,
    origin: RustEntryOrigin,
}

impl RustParsedEntry {
    pub(in crate::languages::rust) fn from_source(
        id: RustItemId,
        declaration: RustDeclaration,
        source_span: RustSourceSpan,
    ) -> Self {
        Self {
            id,
            declaration,
            origin: RustEntryOrigin::ExactSource(source_span),
        }
    }

    pub(in crate::languages::rust) fn from_compiler_generated(
        id: RustItemId,
        declaration: RustDeclaration,
        crate_root: CatalogPath,
    ) -> Self {
        Self {
            id,
            declaration,
            origin: RustEntryOrigin::CompilerGenerated { crate_root },
        }
    }

    fn from_contract(
        id: RustItemId,
        declaration: RustDeclaration,
        contract_file: CatalogPath,
    ) -> Self {
        Self {
            id,
            declaration,
            origin: RustEntryOrigin::Contract(contract_file),
        }
    }

    pub(in crate::languages::rust) fn id(&self) -> &RustItemId {
        &self.id
    }

    pub(in crate::languages::rust) fn allocate_id(
        mut self,
        allocator: &mut RustItemIdAllocator,
    ) -> Result<Self, SignatureContractKitError> {
        self.id = allocator.allocate(self.id)?;
        Ok(self)
    }

    pub(in crate::languages::rust) fn declaration(&self) -> &RustDeclaration {
        &self.declaration
    }

    pub(in crate::languages::rust) fn file(&self) -> &CatalogPath {
        match &self.origin {
            RustEntryOrigin::ExactSource(span) => span.file(),
            RustEntryOrigin::CompilerGenerated { crate_root } => crate_root,
            RustEntryOrigin::Contract(file) => file,
        }
    }

    pub(in crate::languages::rust) fn source_span(
        &self,
    ) -> Result<&RustSourceSpan, SignatureContractKitError> {
        match &self.origin {
            RustEntryOrigin::ExactSource(span) => Ok(span),
            RustEntryOrigin::CompilerGenerated { crate_root } => {
                Err(SignatureContractKitError::conversion_failed(format!(
                    "compiler-generated declaration owned by crate root {crate_root} has no exact Rust source text; linked sketch seed resolution requires an item with exact source provenance"
                )))
            }
            RustEntryOrigin::Contract(file) => Err(SignatureContractKitError::conversion_failed(
                format!("contract declaration in {file} has no Rust source span"),
            )),
        }
    }

    pub(in crate::languages::rust) fn implementation_descriptor(
        &self,
    ) -> Result<(RustItemId, Vec<u8>), SignatureContractKitError> {
        let RustDeclaration::Implementation(implementation) = &self.declaration else {
            return Err(SignatureContractKitError::conversion_failed(
                "only Rust implementation entries have an implementation descriptor",
            ));
        };
        let descriptor = implementation.descriptor_bytes().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to encode Rust implementation descriptor: {error}"
            ))
        })?;
        Ok((implementation.owner().id().clone(), descriptor))
    }

    pub(in crate::languages::rust) fn merge_same_implementation(
        &mut self,
        incoming: Self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        if self.id != incoming.id {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "cannot merge Rust implementation entries with different identities {} and {}",
                self.id.render(),
                incoming.id.render()
            )));
        }
        let RustParsedEntry {
            declaration: incoming_declaration,
            origin: incoming_origin,
            ..
        } = incoming;
        let RustDeclaration::Implementation(mut incoming) = incoming_declaration else {
            return Err(SignatureContractKitError::conversion_failed(
                "incoming Rust implementation entry contains a non-implementation declaration",
            ));
        };
        let RustDeclaration::Implementation(current) = &mut self.declaration else {
            return Err(SignatureContractKitError::conversion_failed(
                "existing Rust implementation entry contains a non-implementation declaration",
            ));
        };
        if incoming_origin.precedes(&self.origin)? {
            std::mem::swap(current, &mut incoming);
            self.origin = incoming_origin;
            current.append_same_descriptor(incoming)?;
        } else {
            current.append_same_descriptor(incoming)?;
        }
        Ok(())
    }

    pub(in crate::languages::rust) fn finalize_implementation(
        &mut self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let RustDeclaration::Implementation(implementation) = &mut self.declaration else {
            return Err(SignatureContractKitError::conversion_failed(
                "only Rust implementation entries can finalize implementation members",
            ));
        };
        implementation.sort_associated_items(cancellation)
    }

    fn into_signature_entry(
        self,
        group_id: SignatureId,
        document: Option<&str>,
    ) -> Result<SignatureEntry, SignatureContractKitError> {
        let id = SignatureId::scoped(document, self.id.render());
        let canonical_bytes = self.declaration.canonical_bytes()?;

        Ok(SignatureEntry::from_grouped_canonical_bytes(
            id,
            group_id,
            &canonical_bytes,
        ))
    }
}

struct RustDeclarationIndex {
    owners: BTreeMap<RustItemId, RustImplementationContext>,
}

impl RustDeclarationIndex {
    fn new<'entry>(
        entries: impl Iterator<Item = &'entry RustParsedEntry>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut owners = BTreeMap::new();
        for entry in entries {
            cancellation.checkpoint()?;
            let Some(base) = entry.declaration().implementation_owner_base() else {
                continue;
            };
            if owners
                .insert(
                    entry.id().clone(),
                    RustImplementationContext::new(entry.id(), base)?,
                )
                .is_some()
            {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "duplicate Rust owner declaration context for {}",
                    entry.id().render()
                )));
            }
        }
        Ok(Self { owners })
    }

    fn normalize(
        &self,
        entry: &mut RustParsedEntry,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let RustDeclaration::Implementation(implementation) = &mut entry.declaration else {
            return Ok(());
        };
        let owner_id = implementation.owner().id();
        let context = self.owners.get(owner_id).ok_or_else(|| {
            SignatureContractKitError::conversion_failed(format!(
                "Rust implementation {} references owner {} without a declaration context",
                entry.id.render(),
                owner_id.render()
            ))
        })?;
        implementation.normalize_for_owner(context, cancellation)
    }
}

impl PartialEq for RustParsedEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.declaration == other.declaration
    }
}

impl Eq for RustParsedEntry {}

pub(in crate::languages::rust) struct RustParsedProjection {
    entries: Vec<RustParsedEntry>,
}

impl RustParsedProjection {
    pub(in crate::languages::rust) fn new(
        entries: Vec<RustParsedEntry>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut ordered = BTreeMap::new();
        for entry in entries {
            cancellation.checkpoint()?;
            let id = entry.id.clone();
            if ordered.insert(id.clone(), entry).is_some() {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "duplicate projected Rust identity {}",
                    id.render()
                )));
            }
        }
        let declarations = RustDeclarationIndex::new(ordered.values(), cancellation)?;
        for entry in ordered.values_mut() {
            cancellation.checkpoint()?;
            declarations.normalize(entry, cancellation)?;
        }
        let mut entries = Vec::with_capacity(ordered.len());
        for entry in ordered.into_values() {
            cancellation.checkpoint()?;
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    pub(in crate::languages::rust) fn entries(&self) -> &[RustParsedEntry] {
        &self.entries
    }

    fn entry(&self, id: &RustItemId) -> Option<&RustParsedEntry> {
        self.entries
            .binary_search_by(|entry| entry.id().cmp(id))
            .ok()
            .map(|index| &self.entries[index])
    }

    fn into_inventory(
        self,
        document: Option<&str>,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let mut inventory = SignatureInventory::default();
        for entry in self.entries {
            cancellation.checkpoint()?;
            let semantic_group_id = match entry.declaration() {
                RustDeclaration::Implementation(implementation) => {
                    implementation.owner().id().render()
                }
                _ => entry.id().render(),
            };
            let group_id = SignatureId::scoped(document, semantic_group_id);
            inventory.insert(entry.into_signature_entry(group_id, document)?)?;
        }
        Ok(inventory)
    }
}

pub(crate) struct ParsedSignatureCheck {
    source: SignatureInventory,
    contract: SignatureInventory,
    capability_diagnostics: Vec<RustCapabilityDiagnostic>,
}

impl ParsedSignatureCheck {
    fn new(
        source: SignatureInventory,
        contract: SignatureInventory,
        capability_diagnostics: Vec<RustCapabilityDiagnostic>,
    ) -> Self {
        Self {
            source,
            contract,
            capability_diagnostics,
        }
    }

    fn from_documents<'diagnostic_limits>(
        contracts: yaml::RustContractDocuments,
        parsed: &RustParsedFiles,
        usage: &mut RustExtractionUsage<'_>,
        mut diagnostics: RustCapabilityDiagnostics<'diagnostic_limits>,
        diagnostic_limits: &'diagnostic_limits DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut source_inventories = Vec::with_capacity(contracts.documents().len());

        for document in contracts.documents() {
            cancellation.checkpoint()?;
            let Some(extraction) = document.rust_extraction(cancellation)? else {
                continue;
            };
            let projection = parsed.project_for_extraction(
                &extraction,
                usage,
                &mut diagnostics,
                diagnostic_limits,
                cancellation,
            )?;
            let inventory_scope = document.inventory_scope();
            source_inventories
                .push(projection.into_inventory(Some(&inventory_scope), cancellation)?);
        }

        let source = SignatureInventory::merge_all(source_inventories, cancellation)?;
        let contract = contracts.into_inventory(cancellation)?;
        Ok(Self::new(
            source,
            contract,
            diagnostics.into_values(cancellation)?,
        ))
    }

    fn from_compiler_documents(
        contracts: yaml::RustContractDocuments,
        extraction: RustCompilerExtraction,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let inventory_scope = {
            let document = contracts.compiler_document(cancellation)?;
            document.validate_compiler_context(extraction.context(), cancellation)?
        };
        let (_, _, projection) = extraction.into_parts();
        let source = projection.into_inventory(Some(&inventory_scope), cancellation)?;
        let contract = contracts.into_inventory(cancellation)?;
        Ok(Self::new(source, contract, Vec::new()))
    }

    pub(crate) fn compare(
        &self,
        limits: &DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<InventoryComparison, SignatureContractKitError> {
        self.source
            .compare_against(&self.contract, limits, cancellation)
    }

    pub(crate) fn capability_warning_messages(&self) -> impl Iterator<Item = String> + '_ {
        self.capability_diagnostics
            .iter()
            .map(RustCapabilityDiagnostic::warning_message)
    }
}

impl SignatureParser {
    pub(crate) fn parse_check_inventories(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        extraction: RustExtractionInput,
        cancellation: &CancellationProbe,
    ) -> Result<ParsedSignatureCheck, SignatureContractKitError> {
        let mut usage = self.limits.rust.usage();
        let mut yaml_usage = self.limits.yaml.usage();
        let contracts = yaml::RustContractDocuments::parse(
            contract_files,
            &mut yaml_usage,
            &mut usage,
            cancellation,
        )?;
        RustBackend::from_input(extraction).check(
            self,
            source_files,
            contracts,
            &mut usage,
            cancellation,
        )
    }

    pub(crate) fn parse_contract_diff_inventories(
        &self,
        current_contract_files: FileCatalog,
        previous_contract_files: FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<(SignatureInventory, SignatureInventory), SignatureContractKitError> {
        let mut usage = self.limits.rust.usage();
        let mut yaml_usage = self.limits.yaml.usage();
        let current = yaml::RustContractDocuments::parse(
            current_contract_files,
            &mut yaml_usage,
            &mut usage,
            cancellation,
        )?
        .into_inventory(cancellation)?;
        cancellation.checkpoint()?;
        let previous = yaml::RustContractDocuments::parse(
            previous_contract_files,
            &mut yaml_usage,
            &mut usage,
            cancellation,
        )?
        .into_inventory(cancellation)?;
        Ok((current, previous))
    }

    pub(crate) fn generate_contract_files(
        &self,
        source_files: FileCatalog,
        target: GenerateTarget,
        extraction: RustExtractionInput,
        scope: ContractScope,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let mut usage = self.limits.rust.usage();
        let mut yaml_usage = self.limits.yaml.usage();
        let plan =
            yaml::RustGenerationPlan::parse(target, &mut yaml_usage, &mut usage, cancellation)?;
        let allowlist = plan.source_allowlist(cancellation)?;
        RustBackend::from_input(extraction).generate(RustGenerationContext {
            parser: self,
            source_files,
            allowlist,
            plan,
            scope,
            usage: &mut usage,
            yaml_usage: &mut yaml_usage,
            cancellation,
        })
    }

    pub(crate) fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
        cancellation: &CancellationProbe,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        let ResolveSketchesRequest {
            source_files,
            contract_files,
            extraction,
        } = request;
        let mut usage = self.limits.rust.usage();
        let mut yaml_usage = self.limits.yaml.usage();
        let documents = yaml::RustContractDocuments::parse(
            contract_files,
            &mut yaml_usage,
            &mut usage,
            cancellation,
        )?;
        RustBackend::from_input(extraction).resolve_sketches(
            self,
            source_files,
            documents,
            &mut usage,
            cancellation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::impl_type::{ImplementationType, RustImplementationOwner};

    #[test]
    fn parsed_check_deduplicates_and_orders_capability_evidence() {
        let cancellation = crate::work::CancellationProbe::new();
        let limits = DiagnosticLimits::default();
        let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
        let earlier = RustCapabilityDiagnostic::ConditionalModule {
            module: RustModuleId::new(
                RustCrateId::new("alpha", &cancellation).expect("valid crate id"),
                RustModulePath::default(),
            ),
        };
        let later = RustCapabilityDiagnostic::ConditionalModule {
            module: RustModuleId::new(
                RustCrateId::new("beta", &cancellation).expect("valid crate id"),
                RustModulePath::default(),
            ),
        };
        diagnostics.insert(later.clone()).expect("later diagnostic");
        diagnostics.insert(earlier).expect("earlier diagnostic");
        diagnostics.insert(later).expect("duplicate diagnostic");
        let parsed = ParsedSignatureCheck::new(
            SignatureInventory::default(),
            SignatureInventory::default(),
            diagnostics
                .into_values(&cancellation)
                .expect("canonical capability evidence"),
        );

        assert_eq!(
            parsed
                .capability_warning_messages()
                .collect::<Vec<_>>(),
            vec![
                "rust_syntax_v2 capability warning: cfg/cfg_attr on module alpha is retained as syntax but not evaluated".to_owned(),
                "rust_syntax_v2 capability warning: cfg/cfg_attr on module beta is retained as syntax but not evaluated".to_owned(),
            ]
        );
    }

    #[test]
    fn parsed_entry_merging_keeps_the_earliest_deterministic_implementation_owner() {
        let module_id = RustModuleId::new(
            RustCrateId::new("sample", &CancellationProbe::new()).expect("crate ID"),
            RustModulePath::default(),
        );
        let owner_id = RustItemId::new(
            module_id.clone(),
            crate::languages::rust::types::declaration::RustItemKind::Struct,
            "Widget".to_owned(),
        );
        let entry_id = RustItemId::new(
            module_id,
            crate::languages::rust::types::declaration::RustItemKind::Implementation,
            owner_id.render(),
        );
        let later = ImplementationType::new(
            RustImplementationOwner::new(owner_id.clone(), "crate::Widget".to_owned())
                .expect("qualified owner spelling"),
        );
        let earlier = ImplementationType::new(
            RustImplementationOwner::new(owner_id, "Widget".to_owned())
                .expect("bare owner spelling"),
        );
        let earlier_root = CatalogPath::new("a.rs").expect("earlier root");
        let mut merged = RustParsedEntry::from_compiler_generated(
            entry_id.clone(),
            RustDeclaration::Implementation(later),
            CatalogPath::new("z.rs").expect("later root"),
        );

        merged
            .merge_same_implementation(
                RustParsedEntry::from_compiler_generated(
                    entry_id,
                    RustDeclaration::Implementation(earlier.clone()),
                    earlier_root.clone(),
                ),
                &CancellationProbe::new(),
            )
            .expect("same-descriptor implementation entries merge");

        assert_eq!(merged.file(), &earlier_root);
        assert_eq!(
            merged.declaration(),
            &RustDeclaration::Implementation(earlier)
        );
    }

    #[test]
    fn parsed_entry_merging_rejects_contract_provenance() {
        let module_id = RustModuleId::new(
            RustCrateId::new("sample", &CancellationProbe::new()).expect("crate ID"),
            RustModulePath::default(),
        );
        let owner_id = RustItemId::new(
            module_id.clone(),
            crate::languages::rust::types::declaration::RustItemKind::Struct,
            "Widget".to_owned(),
        );
        let entry_id = RustItemId::new(
            module_id,
            crate::languages::rust::types::declaration::RustItemKind::Implementation,
            owner_id.render(),
        );
        let implementation = ImplementationType::new(
            RustImplementationOwner::new(owner_id, "Widget".to_owned()).expect("owner spelling"),
        );
        let contract_file = CatalogPath::new("contract.yaml").expect("contract file");
        let crate_root = CatalogPath::new("lib.rs").expect("crate root");

        let mut extracted = RustParsedEntry::from_compiler_generated(
            entry_id.clone(),
            RustDeclaration::Implementation(implementation.clone()),
            crate_root.clone(),
        );
        let incoming_contract = RustParsedEntry::from_contract(
            entry_id.clone(),
            RustDeclaration::Implementation(implementation.clone()),
            contract_file.clone(),
        );
        let incoming_error = extracted
            .merge_same_implementation(incoming_contract, &CancellationProbe::new())
            .expect_err("contract provenance cannot enter extractor implementation merging");

        assert!(incoming_error.to_string().contains(
            "contract provenance cannot participate in Rust extractor implementation merging"
        ));

        let mut contract = RustParsedEntry::from_contract(
            entry_id.clone(),
            RustDeclaration::Implementation(implementation.clone()),
            contract_file,
        );
        let incoming_extracted = RustParsedEntry::from_compiler_generated(
            entry_id,
            RustDeclaration::Implementation(implementation),
            crate_root,
        );
        let existing_error = contract
            .merge_same_implementation(incoming_extracted, &CancellationProbe::new())
            .expect_err("contract provenance cannot own extractor implementation merging");

        assert!(existing_error.to_string().contains(
            "contract provenance cannot participate in Rust extractor implementation merging"
        ));
    }
}
