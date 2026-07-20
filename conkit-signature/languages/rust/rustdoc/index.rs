use super::artifact::{
    CompilerSourcePath, RustCompilerArtifactFailure, RustCompilerExtraction,
    RustCompilerExtractionContext,
};
use super::declarations::RustdocDeclarationLowerer;
use super::modules::RustdocModuleCollector;
use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::signature_id::RustItemIdAllocator;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::parser::{RustParsedEntry, RustParsedProjection};
use crate::languages::rust::source::RustSourceCatalog;
use crate::limits::{RustExtractionLimits, RustExtractionUsage};
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct RustdocIndex {
    pub(super) context: RustCompilerExtractionContext,
    pub(super) document: rustdoc_types::Crate,
    pub(super) source_map: BTreeMap<u32, CompilerSourcePath>,
}

pub(super) struct CompilerInventory<'operation, 'limits> {
    pub(super) sources: RustSourceCatalog,
    pub(super) limits: &'limits RustExtractionLimits,
    pub(super) usage: &'operation mut RustExtractionUsage<'limits>,
    pub(super) cancellation: &'operation CancellationProbe,
    pub(super) entries: Vec<RustParsedEntry>,
    pub(super) converted_items: BTreeSet<u32>,
    pub(super) implementation_ids: BTreeSet<u32>,
    pub(super) id_allocator: RustItemIdAllocator,
}

impl<'operation, 'limits> CompilerInventory<'operation, 'limits> {
    pub(super) fn new(
        sources: RustSourceCatalog,
        limits: &'limits RustExtractionLimits,
        usage: &'operation mut RustExtractionUsage<'limits>,
        cancellation: &'operation CancellationProbe,
    ) -> Self {
        Self {
            sources,
            limits,
            usage,
            cancellation,
            entries: Vec::new(),
            converted_items: BTreeSet::new(),
            implementation_ids: BTreeSet::new(),
            id_allocator: RustItemIdAllocator::default(),
        }
    }

    pub(super) fn extract(
        mut self,
        index: RustdocIndex,
    ) -> Result<RustCompilerExtraction, SignatureContractKitError> {
        let root = index.item(index.document.root)?;
        let rustdoc_types::ItemEnum::Module(root_module) = &root.inner else {
            return Err(RustCompilerArtifactFailure::invalid_item(
                root.id.0,
                "rustdoc crate root is not a module",
            ));
        };
        if !root_module.is_crate {
            return Err(RustCompilerArtifactFailure::invalid_item(
                root.id.0,
                "rustdoc root module is not marked as the crate root",
            ));
        }
        {
            let mut lowering = RustdocDeclarationLowerer {
                index: &index,
                inventory: &mut self,
            };
            RustdocModuleCollector {
                lowering: &mut lowering,
            }
            .collect(root.id, root_module)?;
            lowering.convert_implementations()?;
        }
        let projection = RustParsedProjection::new(self.entries, self.cancellation)?;
        self.usage.record_signatures(projection.entries().len())?;
        Ok(RustCompilerExtraction {
            context: index.context,
            sources: self.sources,
            projection,
        })
    }
}

impl RustdocIndex {
    pub(super) fn item(
        &self,
        id: rustdoc_types::Id,
    ) -> Result<&rustdoc_types::Item, SignatureContractKitError> {
        self.document.index.get(&id).ok_or_else(|| {
            RustCompilerArtifactFailure::invalid_item(
                id.0,
                "referenced ID is absent from the rustdoc index",
            )
        })
    }

    pub(super) fn item_name(
        &self,
        item: &rustdoc_types::Item,
    ) -> Result<String, SignatureContractKitError> {
        item.name.clone().ok_or_else(|| {
            RustCompilerArtifactFailure::invalid_item(item.id.0, "modeled declaration has no name")
        })
    }

    pub(super) fn module_id(&self, module_path: RustModulePath) -> RustModuleId {
        RustModuleId::new(self.context.canonical_crate_id.clone(), module_path)
    }

    pub(super) fn module_id_for_summary(
        &self,
        summary: &rustdoc_types::ItemSummary,
    ) -> Result<RustModuleId, SignatureContractKitError> {
        if summary.path.len() < 2 {
            return Err(RustCompilerArtifactFailure::invalid_item(
                0,
                "canonical path summary lacks crate and item segments",
            ));
        }
        let module_segments = summary.path[1..summary.path.len() - 1].to_vec();
        Ok(self.module_id(RustModulePath::new(module_segments)?))
    }

    pub(super) fn unsupported_item(
        &self,
        item: &rustdoc_types::Item,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        RustCompilerArtifactFailure::unsupported_item(
            item.id.0,
            format!("{:?}", item.inner.item_kind()),
            reason,
        )
    }

    pub(super) fn unsupported_type(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Type,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        RustCompilerArtifactFailure::unsupported_type(owner.0, format!("{value:?}"), reason)
    }
}
