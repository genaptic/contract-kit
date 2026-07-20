use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::inventory_collector::RustItemContext;
use crate::languages::rust::parser::source_graph::{
    RustAssociatedMacroContainer, RustCapabilityDiagnostics, RustModuleId, RustModulePath,
};
use crate::languages::rust::parser::type_converter::RustTypeConverter;
use crate::languages::rust::parser::visibility_converter::RustVisibilityConverter;
use crate::languages::rust::source::RustSourceSpan;
use crate::languages::rust::types::associated_item::{
    RustAssociatedConstant, RustAssociatedItem, RustAssociatedType,
};
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{
    RustCallableSignature, RustFunctionAbi, RustMethod, RustReceiver, RustVariadicParameter,
};
use crate::languages::rust::types::declaration::{
    ConstantType, ExternCrateType, ForeignModuleType, ModuleDeclarationType, ReexportType,
    RustDeclaration, RustForeignFunction, RustForeignItem, RustForeignMacro, RustForeignStatic,
    RustForeignType, TraitAliasType,
};
use crate::languages::rust::types::enum_type::{EnumType, EnumVariant, EnumVariantField};
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementationOwner, RustImplementedTrait,
};
use crate::languages::rust::types::macro_type::MacroType;
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustGenericParameter, RustType, Visibility,
};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::{StructField, StructType};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use crate::work::CancellationProbe;
use quote::ToTokens;
use syn::spanned::Spanned as _;

#[derive(Debug)]
pub(super) enum RustItemConversion {
    Declaration(RustConvertedDeclaration),
    PublicReexport {
        declaration: RustConvertedDeclaration,
        binding: RustImportBinding,
    },
    PrivateImport(RustImportBinding),
}

#[derive(Clone, Debug)]
pub(super) struct RustImportBinding {
    declared_in: RustModuleId,
    span: RustSourceSpan,
    leading_colon: bool,
    target_segments: Vec<String>,
    alias: Option<String>,
    local_name: String,
    attributes: RustAttributes,
}

impl RustImportBinding {
    pub(super) fn new(
        declared_in: RustModuleId,
        span: RustSourceSpan,
        leading_colon: bool,
        target_segments: Vec<String>,
        alias: Option<String>,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        if target_segments.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust import target cannot be empty",
            ));
        }
        let mut path = if leading_colon {
            "::".to_owned()
        } else {
            String::new()
        };
        path.push_str(&target_segments.join("::"));
        let local_name = ReexportType::visible_name(&path, alias.as_deref())?.to_owned();
        Ok(Self {
            declared_in,
            span,
            leading_colon,
            target_segments,
            alias,
            local_name,
            attributes,
        })
    }

    pub(super) fn declared_in(&self) -> &RustModuleId {
        &self.declared_in
    }

    pub(super) fn span(&self) -> &RustSourceSpan {
        &self.span
    }

    pub(super) fn leading_colon(&self) -> bool {
        self.leading_colon
    }

    pub(super) fn target_segments(&self) -> &[String] {
        &self.target_segments
    }

    pub(super) fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    pub(super) fn render_path(&self) -> String {
        let mut path = if self.leading_colon {
            "::".to_owned()
        } else {
            String::new()
        };
        path.push_str(&self.target_segments.join("::"));
        path
    }

    pub(super) fn local_name(&self) -> &str {
        &self.local_name
    }

    pub(super) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
    }
}

#[derive(Debug)]
pub(super) struct RustConvertedDeclaration {
    semantic_name: String,
    declaration: RustDeclaration,
}

impl RustConvertedDeclaration {
    fn new(
        semantic_name: String,
        declaration: RustDeclaration,
    ) -> Result<Self, SignatureContractKitError> {
        if semantic_name.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust declaration semantic name cannot be empty",
            ));
        }

        Ok(Self {
            semantic_name,
            declaration,
        })
    }

    pub(super) fn into_parts(self) -> (String, RustDeclaration) {
        (self.semantic_name, self.declaration)
    }
}

pub(super) struct RustItemConverter {
    visibility_converter: RustVisibilityConverter,
    cancellation: CancellationProbe,
}

impl RustItemConverter {
    pub(super) fn new(cancellation: &CancellationProbe) -> Self {
        Self {
            visibility_converter: RustVisibilityConverter,
            cancellation: cancellation.clone(),
        }
    }

    pub(super) fn convert_non_implementation_item(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::Item,
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        match item {
            syn::Item::Const(item) => self.convert_constant(context, item),
            syn::Item::Enum(item) => self.convert_enum(context, item),
            syn::Item::ExternCrate(item) => self.convert_extern_crate(context, item),
            syn::Item::Fn(item) => self.convert_function(context, item),
            syn::Item::ForeignMod(item) => self.convert_foreign_module(context, item),
            syn::Item::Impl(item) => context.unsupported_syntax(
                "implementation reached pass-one conversion before owner resolution",
                item.span(),
            ),
            syn::Item::Macro(item) => self.convert_macro(context, item),
            syn::Item::Mod(item) => self.convert_module(context, item),
            syn::Item::Static(item) => self.convert_static(context, item),
            syn::Item::Struct(item) => self.convert_struct(context, item),
            syn::Item::Trait(item) => self.convert_trait(context, item, diagnostics),
            syn::Item::TraitAlias(item) => self.convert_trait_alias(context, item),
            syn::Item::Type(item) => self.convert_type_alias(context, item),
            syn::Item::Union(item) => self.convert_union(context, item),
            syn::Item::Use(item) => self.convert_use(context, item),
            syn::Item::Verbatim(tokens) => {
                context.unsupported_syntax("verbatim item", tokens.span())
            }
            unsupported => context.unsupported_syntax("future top-level item", unsupported.span()),
        }
    }

    fn convert_constant(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemConst,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        self.require_empty_generics(
            context,
            &item.generics,
            "generic constant item",
            item.span(),
        )?;
        let name = RustModulePath::semantic_ident(&item.ident);
        let declaration = RustDeclaration::Constant(ConstantType::new(
            self.base_type(context, &name, &item.vis, &item.attrs)?,
            self.convert_type(
                context,
                &RustTypeConverter::new(&self.cancellation),
                (*item.ty).clone(),
            )?,
            RustSyntaxText::from_expression(&item.expr),
        )?);
        self.declaration(name, declaration)
    }

    fn convert_enum(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemEnum,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let type_converter = self.type_converter(context, &[], &item.generics)?;
        let mut variants = Vec::with_capacity(item.variants.len());
        for variant in &item.variants {
            self.cancellation.checkpoint()?;
            variants.push(self.convert_enum_variant(context, variant, &type_converter)?);
        }
        let declaration = RustDeclaration::Enumeration(
            EnumType::new(self.base_type(context, &name, &item.vis, &item.attrs)?)
                .with_generic_metadata(self.convert_generics(context, &item.generics)?)
                .with_variants(variants),
        );
        self.declaration(name, declaration)
    }

    fn convert_extern_crate(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemExternCrate,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let declaration = RustDeclaration::ExternCrate(ExternCrateType::new(
            self.base_type(context, &name, &item.vis, &item.attrs)?,
            item.rename
                .as_ref()
                .map(|(_, alias)| RustModulePath::semantic_ident(alias)),
        )?);
        self.declaration(name, declaration)
    }

    fn convert_function(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemFn,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.sig.ident);
        let callable = self.convert_signature(context, &item.sig, &[])?;
        if callable.receiver.is_some() {
            return context.unsupported_syntax("receiver on free function", item.span());
        }
        let declaration = RustDeclaration::Function(
            FunctionType::new(self.base_type(context, &name, &item.vis, &item.attrs)?)
                .with_callable_signature(callable.signature),
        );
        self.declaration(name, declaration)
    }

    fn convert_foreign_module(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemForeignMod,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let abi = self.convert_abi(Some(&item.abi));
        let mut items = Vec::with_capacity(item.items.len());
        for item in &item.items {
            self.cancellation.checkpoint()?;
            items.push(self.convert_foreign_item(context, item)?);
        }
        let declaration = RustDeclaration::ForeignModule(ForeignModuleType::new(
            context.file().clone(),
            context.module_id().clone(),
            abi.clone(),
            item.unsafety.is_some(),
            self.convert_attributes(context, &item.attrs)?,
            items,
        )?);
        self.declaration(self.foreign_module_semantic_name(&abi), declaration)
    }

    pub(super) fn convert_implementation(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemImpl,
        owner: RustImplementationOwner,
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
    ) -> Result<RustConvertedDeclaration, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        let implemented_trait =
            self.convert_implemented_trait(&item.modifiers, item.trait_.as_ref())?;
        let semantic_name = owner.id().render();
        let owner_generics = self.generic_type_names(context, &item.generics)?;
        let associated =
            self.convert_impl_items(context, &owner_generics, &item.items, diagnostics)?;
        let declaration = RustDeclaration::Implementation(
            ImplementationType::new(owner)
                .with_implemented_trait(implemented_trait)
                .with_qualifiers(
                    item.modifiers.defaultness.is_some(),
                    item.unsafety.is_some(),
                )
                .with_generic_metadata(self.convert_generics(context, &item.generics)?)
                .with_attributes(self.convert_attributes(context, &item.attrs)?)
                .with_items(associated),
        );
        RustConvertedDeclaration::new(semantic_name, declaration)
    }

    fn convert_macro(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemMacro,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = item
            .ident
            .as_ref()
            .map(RustModulePath::semantic_ident)
            .unwrap_or_else(|| self.tokens(&item.mac.path));
        let declaration = RustDeclaration::Macro(MacroType::new(
            self.base_type(context, &name, &syn::Visibility::Inherited, &item.attrs)?,
            self.tokens(&item.mac),
        )?);
        self.declaration(name, declaration)
    }

    fn convert_module(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemMod,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let attributes = self.module_attributes(context, item)?;
        let base = BaseType::new(
            name.clone(),
            self.visibility_converter
                .convert_visibility(item.vis.clone(), context.module_id())?,
            context.file().clone(),
            context.module_id().clone(),
            attributes,
        );
        let source_span = context.source_span(item.span())?;
        let declaration = RustDeclaration::Module(
            ModuleDeclarationType::from_syn(base, item, &self.cancellation).map_err(|error| {
                if error.is_operation_canceled() {
                    error
                } else {
                    SignatureContractKitError::invalid_rust_source(
                        context.module_id().clone(),
                        source_span,
                        error.to_string(),
                    )
                }
            })?,
        );
        self.declaration(name, declaration)
    }

    fn module_attributes(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemMod,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        let mut semantic_attributes = Vec::with_capacity(item.attrs.len());

        for attribute in &item.attrs {
            self.cancellation.checkpoint()?;
            if !attribute.path().is_ident("path") {
                semantic_attributes.push(attribute.clone());
            }
        }

        self.convert_attributes(context, &semantic_attributes)
    }

    fn convert_static(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemStatic,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let mutable = self.convert_static_mutability(context, &item.mutability, item.span())?;
        let declaration = RustDeclaration::Static(StaticType::new(
            self.base_type(context, &name, &item.vis, &item.attrs)?,
            mutable,
            self.convert_type(
                context,
                &RustTypeConverter::new(&self.cancellation),
                (*item.ty).clone(),
            )?,
        ));
        self.declaration(name, declaration)
    }

    fn convert_struct(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemStruct,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let type_converter = self.type_converter(context, &[], &item.generics)?;
        let declaration = RustDeclaration::Structure(
            StructType::new(self.base_type(context, &name, &item.vis, &item.attrs)?)
                .with_generic_metadata(self.convert_generics(context, &item.generics)?)
                .with_fields(self.convert_struct_fields(context, &item.fields, &type_converter)?),
        );
        self.declaration(name, declaration)
    }

    fn convert_trait(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemTrait,
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let owner_generics = self.generic_type_names(context, &item.generics)?;
        let associated =
            self.convert_trait_items(context, &owner_generics, &item.items, diagnostics)?;
        let trait_type = TraitType::new(self.base_type(context, &name, &item.vis, &item.attrs)?)
            .with_qualifiers(item.unsafety.is_some(), item.modifiers.auto_token.is_some())
            .with_generic_metadata(self.convert_generics(context, &item.generics)?)
            .with_supertraits(
                item.supertraits
                    .iter()
                    .map(|bound| self.tokens(bound))
                    .collect(),
            )?
            .with_items(associated);
        let declaration = RustDeclaration::Trait(trait_type);
        self.declaration(name, declaration)
    }

    fn convert_trait_alias(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemTraitAlias,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let declaration = RustDeclaration::TraitAlias(TraitAliasType::new(
            self.base_type(context, &name, &item.vis, &item.attrs)?,
            self.convert_generics(context, &item.generics)?,
            item.bounds
                .iter()
                .map(RustSyntaxText::from_type_bound)
                .collect(),
        )?);
        self.declaration(name, declaration)
    }

    fn convert_type_alias(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemType,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let type_converter = self.type_converter(context, &[], &item.generics)?;
        let declaration = RustDeclaration::TypeAlias(TypeAliasType::new(
            self.base_type(context, &name, &item.vis, &item.attrs)?,
            self.convert_generics(context, &item.generics)?,
            self.convert_type(context, &type_converter, (*item.ty).clone())?,
        ));
        self.declaration(name, declaration)
    }

    fn convert_union(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemUnion,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let name = RustModulePath::semantic_ident(&item.ident);
        let type_converter = self.type_converter(context, &[], &item.generics)?;
        let fields = syn::Fields::Named(item.fields.clone());
        let declaration = RustDeclaration::Union(
            UnionType::new(self.base_type(context, &name, &item.vis, &item.attrs)?)
                .with_generic_metadata(self.convert_generics(context, &item.generics)?)
                .with_fields(self.convert_struct_fields(context, &fields, &type_converter)?),
        );
        self.declaration(name, declaration)
    }

    fn convert_use(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ItemUse,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        let mut path = RustUsePath::new(item.leading_colon.is_some());
        path.collect(context, &item.tree, item.span())?;
        let visibility = self
            .visibility_converter
            .convert_visibility(item.vis.clone(), context.module_id())?;
        let attributes = self.convert_attributes(context, &item.attrs)?;
        let binding = path.into_binding(context, attributes.clone(), item.span())?;
        if matches!(visibility, Visibility::Private) {
            return Ok(RustItemConversion::PrivateImport(binding));
        }
        let path = binding.render_path();
        let alias = binding.alias().map(ToOwned::to_owned);
        let name = ReexportType::visible_name(&path, alias.as_deref())?.to_owned();
        let declaration = RustDeclaration::Reexport(ReexportType::new(
            BaseType::new(
                name.clone(),
                visibility,
                context.file().clone(),
                context.module_id().clone(),
                attributes,
            ),
            path,
            alias,
        )?);
        let declaration = RustConvertedDeclaration::new(name, declaration)?;
        Ok(RustItemConversion::PublicReexport {
            declaration,
            binding,
        })
    }

    fn declaration(
        &self,
        semantic_name: String,
        declaration: RustDeclaration,
    ) -> Result<RustItemConversion, SignatureContractKitError> {
        RustConvertedDeclaration::new(semantic_name, declaration)
            .map(RustItemConversion::Declaration)
    }

    fn convert_attributes(
        &self,
        context: &RustItemContext<'_>,
        attributes: &[syn::Attribute],
    ) -> Result<RustAttributes, SignatureContractKitError> {
        let mut converted = RustAttributes::default();
        for attribute in attributes {
            self.cancellation.checkpoint()?;
            let source_span = context.source_span(attribute.span())?;
            converted
                .append_from_syn(attribute, &self.cancellation)
                .map_err(|error| {
                    SignatureContractKitError::invalid_rust_source(
                        context.module_id().clone(),
                        source_span,
                        error.to_string(),
                    )
                })?;
        }
        Ok(converted)
    }

    fn convert_type(
        &self,
        context: &RustItemContext<'_>,
        converter: &RustTypeConverter,
        rust_type: syn::Type,
    ) -> Result<RustType, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        let source_span = context.source_span(rust_type.span())?;
        let converted = converter.convert_type(rust_type).map_err(|error| {
            SignatureContractKitError::invalid_rust_source(
                context.module_id().clone(),
                source_span,
                error.to_string(),
            )
        })?;
        self.cancellation.checkpoint()?;
        Ok(converted)
    }

    fn base_type(
        &self,
        context: &RustItemContext<'_>,
        name: &str,
        visibility: &syn::Visibility,
        attributes: &[syn::Attribute],
    ) -> Result<BaseType, SignatureContractKitError> {
        Ok(BaseType::new(
            name.to_owned(),
            self.visibility_converter
                .convert_visibility(visibility.clone(), context.module_id())?,
            context.file().clone(),
            context.module_id().clone(),
            self.convert_attributes(context, attributes)?,
        ))
    }

    fn convert_struct_fields(
        &self,
        context: &RustItemContext<'_>,
        fields: &syn::Fields,
        type_converter: &RustTypeConverter,
    ) -> Result<Vec<StructField>, SignatureContractKitError> {
        match fields {
            syn::Fields::Named(fields) => {
                let mut converted = Vec::with_capacity(fields.named.len());
                for field in &fields.named {
                    self.cancellation.checkpoint()?;
                    converted.push(self.convert_struct_field(context, field, type_converter)?);
                }
                Ok(converted)
            }
            syn::Fields::Unnamed(fields) => {
                let mut converted = Vec::with_capacity(fields.unnamed.len());
                for field in &fields.unnamed {
                    self.cancellation.checkpoint()?;
                    converted.push(self.convert_struct_field(context, field, type_converter)?);
                }
                Ok(converted)
            }
            syn::Fields::Unit => Ok(Vec::new()),
        }
    }

    fn convert_struct_field(
        &self,
        context: &RustItemContext<'_>,
        field: &syn::Field,
        type_converter: &RustTypeConverter,
    ) -> Result<StructField, SignatureContractKitError> {
        StructField::new(
            field.ident.as_ref().map(RustModulePath::semantic_ident),
            self.visibility_converter
                .convert_visibility(field.vis.clone(), context.module_id())?,
            self.convert_type(context, type_converter, field.ty.clone())?,
            self.convert_attributes(context, &field.attrs)?,
        )
    }

    fn convert_enum_variant(
        &self,
        context: &RustItemContext<'_>,
        variant: &syn::Variant,
        type_converter: &RustTypeConverter,
    ) -> Result<EnumVariant, SignatureContractKitError> {
        let fields = match &variant.fields {
            syn::Fields::Named(fields) => {
                let mut converted = Vec::with_capacity(fields.named.len());
                for field in &fields.named {
                    self.cancellation.checkpoint()?;
                    converted.push(self.convert_enum_field(context, field, type_converter)?);
                }
                converted
            }
            syn::Fields::Unnamed(fields) => {
                let mut converted = Vec::with_capacity(fields.unnamed.len());
                for field in &fields.unnamed {
                    self.cancellation.checkpoint()?;
                    converted.push(self.convert_enum_field(context, field, type_converter)?);
                }
                converted
            }
            syn::Fields::Unit => Vec::new(),
        };
        EnumVariant::new(
            RustModulePath::semantic_ident(&variant.ident),
            fields,
            variant
                .discriminant
                .as_ref()
                .map(|(_, expression)| RustSyntaxText::from_expression(expression)),
            self.convert_attributes(context, &variant.attrs)?,
        )
    }

    fn convert_enum_field(
        &self,
        context: &RustItemContext<'_>,
        field: &syn::Field,
        type_converter: &RustTypeConverter,
    ) -> Result<EnumVariantField, SignatureContractKitError> {
        EnumVariantField::new(
            field.ident.as_ref().map(RustModulePath::semantic_ident),
            self.convert_type(context, type_converter, field.ty.clone())?,
            self.convert_attributes(context, &field.attrs)?,
        )
    }

    fn convert_trait_items(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        items: &[syn::TraitItem],
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
    ) -> Result<Vec<RustAssociatedItem>, SignatureContractKitError> {
        let mut converted = Vec::new();
        for item in items {
            self.cancellation.checkpoint()?;
            match item {
                syn::TraitItem::Const(item) => converted.push(RustAssociatedItem::Constant(
                    self.convert_trait_constant(context, owner_generics, item)?,
                )),
                syn::TraitItem::Fn(item) => converted.push(RustAssociatedItem::Method(Box::new(
                    self.convert_trait_method(context, owner_generics, item)?,
                ))),
                syn::TraitItem::Type(item) => converted.push(RustAssociatedItem::Type(
                    self.convert_trait_type(context, owner_generics, item)?,
                )),
                syn::TraitItem::Macro(item) => {
                    self.convert_attributes(context, &item.attrs)?;
                    diagnostics.insert(context.associated_macro_diagnostic(
                        RustAssociatedMacroContainer::Trait,
                        item.span(),
                    )?)?;
                }
                syn::TraitItem::Verbatim(tokens) => {
                    return context.unsupported_syntax("verbatim trait item", tokens.span());
                }
                unsupported => {
                    return context.unsupported_syntax("future trait item", unsupported.span());
                }
            }
        }
        Ok(converted)
    }

    fn convert_impl_items(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        items: &[syn::ImplItem],
        diagnostics: &mut RustCapabilityDiagnostics<'_>,
    ) -> Result<Vec<RustAssociatedItem>, SignatureContractKitError> {
        let mut converted = Vec::new();
        for item in items {
            self.cancellation.checkpoint()?;
            match item {
                syn::ImplItem::Const(item) => converted.push(RustAssociatedItem::Constant(
                    self.convert_impl_constant(context, owner_generics, item)?,
                )),
                syn::ImplItem::Fn(item) => converted.push(RustAssociatedItem::Method(Box::new(
                    self.convert_impl_method(context, owner_generics, item)?,
                ))),
                syn::ImplItem::Type(item) => converted.push(RustAssociatedItem::Type(
                    self.convert_impl_type(context, owner_generics, item)?,
                )),
                syn::ImplItem::Macro(item) => {
                    self.convert_attributes(context, &item.attrs)?;
                    diagnostics.insert(context.associated_macro_diagnostic(
                        RustAssociatedMacroContainer::Implementation,
                        item.span(),
                    )?)?;
                }
                syn::ImplItem::Verbatim(tokens) => {
                    return context.unsupported_syntax("verbatim impl item", tokens.span());
                }
                unsupported => {
                    return context.unsupported_syntax("future impl item", unsupported.span());
                }
            }
        }
        Ok(converted)
    }

    fn convert_trait_constant(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::TraitItemConst,
    ) -> Result<RustAssociatedConstant, SignatureContractKitError> {
        self.require_empty_generics(
            context,
            &item.generics,
            "generic trait associated constant",
            item.span(),
        )?;
        RustAssociatedConstant::new(
            RustModulePath::semantic_ident(&item.ident),
            Visibility::Public,
            self.convert_type(
                context,
                &RustTypeConverter::with_generic_parameters(
                    owner_generics.to_vec(),
                    &self.cancellation,
                ),
                item.ty.clone(),
            )?,
            item.default
                .as_ref()
                .map(|(_, expression)| RustSyntaxText::from_expression(expression)),
            false,
            self.convert_attributes(context, &item.attrs)?,
        )
    }

    fn convert_impl_constant(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::ImplItemConst,
    ) -> Result<RustAssociatedConstant, SignatureContractKitError> {
        self.require_empty_generics(
            context,
            &item.generics,
            "generic impl associated constant",
            item.span(),
        )?;
        RustAssociatedConstant::new(
            RustModulePath::semantic_ident(&item.ident),
            self.visibility_converter
                .convert_visibility(item.vis.clone(), context.module_id())?,
            self.convert_type(
                context,
                &RustTypeConverter::with_generic_parameters(
                    owner_generics.to_vec(),
                    &self.cancellation,
                ),
                item.ty.clone(),
            )?,
            Some(RustSyntaxText::from_expression(&item.expr)),
            item.modifiers.defaultness.is_some(),
            self.convert_attributes(context, &item.attrs)?,
        )
    }

    fn convert_trait_type(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::TraitItemType,
    ) -> Result<RustAssociatedType, SignatureContractKitError> {
        let type_converter = self.type_converter(context, owner_generics, &item.generics)?;
        RustAssociatedType::new(
            RustModulePath::semantic_ident(&item.ident),
            Visibility::Public,
            self.convert_generics(context, &item.generics)?,
            item.bounds
                .iter()
                .map(RustSyntaxText::from_type_bound)
                .collect(),
            item.default
                .as_ref()
                .map(|(_, rust_type)| {
                    self.convert_type(context, &type_converter, rust_type.clone())
                })
                .transpose()?,
            false,
            self.convert_attributes(context, &item.attrs)?,
        )
    }

    fn convert_impl_type(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::ImplItemType,
    ) -> Result<RustAssociatedType, SignatureContractKitError> {
        let type_converter = self.type_converter(context, owner_generics, &item.generics)?;
        RustAssociatedType::new(
            RustModulePath::semantic_ident(&item.ident),
            self.visibility_converter
                .convert_visibility(item.vis.clone(), context.module_id())?,
            self.convert_generics(context, &item.generics)?,
            Vec::new(),
            Some(self.convert_type(context, &type_converter, item.ty.clone())?),
            item.modifiers.defaultness.is_some(),
            self.convert_attributes(context, &item.attrs)?,
        )
    }

    fn convert_trait_method(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::TraitItemFn,
    ) -> Result<RustMethod, SignatureContractKitError> {
        self.convert_method(context, owner_generics, RustMethodSyntax::Trait(item))
    }

    fn convert_impl_method(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        item: &syn::ImplItemFn,
    ) -> Result<RustMethod, SignatureContractKitError> {
        self.convert_method(
            context,
            owner_generics,
            RustMethodSyntax::Implementation(item),
        )
    }

    fn convert_method(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        method: RustMethodSyntax<'_>,
    ) -> Result<RustMethod, SignatureContractKitError> {
        let (signature, visibility, attributes, has_default_body, is_specialization_default) =
            match method {
                RustMethodSyntax::Trait(item) => (
                    &item.sig,
                    Visibility::Public,
                    item.attrs.as_slice(),
                    item.default.is_some(),
                    false,
                ),
                RustMethodSyntax::Implementation(item) => (
                    &item.sig,
                    self.visibility_converter
                        .convert_visibility(item.vis.clone(), context.module_id())?,
                    item.attrs.as_slice(),
                    true,
                    item.modifiers.defaultness.is_some(),
                ),
            };
        let name = RustModulePath::semantic_ident(&signature.ident);
        let callable = self.convert_signature(context, signature, owner_generics)?;
        let function = FunctionType::new(BaseType::new(
            name,
            visibility,
            context.file().clone(),
            context.module_id().clone(),
            self.convert_attributes(context, attributes)?,
        ))
        .with_callable_signature(callable.signature);
        Ok(RustMethod::new(
            function,
            callable.receiver,
            has_default_body,
            is_specialization_default,
            callable.receiver_attributes,
        ))
    }

    fn convert_foreign_item(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ForeignItem,
    ) -> Result<RustForeignItem, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        match item {
            syn::ForeignItem::Fn(item) => self
                .convert_foreign_function(context, item)
                .map(RustForeignItem::Function),
            syn::ForeignItem::Static(item) => self
                .convert_foreign_static(context, item)
                .map(RustForeignItem::Static),
            syn::ForeignItem::Type(item) => self
                .convert_foreign_type(context, item)
                .map(RustForeignItem::Type),
            syn::ForeignItem::Macro(item) => self
                .convert_foreign_macro(context, item)
                .map(RustForeignItem::Macro),
            syn::ForeignItem::Verbatim(tokens) => {
                context.unsupported_syntax("verbatim foreign item", tokens.span())
            }
            unsupported => context.unsupported_syntax("future foreign item", unsupported.span()),
        }
    }

    fn convert_foreign_function(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ForeignItemFn,
    ) -> Result<RustForeignFunction, SignatureContractKitError> {
        if item.sig.abi.is_some() {
            return context.unsupported_syntax("foreign function with item-level ABI", item.span());
        }
        let callable = self.convert_signature(context, &item.sig, &[])?;
        if callable.receiver.is_some() {
            return context.unsupported_syntax("receiver on foreign function", item.span());
        }
        RustForeignFunction::new(
            FunctionType::new(self.base_type(
                context,
                &RustModulePath::semantic_ident(&item.sig.ident),
                &item.vis,
                &item.attrs,
            )?)
            .with_callable_signature(callable.signature),
        )
    }

    fn convert_foreign_static(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ForeignItemStatic,
    ) -> Result<RustForeignStatic, SignatureContractKitError> {
        RustForeignStatic::new(StaticType::new(
            self.base_type(
                context,
                &RustModulePath::semantic_ident(&item.ident),
                &item.vis,
                &item.attrs,
            )?,
            self.convert_static_mutability(context, &item.mutability, item.span())?,
            self.convert_type(
                context,
                &RustTypeConverter::new(&self.cancellation),
                (*item.ty).clone(),
            )?,
        ))
    }

    fn convert_foreign_type(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ForeignItemType,
    ) -> Result<RustForeignType, SignatureContractKitError> {
        self.require_empty_generics(context, &item.generics, "generic foreign type", item.span())?;
        RustForeignType::new(self.base_type(
            context,
            &RustModulePath::semantic_ident(&item.ident),
            &item.vis,
            &item.attrs,
        )?)
    }

    fn convert_foreign_macro(
        &self,
        context: &RustItemContext<'_>,
        item: &syn::ForeignItemMacro,
    ) -> Result<RustForeignMacro, SignatureContractKitError> {
        RustForeignMacro::new(
            self.tokens(&item.mac),
            self.convert_attributes(context, &item.attrs)?,
        )
    }

    fn convert_signature(
        &self,
        context: &RustItemContext<'_>,
        signature: &syn::Signature,
        owner_generics: &[String],
    ) -> Result<RustCallableConversion, SignatureContractKitError> {
        let type_converter = self.type_converter(context, owner_generics, &signature.generics)?;
        let mut receiver = None;
        let mut receiver_attributes = RustAttributes::default();
        let mut parameters = Vec::new();
        for input in &signature.inputs {
            self.cancellation.checkpoint()?;
            match input {
                syn::FnArg::Receiver(value) => {
                    receiver = Some(self.convert_receiver(context, value, &type_converter)?);
                    receiver_attributes = self.convert_attributes(context, &value.attrs)?;
                }
                syn::FnArg::Typed(value) => parameters.push(
                    RustFunctionParameter::new(
                        Some(RustSyntaxText::from_pattern(&value.pat)),
                        self.convert_type(context, &type_converter, (*value.ty).clone())?,
                    )
                    .with_attributes(self.convert_attributes(context, &value.attrs)?),
                ),
            }
        }

        Ok(RustCallableConversion {
            signature: RustCallableSignature::builder()
                .with_const(signature.constness.is_some())
                .with_async(signature.asyncness.is_some())
                .with_unsafe(matches!(signature.safety, syn::Safety::Unsafe(_)))
                .with_abi(self.convert_abi(signature.abi.as_ref()))
                .with_variadic(self.convert_variadic(context, signature.variadic.as_ref())?)
                .with_generics(self.convert_generics(context, &signature.generics)?)
                .with_parameters(parameters)
                .with_return_type(self.convert_return_type(
                    context,
                    &signature.output,
                    &type_converter,
                )?)
                .build(),
            receiver,
            receiver_attributes,
        })
    }

    fn convert_receiver(
        &self,
        context: &RustItemContext<'_>,
        receiver: &syn::Receiver,
        type_converter: &RustTypeConverter,
    ) -> Result<RustReceiver, SignatureContractKitError> {
        #[allow(unreachable_patterns)]
        match &receiver.kind {
            syn::ReceiverKind::Value => Ok(RustReceiver::value(receiver.mutability.is_some())),
            syn::ReceiverKind::Reference(_, lifetime, mutability) => Ok(RustReceiver::reference(
                lifetime.as_ref().map(|lifetime| self.tokens(lifetime)),
                mutability.is_some(),
            )),
            syn::ReceiverKind::Typed(_, receiver_type) => Ok(RustReceiver::typed(
                receiver.mutability.is_some(),
                self.convert_type(context, type_converter, (**receiver_type).clone())?,
            )),
            _ => context.unsupported_syntax("future method receiver", receiver.span()),
        }
    }

    fn convert_implemented_trait(
        &self,
        modifiers: &syn::ImplModifiers,
        implemented_trait: Option<&(syn::Path, syn::Token![for])>,
    ) -> Result<RustImplementedTrait, SignatureContractKitError> {
        match (implemented_trait, &modifiers.polarity) {
            (None, None) => Ok(RustImplementedTrait::Inherent),
            (None, Some(_)) => Err(SignatureContractKitError::conversion_failed(
                "unsupported Rust syntax negative inherent implementation",
            )),
            (Some((path, _)), polarity) => RustImplementedTrait::for_trait(
                self.tokens(path),
                if polarity.is_some() {
                    RustImplPolarity::Negative
                } else {
                    RustImplPolarity::Positive
                },
            ),
        }
    }

    fn foreign_module_semantic_name(&self, abi: &RustFunctionAbi) -> String {
        match abi {
            RustFunctionAbi::Extern { name: Some(name) } => {
                format!("extern:{}:{name}", name.len())
            }
            RustFunctionAbi::Extern { name: None } => "extern:default".to_owned(),
            RustFunctionAbi::Rust => "rust".to_owned(),
        }
    }

    fn convert_abi(&self, abi: Option<&syn::Abi>) -> RustFunctionAbi {
        match abi {
            None => RustFunctionAbi::Rust,
            Some(abi) => RustFunctionAbi::Extern {
                name: abi.name.as_ref().map(syn::LitStr::value),
            },
        }
    }

    fn convert_variadic(
        &self,
        context: &RustItemContext<'_>,
        variadic: Option<&syn::Variadic>,
    ) -> Result<Option<RustVariadicParameter>, SignatureContractKitError> {
        variadic
            .map(|variadic| {
                Ok(RustVariadicParameter::new(
                    variadic
                        .pat
                        .as_ref()
                        .map(|(pattern, _)| RustSyntaxText::from_pattern(pattern)),
                    self.convert_attributes(context, &variadic.attrs)?,
                ))
            })
            .transpose()
    }

    fn convert_generics(
        &self,
        context: &RustItemContext<'_>,
        generics: &syn::Generics,
    ) -> Result<RustGenericMetadata, SignatureContractKitError> {
        let mut parameters = Vec::with_capacity(generics.params.len());
        for parameter in &generics.params {
            self.cancellation.checkpoint()?;
            #[allow(unreachable_patterns)]
            let converted = match parameter {
                syn::GenericParam::Type(parameter) => RustGenericParameter::type_parameter(
                    RustModulePath::semantic_ident(&parameter.ident),
                    parameter
                        .bounds
                        .iter()
                        .map(|bound| self.tokens(bound))
                        .collect(),
                    parameter
                        .default
                        .as_ref()
                        .map(|(_, value)| self.tokens(value)),
                )
                .with_attributes(self.convert_attributes(context, &parameter.attrs)?),
                syn::GenericParam::Lifetime(parameter) => RustGenericParameter::lifetime_parameter(
                    parameter.lifetime.to_string(),
                    parameter
                        .bounds
                        .iter()
                        .map(|bound| self.tokens(bound))
                        .collect(),
                )
                .with_attributes(self.convert_attributes(context, &parameter.attrs)?),
                syn::GenericParam::Const(parameter) => RustGenericParameter::const_parameter(
                    RustModulePath::semantic_ident(&parameter.ident),
                    self.tokens(&parameter.ty),
                    parameter
                        .default
                        .as_ref()
                        .map(|(_, value)| self.tokens(value)),
                )
                .with_attributes(self.convert_attributes(context, &parameter.attrs)?),
                unsupported => {
                    return context
                        .unsupported_syntax("future generic parameter", unsupported.span());
                }
            };
            parameters.push(converted);
        }
        let where_predicates = generics
            .where_clause
            .as_ref()
            .map(|where_clause| {
                where_clause
                    .predicates
                    .iter()
                    .map(RustSyntaxText::from_where_predicate)
                    .collect()
            })
            .unwrap_or_default();
        Ok(RustGenericMetadata::new(parameters).with_where_predicates(where_predicates))
    }

    fn convert_return_type(
        &self,
        context: &RustItemContext<'_>,
        return_type: &syn::ReturnType,
        type_converter: &RustTypeConverter,
    ) -> Result<Option<RustType>, SignatureContractKitError> {
        match return_type {
            syn::ReturnType::Default => Ok(None),
            syn::ReturnType::Type(_, value) => self
                .convert_type(context, type_converter, (**value).clone())
                .map(Some),
        }
    }

    fn type_converter(
        &self,
        context: &RustItemContext<'_>,
        owner_generics: &[String],
        generics: &syn::Generics,
    ) -> Result<RustTypeConverter, SignatureContractKitError> {
        let mut names = owner_generics.to_vec();
        names.extend(self.generic_type_names(context, generics)?);
        Ok(RustTypeConverter::with_generic_parameters(
            names,
            &self.cancellation,
        ))
    }

    fn generic_type_names(
        &self,
        context: &RustItemContext<'_>,
        generics: &syn::Generics,
    ) -> Result<Vec<String>, SignatureContractKitError> {
        let mut names = Vec::new();
        for parameter in &generics.params {
            #[allow(unreachable_patterns)]
            match parameter {
                syn::GenericParam::Type(parameter) => {
                    names.push(RustModulePath::semantic_ident(&parameter.ident));
                }
                syn::GenericParam::Lifetime(_) | syn::GenericParam::Const(_) => {}
                unsupported => {
                    return context
                        .unsupported_syntax("future generic parameter", unsupported.span());
                }
            }
        }
        Ok(names)
    }

    fn require_empty_generics(
        &self,
        context: &RustItemContext<'_>,
        generics: &syn::Generics,
        syntax_kind: &'static str,
        span: proc_macro2::Span,
    ) -> Result<(), SignatureContractKitError> {
        if generics.params.is_empty() && generics.where_clause.is_none() {
            Ok(())
        } else {
            context.unsupported_syntax(syntax_kind, span)
        }
    }

    fn convert_static_mutability(
        &self,
        context: &RustItemContext<'_>,
        mutability: &syn::StaticMutability,
        span: proc_macro2::Span,
    ) -> Result<bool, SignatureContractKitError> {
        match mutability {
            syn::StaticMutability::Mut(_) => Ok(true),
            syn::StaticMutability::None => Ok(false),
            _ => context.unsupported_syntax("future static mutability", span),
        }
    }

    fn tokens(&self, value: &impl ToTokens) -> String {
        value.to_token_stream().to_string()
    }
}

struct RustCallableConversion {
    signature: RustCallableSignature,
    receiver: Option<RustReceiver>,
    receiver_attributes: RustAttributes,
}

enum RustMethodSyntax<'syntax> {
    Trait(&'syntax syn::TraitItemFn),
    Implementation(&'syntax syn::ImplItemFn),
}

struct RustUsePath {
    absolute: bool,
    segments: Vec<String>,
    alias: Option<String>,
}

impl RustUsePath {
    fn new(absolute: bool) -> Self {
        Self {
            absolute,
            segments: Vec::new(),
            alias: None,
        }
    }

    fn collect(
        &mut self,
        context: &RustItemContext<'_>,
        tree: &syn::UseTree,
        span: proc_macro2::Span,
    ) -> Result<(), SignatureContractKitError> {
        #[allow(unreachable_patterns)]
        match tree {
            syn::UseTree::Path(path) => {
                self.segments
                    .push(RustModulePath::semantic_ident(&path.ident));
                self.collect(context, &path.tree, span)
            }
            syn::UseTree::Name(name) => {
                self.segments
                    .push(RustModulePath::semantic_ident(&name.ident));
                Ok(())
            }
            syn::UseTree::Rename(rename) => {
                self.segments
                    .push(RustModulePath::semantic_ident(&rename.ident));
                self.alias = Some(RustModulePath::semantic_ident(&rename.rename));
                Ok(())
            }
            syn::UseTree::Glob(_) => context.unsupported_syntax("glob use item", span),
            syn::UseTree::Group(_) => context.unsupported_syntax("grouped use item", span),
            unsupported => context.unsupported_syntax("future use tree", unsupported.span()),
        }
    }

    fn into_binding(
        self,
        context: &RustItemContext<'_>,
        attributes: RustAttributes,
        span: proc_macro2::Span,
    ) -> Result<RustImportBinding, SignatureContractKitError> {
        RustImportBinding::new(
            context.module_id().clone(),
            context.source_span(span)?,
            self.absolute,
            self.segments,
            self.alias,
            attributes,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use quote::ToTokens as _;

    use super::{RustItemConversion, RustItemConverter};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::languages::rust::parser::RustCapabilityDiagnostics;
    use crate::languages::rust::parser::inventory_collector::RustItemContext;
    use crate::languages::rust::parser::signature_id::RustItemId;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::source::RustSourceCatalog;
    use crate::languages::rust::types::associated_item::RustAssociatedItem;
    use crate::languages::rust::types::attributes::RustAttribute;
    use crate::languages::rust::types::callable_type::{RustFunctionAbi, RustReceiver};
    use crate::languages::rust::types::declaration::{
        RustDeclaration, RustForeignItem, RustItemKind,
    };
    use crate::languages::rust::types::impl_type::{
        RustImplPolarity, RustImplementationOwner, RustImplementedTrait,
    };
    use crate::languages::rust::types::primitive_types::{RustGenericParameter, Visibility};

    struct ConverterFixture {
        sources: RustSourceCatalog,
        file: CatalogPath,
        module_id: RustModuleId,
        converter: RustItemConverter,
    }

    impl ConverterFixture {
        fn in_module(module_path: &[&str]) -> Self {
            let cancellation = crate::work::CancellationProbe::new();
            let file = CatalogPath::new("lib.rs").expect("fixture file");
            let mut catalog = FileCatalog::new();
            catalog
                .insert(
                    file.clone(),
                    format!("//{}", "x".repeat(4_096)).into_bytes(),
                )
                .expect("unique fixture file");
            let sources = RustSourceCatalog::parse_allowlist(
                &BTreeSet::from([file.clone()]),
                catalog,
                &cancellation,
            )
            .expect("parsed fixture source");
            let module_id = RustModuleId::new(
                RustCrateId::new("sample", &cancellation).expect("fixture crate id"),
                RustModulePath::new(
                    module_path
                        .iter()
                        .map(|segment| (*segment).to_owned())
                        .collect(),
                )
                .expect("fixture module path"),
            );

            Self {
                sources,
                file,
                module_id,
                converter: RustItemConverter::new(&cancellation),
            }
        }

        fn context(&self) -> RustItemContext<'_> {
            RustItemContext::new(&self.sources, self.file.clone(), self.module_id.clone())
        }

        fn convert(
            &self,
            item: &syn::Item,
        ) -> Result<RustItemConversion, crate::SignatureContractKitError> {
            let limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
            self.converter
                .convert_non_implementation_item(&self.context(), item, &mut diagnostics)
        }

        fn declaration(&self, item: syn::Item) -> RustDeclaration {
            self.declaration_with_diagnostics(item).0
        }

        fn declaration_with_diagnostics(
            &self,
            item: syn::Item,
        ) -> (
            RustDeclaration,
            Vec<crate::languages::rust::parser::source_graph::RustCapabilityDiagnostic>,
        ) {
            let limits = crate::limits::DiagnosticLimits::default();
            let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
            let declaration = match &item {
                syn::Item::Impl(implementation) => self
                    .converter
                    .convert_implementation(
                        &self.context(),
                        implementation,
                        self.implementation_owner(implementation),
                        &mut diagnostics,
                    )
                    .expect("implementation conversion"),
                _ => match self
                    .converter
                    .convert_non_implementation_item(&self.context(), &item, &mut diagnostics)
                    .expect("item conversion")
                {
                    RustItemConversion::Declaration(declaration)
                    | RustItemConversion::PublicReexport { declaration, .. } => declaration,
                    RustItemConversion::PrivateImport(_) => {
                        panic!("expected an inventory declaration");
                    }
                },
            };
            let (_, declaration) = declaration.into_parts();
            (
                declaration,
                diagnostics
                    .into_values(&crate::work::CancellationProbe::new())
                    .expect("fixture capability diagnostics"),
            )
        }

        fn implementation_owner(&self, implementation: &syn::ItemImpl) -> RustImplementationOwner {
            let syn::Type::Path(owner) = implementation.self_ty.as_ref() else {
                panic!("fixture implementation owner must be a type path");
            };
            let name = &owner
                .path
                .segments
                .last()
                .expect("fixture owner segment")
                .ident;
            let name = RustModulePath::semantic_ident(name);
            RustImplementationOwner::new(
                RustItemId::new(self.module_id.clone(), RustItemKind::Struct, name),
                implementation.self_ty.to_token_stream().to_string(),
            )
            .expect("fixture implementation owner")
        }

        fn function(
            &self,
            item: syn::ItemFn,
        ) -> crate::languages::rust::types::function_type::FunctionType {
            let RustDeclaration::Function(function) = self.declaration(syn::Item::Fn(item)) else {
                panic!("function declaration");
            };
            function
        }

        fn canonical_bytes(&self, source: &str) -> Vec<u8> {
            let item = syn::parse_str::<syn::Item>(source).expect("valid mutation fixture item");
            self.declaration(item)
                .canonical_bytes()
                .expect("canonical mutation fixture bytes")
        }

        fn assert_semantic_mutations(&self, cases: &[(&str, &str, &str)]) {
            for (name, baseline, changed) in cases {
                assert_ne!(
                    self.canonical_bytes(baseline),
                    self.canonical_bytes(changed),
                    "{name} must participate in rust_api_v1 canonical semantics"
                );
            }
        }
    }

    #[test]
    fn nested_public_main_is_an_ordinary_public_function() {
        let fixture = ConverterFixture::in_module(&["framework"]);
        let function = fixture.function(syn::parse_quote! {
            pub fn main() {}
        });

        assert_eq!(function.base().name(), "main");
        assert_eq!(function.base().visibility(), &Visibility::Public);
    }

    #[test]
    fn method_receivers_are_converted_into_closed_semantic_forms() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Implementation(implementation) =
            fixture.declaration(syn::parse_quote! {
                impl Handler {
                    fn by_value(self) {}
                    fn by_mutable_value(mut self) {}
                    fn by_reference(&self) {}
                    fn by_mutable_reference<'a>(&'a mut self) {}
                    fn by_typed_receiver(self: Box<Self>) {}
                }
            })
        else {
            panic!("implementation declaration");
        };
        let receivers = implementation
            .items()
            .iter()
            .map(|item| match item {
                RustAssociatedItem::Method(method) => {
                    method.receiver().cloned().expect("method receiver")
                }
                RustAssociatedItem::Constant(_) | RustAssociatedItem::Type(_) => {
                    panic!("method item")
                }
            })
            .collect::<Vec<_>>();

        assert_eq!(receivers[0], RustReceiver::value(false));
        assert_eq!(receivers[1], RustReceiver::value(true));
        assert_eq!(receivers[2], RustReceiver::reference(None, false));
        assert_eq!(
            receivers[3],
            RustReceiver::reference(Some("'a".to_owned()), true)
        );
        let Some(crate::languages::rust::types::primitive_types::RustType::TypePath(path)) =
            receivers[4].receiver_type()
        else {
            panic!("typed receiver type path");
        };
        assert_eq!(path.source_syntax(), "Box < Self >");
    }

    #[test]
    fn generic_defaults_survive_syn_three_tuple_containers() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Structure(structure) = fixture.declaration(syn::parse_quote! {
            pub struct Packet<T = String, const N: usize = 4>([T; N]);
        }) else {
            panic!("structure declaration");
        };

        assert!(matches!(
            &structure.generics().parameters()[0],
            RustGenericParameter::Type {
                name,
                default: Some(default),
                ..
            } if name == "T" && default == "String"
        ));
        assert!(matches!(
            &structure.generics().parameters()[1],
            RustGenericParameter::Const {
                name,
                parameter_type,
                default: Some(default),
                ..
            } if name == "N" && parameter_type == "usize" && default == "4"
        ));
    }

    #[test]
    fn impl_modifiers_preserve_specialization_and_negative_polarity() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Implementation(specialized) = fixture.declaration(syn::parse_quote! {
            default impl Service for Handler {
                default const LIMIT: usize = 4;
                default type Output = String;
                default fn execute(&self) {}
            }
        }) else {
            panic!("specialized implementation declaration");
        };

        assert!(specialized.is_default());
        assert!(specialized.items().iter().all(|item| match item {
            RustAssociatedItem::Constant(constant) => constant.is_specialization_default(),
            RustAssociatedItem::Type(associated_type) => {
                associated_type.is_specialization_default()
            }
            RustAssociatedItem::Method(method) => method.is_specialization_default(),
        }));

        let RustDeclaration::Implementation(negative) = fixture.declaration(syn::parse_quote! {
            impl !Send for Handler {}
        }) else {
            panic!("negative implementation declaration");
        };
        assert!(matches!(
            negative.implemented_trait(),
            RustImplementedTrait::Trait {
                polarity: RustImplPolarity::Negative,
                ..
            }
        ));
    }

    #[test]
    fn declaration_family_fields_change_canonical_semantics_independently() {
        let fixture = ConverterFixture::in_module(&[]);

        fixture.assert_semantic_mutations(&[
            (
                "enum discriminant",
                "pub enum State { Ready = 1 }",
                "pub enum State { Ready = 2 }",
            ),
            (
                "enum tuple field type",
                "pub enum State { Ready(u8) }",
                "pub enum State { Ready(u16) }",
            ),
            (
                "enum named field type",
                "pub enum State { Ready { value: u8 } }",
                "pub enum State { Ready { value: u16 } }",
            ),
            (
                "extern crate alias",
                "pub extern crate core as rust_core;",
                "pub extern crate core as core_api;",
            ),
            (
                "macro invocation tokens",
                "contract_item!(first);",
                "contract_item!(second);",
            ),
            (
                "static mutability",
                "pub static VALUE: u8 = 1;",
                "pub static mut VALUE: u8 = 1;",
            ),
            (
                "static type",
                "pub static VALUE: u8 = 1;",
                "pub static VALUE: u16 = 1;",
            ),
            (
                "struct named field type",
                "pub struct Packet { pub value: u8 }",
                "pub struct Packet { pub value: u16 }",
            ),
            (
                "struct field order",
                "pub struct Packet(pub u8, pub u16);",
                "pub struct Packet(pub u16, pub u8);",
            ),
            (
                "union field type",
                "pub union Storage { pub value: u8 }",
                "pub union Storage { pub value: u16 }",
            ),
            (
                "union field order",
                "pub union Storage { pub narrow: u8, pub wide: u16 }",
                "pub union Storage { pub wide: u16, pub narrow: u8 }",
            ),
            (
                "type alias target",
                "pub type Value<T> = T;",
                "pub type Value<T> = Option<T>;",
            ),
            (
                "type alias generic bounds",
                "pub type Value<T> = T;",
                "pub type Value<T: Clone> = T;",
            ),
        ]);
    }

    #[test]
    fn callable_fields_change_canonical_semantics_independently() {
        let fixture = ConverterFixture::in_module(&[]);

        fixture.assert_semantic_mutations(&[
            (
                "const qualifier",
                "pub fn call(value: u8) -> u8 { value }",
                "pub const fn call(value: u8) -> u8 { value }",
            ),
            (
                "async qualifier",
                "pub fn call(value: u8) -> u8 { value }",
                "pub async fn call(value: u8) -> u8 { value }",
            ),
            (
                "unsafe qualifier",
                "pub fn call(value: u8) -> u8 { value }",
                "pub unsafe fn call(value: u8) -> u8 { value }",
            ),
            (
                "callable ABI",
                "pub extern \"C\" fn call(value: u8) -> u8 { value }",
                "pub extern \"system\" fn call(value: u8) -> u8 { value }",
            ),
            (
                "parameter type",
                "pub fn call(value: u8) {}",
                "pub fn call(value: u16) {}",
            ),
            (
                "parameter order",
                "pub fn call(first: u8, second: u16) {}",
                "pub fn call(first: u16, second: u8) {}",
            ),
            (
                "return type",
                "pub fn call() -> u8 { 1 }",
                "pub fn call() -> u16 { 1 }",
            ),
            (
                "variadic presence",
                "pub unsafe extern \"C\" fn call(format: *const u8) {}",
                "pub unsafe extern \"C\" fn call(format: *const u8, ...) {}",
            ),
            (
                "receiver form",
                "impl Handler { pub fn call(&self) {} }",
                "impl Handler { pub fn call(&mut self) {} }",
            ),
            (
                "typed receiver type",
                "impl Handler { pub fn call(self: Box<Self>) {} }",
                "impl Handler { pub fn call(self: std::pin::Pin<Box<Self>>) {} }",
            ),
        ]);
    }

    #[test]
    fn trait_and_implementation_fields_change_canonical_semantics_independently() {
        let fixture = ConverterFixture::in_module(&[]);

        fixture.assert_semantic_mutations(&[
            (
                "unsafe trait",
                "pub trait Service {}",
                "pub unsafe trait Service {}",
            ),
            (
                "auto trait",
                "pub trait Service {}",
                "pub auto trait Service {}",
            ),
            (
                "trait supertrait",
                "pub trait Service: Clone {}",
                "pub trait Service: Send {}",
            ),
            (
                "trait method default-body presence",
                "pub trait Service { fn call(&self); }",
                "pub trait Service { fn call(&self) {} }",
            ),
            (
                "trait associated constant type",
                "pub trait Service { const LIMIT: u8; }",
                "pub trait Service { const LIMIT: u16; }",
            ),
            (
                "trait associated type bounds",
                "pub trait Service { type Output: Clone; }",
                "pub trait Service { type Output: Send; }",
            ),
            (
                "trait method receiver",
                "pub trait Service { fn call(&self); }",
                "pub trait Service { fn call(&mut self); }",
            ),
            (
                "impl associated constant value",
                "impl Handler { pub const LIMIT: u8 = 1; }",
                "impl Handler { pub const LIMIT: u8 = 2; }",
            ),
            (
                "impl associated type target",
                "impl Handler { pub type Output = u8; }",
                "impl Handler { pub type Output = u16; }",
            ),
            (
                "impl method signature",
                "impl Handler { pub fn call(&self, value: u8) {} }",
                "impl Handler { pub fn call(&self, value: u16) {} }",
            ),
        ]);
    }

    #[test]
    fn foreign_module_and_item_fields_change_canonical_semantics_independently() {
        let fixture = ConverterFixture::in_module(&[]);

        fixture.assert_semantic_mutations(&[
            (
                "foreign module ABI",
                "unsafe extern \"C\" { pub fn call(); }",
                "unsafe extern \"system\" { pub fn call(); }",
            ),
            (
                "foreign module safety",
                "extern \"C\" { pub fn call(); }",
                "unsafe extern \"C\" { pub fn call(); }",
            ),
            (
                "foreign function parameter type",
                "unsafe extern \"C\" { pub fn call(value: u8); }",
                "unsafe extern \"C\" { pub fn call(value: u16); }",
            ),
            (
                "foreign function variadic presence",
                "unsafe extern \"C\" { pub fn call(format: *const u8); }",
                "unsafe extern \"C\" { pub fn call(format: *const u8, ...); }",
            ),
            (
                "foreign static mutability",
                "unsafe extern \"C\" { pub static VALUE: u8; }",
                "unsafe extern \"C\" { pub static mut VALUE: u8; }",
            ),
            (
                "foreign static type",
                "unsafe extern \"C\" { pub static VALUE: u8; }",
                "unsafe extern \"C\" { pub static VALUE: u16; }",
            ),
            (
                "foreign macro tokens",
                "unsafe extern \"C\" { native_items!(first); }",
                "unsafe extern \"C\" { native_items!(second); }",
            ),
        ]);
    }

    #[test]
    fn remaining_typed_attribute_variants_change_api_canonical_bytes() {
        let fixture = ConverterFixture::in_module(&[]);

        fixture.assert_semantic_mutations(&[
            (
                "derive",
                "pub struct Value;",
                "#[derive(Clone)]\npub struct Value;",
            ),
            (
                "cfg_attr",
                "#[cfg_attr(unix, repr(C))]\npub struct Value;",
                "#[cfg_attr(windows, repr(C))]\npub struct Value;",
            ),
            (
                "deprecated",
                "#[deprecated(note = \"old contract\")]\npub struct Value;",
                "#[deprecated(note = \"new contract\")]\npub struct Value;",
            ),
            (
                "doc_hidden",
                "pub struct Value;",
                "#[doc(hidden)]\npub struct Value;",
            ),
            (
                "unresolved",
                "#[contract_runtime(mode = \"old\")]\npub struct Value;",
                "#[contract_runtime(mode = \"new\")]\npub struct Value;",
            ),
        ]);
    }

    #[test]
    fn visibility_and_attributes_change_each_applicable_family_semantics() {
        let fixture = ConverterFixture::in_module(&[]);
        let visibility_cases = [
            (
                "constant",
                "const VALUE: u8 = 1;",
                "pub const VALUE: u8 = 1;",
            ),
            ("enum", "enum Value { One }", "pub enum Value { One }"),
            (
                "extern crate",
                "extern crate core as rust_core;",
                "pub extern crate core as rust_core;",
            ),
            ("function", "fn value() {}", "pub fn value() {}"),
            ("module", "mod value {}", "pub mod value {}"),
            (
                "static",
                "static VALUE: u8 = 1;",
                "pub static VALUE: u8 = 1;",
            ),
            ("struct", "struct Value;", "pub struct Value;"),
            ("trait", "trait Value {}", "pub trait Value {}"),
            (
                "trait alias",
                "trait Value = Send;",
                "pub trait Value = Send;",
            ),
            ("type alias", "type Value = u8;", "pub type Value = u8;"),
            (
                "union",
                "union Value { field: u8 }",
                "pub union Value { field: u8 }",
            ),
            (
                "struct field",
                "pub struct Value { field: u8 }",
                "pub struct Value { pub field: u8 }",
            ),
            (
                "union field",
                "pub union Value { field: u8 }",
                "pub union Value { pub field: u8 }",
            ),
            (
                "impl associated constant",
                "impl Handler { const VALUE: u8 = 1; }",
                "impl Handler { pub const VALUE: u8 = 1; }",
            ),
            (
                "impl associated type",
                "impl Handler { type Value = u8; }",
                "impl Handler { pub type Value = u8; }",
            ),
            (
                "impl method",
                "impl Handler { fn value(&self) {} }",
                "impl Handler { pub fn value(&self) {} }",
            ),
            (
                "foreign function",
                "unsafe extern \"C\" { fn value(); }",
                "unsafe extern \"C\" { pub fn value(); }",
            ),
            (
                "foreign static",
                "unsafe extern \"C\" { static VALUE: u8; }",
                "unsafe extern \"C\" { pub static VALUE: u8; }",
            ),
            (
                "foreign type",
                "unsafe extern \"C\" { type Value; }",
                "unsafe extern \"C\" { pub type Value; }",
            ),
        ];
        fixture.assert_semantic_mutations(&visibility_cases);

        let attribute_cases = [
            (
                "constant",
                "pub const VALUE: u8 = 1;",
                "#[cfg(unix)] pub const VALUE: u8 = 1;",
            ),
            (
                "enum",
                "pub enum Value { One }",
                "#[cfg(unix)] pub enum Value { One }",
            ),
            (
                "extern crate",
                "pub extern crate core as rust_core;",
                "#[cfg(unix)] pub extern crate core as rust_core;",
            ),
            (
                "function",
                "pub fn value() {}",
                "#[cfg(unix)] pub fn value() {}",
            ),
            (
                "foreign module",
                "unsafe extern \"C\" { pub fn value(); }",
                "#[cfg(unix)] unsafe extern \"C\" { pub fn value(); }",
            ),
            (
                "implementation",
                "impl Handler {}",
                "#[cfg(unix)] impl Handler {}",
            ),
            (
                "macro",
                "contract_item!();",
                "#[cfg(unix)] contract_item!();",
            ),
            (
                "module",
                "pub mod value {}",
                "#[cfg(unix)] pub mod value {}",
            ),
            (
                "static",
                "pub static VALUE: u8 = 1;",
                "#[cfg(unix)] pub static VALUE: u8 = 1;",
            ),
            (
                "struct",
                "pub struct Value;",
                "#[cfg(unix)] pub struct Value;",
            ),
            (
                "trait",
                "pub trait Value {}",
                "#[cfg(unix)] pub trait Value {}",
            ),
            (
                "trait alias",
                "pub trait Value = Send;",
                "#[cfg(unix)] pub trait Value = Send;",
            ),
            (
                "type alias",
                "pub type Value = u8;",
                "#[cfg(unix)] pub type Value = u8;",
            ),
            (
                "union",
                "pub union Value { field: u8 }",
                "#[cfg(unix)] pub union Value { field: u8 }",
            ),
            (
                "reexport",
                "pub use crate::Value;",
                "#[cfg(unix)] pub use crate::Value;",
            ),
            (
                "enum variant",
                "pub enum Value { One }",
                "pub enum Value { #[cfg(unix)] One }",
            ),
            (
                "enum field",
                "pub enum Value { One { field: u8 } }",
                "pub enum Value { One { #[cfg(unix)] field: u8 } }",
            ),
            (
                "struct field",
                "pub struct Value { field: u8 }",
                "pub struct Value { #[cfg(unix)] field: u8 }",
            ),
            (
                "union field",
                "pub union Value { field: u8 }",
                "pub union Value { #[cfg(unix)] field: u8 }",
            ),
            (
                "trait associated constant",
                "pub trait Value { const ITEM: u8; }",
                "pub trait Value { #[cfg(unix)] const ITEM: u8; }",
            ),
            (
                "trait associated type",
                "pub trait Value { type Item; }",
                "pub trait Value { #[cfg(unix)] type Item; }",
            ),
            (
                "trait method",
                "pub trait Value { fn item(&self); }",
                "pub trait Value { #[cfg(unix)] fn item(&self); }",
            ),
            (
                "impl associated constant",
                "impl Handler { const ITEM: u8 = 1; }",
                "impl Handler { #[cfg(unix)] const ITEM: u8 = 1; }",
            ),
            (
                "impl associated type",
                "impl Handler { type Item = u8; }",
                "impl Handler { #[cfg(unix)] type Item = u8; }",
            ),
            (
                "impl method",
                "impl Handler { fn item(&self) {} }",
                "impl Handler { #[cfg(unix)] fn item(&self) {} }",
            ),
            (
                "foreign function",
                "unsafe extern \"C\" { fn item(); }",
                "unsafe extern \"C\" { #[cfg(unix)] fn item(); }",
            ),
            (
                "foreign static",
                "unsafe extern \"C\" { static ITEM: u8; }",
                "unsafe extern \"C\" { #[cfg(unix)] static ITEM: u8; }",
            ),
            (
                "foreign type",
                "unsafe extern \"C\" { type Item; }",
                "unsafe extern \"C\" { #[cfg(unix)] type Item; }",
            ),
            (
                "foreign macro",
                "unsafe extern \"C\" { native_items!(); }",
                "unsafe extern \"C\" { #[cfg(unix)] native_items!(); }",
            ),
        ];
        fixture.assert_semantic_mutations(&attribute_cases);
    }

    #[test]
    fn every_pinned_top_level_item_has_an_explicit_conversion_outcome() {
        let fixture = ConverterFixture::in_module(&[]);
        let declarations = [
            (
                syn::parse_quote!(
                    pub const LIMIT: usize = 4;
                ),
                RustItemKind::Constant,
            ),
            (
                syn::parse_quote!(
                    pub enum Choice {
                        One,
                    }
                ),
                RustItemKind::Enum,
            ),
            (
                syn::parse_quote!(
                    pub extern crate core as rust_core;
                ),
                RustItemKind::ExternCrate,
            ),
            (
                syn::parse_quote!(
                    pub fn execute() {}
                ),
                RustItemKind::Function,
            ),
            (
                syn::parse_quote!(
                    unsafe extern "C" {
                        pub fn native_execute();
                    }
                ),
                RustItemKind::ForeignModule,
            ),
            (
                syn::parse_quote!(impl Handler {}),
                RustItemKind::Implementation,
            ),
            (syn::parse_quote!(contract_item!();), RustItemKind::Macro),
            (
                syn::parse_quote!(
                    pub mod inline {}
                ),
                RustItemKind::Module,
            ),
            (
                syn::parse_quote!(
                    pub static GLOBAL: usize = 4;
                ),
                RustItemKind::Static,
            ),
            (
                syn::parse_quote!(
                    pub struct Handler;
                ),
                RustItemKind::Struct,
            ),
            (
                syn::parse_quote!(
                    pub trait Service {}
                ),
                RustItemKind::Trait,
            ),
            (
                syn::parse_quote!(
                    pub trait ServiceAlias = Send + Sync;
                ),
                RustItemKind::TraitAlias,
            ),
            (
                syn::parse_quote!(
                    pub type ResultValue = Result<(), Error>;
                ),
                RustItemKind::TypeAlias,
            ),
            (
                syn::parse_quote!(
                    pub union Number {
                        integer: u32,
                        float: f32,
                    }
                ),
                RustItemKind::Union,
            ),
            (
                syn::parse_quote!(
                    pub use crate::internal::Handler as PublicHandler;
                ),
                RustItemKind::Reexport,
            ),
        ];

        for (item, expected) in declarations {
            assert_eq!(fixture.declaration(item).kind(), expected);
        }
        assert!(matches!(
            fixture
                .convert(&syn::parse_quote!(
                    use crate::internal::Handler;
                ))
                .expect("private import conversion"),
            RustItemConversion::PrivateImport(_)
        ));
    }

    #[test]
    fn module_conversion_retains_signature_visibility_attributes_shape_and_path() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Module(out_of_line) = fixture.declaration(syn::parse_quote! {
            #[cfg(feature = "transport")]
            #[path = "platform/transport.rs"]
            pub mod transport;
        }) else {
            panic!("out-of-line module declaration");
        };
        let RustDeclaration::Module(inline) = fixture.declaration(syn::parse_quote! {
            pub(crate) mod inline {}
        }) else {
            panic!("inline module declaration");
        };

        assert_eq!(out_of_line.base().visibility(), &Visibility::Public);
        assert!(!out_of_line.is_inline());
        assert_eq!(out_of_line.path_override(), Some("platform/transport.rs"));
        assert!(matches!(
            out_of_line.attributes().values(),
            [RustAttribute::Conditional(_)]
        ));
        assert_eq!(inline.base().visibility(), &Visibility::Crate);
        assert!(inline.is_inline());
        assert_eq!(inline.path_override(), None);
        assert!(inline.attributes().values().is_empty());
    }

    #[test]
    fn module_source_shape_failures_keep_the_declaring_source_context() {
        let fixture = ConverterFixture::in_module(&[]);
        let error = fixture
            .convert(&syn::parse_quote!(
                unsafe mod transport;
            ))
            .expect_err("unsafe module shape is unsupported");
        let message = error.to_string();

        assert!(message.contains("invalid Rust source"), "{message}");
        assert!(message.contains("unsafe module"), "{message}");
        assert!(message.contains("lib.rs"), "{message}");
    }

    #[test]
    fn implementation_conversion_requires_a_resolved_second_pass_owner() {
        let fixture = ConverterFixture::in_module(&[]);
        let implementation: syn::Item = syn::parse_quote! {
            impl Handler {}
        };

        let error = fixture
            .convert(&implementation)
            .expect_err("pass one cannot construct an unresolved implementation");

        assert!(error.to_string().contains("owner resolution"));
    }

    #[test]
    fn private_imports_and_public_reexports_retain_resolution_bindings() {
        let fixture = ConverterFixture::in_module(&[]);
        let private = fixture
            .convert(&syn::parse_quote! {
                #[cfg(feature = "local-handler")]
                use crate::internal::Handler as LocalHandler;
            })
            .expect("private import conversion");
        let public = fixture
            .convert(&syn::parse_quote! {
                pub use crate::internal::Handler as PublicHandler;
            })
            .expect("public reexport conversion");

        let RustItemConversion::PrivateImport(private) = private else {
            panic!("private import binding");
        };
        let RustItemConversion::PublicReexport {
            declaration,
            binding: public,
        } = public
        else {
            panic!("public reexport binding");
        };
        assert_eq!(private.render_path(), "crate::internal::Handler");
        assert_eq!(private.declared_in(), &fixture.module_id);
        assert_eq!(private.span().file(), &fixture.file);
        assert!(!private.leading_colon());
        assert_eq!(private.target_segments(), &["crate", "internal", "Handler"]);
        assert_eq!(private.alias(), Some("LocalHandler"));
        assert_eq!(private.local_name(), "LocalHandler");
        assert!(private.requires_capability_warning());
        assert_eq!(public.render_path(), "crate::internal::Handler");
        assert_eq!(public.local_name(), "PublicHandler");
        assert!(matches!(
            declaration.into_parts().1,
            RustDeclaration::Reexport(_)
        ));
    }

    #[test]
    fn raw_declarations_and_imports_store_semantic_identifier_spellings() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Structure(structure) = fixture.declaration(syn::parse_quote!(
            pub struct r#type;
        )) else {
            panic!("raw structure declaration");
        };
        let function = fixture.function(syn::parse_quote! {
            pub fn r#match() {}
        });
        let RustDeclaration::Module(module) = fixture.declaration(syn::parse_quote!(
            pub mod r#async {}
        )) else {
            panic!("raw module declaration");
        };
        let import = fixture
            .convert(&syn::parse_quote! {
                use crate::r#type::r#match as r#async;
            })
            .expect("raw import conversion");
        let RustItemConversion::PrivateImport(import) = import else {
            panic!("raw private import binding");
        };

        assert_eq!(structure.base().name(), "type");
        assert_eq!(function.base().name(), "match");
        assert_eq!(module.base().name(), "async");
        assert_eq!(import.target_segments(), &["crate", "type", "match"]);
        assert_eq!(import.alias(), Some("async"));
        assert_eq!(import.local_name(), "async");
        assert_eq!(import.render_path(), "crate::type::match");
    }

    #[test]
    fn raw_field_variant_and_associated_names_share_canonical_semantics() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Structure(structure) = fixture.declaration(syn::parse_quote! {
            pub struct Packet { pub r#type: u8 }
        }) else {
            panic!("raw field declaration");
        };
        let RustDeclaration::Enumeration(enumeration) = fixture.declaration(syn::parse_quote! {
            pub enum State { r#match { r#type: u8 } }
        }) else {
            panic!("raw enum declaration");
        };
        let RustDeclaration::Trait(trait_type) = fixture.declaration(syn::parse_quote! {
            pub trait Service {
                const r#match: u8;
                type r#type;
            }
        }) else {
            panic!("raw associated declarations");
        };

        assert_eq!(structure.fields()[0].name(), Some("type"));
        assert_eq!(enumeration.variants()[0].name(), "match");
        assert_eq!(enumeration.variants()[0].fields()[0].name(), Some("type"));
        let RustAssociatedItem::Constant(constant) = &trait_type.items()[0] else {
            panic!("raw associated constant");
        };
        let RustAssociatedItem::Type(associated_type) = &trait_type.items()[1] else {
            panic!("raw associated type");
        };
        assert_eq!(constant.name(), "match");
        assert_eq!(associated_type.name(), "type");
    }

    #[test]
    fn trait_and_impl_items_preserve_constants_types_methods_and_visibility() {
        let fixture = ConverterFixture::in_module(&["framework"]);
        let RustDeclaration::Trait(trait_type) = fixture.declaration(syn::parse_quote! {
            pub trait Service {
                const LIMIT: usize = 4;
                type Output: Clone = String;
                fn execute(&self) -> Self::Output;
            }
        }) else {
            panic!("trait declaration");
        };
        assert!(matches!(
            trait_type.items()[0],
            RustAssociatedItem::Constant(_)
        ));
        assert!(matches!(trait_type.items()[1], RustAssociatedItem::Type(_)));
        assert!(matches!(
            trait_type.items()[2],
            RustAssociatedItem::Method(_)
        ));
        for item in trait_type.items() {
            let visibility = match item {
                RustAssociatedItem::Method(method) => method.visibility(),
                RustAssociatedItem::Constant(constant) => constant.visibility(),
                RustAssociatedItem::Type(associated_type) => associated_type.visibility(),
            };
            assert_eq!(visibility, &Visibility::Public);
        }

        let RustDeclaration::Implementation(implementation) =
            fixture.declaration(syn::parse_quote! {
                impl Handler {
                    pub const LIMIT: usize = 4;
                    pub type Output = String;
                    pub fn execute(&self) -> Self::Output { String::new() }
                }
            })
        else {
            panic!("implementation declaration");
        };
        assert!(matches!(
            implementation.items()[0],
            RustAssociatedItem::Constant(_)
        ));
        assert!(matches!(
            implementation.items()[1],
            RustAssociatedItem::Type(_)
        ));
        assert!(matches!(
            implementation.items()[2],
            RustAssociatedItem::Method(_)
        ));
        for item in implementation.items() {
            let visibility = match item {
                RustAssociatedItem::Method(method) => method.visibility(),
                RustAssociatedItem::Constant(constant) => constant.visibility(),
                RustAssociatedItem::Type(associated_type) => associated_type.visibility(),
            };
            assert_eq!(visibility, &Visibility::Public);
        }
    }

    #[test]
    fn associated_macros_are_retained_as_typed_capability_diagnostics() {
        let fixture = ConverterFixture::in_module(&["framework"]);
        let (trait_declaration, trait_diagnostics) =
            fixture.declaration_with_diagnostics(syn::parse_quote! {
                pub trait Service {
                    generated_trait_items!();
                    fn execute(&self);
                }
            });
        let (implementation, implementation_diagnostics) =
            fixture.declaration_with_diagnostics(syn::parse_quote! {
                impl Handler {
                    generated_impl_items!();
                    pub fn execute(&self) {}
                }
            });

        let RustDeclaration::Trait(trait_type) = trait_declaration else {
            panic!("trait declaration");
        };
        let RustDeclaration::Implementation(implementation_type) = implementation else {
            panic!("implementation declaration");
        };
        assert_eq!(trait_type.items().len(), 1);
        assert_eq!(implementation_type.items().len(), 1);
        assert_eq!(trait_diagnostics.len(), 1);
        assert_eq!(implementation_diagnostics.len(), 1);
        assert!(trait_diagnostics[0].to_string().contains("trait"));
        assert!(
            implementation_diagnostics[0]
                .to_string()
                .contains("implementation")
        );
        assert!(
            trait_diagnostics[0]
                .to_string()
                .contains("sample::framework")
        );
    }

    #[test]
    fn foreign_block_owns_the_abi_and_every_foreign_item_is_retained() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::ForeignModule(module) = fixture.declaration(syn::parse_quote! {
            #[link(name = "native")]
            unsafe extern "C" {
                pub fn native_execute(#[cfg(unix)] value: u8) -> bool;
                pub static mut NATIVE_VALUE: u8;
                pub type NativeOpaque;
                native_items!();
            }
        }) else {
            panic!("foreign module declaration");
        };

        assert_eq!(
            module.abi(),
            &RustFunctionAbi::Extern {
                name: Some("C".to_owned())
            }
        );
        assert!(matches!(module.items()[0], RustForeignItem::Function(_)));
        assert!(matches!(module.items()[1], RustForeignItem::Static(_)));
        assert!(matches!(module.items()[2], RustForeignItem::Type(_)));
        assert!(matches!(module.items()[3], RustForeignItem::Macro(_)));
        let RustForeignItem::Function(function) = &module.items()[0] else {
            panic!("foreign function");
        };
        assert_eq!(
            function.function().signature().abi(),
            &RustFunctionAbi::Rust
        );
        assert!(
            function.function().signature().parameters()[0]
                .attributes()
                .requires_capability_warning()
        );
    }

    #[test]
    fn every_typed_parameter_retains_its_complete_pattern() {
        let fixture = ConverterFixture::in_module(&[]);
        let item: syn::ItemFn = syn::parse_quote! {
            pub fn submit(
                mut value: Request,
                ref borrowed: Request,
                whole @ Some(_): Option<Request>,
                (left, right): (u8, u8),
                _: u8,
            ) {}
        };
        let expected = item
            .sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Typed(parameter) => Some(parameter.pat.to_token_stream().to_string()),
                syn::FnArg::Receiver(_) => None,
            })
            .collect::<Vec<_>>();
        let function = fixture.function(item);
        let actual = function
            .signature()
            .parameters()
            .iter()
            .map(|parameter| parameter.pattern().map(ToOwned::to_owned))
            .collect::<Vec<_>>();

        assert_eq!(actual, expected.into_iter().map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn parameter_pattern_changes_do_not_change_api_canonical_bytes() {
        let fixture = ConverterFixture::in_module(&[]);
        let identifier = fixture.function(syn::parse_quote! {
            pub fn submit(request: Request) {}
        });
        let destructured = fixture.function(syn::parse_quote! {
            pub fn submit(Request { value, .. }: Request) {}
        });

        assert_ne!(
            identifier.signature().parameters()[0].pattern(),
            destructured.signature().parameters()[0].pattern(),
            "rendering metadata must retain the source pattern"
        );
        assert_eq!(
            serde_json::to_vec(&identifier).expect("identifier canonical bytes"),
            serde_json::to_vec(&destructured).expect("destructured canonical bytes"),
            "ordinary binding shape is not API-call semantics"
        );
    }

    #[test]
    fn semantic_attributes_are_attached_at_item_field_variant_and_parameter_sites() {
        let fixture = ConverterFixture::in_module(&[]);
        let RustDeclaration::Structure(structure) = fixture.declaration(syn::parse_quote! {
            #[repr(C)]
            pub struct Packet<#[cfg(feature = "generic")] T> {
                #[cfg(feature = "payload")]
                pub payload: T,
            }
        }) else {
            panic!("struct declaration");
        };
        let RustDeclaration::Enumeration(enumeration) = fixture.declaration(syn::parse_quote! {
            #[repr(u8)]
            pub enum State {
                #[non_exhaustive]
                Ready {
                    #[cfg(feature = "details")]
                    details: u8,
                },
            }
        }) else {
            panic!("enum declaration");
        };
        let RustDeclaration::Union(union) = fixture.declaration(syn::parse_quote! {
            #[repr(C)]
            pub union Storage {
                #[cfg(feature = "integer")]
                integer: u32,
            }
        }) else {
            panic!("union declaration");
        };
        let function = fixture.function(syn::parse_quote! {
            #[must_use = "inspect the result"]
            pub fn execute<#[cfg(feature = "generic")] T>(
                #[cfg(feature = "argument")] value: T,
            ) -> bool { true }
        });

        assert!(matches!(
            structure.base().attributes().values()[0],
            RustAttribute::Repr(_)
        ));
        assert!(
            structure.generics().parameters()[0]
                .attributes()
                .requires_capability_warning()
        );
        assert!(
            structure.fields()[0]
                .attributes()
                .requires_capability_warning()
        );

        assert!(matches!(
            enumeration.base().attributes().values()[0],
            RustAttribute::Repr(_)
        ));
        assert!(matches!(
            enumeration.variants()[0].attributes().values()[0],
            RustAttribute::NonExhaustive
        ));
        assert!(
            enumeration.variants()[0].fields()[0]
                .attributes()
                .requires_capability_warning()
        );

        assert!(matches!(
            union.base().attributes().values()[0],
            RustAttribute::Repr(_)
        ));
        assert!(union.fields()[0].attributes().requires_capability_warning());

        assert!(matches!(
            function.base().attributes().values()[0],
            RustAttribute::MustUse(_)
        ));
        assert!(
            function.signature().generics().parameters()[0]
                .attributes()
                .requires_capability_warning()
        );
        assert!(
            function.signature().parameters()[0]
                .attributes()
                .requires_capability_warning()
        );
    }

    #[test]
    fn variadic_attributes_are_typed_instead_of_token_strings() {
        let fixture = ConverterFixture::in_module(&[]);
        let function = fixture.function(syn::parse_quote! {
            pub unsafe extern "C" fn log(
                format: *const u8,
                #[cfg(feature = "variadic")] args: ...,
            ) {}
        });
        let variadic = function.signature().variadic().expect("variadic parameter");

        assert_eq!(variadic.pattern(), Some("args"));
        assert!(variadic.attributes().requires_capability_warning());
    }

    #[test]
    fn malformed_derive_attributes_fail_atomically_during_item_conversion() {
        let fixture = ConverterFixture::in_module(&[]);
        let item: syn::Item = syn::parse_quote! {
            #[derive(Clone, =)]
            pub struct Packet;
        };

        let error = fixture
            .convert(&item)
            .expect_err("malformed derive must fail");

        let message = error.to_string();
        assert!(message.contains("derive"), "{message}");
        assert!(message.contains("lib.rs"), "{message}");
        assert!(message.contains("sample"), "{message}");
        assert!(message.contains("bytes"), "{message}");
    }

    #[test]
    fn malformed_nested_type_attributes_retain_outer_source_context() {
        let fixture = ConverterFixture::in_module(&["framework"]);
        let item: syn::Item = syn::parse_quote! {
            pub type Callback = fn(#[derive(Clone, =)] u8);
        };

        let error = fixture
            .convert(&item)
            .expect_err("malformed nested derive must fail");

        let message = error.to_string();
        assert!(message.contains("derive"), "{message}");
        assert!(message.contains("lib.rs"), "{message}");
        assert!(message.contains("framework"), "{message}");
        assert!(message.contains("bytes"), "{message}");
    }

    #[test]
    fn source_conversion_observes_cancellation_before_nested_work() {
        let fixture = ConverterFixture::in_module(&[]);
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();
        let converter = RustItemConverter::new(&cancellation);
        let limits = crate::limits::DiagnosticLimits::default();
        let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
        let item: syn::Item = syn::parse_quote! {
            pub struct Packet {
                value: Option<Result<Vec<u8>, String>>,
            }
        };

        let error = converter
            .convert_non_implementation_item(&fixture.context(), &item, &mut diagnostics)
            .expect_err("canceled conversion must stop before nested fields");

        assert_eq!(error.to_string(), "signature operation was canceled");
    }

    #[test]
    fn verbatim_syntax_fails_with_file_module_and_syntax_evidence() {
        let fixture = ConverterFixture::in_module(&["framework"]);
        let cases = [
            (
                syn::parse_str::<syn::Item>("pub impl(crate) trait Restricted {}").expect(
                    "Syn 3 must retain an implementation-restricted trait as verbatim syntax",
                ),
                "verbatim item",
            ),
            (
                syn::Item::Verbatim(quote::quote!(future_item)),
                "verbatim item",
            ),
            {
                let mut item: syn::ItemTrait = syn::parse_quote!(
                    pub trait Service {}
                );
                item.items
                    .push(syn::TraitItem::Verbatim(quote::quote!(future_trait)));
                (syn::Item::Trait(item), "verbatim trait item")
            },
            {
                let mut item: syn::ItemImpl = syn::parse_quote!(impl Handler {});
                item.items
                    .push(syn::ImplItem::Verbatim(quote::quote!(future_impl)));
                (syn::Item::Impl(item), "verbatim impl item")
            },
            {
                let mut item: syn::ItemForeignMod = syn::parse_quote!(
                    extern "C" {}
                );
                item.items
                    .push(syn::ForeignItem::Verbatim(quote::quote!(future_foreign)));
                (syn::Item::ForeignMod(item), "verbatim foreign item")
            },
        ];

        for (item, syntax_kind) in cases {
            let error = match &item {
                syn::Item::Impl(implementation) => {
                    let limits = crate::limits::DiagnosticLimits::default();
                    let mut diagnostics = RustCapabilityDiagnostics::new(&limits);
                    fixture
                        .converter
                        .convert_implementation(
                            &fixture.context(),
                            implementation,
                            fixture.implementation_owner(implementation),
                            &mut diagnostics,
                        )
                        .expect_err("verbatim impl syntax must fail closed")
                }
                _ => fixture
                    .convert(&item)
                    .expect_err("verbatim syntax must fail closed"),
            };
            let message = error.to_string();
            assert!(message.contains("lib.rs"), "{message}");
            assert!(message.contains("sample::framework"), "{message}");
            assert!(message.contains(syntax_kind), "{message}");
        }
    }

    #[test]
    fn unsupported_use_groups_and_globs_fail_instead_of_partially_reexporting() {
        let fixture = ConverterFixture::in_module(&[]);

        for item in [
            syn::parse_quote!(
                pub use crate::api::{First, Second};
            ),
            syn::parse_quote!(
                pub use crate::api::*;
            ),
        ] {
            let error = fixture
                .convert(&item)
                .expect_err("unsupported use tree must fail closed");
            assert!(error.to_string().contains("use"));
        }
    }
}
