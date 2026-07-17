use super::artifact::RustCompilerArtifactFailure;
use super::declarations::RustdocDeclarationLowerer;
use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::source_graph::RustModulePath;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct RustdocModuleCollector<'lowering, 'index, 'inventory, 'operation, 'limits> {
    pub(super) lowering:
        &'lowering mut RustdocDeclarationLowerer<'index, 'inventory, 'operation, 'limits>,
}

enum RustdocModuleWork<'index> {
    Open {
        module_item_id: rustdoc_types::Id,
        module: &'index rustdoc_types::Module,
        module_path: RustModulePath,
        parent: Option<u32>,
    },
    Children {
        module_item_id: rustdoc_types::Id,
        remaining: std::slice::Iter<'index, rustdoc_types::Id>,
        module_path: RustModulePath,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RustdocModuleExport {
    pub(super) visible_name: String,
    pub(super) canonical_path: String,
    pub(super) explicit: bool,
    pub(super) source_use_id: Option<u32>,
}

impl<'lowering, 'index, 'inventory, 'operation, 'limits>
    RustdocModuleCollector<'lowering, 'index, 'inventory, 'operation, 'limits>
{
    pub(super) fn collect(
        mut self,
        root_item_id: rustdoc_types::Id,
        root_module: &'index rustdoc_types::Module,
    ) -> Result<(), SignatureContractKitError> {
        let mut work = vec![RustdocModuleWork::Open {
            module_item_id: root_item_id,
            module: root_module,
            module_path: RustModulePath::default(),
            parent: None,
        }];
        let mut active_modules = BTreeSet::new();
        let mut module_parents = BTreeMap::new();
        let mut module_declarations = Vec::new();

        while let Some(next) = work.pop() {
            self.lowering.inventory.cancellation.checkpoint()?;
            match next {
                RustdocModuleWork::Open {
                    module_item_id,
                    module,
                    module_path,
                    parent,
                } => {
                    if active_modules.contains(&module_item_id.0) {
                        return Err(RustCompilerArtifactFailure::invalid_item(
                            module_item_id.0,
                            "module containment cycle detected",
                        ));
                    }
                    if let Some(previous_parent) = module_parents.get(&module_item_id.0) {
                        return Err(RustCompilerArtifactFailure::invalid_item(
                            module_item_id.0,
                            format!(
                                "module has multiple containment parents: {previous_parent:?} and {parent:?}"
                            ),
                        ));
                    }
                    module_parents.insert(module_item_id.0, parent);
                    active_modules.insert(module_item_id.0);
                    work.push(RustdocModuleWork::Children {
                        module_item_id,
                        remaining: module.items.iter(),
                        module_path,
                    });
                }
                RustdocModuleWork::Children {
                    module_item_id,
                    mut remaining,
                    module_path,
                } => {
                    if let Some(child_id) = remaining.next().copied() {
                        work.push(RustdocModuleWork::Children {
                            module_item_id,
                            remaining,
                            module_path: module_path.clone(),
                        });
                        let child = self.lowering.index.item(child_id)?;
                        if let rustdoc_types::ItemEnum::Module(child_module) = &child.inner {
                            if child_module.is_stripped
                                || !matches!(child.visibility, rustdoc_types::Visibility::Public)
                            {
                                continue;
                            }
                            let name = self.lowering.index.item_name(child)?;
                            module_declarations.push((child.id, module_path.clone()));
                            let mut segments = module_path.segments().to_vec();
                            segments.push(name);
                            work.push(RustdocModuleWork::Open {
                                module_item_id: child.id,
                                module: child_module,
                                module_path: RustModulePath::new(segments)?,
                                parent: Some(module_item_id.0),
                            });
                        } else if matches!(child.visibility, rustdoc_types::Visibility::Public)
                            && !matches!(child.inner, rustdoc_types::ItemEnum::Use(_))
                        {
                            self.lowering.convert_top_level(child, module_path)?;
                        }
                    } else {
                        self.convert_module_exports(module_item_id, module_path)?;
                        if !active_modules.remove(&module_item_id.0) {
                            return Err(RustCompilerArtifactFailure::invalid_item(
                                module_item_id.0,
                                "completed module was absent from the active containment path",
                            ));
                        }
                    }
                }
            }
        }
        for (item_id, module_path) in module_declarations {
            self.lowering.inventory.cancellation.checkpoint()?;
            let item = self.lowering.index.item(item_id)?;
            self.lowering.convert_top_level(item, module_path)?;
        }
        Ok(())
    }

    fn convert_module_exports(
        &mut self,
        module_item_id: rustdoc_types::Id,
        module_path: RustModulePath,
    ) -> Result<(), SignatureContractKitError> {
        let module_id = self.lowering.index.module_id(module_path);
        for export in self
            .lowering
            .module_exports(module_item_id, module_item_id, &mut Vec::new())?
            .into_values()
        {
            let Some(source_use_id) = export.source_use_id else {
                continue;
            };
            let source_item = self.lowering.index.item(rustdoc_types::Id(source_use_id))?;
            self.lowering
                .inventory
                .converted_items
                .insert(source_use_id);
            let declaration =
                self.lowering
                    .reexport_declaration(source_item, &module_id, export)?;
            self.lowering
                .push_declaration(source_item, module_id.clone(), declaration)?;
        }
        Ok(())
    }
}

impl RustdocModuleExport {
    pub(super) fn declaration(visible_name: String, canonical_path: String) -> Self {
        Self {
            visible_name,
            canonical_path,
            explicit: true,
            source_use_id: None,
        }
    }

    pub(super) fn reexport(
        source_use_id: rustdoc_types::Id,
        visible_name: String,
        canonical_path: String,
    ) -> Self {
        Self {
            visible_name,
            canonical_path,
            explicit: true,
            source_use_id: Some(source_use_id.0),
        }
    }

    pub(super) fn imported(mut self, source_use_id: rustdoc_types::Id) -> Self {
        self.explicit = false;
        self.source_use_id = Some(source_use_id.0);
        self
    }
}
