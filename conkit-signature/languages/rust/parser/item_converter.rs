use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::inventory_builder::RustItemContext;
use crate::languages::rust::parser::signature_id::{
    RustImplementationId, RustItemId, RustItemIdAllocator, RustItemKind,
};
use crate::languages::rust::parser::type_converter::RustTypeConverter;
use crate::languages::rust::parser::visibility_converter::RustVisibilityConverter;
use crate::languages::rust::parser::{RustParsedEntry, RustSignature};
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{
    RustCallableSignature, RustFunctionAbi, RustMethod, RustVariadicParameter,
};
use crate::languages::rust::types::enum_type::{EnumType, EnumVariant, EnumVariantField};
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementedTrait,
};
use crate::languages::rust::types::macro_type::MacroType;
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustGenericParameter, Visibility,
};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::{StructField, StructType};
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use quote::ToTokens;

#[derive(Default)]
pub(super) struct RustItemConverter {
    visibility_converter: RustVisibilityConverter,
    item_ids: RustItemIdAllocator,
}

impl RustItemConverter {
    pub(super) fn convert_function(
        &self,
        context: &RustItemContext,
        item: syn::ItemFn,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.sig.ident.to_string();
        let visibility = if name == "main" {
            syn::Visibility::Inherited
        } else {
            item.vis
        };
        let signature = FunctionType::new(self.base_type(&name, visibility, &item.attrs, context))
            .with_callable_signature(self.convert_signature(&item.sig, &[]).signature);

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Function, name),
            RustSignature::Function(signature),
        ))
    }

    pub(super) fn convert_struct(
        &self,
        context: &RustItemContext,
        item: syn::ItemStruct,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let type_converter = self.type_converter_for_generics(&item.generics);
        let signature = StructType::new(self.base_type(&name, item.vis, &item.attrs, context))
            .with_generic_metadata(self.convert_generics(&item.generics))
            .with_fields(self.convert_struct_fields(item.fields, &type_converter));

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Struct, name),
            RustSignature::Struct(signature),
        ))
    }

    pub(super) fn convert_enum(
        &self,
        context: &RustItemContext,
        item: syn::ItemEnum,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let type_converter = self.type_converter_for_generics(&item.generics);
        let variants = item
            .variants
            .into_iter()
            .map(|variant| self.convert_enum_variant(variant, &type_converter))
            .collect();
        let signature = EnumType::new(self.base_type(&name, item.vis, &item.attrs, context))
            .with_generic_metadata(self.convert_generics(&item.generics))
            .with_variants(variants);

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Enum, name),
            RustSignature::Enum(signature),
        ))
    }

    pub(super) fn convert_trait(
        &self,
        context: &RustItemContext,
        item: syn::ItemTrait,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let owner_generics = self.generic_type_names(&item.generics);
        let methods = item
            .items
            .into_iter()
            .filter_map(|item| match item {
                syn::TraitItem::Fn(method) => {
                    Some(self.convert_trait_method(context, &owner_generics, method))
                }
                _ => None,
            })
            .collect();
        let supertraits = item
            .supertraits
            .into_iter()
            .map(|bound| self.tokens(&bound))
            .collect();
        let signature = TraitType::new(self.base_type(&name, item.vis, &item.attrs, context))
            .with_generic_metadata(self.convert_generics(&item.generics))
            .with_supertraits(supertraits)
            .with_methods(methods);

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Trait, name),
            RustSignature::Trait(signature),
        ))
    }

    pub(super) fn convert_impl(
        &self,
        context: &RustItemContext,
        item: syn::ItemImpl,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let owner_type = self.tokens(&item.self_ty);
        let implemented_trait = self.convert_implemented_trait(item.trait_.as_ref());
        let implementation_id = self.implementation_id(owner_type.clone(), &implemented_trait);
        let owner_generics = self.generic_type_names(&item.generics);
        let methods = item
            .items
            .into_iter()
            .filter_map(|item| match item {
                syn::ImplItem::Fn(method) => {
                    Some(self.convert_impl_method(context, &owner_generics, method))
                }
                _ => None,
            })
            .collect();
        let signature = ImplementationType::new(owner_type)
            .with_implemented_trait(implemented_trait)
            .with_qualifiers(item.defaultness.is_some(), item.unsafety.is_some())
            .with_generic_metadata(self.convert_generics(&item.generics))
            .with_methods(methods);

        Ok(RustParsedEntry::new(
            self.item_id(
                context,
                RustItemKind::Implementation,
                implementation_id.render(),
            ),
            RustSignature::Implementation(signature),
        ))
    }

    pub(super) fn convert_union(
        &self,
        context: &RustItemContext,
        item: syn::ItemUnion,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let type_converter = self.type_converter_for_generics(&item.generics);
        let signature = UnionType::new(self.base_type(&name, item.vis, &item.attrs, context))
            .with_generic_metadata(self.convert_generics(&item.generics))
            .with_fields(
                self.convert_struct_fields(syn::Fields::Named(item.fields), &type_converter),
            );

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Union, name),
            RustSignature::Union(signature),
        ))
    }

    pub(super) fn convert_static(
        &self,
        context: &RustItemContext,
        item: syn::ItemStatic,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let type_converter = RustTypeConverter::default();
        let static_type = type_converter.convert_type(*item.ty);
        let signature = StaticType::new(
            self.base_type(&name, item.vis, &item.attrs, context),
            matches!(item.mutability, syn::StaticMutability::Mut(_)),
            static_type,
        );

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::Static, name),
            RustSignature::Static(signature),
        ))
    }

    pub(super) fn convert_macro(
        &mut self,
        context: &RustItemContext,
        item: syn::ItemMacro,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item
            .ident
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.tokens(&item.mac.path));
        let signature = MacroType::new(
            self.base_type(&name, syn::Visibility::Inherited, &item.attrs, context),
            self.tokens(&item.mac),
        );

        let id = self.item_id(context, RustItemKind::Macro, name);
        Ok(RustParsedEntry::new(
            self.item_ids.allocate(id)?,
            RustSignature::Macro(signature),
        ))
    }

    pub(super) fn convert_type_alias(
        &self,
        context: &RustItemContext,
        item: syn::ItemType,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = item.ident.to_string();
        let type_converter = self.type_converter_for_generics(&item.generics);
        let signature = TypeAliasType::new(
            self.base_type(&name, item.vis, &item.attrs, context),
            self.convert_generics(&item.generics),
            type_converter.convert_type(*item.ty),
        );

        Ok(RustParsedEntry::new(
            self.item_id(context, RustItemKind::TypeAlias, name),
            RustSignature::TypeAlias(signature),
        ))
    }

    fn item_id(
        &self,
        context: &RustItemContext,
        kind: RustItemKind,
        name: impl Into<String>,
    ) -> RustItemId {
        RustItemId::new(
            context.file().clone(),
            context.module_path().to_vec(),
            kind,
            name,
        )
    }

    fn base_type(
        &self,
        name: &str,
        visibility: syn::Visibility,
        attrs: &[syn::Attribute],
        context: &RustItemContext,
    ) -> BaseType {
        BaseType::new(
            name.to_owned(),
            self.visibility_converter.convert_visibility(visibility),
            context.file().clone(),
        )
        .with_module_path(context.module_path().to_vec())
        .with_derives(self.derive_names(attrs))
    }

    fn convert_struct_fields(
        &self,
        fields: syn::Fields,
        type_converter: &RustTypeConverter,
    ) -> Vec<StructField> {
        match fields {
            syn::Fields::Named(fields) => fields
                .named
                .into_iter()
                .map(|field| {
                    StructField::new(
                        field.ident.map(|ident| ident.to_string()),
                        self.visibility_converter.convert_visibility(field.vis),
                        type_converter.convert_type(field.ty),
                    )
                })
                .collect(),
            syn::Fields::Unnamed(fields) => fields
                .unnamed
                .into_iter()
                .map(|field| {
                    StructField::new(
                        None,
                        self.visibility_converter.convert_visibility(field.vis),
                        type_converter.convert_type(field.ty),
                    )
                })
                .collect(),
            syn::Fields::Unit => Vec::new(),
        }
    }

    fn convert_enum_variant(
        &self,
        variant: syn::Variant,
        type_converter: &RustTypeConverter,
    ) -> EnumVariant {
        let fields = match variant.fields {
            syn::Fields::Named(fields) => fields
                .named
                .into_iter()
                .map(|field| {
                    EnumVariantField::new(
                        field.ident.map(|ident| ident.to_string()),
                        type_converter.convert_type(field.ty),
                    )
                })
                .collect(),
            syn::Fields::Unnamed(fields) => fields
                .unnamed
                .into_iter()
                .map(|field| EnumVariantField::new(None, type_converter.convert_type(field.ty)))
                .collect(),
            syn::Fields::Unit => Vec::new(),
        };
        let discriminant = variant
            .discriminant
            .map(|(_, expression)| self.tokens(&expression));

        EnumVariant::new(variant.ident.to_string(), fields, discriminant)
    }

    fn convert_trait_method(
        &self,
        context: &RustItemContext,
        owner_generics: &[String],
        method: syn::TraitItemFn,
    ) -> RustMethod {
        self.convert_method(
            context,
            owner_generics,
            method.sig,
            Visibility::Public,
            method.attrs,
        )
    }

    fn convert_impl_method(
        &self,
        context: &RustItemContext,
        owner_generics: &[String],
        method: syn::ImplItemFn,
    ) -> RustMethod {
        let visibility = self.visibility_converter.convert_visibility(method.vis);
        self.convert_method(
            context,
            owner_generics,
            method.sig,
            visibility,
            method.attrs,
        )
    }

    fn convert_method(
        &self,
        context: &RustItemContext,
        owner_generics: &[String],
        signature: syn::Signature,
        visibility: Visibility,
        attrs: Vec<syn::Attribute>,
    ) -> RustMethod {
        let name = signature.ident.to_string();
        let parts = self.convert_signature(&signature, owner_generics);
        let function = FunctionType::new(
            BaseType::new(name, visibility.clone(), context.file().clone())
                .with_module_path(context.module_path().to_vec())
                .with_derives(self.derive_names(&attrs)),
        )
        .with_callable_signature(parts.signature);

        RustMethod::new(function, parts.receiver, visibility)
    }

    fn convert_signature(
        &self,
        signature: &syn::Signature,
        owner_generics: &[String],
    ) -> ConvertedSignatureParts {
        let type_converter = self.type_converter_for_signature(owner_generics, signature);
        let mut receiver = None;
        let mut parameters = Vec::new();

        for input in &signature.inputs {
            match input {
                syn::FnArg::Receiver(value) => {
                    receiver = Some(self.tokens(value));
                }
                syn::FnArg::Typed(value) => {
                    parameters.push(RustFunctionParameter::new(
                        self.parameter_name(&value.pat),
                        type_converter.convert_type((*value.ty).clone()),
                    ));
                }
            }
        }

        ConvertedSignatureParts {
            signature: RustCallableSignature::builder()
                .with_const(signature.constness.is_some())
                .with_async(signature.asyncness.is_some())
                .with_unsafe(signature.unsafety.is_some())
                .with_abi(self.convert_abi(signature.abi.as_ref()))
                .with_variadic(self.convert_variadic(signature.variadic.as_ref()))
                .with_generics(self.convert_generics(&signature.generics))
                .with_parameters(parameters)
                .with_return_type(self.convert_return_type(&signature.output, &type_converter))
                .build(),
            receiver,
        }
    }

    fn convert_implemented_trait(
        &self,
        implemented_trait: Option<&(Option<syn::Token![!]>, syn::Path, syn::Token![for])>,
    ) -> RustImplementedTrait {
        match implemented_trait {
            None => RustImplementedTrait::Inherent,
            Some((polarity, path, _)) => RustImplementedTrait::Trait {
                name: self.tokens(path),
                polarity: if polarity.is_some() {
                    RustImplPolarity::Negative
                } else {
                    RustImplPolarity::Positive
                },
            },
        }
    }

    fn implementation_id(
        &self,
        owner_type: String,
        implemented_trait: &RustImplementedTrait,
    ) -> RustImplementationId {
        match implemented_trait {
            RustImplementedTrait::Inherent => RustImplementationId::inherent(owner_type),
            RustImplementedTrait::Trait { name, polarity } => {
                RustImplementationId::trait_impl(owner_type, name.clone(), *polarity)
            }
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

    fn convert_variadic(&self, variadic: Option<&syn::Variadic>) -> Option<RustVariadicParameter> {
        variadic.map(|variadic| {
            RustVariadicParameter::new(
                variadic
                    .pat
                    .as_ref()
                    .map(|(pattern, _)| self.tokens(pattern)),
                variadic
                    .attrs
                    .iter()
                    .map(|attribute| self.tokens(attribute))
                    .collect(),
            )
        })
    }

    fn convert_generics(&self, generics: &syn::Generics) -> RustGenericMetadata {
        let parameters = generics
            .params
            .iter()
            .map(|parameter| match parameter {
                syn::GenericParam::Type(parameter) => RustGenericParameter::type_parameter(
                    parameter.ident.to_string(),
                    parameter
                        .bounds
                        .iter()
                        .map(|bound| self.tokens(bound))
                        .collect(),
                    parameter.default.as_ref().map(|value| self.tokens(value)),
                ),
                syn::GenericParam::Lifetime(parameter) => RustGenericParameter::lifetime_parameter(
                    parameter.lifetime.to_string(),
                    parameter
                        .bounds
                        .iter()
                        .map(|bound| self.tokens(bound))
                        .collect(),
                ),
                syn::GenericParam::Const(parameter) => RustGenericParameter::const_parameter(
                    parameter.ident.to_string(),
                    self.tokens(&parameter.ty),
                    parameter.default.as_ref().map(|value| self.tokens(value)),
                ),
            })
            .collect();
        let where_predicates = generics
            .where_clause
            .as_ref()
            .map(|where_clause| {
                where_clause
                    .predicates
                    .iter()
                    .map(|predicate| self.tokens(predicate))
                    .collect()
            })
            .unwrap_or_default();

        RustGenericMetadata::new(parameters).with_where_predicates(where_predicates)
    }

    fn convert_return_type(
        &self,
        return_type: &syn::ReturnType,
        type_converter: &RustTypeConverter,
    ) -> Option<crate::languages::rust::types::primitive_types::RustType> {
        match return_type {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, value) => Some(type_converter.convert_type((**value).clone())),
        }
    }

    fn type_converter_for_generics(&self, generics: &syn::Generics) -> RustTypeConverter {
        RustTypeConverter::with_generic_parameters(self.generic_type_names(generics))
    }

    fn type_converter_for_signature(
        &self,
        owner_generics: &[String],
        signature: &syn::Signature,
    ) -> RustTypeConverter {
        let mut generic_type_names = owner_generics.to_vec();
        generic_type_names.extend(self.generic_type_names(&signature.generics));
        RustTypeConverter::with_generic_parameters(generic_type_names)
    }

    fn generic_type_names(&self, generics: &syn::Generics) -> Vec<String> {
        generics
            .params
            .iter()
            .filter_map(|parameter| match parameter {
                syn::GenericParam::Type(parameter) => Some(parameter.ident.to_string()),
                syn::GenericParam::Lifetime(_) | syn::GenericParam::Const(_) => None,
            })
            .collect()
    }

    fn parameter_name(&self, pattern: &syn::Pat) -> Option<String> {
        match pattern {
            syn::Pat::Ident(ident) => Some(ident.ident.to_string()),
            syn::Pat::Wild(_) => None,
            other => Some(self.tokens(other)),
        }
    }

    fn derive_names(&self, attrs: &[syn::Attribute]) -> Vec<String> {
        let mut names = Vec::new();

        for attr in attrs {
            if !attr.path().is_ident("derive") {
                continue;
            }

            let _ = attr.parse_nested_meta(|meta| {
                names.push(self.tokens(&meta.path));
                Ok(())
            });
        }

        names
    }

    fn tokens(&self, value: &impl ToTokens) -> String {
        value.to_token_stream().to_string()
    }
}

struct ConvertedSignatureParts {
    signature: RustCallableSignature,
    receiver: Option<String>,
}
