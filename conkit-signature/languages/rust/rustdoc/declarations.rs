use super::artifact::RustCompilerArtifactFailure;
use super::index::{CompilerInventory, RustdocIndex};
use super::modules::RustdocModuleExport;
use super::provenance::RustdocProvenanceResolver;
use super::types::{RustdocTypeContext, RustdocTypeLowerer};
use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::RustParsedEntry;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::associated_item::{
    RustAssociatedConstant, RustAssociatedItem, RustAssociatedType,
};
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{
    RustCallableSignature, RustMethod, RustReceiver, RustVariadicParameter,
};
use crate::languages::rust::types::declaration::{
    ConstantType, ExternCrateType, ModuleDeclarationType, ReexportType, RustDeclaration,
    TraitAliasType,
};
use crate::languages::rust::types::enum_type::{EnumType, EnumVariant, EnumVariantField};
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementationOwner, RustImplementedTrait,
};
use crate::languages::rust::types::primitive_types::{RustFunctionParameter, Visibility};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::{StructField, StructType};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use std::collections::BTreeMap;

pub(super) struct RustdocDeclarationLowerer<'index, 'inventory, 'operation, 'limits> {
    pub(super) index: &'index RustdocIndex,
    pub(super) inventory: &'inventory mut CompilerInventory<'operation, 'limits>,
}

impl<'index, 'inventory, 'operation, 'limits>
    RustdocDeclarationLowerer<'index, 'inventory, 'operation, 'limits>
{
    pub(super) fn types(&self) -> RustdocTypeLowerer<'index> {
        RustdocTypeLowerer { index: self.index }
    }

    pub(super) fn convert_top_level(
        &mut self,
        item: &'index rustdoc_types::Item,
        module_path: RustModulePath,
    ) -> Result<(), SignatureContractKitError> {
        if !self.inventory.converted_items.insert(item.id.0) {
            return Ok(());
        }
        let module_id = self.index.module_id(module_path);
        match &item.inner {
            rustdoc_types::ItemEnum::Module(module) => {
                if module.is_crate {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        item.id.0,
                        "crate-root module entered declaration lowering",
                    ));
                }
                let entry = self.convert_module_entry(item, &module_id)?;
                self.push_entry(entry)?;
            }
            rustdoc_types::ItemEnum::Function(function) => {
                let declaration = RustDeclaration::Function(self.convert_function_with_inputs(
                    item,
                    function,
                    &module_id,
                    Visibility::Public,
                    0,
                )?);
                self.push_declaration(item, module_id, declaration)?;
            }
            rustdoc_types::ItemEnum::TypeAlias(alias) => {
                let generics = self.types().convert_generics(item.id, &alias.generics)?;
                let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
                let target = self.types().convert_type(
                    item.id,
                    &alias.type_,
                    &mut RustdocTypeContext::default(),
                )?;
                self.push_declaration(
                    item,
                    module_id,
                    RustDeclaration::TypeAlias(TypeAliasType::new(base, generics, target)),
                )?;
            }
            rustdoc_types::ItemEnum::TraitAlias(alias) => {
                let generics = self.types().convert_generics(item.id, &alias.generics)?;
                let supertraits = self.types().convert_bounds(item.id, &alias.params)?;
                let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
                self.push_declaration(
                    item,
                    module_id,
                    RustDeclaration::TraitAlias(TraitAliasType::new(base, generics, supertraits)?),
                )?;
            }
            rustdoc_types::ItemEnum::Constant { type_, const_ } => {
                let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
                let constant_type = self.types().convert_type(
                    item.id,
                    type_,
                    &mut RustdocTypeContext::default(),
                )?;
                let value = RustSyntaxText::parse_expression(&const_.expr).map_err(|error| {
                    RustCompilerArtifactFailure::unsupported_item(
                        item.id.0,
                        "constant",
                        format!("rustdoc constant expression is not valid Rust syntax: {error}"),
                    )
                })?;
                self.push_declaration(
                    item,
                    module_id,
                    RustDeclaration::Constant(ConstantType::new(base, constant_type, value)?),
                )?;
            }
            rustdoc_types::ItemEnum::Static(value) => {
                if value.is_unsafe {
                    return Err(self.index.unsupported_item(
                        item,
                        "unsafe foreign static requires foreign-module ownership",
                    ));
                }
                let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
                let static_type = self.types().convert_type(
                    item.id,
                    &value.type_,
                    &mut RustdocTypeContext::default(),
                )?;
                self.push_declaration(
                    item,
                    module_id,
                    RustDeclaration::Static(StaticType::new(base, value.is_mutable, static_type)),
                )?;
            }
            rustdoc_types::ItemEnum::Use(_) => {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    item.id.0,
                    "use item bypassed module-level effective export resolution",
                ));
            }
            rustdoc_types::ItemEnum::ExternCrate { name, rename } => {
                let visible_name = rename.clone().unwrap_or_else(|| name.clone());
                let base = self.base_type_with_name(
                    item,
                    module_id.clone(),
                    Visibility::Public,
                    visible_name,
                )?;
                self.push_declaration(
                    item,
                    module_id,
                    RustDeclaration::ExternCrate(ExternCrateType::new(base, rename.clone())?),
                )?;
            }
            rustdoc_types::ItemEnum::Struct(value) => {
                let declaration = self.convert_struct(item, value, &module_id)?;
                self.push_declaration(item, module_id, declaration)?;
                self.register_implementations(&value.impls)?;
            }
            rustdoc_types::ItemEnum::Enum(value) => {
                let declaration = self.convert_enum(item, value, &module_id)?;
                self.push_declaration(item, module_id, declaration)?;
                self.register_implementations(&value.impls)?;
            }
            rustdoc_types::ItemEnum::Union(value) => {
                let declaration = self.convert_union(item, value, &module_id)?;
                self.push_declaration(item, module_id, declaration)?;
                self.register_implementations(&value.impls)?;
            }
            rustdoc_types::ItemEnum::Trait(value) => {
                let declaration = self.convert_trait(item, value, &module_id)?;
                self.push_declaration(item, module_id, declaration)?;
                self.register_implementations(&value.implementations)?;
            }
            rustdoc_types::ItemEnum::Macro(_) => {
                return Err(self.index.unsupported_item(
                    item,
                    "rustdoc strips declarative-macro matcher patterns, so its macro text cannot populate the lossless common Rust macro model",
                ));
            }
            rustdoc_types::ItemEnum::ProcMacro(_) => {
                return Err(self.index.unsupported_item(
                    item,
                    "rustdoc exposes only the procedural-macro invocation kind and helper names, not a lossless declaration signature for the common Rust macro model",
                ));
            }
            rustdoc_types::ItemEnum::ExternType => {
                return Err(self.index.unsupported_item(
                    item,
                    "rustdoc does not retain the enclosing foreign module ABI required by the common Rust foreign-item model",
                ));
            }
            rustdoc_types::ItemEnum::Primitive(_) => {
                return Err(self.index.unsupported_item(
                    item,
                    "primitive declarations belong to the compiler's core-library model and cannot be represented as a declaration owned by the selected local crate",
                ));
            }
            rustdoc_types::ItemEnum::Impl(_)
            | rustdoc_types::ItemEnum::StructField(_)
            | rustdoc_types::ItemEnum::Variant(_)
            | rustdoc_types::ItemEnum::AssocConst { .. }
            | rustdoc_types::ItemEnum::AssocType { .. } => {
                return Err(self.index.unsupported_item(
                    item,
                    "the compiler-reachable declaration is not yet represented losslessly",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn convert_module_entry(
        &mut self,
        item: &rustdoc_types::Item,
        module_id: &RustModuleId,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let (source_item, source_span) = RustdocProvenanceResolver {
            index: self.index,
            sources: &mut self.inventory.sources,
            limits: self.inventory.limits,
            cancellation: self.inventory.cancellation,
        }
        .exact_module_source(item)?;
        let rustdoc_name = self.index.item_name(item)?;
        let source_name = RustModulePath::semantic_ident(&source_item.ident);
        if source_name != rustdoc_name {
            return Err(RustCompilerArtifactFailure::invalid_item(
                item.id.0,
                format!(
                    "exact source module name {source_name:?} contradicts rustdoc name {rustdoc_name:?}"
                ),
            ));
        }
        if !matches!(source_item.vis, syn::Visibility::Public(_)) {
            return Err(RustCompilerArtifactFailure::invalid_item(
                item.id.0,
                "public rustdoc module has a non-public exact source declaration",
            ));
        }
        let attributes = self.module_attributes(item)?;
        let base = self.base_type_with_attributes(
            item,
            module_id.clone(),
            Visibility::Public,
            rustdoc_name,
            attributes,
        )?;
        let declaration = RustDeclaration::Module(
            ModuleDeclarationType::from_syn(base, &source_item, self.inventory.cancellation)
                .map_err(|error| {
                    if error.is_operation_canceled() {
                        error
                    } else {
                        self.index.unsupported_item(
                            item,
                            format!("exact source module shape is unsupported: {error}"),
                        )
                    }
                })?,
        );
        let id = self.declaration_id(item, module_id.clone(), &declaration)?;
        Ok(RustParsedEntry::from_source(id, declaration, source_span))
    }

    pub(super) fn convert_struct(
        &self,
        item: &rustdoc_types::Item,
        value: &rustdoc_types::Struct,
        module_id: &RustModuleId,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        let generics = self.types().convert_generics(item.id, &value.generics)?;
        let field_ids = match &value.kind {
            rustdoc_types::StructKind::Unit => Vec::new(),
            rustdoc_types::StructKind::Tuple(fields) => {
                if fields.iter().any(Option::is_none) {
                    return Err(self.index.unsupported_item(
                        item,
                        "tuple struct contains a stripped private or doc-hidden field",
                    ));
                }
                fields.iter().flatten().copied().collect()
            }
            rustdoc_types::StructKind::Plain {
                fields,
                has_stripped_fields,
            } => {
                if *has_stripped_fields {
                    return Err(self.index.unsupported_item(
                        item,
                        "struct contains stripped private or doc-hidden fields",
                    ));
                }
                fields.clone()
            }
        };
        let mut fields = Vec::with_capacity(field_ids.len());
        for field_id in field_ids {
            let field = self.index.item(field_id)?;
            let rustdoc_types::ItemEnum::StructField(field_type) = &field.inner else {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    field.id.0,
                    "struct field ID does not identify a struct field",
                ));
            };
            let name = field.name.clone();
            let visibility = self.visibility(&field.visibility, module_id)?;
            let field_type = self.types().convert_type(
                item.id,
                field_type,
                &mut RustdocTypeContext::default(),
            )?;
            fields.push(StructField::new(
                name,
                visibility,
                field_type,
                self.attributes(field)?,
            )?);
        }
        let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
        Ok(RustDeclaration::Structure(
            StructType::new(base)
                .with_generic_metadata(generics)
                .with_fields(fields),
        ))
    }

    pub(super) fn convert_enum(
        &self,
        item: &rustdoc_types::Item,
        value: &rustdoc_types::Enum,
        module_id: &RustModuleId,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        let generics = self.types().convert_generics(item.id, &value.generics)?;
        if value.has_stripped_variants {
            return Err(self.index.unsupported_item(
                item,
                "enum contains stripped private or doc-hidden variants",
            ));
        }

        let mut variants = Vec::with_capacity(value.variants.len());
        for variant_id in &value.variants {
            let variant_item = self.index.item(*variant_id)?;
            let rustdoc_types::ItemEnum::Variant(variant) = &variant_item.inner else {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    variant_item.id.0,
                    "enum variant ID does not identify a variant",
                ));
            };
            let fields = self.convert_variant_fields(item.id, variant_item, &variant.kind)?;
            let discriminant = variant
                .discriminant
                .as_ref()
                .map(|value| {
                    RustSyntaxText::parse_expression(&value.expr).map_err(|error| {
                        RustCompilerArtifactFailure::unsupported_item(
                            variant_item.id.0,
                            "enum variant",
                            format!(
                                "rustdoc discriminant expression is not valid Rust syntax: {error}"
                            ),
                        )
                    })
                })
                .transpose()?;
            variants.push(EnumVariant::new(
                self.index.item_name(variant_item)?,
                fields,
                discriminant,
                self.attributes(variant_item)?,
            )?);
        }

        let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
        Ok(RustDeclaration::Enumeration(
            EnumType::new(base)
                .with_generic_metadata(generics)
                .with_variants(variants),
        ))
    }

    pub(super) fn convert_variant_fields(
        &self,
        owner: rustdoc_types::Id,
        variant_item: &rustdoc_types::Item,
        kind: &rustdoc_types::VariantKind,
    ) -> Result<Vec<EnumVariantField>, SignatureContractKitError> {
        let (field_ids, named) = match kind {
            rustdoc_types::VariantKind::Plain => return Ok(Vec::new()),
            rustdoc_types::VariantKind::Tuple(fields) => {
                if fields.iter().any(Option::is_none) {
                    return Err(self.index.unsupported_item(
                        variant_item,
                        "tuple variant contains a stripped doc-hidden field",
                    ));
                }
                (fields.iter().flatten().copied().collect(), false)
            }
            rustdoc_types::VariantKind::Struct {
                fields,
                has_stripped_fields,
            } => {
                if *has_stripped_fields {
                    return Err(self.index.unsupported_item(
                        variant_item,
                        "struct variant contains stripped doc-hidden fields",
                    ));
                }
                (fields.clone(), true)
            }
        };

        let mut converted = Vec::with_capacity(field_ids.len());
        for field_id in field_ids {
            let field = self.index.item(field_id)?;
            let rustdoc_types::ItemEnum::StructField(field_type) = &field.inner else {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    field.id.0,
                    "variant field ID does not identify a struct field",
                ));
            };
            converted.push(EnumVariantField::new(
                if named {
                    Some(self.index.item_name(field)?)
                } else {
                    None
                },
                self.types()
                    .convert_type(owner, field_type, &mut RustdocTypeContext::default())?,
                self.attributes(field)?,
            )?);
        }
        Ok(converted)
    }

    pub(super) fn convert_union(
        &self,
        item: &rustdoc_types::Item,
        value: &rustdoc_types::Union,
        module_id: &RustModuleId,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        let generics = self.types().convert_generics(item.id, &value.generics)?;
        if value.has_stripped_fields {
            return Err(self
                .index
                .unsupported_item(item, "union contains stripped private or doc-hidden fields"));
        }
        let mut fields = Vec::with_capacity(value.fields.len());
        for field_id in &value.fields {
            let field = self.index.item(*field_id)?;
            let rustdoc_types::ItemEnum::StructField(field_type) = &field.inner else {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    field.id.0,
                    "union field ID does not identify a struct field",
                ));
            };
            fields.push(StructField::new(
                Some(self.index.item_name(field)?),
                self.visibility(&field.visibility, module_id)?,
                self.types().convert_type(
                    item.id,
                    field_type,
                    &mut RustdocTypeContext::default(),
                )?,
                self.attributes(field)?,
            )?);
        }
        let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
        Ok(RustDeclaration::Union(
            UnionType::new(base)
                .with_generic_metadata(generics)
                .with_fields(fields),
        ))
    }

    pub(super) fn convert_trait(
        &mut self,
        item: &rustdoc_types::Item,
        value: &rustdoc_types::Trait,
        module_id: &RustModuleId,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        let generics = self.types().convert_generics(item.id, &value.generics)?;
        let supertraits = self
            .types()
            .convert_bounds(item.id, &value.bounds)?
            .into_iter()
            .map(|bound| bound.as_str().to_owned())
            .collect();
        let mut items = Vec::with_capacity(value.items.len());
        for associated_id in &value.items {
            items.push(self.convert_associated_item(*associated_id, module_id, true)?);
        }
        let base = self.base_type(item, module_id.clone(), Visibility::Public)?;
        Ok(RustDeclaration::Trait(
            TraitType::new(base)
                .with_qualifiers(value.is_unsafe, value.is_auto)
                .with_generic_metadata(generics)
                .with_supertraits(supertraits)?
                .with_items(items),
        ))
    }

    pub(super) fn register_implementations(
        &mut self,
        implementation_ids: &[rustdoc_types::Id],
    ) -> Result<(), SignatureContractKitError> {
        for implementation_id in implementation_ids {
            self.inventory.cancellation.checkpoint()?;
            self.inventory
                .implementation_ids
                .insert(implementation_id.0);
        }
        Ok(())
    }

    pub(super) fn convert_implementations(&mut self) -> Result<(), SignatureContractKitError> {
        let mut grouped = BTreeMap::new();
        for implementation_id in std::mem::take(&mut self.inventory.implementation_ids) {
            self.inventory.cancellation.checkpoint()?;
            let Some(entry) = self.convert_implementation(rustdoc_types::Id(implementation_id))?
            else {
                continue;
            };
            let key = entry.implementation_descriptor()?;
            match grouped.entry(key) {
                std::collections::btree_map::Entry::Vacant(group) => {
                    group.insert(entry);
                }
                std::collections::btree_map::Entry::Occupied(mut group) => {
                    group
                        .get_mut()
                        .merge_same_implementation(entry, self.inventory.cancellation)?;
                }
            }
        }
        for mut entry in grouped.into_values() {
            self.inventory.cancellation.checkpoint()?;
            entry.finalize_implementation(self.inventory.cancellation)?;
            self.push_entry(entry)?;
        }
        Ok(())
    }

    pub(super) fn convert_implementation(
        &mut self,
        implementation_id: rustdoc_types::Id,
    ) -> Result<Option<RustParsedEntry>, SignatureContractKitError> {
        if !self.inventory.converted_items.insert(implementation_id.0) {
            return Ok(None);
        }
        let item = self.index.item(implementation_id)?;
        let rustdoc_types::ItemEnum::Impl(value) = &item.inner else {
            return Err(RustCompilerArtifactFailure::invalid_item(
                item.id.0,
                "implementation ID does not identify an impl",
            ));
        };
        if value.is_synthetic || item.crate_id != 0 {
            return Ok(None);
        }
        let trait_owned = value.trait_.is_some();
        let mut associated_ids = Vec::with_capacity(value.items.len());
        for associated_id in &value.items {
            self.inventory.cancellation.checkpoint()?;
            if trait_owned
                || matches!(
                    &self.index.item(*associated_id)?.visibility,
                    rustdoc_types::Visibility::Public
                )
            {
                associated_ids.push(*associated_id);
            }
        }
        if !trait_owned && associated_ids.is_empty() {
            return Ok(None);
        }
        if value.blanket_impl.is_some() {
            return Err(self.index.unsupported_item(
                item,
                "explicit blanket implementations require generic metadata conversion",
            ));
        }
        let generics = self.types().convert_generics(item.id, &value.generics)?;
        let owner_id = self.types().owner_id(item.id, &value.for_)?;
        let owner_module = owner_id.module_id().clone();
        let owner_spelling = self.types().type_source(item.id, &value.for_)?;
        let owner = RustImplementationOwner::new(owner_id.clone(), owner_spelling)?;
        let implemented_trait = match &value.trait_ {
            Some(path) => RustImplementedTrait::for_trait(
                self.types().resolved_path_source(item.id, path)?,
                if value.is_negative {
                    RustImplPolarity::Negative
                } else {
                    RustImplPolarity::Positive
                },
            )?,
            None if value.is_negative => {
                return Err(self
                    .index
                    .unsupported_item(item, "negative inherent implementations are invalid"));
            }
            None => RustImplementedTrait::Inherent,
        };
        let mut associated_items = Vec::with_capacity(associated_ids.len());
        for associated_id in associated_ids {
            self.inventory.cancellation.checkpoint()?;
            associated_items.push(self.convert_associated_item(
                associated_id,
                &owner_module,
                trait_owned,
            )?);
        }
        let implementation = ImplementationType::new(owner)
            .with_implemented_trait(implemented_trait)
            .with_qualifiers(false, value.is_unsafe)
            .with_generic_metadata(generics)
            .with_attributes(self.attributes(item)?)
            .with_items(associated_items);
        let entry = self.parsed_declaration(
            item,
            owner_module,
            RustDeclaration::Implementation(implementation),
        )?;
        Ok(Some(entry))
    }

    pub(super) fn convert_associated_item(
        &mut self,
        item_id: rustdoc_types::Id,
        module_id: &RustModuleId,
        trait_owned: bool,
    ) -> Result<RustAssociatedItem, SignatureContractKitError> {
        let item = self.index.item(item_id)?;
        let visibility = if trait_owned {
            Visibility::Public
        } else {
            self.visibility(&item.visibility, module_id)?
        };
        match &item.inner {
            rustdoc_types::ItemEnum::Function(function) => {
                let receiver = self.receiver(item.id, function)?;
                let converted = self.convert_function_with_inputs(
                    item,
                    function,
                    module_id,
                    visibility,
                    usize::from(receiver.is_some()),
                )?;
                Ok(RustAssociatedItem::Method(Box::new(RustMethod::new(
                    converted,
                    receiver,
                    function.has_body,
                    false,
                    RustAttributes::default(),
                ))))
            }
            rustdoc_types::ItemEnum::AssocConst {
                type_,
                value,
                default_unstable,
            } => {
                if default_unstable.is_some() {
                    return Err(self.index.unsupported_item(
                        item,
                        "unstable associated-constant defaults are not represented by rust_api_v1",
                    ));
                }
                let default_value = value
                    .as_deref()
                    .map(RustSyntaxText::parse_expression)
                    .transpose()
                    .map_err(|error| {
                        RustCompilerArtifactFailure::unsupported_item(
                            item.id.0,
                            "associated constant",
                            format!("rustdoc value is not valid Rust syntax: {error}"),
                        )
                    })?;
                Ok(RustAssociatedItem::Constant(RustAssociatedConstant::new(
                    self.index.item_name(item)?,
                    visibility,
                    self.types().convert_type(
                        item.id,
                        type_,
                        &mut RustdocTypeContext::default(),
                    )?,
                    default_value,
                    false,
                    self.attributes(item)?,
                )?))
            }
            rustdoc_types::ItemEnum::AssocType {
                generics,
                bounds,
                type_,
                default_unstable,
            } => {
                if default_unstable.is_some() {
                    return Err(self.index.unsupported_item(
                        item,
                        "unstable associated-type defaults are not represented by rust_api_v1",
                    ));
                }
                let generics = self.types().convert_generics(item.id, generics)?;
                let bounds = self.types().convert_bounds(item.id, bounds)?;
                let default_type = type_
                    .as_ref()
                    .map(|value| {
                        self.types().convert_type(
                            item.id,
                            value,
                            &mut RustdocTypeContext::default(),
                        )
                    })
                    .transpose()?;
                Ok(RustAssociatedItem::Type(RustAssociatedType::new(
                    self.index.item_name(item)?,
                    visibility,
                    generics,
                    bounds,
                    default_type,
                    false,
                    self.attributes(item)?,
                )?))
            }
            _ => Err(self
                .index
                .unsupported_item(item, "unsupported associated item in compiler extraction")),
        }
    }

    pub(super) fn convert_function_with_inputs(
        &self,
        item: &rustdoc_types::Item,
        function: &rustdoc_types::Function,
        module_id: &RustModuleId,
        visibility: Visibility,
        skip_inputs: usize,
    ) -> Result<FunctionType, SignatureContractKitError> {
        if function.default_unstable.is_some() {
            return Err(self.index.unsupported_item(
                item,
                "unstable provided function defaults are not represented by rust_api_v1",
            ));
        }
        let generics = self.types().convert_generics(item.id, &function.generics)?;
        let mut parameters = Vec::with_capacity(function.sig.inputs.len());
        for (_, parameter_type) in function.sig.inputs.iter().skip(skip_inputs) {
            parameters.push(RustFunctionParameter::new(
                None,
                self.types().convert_type(
                    item.id,
                    parameter_type,
                    &mut RustdocTypeContext::default(),
                )?,
            ));
        }
        let return_type = function
            .sig
            .output
            .as_ref()
            .map(|value| {
                self.types()
                    .convert_type(item.id, value, &mut RustdocTypeContext::default())
            })
            .transpose()?;
        let variadic = function
            .sig
            .is_c_variadic
            .then(|| RustVariadicParameter::new(None, RustAttributes::default()));
        let signature = RustCallableSignature::builder()
            .with_const(function.header.is_const)
            .with_async(function.header.is_async)
            .with_unsafe(function.header.is_unsafe)
            .with_abi(self.types().function_abi(&function.header.abi))
            .with_generics(generics)
            .with_variadic(variadic)
            .with_parameters(parameters)
            .with_return_type(return_type)
            .build();
        Ok(
            FunctionType::new(self.base_type(item, module_id.clone(), visibility)?)
                .with_callable_signature(signature),
        )
    }

    pub(super) fn receiver(
        &self,
        item_id: rustdoc_types::Id,
        function: &rustdoc_types::Function,
    ) -> Result<Option<RustReceiver>, SignatureContractKitError> {
        let Some((name, receiver_type)) = function.sig.inputs.first() else {
            return Ok(None);
        };
        let receiver = match syn::parse_str::<syn::FnArg>(name) {
            Ok(syn::FnArg::Receiver(receiver)) => receiver,
            Ok(syn::FnArg::Typed(_)) | Err(_) => return Ok(None),
        };
        match receiver_type {
            rustdoc_types::Type::Generic(name) if name == "Self" => {
                Ok(Some(RustReceiver::value(receiver.mutability.is_some())))
            }
            rustdoc_types::Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } if matches!(type_.as_ref(), rustdoc_types::Type::Generic(name) if name == "Self") => {
                Ok(Some(RustReceiver::reference(lifetime.clone(), *is_mutable)))
            }
            other => Ok(Some(RustReceiver::typed(
                receiver.mutability.is_some(),
                self.types()
                    .convert_type(item_id, other, &mut RustdocTypeContext::default())?,
            ))),
        }
    }

    pub(super) fn reexport_declaration(
        &mut self,
        source_item: &rustdoc_types::Item,
        module_id: &RustModuleId,
        export: RustdocModuleExport,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        let target_name = export
            .canonical_path
            .rsplit("::")
            .next()
            .unwrap_or(export.canonical_path.as_str());
        let alias = (target_name != export.visible_name).then(|| export.visible_name.clone());
        let base = self.base_type_with_name(
            source_item,
            module_id.clone(),
            Visibility::Public,
            export.visible_name,
        )?;
        Ok(RustDeclaration::Reexport(ReexportType::new(
            base,
            export.canonical_path,
            alias,
        )?))
    }

    pub(super) fn module_exports(
        &self,
        owner: rustdoc_types::Id,
        module_id: rustdoc_types::Id,
        active_modules: &mut Vec<u32>,
    ) -> Result<BTreeMap<String, RustdocModuleExport>, SignatureContractKitError> {
        if active_modules.contains(&module_id.0) {
            return Ok(BTreeMap::new());
        }
        let module_item = self.index.document.index.get(&module_id).ok_or_else(|| {
            let canonical = self
                .index
                .document
                .paths
                .get(&module_id)
                .map(|summary| summary.path.join("::"))
                .unwrap_or_else(|| format!("rustdoc item {}", module_id.0));
            RustCompilerArtifactFailure::unsupported_item(
                owner.0,
                "glob reexport",
                format!(
                    "rustdoc exposes target module {canonical} but not its item set in this artifact"
                ),
            )
        })?;
        let rustdoc_types::ItemEnum::Module(module) = &module_item.inner else {
            return Err(RustCompilerArtifactFailure::invalid_item(
                module_id.0,
                "glob reexport target does not identify a module",
            ));
        };

        active_modules.push(module_id.0);
        let result = (|| {
            let mut exports = BTreeMap::new();
            for child_id in &module.items {
                self.inventory.cancellation.checkpoint()?;
                let child = self.index.item(*child_id)?;
                if !matches!(child.visibility, rustdoc_types::Visibility::Public) {
                    continue;
                }
                match &child.inner {
                    rustdoc_types::ItemEnum::Use(value) if value.is_glob => {
                        let nested_id = value.id.ok_or_else(|| {
                            self.index.unsupported_item(
                                child,
                                "nested glob reexport has no rustdoc target ID, so the effective target set is unavailable",
                            )
                        })?;
                        for nested in self
                            .module_exports(owner, nested_id, active_modules)?
                            .into_values()
                            .map(|export| export.imported(child.id))
                        {
                            self.merge_module_export(owner, &mut exports, nested)?;
                        }
                    }
                    rustdoc_types::ItemEnum::Use(value) => {
                        let direct = self.explicit_use_export(child.id, value)?;
                        self.merge_module_export(owner, &mut exports, direct)?;
                    }
                    _ => {
                        let summary = self.index.document.paths.get(child_id).ok_or_else(|| {
                            RustCompilerArtifactFailure::unsupported_item(
                                owner.0,
                                "glob reexport",
                                format!(
                                    "public target item {} has no canonical rustdoc path summary",
                                    child_id.0
                                ),
                            )
                        })?;
                        let visible_name = self.index.item_name(child)?;
                        self.merge_module_export(
                            owner,
                            &mut exports,
                            RustdocModuleExport::declaration(visible_name, summary.path.join("::")),
                        )?;
                    }
                }
            }
            Ok(exports)
        })();
        active_modules.pop();
        result
    }

    pub(super) fn explicit_use_export(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Use,
    ) -> Result<RustdocModuleExport, SignatureContractKitError> {
        let canonical_path = match value.id {
            Some(target_id) => self
                .index
                .document
                .paths
                .get(&target_id)
                .map(|summary| summary.path.join("::"))
                .ok_or_else(|| {
                    RustCompilerArtifactFailure::unsupported_item(
                        owner.0,
                        "reexport",
                        format!(
                            "target item {} has no canonical rustdoc path summary",
                            target_id.0
                        ),
                    )
                })?,
            None => value.source.clone(),
        };
        if canonical_path.is_empty() {
            return Err(RustCompilerArtifactFailure::invalid_item(
                owner.0,
                "reexport target path is empty",
            ));
        }
        Ok(RustdocModuleExport::reexport(
            owner,
            value.name.clone(),
            canonical_path,
        ))
    }

    pub(super) fn merge_module_export(
        &self,
        owner: rustdoc_types::Id,
        exports: &mut BTreeMap<String, RustdocModuleExport>,
        incoming: RustdocModuleExport,
    ) -> Result<(), SignatureContractKitError> {
        match exports.get(&incoming.visible_name) {
            None => {
                exports.insert(incoming.visible_name.clone(), incoming);
            }
            Some(existing)
                if existing.canonical_path == incoming.canonical_path
                    && incoming.explicit
                    && !existing.explicit =>
            {
                exports.insert(incoming.visible_name.clone(), incoming);
            }
            Some(existing) if existing.canonical_path == incoming.canonical_path => {}
            Some(existing) if incoming.explicit && !existing.explicit => {
                exports.insert(incoming.visible_name.clone(), incoming);
            }
            Some(existing) if !incoming.explicit && existing.explicit => {}
            Some(existing) => {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    owner.0,
                    format!(
                        "effective public glob exports collide for {:?}: {} and {}",
                        incoming.visible_name, existing.canonical_path, incoming.canonical_path
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl<'index, 'inventory, 'operation, 'limits>
    RustdocDeclarationLowerer<'index, 'inventory, 'operation, 'limits>
{
    pub(super) fn push_declaration(
        &mut self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        declaration: RustDeclaration,
    ) -> Result<(), SignatureContractKitError> {
        let entry = self.parsed_declaration(item, module_id, declaration)?;
        self.push_entry(entry)
    }

    pub(super) fn parsed_declaration(
        &mut self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        declaration: RustDeclaration,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let index = self.index;
        let provenance = &index
            .source_map
            .get(&item.id.0)
            .ok_or_else(|| {
                RustCompilerArtifactFailure::source_map(
                    Some(item.id.0),
                    "compiler-reachable declaration has no logical source provenance",
                )
            })?
            .provenance;
        let id = self.declaration_id(item, module_id, &declaration)?;
        RustdocProvenanceResolver {
            index,
            sources: &mut self.inventory.sources,
            limits: self.inventory.limits,
            cancellation: self.inventory.cancellation,
        }
        .parsed_declaration(item.id.0, id, declaration, provenance)
    }

    pub(super) fn base_type(
        &self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        visibility: Visibility,
    ) -> Result<BaseType, SignatureContractKitError> {
        self.base_type_with_name(item, module_id, visibility, self.index.item_name(item)?)
    }

    pub(super) fn base_type_with_name(
        &self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        visibility: Visibility,
        name: String,
    ) -> Result<BaseType, SignatureContractKitError> {
        let attributes = self.attributes(item)?;
        self.base_type_with_attributes(item, module_id, visibility, name, attributes)
    }

    pub(super) fn base_type_with_attributes(
        &self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        visibility: Visibility,
        name: String,
        attributes: RustAttributes,
    ) -> Result<BaseType, SignatureContractKitError> {
        let file = self
            .index
            .source_map
            .get(&item.id.0)
            .map(|mapping| mapping.provenance.file().clone())
            .ok_or_else(|| {
                RustCompilerArtifactFailure::source_map(
                    Some(item.id.0),
                    "compiler-reachable declaration has no logical source provenance",
                )
            })?;
        Ok(BaseType::new(name, visibility, file, module_id, attributes))
    }

    pub(super) fn attributes(
        &self,
        item: &rustdoc_types::Item,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        RustAttributes::from_meta_sources(
            self.attribute_sources(item)?,
            self.inventory.cancellation,
        )
    }

    pub(super) fn module_attributes(
        &self,
        item: &rustdoc_types::Item,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        let mut retained = Vec::new();
        for source in self.attribute_sources(item)? {
            self.inventory.cancellation.checkpoint()?;
            let meta = syn::parse_str::<syn::Meta>(&source).map_err(|error| {
                self.index.unsupported_item(
                    item,
                    format!("rustdoc attribute is not valid Rust meta syntax: {error}"),
                )
            })?;
            if meta.path().is_ident("path") {
                let syn::Meta::NameValue(value) = meta else {
                    return Err(self.index.unsupported_item(
                        item,
                        "module #[path] must be a string name-value attribute",
                    ));
                };
                if !matches!(
                    value.value,
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(_),
                        ..
                    })
                ) {
                    return Err(self
                        .index
                        .unsupported_item(item, "module #[path] must contain a string literal"));
                }
                continue;
            }
            retained.push(source);
        }
        RustAttributes::from_meta_sources(retained, self.inventory.cancellation)
    }

    pub(super) fn attribute_sources(
        &self,
        item: &rustdoc_types::Item,
    ) -> Result<Vec<String>, SignatureContractKitError> {
        self.inventory.cancellation.checkpoint()?;
        if item.stability.is_some() || item.const_stability.is_some() {
            return Err(self.index.unsupported_item(
                item,
                "stability attributes are not represented by rust_api_v1",
            ));
        }
        let mut sources =
            Vec::with_capacity(item.attrs.len() + usize::from(item.deprecation.is_some()));
        for attribute in &item.attrs {
            self.inventory.cancellation.checkpoint()?;
            if let Some(source) = self.attribute_source(attribute) {
                sources.push(source);
            }
        }
        self.inventory.cancellation.checkpoint()?;
        if let Some(deprecation) = &item.deprecation {
            let mut fields = Vec::new();
            if let Some(since) = &deprecation.since {
                fields.push(format!("since = {since:?}"));
            }
            if let Some(note) = &deprecation.note {
                fields.push(format!("note = {note:?}"));
            }
            sources.push(if fields.is_empty() {
                "deprecated".to_owned()
            } else {
                format!("deprecated({})", fields.join(", "))
            });
        }
        Ok(sources)
    }

    pub(super) fn attribute_source(&self, attribute: &rustdoc_types::Attribute) -> Option<String> {
        let source = match attribute {
            rustdoc_types::Attribute::NonExhaustive => "non_exhaustive".to_owned(),
            rustdoc_types::Attribute::MustUse { reason: None } => "must_use".to_owned(),
            rustdoc_types::Attribute::MustUse {
                reason: Some(reason),
            } => format!("must_use = {reason:?}"),
            rustdoc_types::Attribute::MacroExport => "macro_export".to_owned(),
            rustdoc_types::Attribute::ExportName(name) => {
                format!("export_name = {name:?}")
            }
            rustdoc_types::Attribute::LinkSection(name) => {
                format!("link_section = {name:?}")
            }
            rustdoc_types::Attribute::AutomaticallyDerived => return None,
            rustdoc_types::Attribute::Repr(repr) => {
                let mut parts = Vec::new();
                match repr.kind {
                    rustdoc_types::ReprKind::Rust => {}
                    rustdoc_types::ReprKind::C => parts.push("C".to_owned()),
                    rustdoc_types::ReprKind::Transparent => {
                        parts.push("transparent".to_owned());
                    }
                    rustdoc_types::ReprKind::Simd => parts.push("simd".to_owned()),
                }
                if let Some(align) = repr.align {
                    parts.push(format!("align({align})"));
                }
                if let Some(packed) = repr.packed {
                    parts.push(format!("packed({packed})"));
                }
                if let Some(integer) = &repr.int {
                    parts.push(integer.clone());
                }
                if parts.is_empty() {
                    return None;
                }
                format!("repr({})", parts.join(", "))
            }
            rustdoc_types::Attribute::NoMangle => "no_mangle".to_owned(),
            rustdoc_types::Attribute::TargetFeature { enable } => format!(
                "target_feature({})",
                enable
                    .iter()
                    .map(|feature| format!("enable = {feature:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            rustdoc_types::Attribute::Other(source) => source
                .strip_prefix("#[")
                .and_then(|source| source.strip_suffix(']'))
                .unwrap_or(source)
                .to_owned(),
        };
        Some(source)
    }

    pub(super) fn visibility(
        &self,
        visibility: &rustdoc_types::Visibility,
        current_module: &RustModuleId,
    ) -> Result<Visibility, SignatureContractKitError> {
        match visibility {
            rustdoc_types::Visibility::Public => Ok(Visibility::Public),
            rustdoc_types::Visibility::Crate => Ok(Visibility::Crate),
            rustdoc_types::Visibility::Default => Ok(Visibility::Private),
            rustdoc_types::Visibility::Restricted { parent, path } => {
                let summary = self.index.document.paths.get(parent).ok_or_else(|| {
                    RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility parent has no path summary",
                    )
                })?;
                if summary.path.is_empty() {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility parent path is empty",
                    ));
                }
                if path.is_empty() {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility path is empty",
                    ));
                }
                if summary.crate_id != 0 {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility parent belongs to an external crate",
                    ));
                }
                if !matches!(summary.kind, rustdoc_types::ItemKind::Module) {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility parent does not identify a module",
                    ));
                }
                let module = self.index.module_id(RustModulePath::new(
                    summary.path.iter().skip(1).cloned().collect(),
                )?);
                let crate_root = current_module.crate_root();
                if path == "crate" && module != crate_root {
                    return Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility path `crate` does not resolve to the crate root",
                    ));
                }
                if module == *current_module {
                    if module == crate_root && path == "crate" {
                        Ok(Visibility::Crate)
                    } else {
                        Ok(Visibility::Private)
                    }
                } else if module == crate_root {
                    Ok(Visibility::Crate)
                } else if module.is_strict_ancestor_of(current_module) {
                    Ok(Visibility::Module(module))
                } else {
                    Err(RustCompilerArtifactFailure::invalid_item(
                        parent.0,
                        "restricted visibility parent is not an ancestor of the item module",
                    ))
                }
            }
        }
    }
}

impl RustdocDeclarationLowerer<'_, '_, '_, '_> {
    pub(super) fn declaration_id(
        &self,
        item: &rustdoc_types::Item,
        module_id: RustModuleId,
        declaration: &RustDeclaration,
    ) -> Result<RustItemId, SignatureContractKitError> {
        let name = match declaration {
            RustDeclaration::Implementation(value) => value.owner().id().render(),
            RustDeclaration::Reexport(value) => value.base().name().to_owned(),
            _ => self.index.item_name(item)?,
        };
        let kind = declaration.kind();
        Ok(RustItemId::new(module_id, kind, name))
    }

    pub(super) fn push_entry(
        &mut self,
        entry: RustParsedEntry,
    ) -> Result<(), SignatureContractKitError> {
        self.inventory
            .usage
            .record_items(entry.declaration().item_count(), Some(entry.file()))?;
        let entry = entry.allocate_id(&mut self.inventory.id_allocator)?;
        self.inventory.entries.push(entry);
        Ok(())
    }
}
