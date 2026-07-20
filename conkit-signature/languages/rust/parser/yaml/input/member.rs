use super::super::type_text::{RustYamlGenericContext, RustYamlTypeText};
use super::declaration::{RustYamlCallableInput, RustYamlFieldPresence};
use super::metadata::{
    RustYamlAttributesValue, RustYamlCallableQualifierInput, RustYamlGenericParametersInput,
    RustYamlImplementationQualifierInput, RustYamlShorthandVariadicInput, RustYamlVisibilityInput,
};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::source_graph::RustModuleId;
use crate::languages::rust::types::associated_item::{
    RustAssociatedConstant, RustAssociatedItem, RustAssociatedType,
};
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{RustMethod, RustReceiver};
use crate::languages::rust::types::declaration::{
    RustForeignFunction, RustForeignItem, RustForeignMacro, RustForeignStatic, RustForeignType,
    RustIdentifier,
};
use crate::languages::rust::types::enum_type::{EnumVariant, EnumVariantField};
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementationOwner, RustImplementedTrait,
};
use crate::languages::rust::types::primitive_types::{RustFunctionParameter, Visibility};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::StructField;
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::work::CancellationProbe;
use quote::ToTokens;
use serde::{Deserialize, de};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlShorthandFields {
    List(Vec<RustYamlStructFieldInput>),
    Map(RustYamlOrderedMap<RustYamlShorthandFieldValueInput>),
}

pub(super) struct RustYamlOrderedMap<T> {
    pub(super) entries: Vec<(String, T)>,
}

impl<T> RustYamlOrderedMap<T> {
    pub(super) fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
}

impl<'de, T> Deserialize<'de> for RustYamlOrderedMap<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct OrderedMapVisitor<T> {
            value: std::marker::PhantomData<T>,
        }

        impl<'de, T> de::Visitor<'de> for OrderedMapVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = RustYamlOrderedMap<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an ordered YAML mapping")
            }

            fn visit_map<M>(self, mut mapping: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut entries = Vec::with_capacity(mapping.size_hint().unwrap_or_default());
                while let Some(entry) = mapping.next_entry()? {
                    entries.push(entry);
                }
                Ok(RustYamlOrderedMap { entries })
            }
        }

        deserializer.deserialize_map(OrderedMapVisitor {
            value: std::marker::PhantomData,
        })
    }
}

impl RustYamlShorthandFields {
    pub(super) fn to_struct_fields(
        &self,
        context: &RustYamlGenericContext,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<StructField>, SignatureContractKitError> {
        match self {
            Self::List(fields) => {
                let mut output = Vec::with_capacity(fields.len());
                for field in fields {
                    cancellation.checkpoint()?;
                    output.push(field.to_struct_field(
                        context,
                        module_id,
                        catalog_name,
                        cancellation,
                    )?);
                }
                Ok(output)
            }
            Self::Map(fields) => {
                let mut output = Vec::with_capacity(fields.entries.len());
                for (name, field) in fields.iter() {
                    cancellation.checkpoint()?;
                    output.push(field.to_named_struct_field(
                        name,
                        context,
                        module_id,
                        catalog_name,
                        cancellation,
                    )?);
                }
                Ok(output)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlStructFieldInput {
    UnnamedType(String),
    UnnamedDetails(RustYamlShorthandFieldDetailsInput),
    Named(BTreeMap<String, RustYamlShorthandFieldValueInput>),
}

impl RustYamlStructFieldInput {
    pub(super) fn to_struct_field(
        &self,
        context: &RustYamlGenericContext,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<StructField, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::UnnamedType(field_type) => StructField::new(
                None,
                Visibility::Private,
                RustYamlTypeText::from_text(field_type.clone()).parse(context, cancellation)?,
                RustAttributes::default(),
            ),
            Self::UnnamedDetails(details) => StructField::new(
                None,
                details.visibility(module_id, catalog_name)?,
                RustYamlTypeText::from_text(details.field_type.clone())
                    .parse(context, cancellation)?,
                details
                    .attributes
                    .to_attributes(catalog_name, cancellation)?,
            ),
            Self::Named(field) => {
                if field.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand field entries must contain exactly one field",
                    ));
                }

                let Some((name, field)) = field.iter().next() else {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand field entry is empty",
                    ));
                };

                StructField::new(
                    Some(name.clone()),
                    field.visibility(module_id, catalog_name)?,
                    RustYamlTypeText::from_text(field.field_type()).parse(context, cancellation)?,
                    field.attributes(catalog_name, cancellation)?,
                )
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlShorthandFieldValueInput {
    Type(String),
    Details(RustYamlShorthandFieldDetailsInput),
}

impl RustYamlShorthandFieldValueInput {
    pub(super) fn to_named_struct_field(
        &self,
        name: &str,
        context: &RustYamlGenericContext,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<StructField, SignatureContractKitError> {
        cancellation.checkpoint()?;
        StructField::new(
            Some(name.to_owned()),
            self.visibility(module_id, catalog_name)?,
            RustYamlTypeText::from_text(self.field_type()).parse(context, cancellation)?,
            self.attributes(catalog_name, cancellation)?,
        )
    }

    pub(super) fn field_type(&self) -> String {
        match self {
            Self::Type(value) => value.clone(),
            Self::Details(value) => value.field_type.clone(),
        }
    }

    pub(super) fn visibility(
        &self,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        match self {
            Self::Type(_) => Ok(Visibility::Private),
            Self::Details(value) => value.visibility(module_id, catalog_name),
        }
    }

    pub(super) fn attributes(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Type(_) => Ok(RustAttributes::default()),
            Self::Details(value) => value.attributes.to_attributes(catalog_name, cancellation),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlShorthandFieldDetailsInput {
    #[serde(rename = "type")]
    pub(super) field_type: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

impl RustYamlShorthandFieldDetailsInput {
    pub(super) fn visibility(
        &self,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        self.visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(module_id, catalog_name))
            .unwrap_or(Ok(Visibility::Private))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlShorthandVariant {
    Unit(String),
    Tuple(BTreeMap<String, Vec<String>>),
    Details(BTreeMap<String, RustYamlShorthandVariantDetailsInput>),
}

impl RustYamlShorthandVariant {
    pub(super) fn to_variant(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<EnumVariant, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Unit(name) => {
                EnumVariant::new(name.clone(), Vec::new(), None, RustAttributes::default())
            }
            Self::Tuple(variant) => {
                if variant.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand variant entries must contain exactly one variant",
                    ));
                }

                let Some((name, fields)) = variant.iter().next() else {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand variant entry is empty",
                    ));
                };

                let mut parsed_fields = Vec::with_capacity(fields.len());
                for field_type in fields {
                    cancellation.checkpoint()?;
                    parsed_fields.push(EnumVariantField::new(
                        None,
                        RustYamlTypeText::from_text(field_type.clone())
                            .parse(context, cancellation)?,
                        RustAttributes::default(),
                    )?);
                }
                EnumVariant::new(name.clone(), parsed_fields, None, RustAttributes::default())
            }
            Self::Details(variant) => {
                if variant.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand variant entries must contain exactly one variant",
                    ));
                }

                let Some((name, details)) = variant.iter().next() else {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand variant entry is empty",
                    ));
                };

                details.to_variant(name, context, catalog_name, cancellation)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlShorthandVariantDetailsInput {
    pub(super) fields: Option<RustYamlShorthandVariantFieldsInput>,
    pub(super) discriminant: Option<RustYamlDiscriminantInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

impl RustYamlShorthandVariantDetailsInput {
    pub(super) fn to_variant(
        &self,
        name: &str,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<EnumVariant, SignatureContractKitError> {
        cancellation.checkpoint()?;
        EnumVariant::new(
            name.to_owned(),
            self.fields
                .as_ref()
                .map(|fields| fields.to_fields(context, catalog_name, cancellation))
                .unwrap_or_else(|| Ok(Vec::new()))?,
            self.discriminant
                .as_ref()
                .map(|value| value.to_discriminant(catalog_name))
                .transpose()?,
            self.attributes.to_attributes(catalog_name, cancellation)?,
        )
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlShorthandVariantFieldsInput {
    List(Vec<RustYamlEnumVariantFieldInput>),
    Map(RustYamlOrderedMap<RustYamlVariantFieldValueInput>),
}

impl RustYamlShorthandVariantFieldsInput {
    pub(super) fn to_fields(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<EnumVariantField>, SignatureContractKitError> {
        match self {
            Self::List(fields) => {
                let mut output = Vec::with_capacity(fields.len());
                for field in fields {
                    cancellation.checkpoint()?;
                    output.push(field.to_variant_field(context, catalog_name, cancellation)?);
                }
                Ok(output)
            }
            Self::Map(fields) => {
                let mut output = Vec::with_capacity(fields.entries.len());
                for (name, field) in fields.iter() {
                    cancellation.checkpoint()?;
                    output.push(RustYamlMapVariantField::new(name, field).to_variant_field(
                        context,
                        catalog_name,
                        cancellation,
                    )?);
                }
                Ok(output)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlEnumVariantFieldInput {
    UnnamedType(String),
    UnnamedDetails(RustYamlVariantFieldDetailsInput),
    Named(BTreeMap<String, RustYamlVariantFieldValueInput>),
}

impl RustYamlEnumVariantFieldInput {
    pub(super) fn to_variant_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<EnumVariantField, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::UnnamedType(field_type) => EnumVariantField::new(
                None,
                RustYamlTypeText::from_text(field_type.clone()).parse(context, cancellation)?,
                RustAttributes::default(),
            ),
            Self::UnnamedDetails(details) => EnumVariantField::new(
                None,
                RustYamlTypeText::from_text(details.field_type.clone())
                    .parse(context, cancellation)?,
                details
                    .attributes
                    .to_attributes(catalog_name, cancellation)?,
            ),
            Self::Named(field) => {
                if field.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand enum variant field entries must contain exactly one field",
                    ));
                }

                let Some((name, field)) = field.iter().next() else {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand enum variant field entry is empty",
                    ));
                };

                EnumVariantField::new(
                    Some(name.clone()),
                    RustYamlTypeText::from_text(field.field_type().to_owned())
                        .parse(context, cancellation)?,
                    field.attributes(catalog_name, cancellation)?,
                )
            }
        }
    }
}

pub(super) struct RustYamlMapVariantField<'a> {
    pub(super) name: &'a str,
    pub(super) field: &'a RustYamlVariantFieldValueInput,
}

impl<'a> RustYamlMapVariantField<'a> {
    pub(super) fn new(name: &'a str, field: &'a RustYamlVariantFieldValueInput) -> Self {
        Self { name, field }
    }

    pub(super) fn to_variant_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<EnumVariantField, SignatureContractKitError> {
        cancellation.checkpoint()?;
        EnumVariantField::new(
            Some(self.name.to_owned()),
            RustYamlTypeText::from_text(self.field.field_type().to_owned())
                .parse(context, cancellation)?,
            self.field.attributes(catalog_name, cancellation)?,
        )
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlVariantFieldValueInput {
    Type(String),
    Details(RustYamlVariantFieldDetailsInput),
}

impl RustYamlVariantFieldValueInput {
    pub(super) fn field_type(&self) -> &str {
        match self {
            Self::Type(value) => value,
            Self::Details(details) => &details.field_type,
        }
    }

    pub(super) fn attributes(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Type(_) => Ok(RustAttributes::default()),
            Self::Details(details) => details.attributes.to_attributes(catalog_name, cancellation),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlVariantFieldDetailsInput {
    #[serde(rename = "type")]
    pub(super) field_type: String,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

pub(super) struct RustYamlDiscriminantInput {
    pub(super) value: String,
}

impl RustYamlDiscriminantInput {
    pub(super) fn to_discriminant(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustSyntaxText, SignatureContractKitError> {
        RustSyntaxText::parse_expression(&self.value).map_err(|source| {
            SignatureContractKitError::parse_failed(catalog_name, source.to_string())
        })
    }
}

impl<'de> Deserialize<'de> for RustYamlDiscriminantInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DiscriminantVisitor;

        impl<'de> de::Visitor<'de> for DiscriminantVisitor {
            type Value = String;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("shorthand text or a number")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(value.to_owned())
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(value)
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(value.to_string())
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(value.to_string())
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(value.to_string())
            }
        }

        deserializer
            .deserialize_any(DiscriminantVisitor)
            .map(|value| Self { value })
            .map_err(|_| {
                de::Error::custom("Rust YAML enum discriminant must be shorthand text or a number")
            })
    }
}

#[derive(Deserialize)]
#[serde(tag = "signature_type", rename_all = "snake_case")]
pub(super) enum RustYamlNestedItemInput {
    Method(RustYamlMethodInput),
    AssociatedConstant(RustYamlAssociatedConstantInput),
    AssociatedType(RustYamlAssociatedTypeInput),
    ForeignFunction(RustYamlForeignFunctionInput),
    ForeignStatic(RustYamlForeignStaticInput),
    ForeignType(RustYamlForeignTypeInput),
    ForeignMacro(RustYamlForeignMacroInput),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlForeignFunctionInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
    #[serde(default)]
    pub(super) qualifiers: Vec<RustYamlCallableQualifierInput>,
    pub(super) variadic: Option<RustYamlShorthandVariadicInput>,
    pub(super) generics: Option<RustYamlGenericParametersInput>,
    #[serde(rename = "where")]
    pub(super) where_predicates: Option<Vec<String>>,
    pub(super) parameters: Option<Vec<RustYamlFunctionParameterInput>>,
    pub(super) return_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlForeignStaticInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) mutable: bool,
    #[serde(rename = "type")]
    pub(super) type_text: String,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlForeignTypeInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlForeignMacroInput {
    pub(super) tokens: String,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlAssociatedConstantInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(rename = "type")]
    pub(super) type_text: String,
    pub(super) default_value: Option<String>,
    #[serde(default)]
    pub(super) specialization_default: bool,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlAssociatedTypeInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    pub(super) generics: Option<RustYamlGenericParametersInput>,
    #[serde(rename = "where")]
    pub(super) where_predicates: Option<Vec<String>>,
    #[serde(default)]
    pub(super) bounds: Vec<String>,
    pub(super) default_type: Option<String>,
    #[serde(default)]
    pub(super) specialization_default: bool,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

impl RustYamlNestedItemInput {
    pub(super) fn into_trait_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAssociatedItem, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Method(method) => {
                if method.specialization_default {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait declaration items cannot be specialization defaults",
                    ));
                }
                let method = method.into_method(
                    source_file,
                    module_id,
                    RustYamlMethodContainer::Trait,
                    owner_context,
                    catalog_name,
                    cancellation,
                )?;
                if !matches!(method.visibility(), Visibility::Public) {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait methods require public visibility",
                    ));
                }
                Ok(RustAssociatedItem::Method(Box::new(method)))
            }
            Self::AssociatedConstant(constant) => {
                if constant.specialization_default {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait associated constants cannot be specialization defaults",
                    ));
                }
                let visibility = constant
                    .visibility
                    .as_ref()
                    .map(|visibility| visibility.to_visibility(module_id, catalog_name))
                    .unwrap_or(Ok(Visibility::Public))?;
                if !matches!(visibility, Visibility::Public) {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait associated constants require public visibility",
                    ));
                }
                constant
                    .into_constant(visibility, owner_context, catalog_name, cancellation)
                    .map(RustAssociatedItem::Constant)
            }
            Self::AssociatedType(associated_type) => {
                if associated_type.specialization_default {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait associated types cannot be specialization defaults",
                    ));
                }
                let visibility = associated_type
                    .visibility
                    .as_ref()
                    .map(|visibility| visibility.to_visibility(module_id, catalog_name))
                    .unwrap_or(Ok(Visibility::Public))?;
                if !matches!(visibility, Visibility::Public) {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "trait associated types require public visibility",
                    ));
                }
                associated_type
                    .into_type(visibility, owner_context, catalog_name, cancellation)
                    .map(RustAssociatedItem::Type)
            }
            Self::ForeignFunction(_)
            | Self::ForeignStatic(_)
            | Self::ForeignType(_)
            | Self::ForeignMacro(_) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "trait items must use method, associated_constant, or associated_type",
            )),
        }
    }

    pub(super) fn into_implementation_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAssociatedItem, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Method(method) => {
                if method.default_body.is_some() {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "implementation methods cannot specify default_body",
                    ));
                }
                method
                    .into_method(
                        source_file,
                        module_id,
                        RustYamlMethodContainer::Implementation,
                        owner_context,
                        catalog_name,
                        cancellation,
                    )
                    .map(|method| RustAssociatedItem::Method(Box::new(method)))
            }
            Self::AssociatedConstant(constant) => {
                if constant.default_value.is_none() {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        format!(
                            "implementation associated constant {} requires default_value",
                            constant.name
                        ),
                    ));
                }
                let visibility = constant
                    .visibility
                    .as_ref()
                    .map(|visibility| visibility.to_visibility(module_id, catalog_name))
                    .unwrap_or(Ok(Visibility::Private))?;
                constant
                    .into_constant(visibility, owner_context, catalog_name, cancellation)
                    .map(RustAssociatedItem::Constant)
            }
            Self::AssociatedType(associated_type) => {
                if associated_type.default_type.is_none() {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        format!(
                            "implementation associated type {} requires default_type",
                            associated_type.name
                        ),
                    ));
                }
                let visibility = associated_type
                    .visibility
                    .as_ref()
                    .map(|visibility| visibility.to_visibility(module_id, catalog_name))
                    .unwrap_or(Ok(Visibility::Private))?;
                associated_type
                    .into_type(visibility, owner_context, catalog_name, cancellation)
                    .map(RustAssociatedItem::Type)
            }
            Self::ForeignFunction(_)
            | Self::ForeignStatic(_)
            | Self::ForeignType(_)
            | Self::ForeignMacro(_) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "implementation items must use method, associated_constant, or associated_type",
            )),
        }
    }

    pub(super) fn into_foreign_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustForeignItem, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::ForeignFunction(function) => function
                .into_item(source_file, module_id, catalog_name, cancellation)
                .map(RustForeignItem::Function),
            Self::ForeignStatic(value) => value
                .into_item(source_file, module_id, catalog_name, cancellation)
                .map(RustForeignItem::Static),
            Self::ForeignType(value) => value
                .into_item(source_file, module_id, catalog_name, cancellation)
                .map(RustForeignItem::Type),
            Self::ForeignMacro(value) => value
                .into_item(catalog_name, cancellation)
                .map(RustForeignItem::Macro),
            Self::Method(_) | Self::AssociatedConstant(_) | Self::AssociatedType(_) => {
                Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "foreign module items must use a foreign_* signature_type",
                ))
            }
        }
    }
}

impl RustYamlAssociatedConstantInput {
    pub(super) fn into_constant(
        self,
        visibility: Visibility,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAssociatedConstant, SignatureContractKitError> {
        cancellation.checkpoint()?;
        RustAssociatedConstant::new(
            self.name,
            visibility,
            RustYamlTypeText::from_text(self.type_text).parse(context, cancellation)?,
            self.default_value
                .as_ref()
                .map(|value| RustSyntaxText::parse_expression(value))
                .transpose()
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                })?,
            self.specialization_default,
            self.attributes.to_attributes(catalog_name, cancellation)?,
        )
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

impl RustYamlAssociatedTypeInput {
    pub(super) fn into_type(
        self,
        visibility: Visibility,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAssociatedType, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            self.where_predicates.as_deref().unwrap_or_default(),
            catalog_name,
            cancellation,
        )?;
        let context = owner_context.with_metadata(&generics, cancellation)?;
        let mut bounds = Vec::with_capacity(self.bounds.len());
        for bound in &self.bounds {
            cancellation.checkpoint()?;
            bounds.push(RustSyntaxText::parse_type_bound(bound).map_err(|source| {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            })?);
        }
        RustAssociatedType::new(
            self.name,
            visibility,
            generics,
            bounds,
            self.default_type
                .map(|value| RustYamlTypeText::from_text(value).parse(&context, cancellation))
                .transpose()?,
            self.specialization_default,
            self.attributes.to_attributes(catalog_name, cancellation)?,
        )
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

impl RustYamlForeignFunctionInput {
    pub(super) fn into_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustForeignFunction, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let visibility = self
            .visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(module_id, catalog_name))
            .unwrap_or(Ok(Visibility::Private))?;
        let callable = RustYamlCallableInput {
            qualifiers: self.qualifiers,
            abi: None,
            variadic: self.variadic,
            generics: self.generics,
            where_predicates: self.where_predicates.unwrap_or_default(),
            parameters: self.parameters.unwrap_or_default(),
            return_type: self.return_type,
        }
        .into_signature(
            &RustYamlGenericContext::default(),
            catalog_name,
            cancellation,
        )?;
        RustForeignFunction::new(
            FunctionType::new(BaseType::new(
                self.name.clone(),
                visibility,
                source_file.clone(),
                module_id.clone(),
                self.attributes.to_attributes(catalog_name, cancellation)?,
            ))
            .with_callable_signature(callable),
        )
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

impl RustYamlForeignStaticInput {
    pub(super) fn into_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustForeignStatic, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let visibility = self
            .visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(module_id, catalog_name))
            .unwrap_or(Ok(Visibility::Private))?;
        RustForeignStatic::new(StaticType::new(
            BaseType::new(
                self.name,
                visibility,
                source_file.clone(),
                module_id.clone(),
                self.attributes.to_attributes(catalog_name, cancellation)?,
            ),
            self.mutable,
            RustYamlTypeText::from_text(self.type_text)
                .parse(&RustYamlGenericContext::default(), cancellation)?,
        ))
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

impl RustYamlForeignTypeInput {
    pub(super) fn into_item(
        self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustForeignType, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let visibility = self
            .visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(module_id, catalog_name))
            .unwrap_or(Ok(Visibility::Private))?;
        RustForeignType::new(BaseType::new(
            self.name,
            visibility,
            source_file.clone(),
            module_id.clone(),
            self.attributes.to_attributes(catalog_name, cancellation)?,
        ))
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

impl RustYamlForeignMacroInput {
    pub(super) fn into_item(
        self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustForeignMacro, SignatureContractKitError> {
        cancellation.checkpoint()?;
        RustForeignMacro::new(
            self.tokens,
            self.attributes.to_attributes(catalog_name, cancellation)?,
        )
        .map_err(|source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlMethodInput {
    pub(super) name: String,
    pub(super) visibility: Option<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
    pub(super) receiver: Option<String>,
    #[serde(default)]
    pub(super) receiver_attributes: RustYamlAttributesValue,
    pub(super) default_body: Option<bool>,
    #[serde(default)]
    pub(super) specialization_default: bool,
    #[serde(default)]
    pub(super) qualifiers: Vec<RustYamlCallableQualifierInput>,
    #[serde(default)]
    pub(super) abi: RustYamlFieldPresence<String>,
    pub(super) variadic: Option<RustYamlShorthandVariadicInput>,
    pub(super) generics: Option<RustYamlGenericParametersInput>,
    #[serde(rename = "where")]
    pub(super) where_predicates: Option<Vec<String>>,
    pub(super) parameters: Option<Vec<RustYamlFunctionParameterInput>>,
    pub(super) return_type: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum RustYamlMethodContainer {
    Trait,
    Implementation,
}

impl RustYamlMethodInput {
    pub(super) fn into_method(
        mut self,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        container: RustYamlMethodContainer,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustMethod, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (default_visibility, default_body) = match container {
            RustYamlMethodContainer::Trait => (Visibility::Public, false),
            RustYamlMethodContainer::Implementation => (Visibility::Private, true),
        };
        let visibility = self
            .visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(module_id, catalog_name))
            .unwrap_or(Ok(default_visibility))?;
        let callable = RustYamlCallableInput {
            qualifiers: self.qualifiers,
            abi: self.abi.take(),
            variadic: self.variadic,
            generics: self.generics,
            where_predicates: self.where_predicates.unwrap_or_default(),
            parameters: self.parameters.unwrap_or_default(),
            return_type: self.return_type,
        }
        .into_signature(owner_context, catalog_name, cancellation)?;
        let receiver_context = owner_context.with_metadata(callable.generics(), cancellation)?;
        let name = RustIdentifier::new(self.name, "method name")
            .map(|identifier| identifier.as_str().to_owned())
            .map_err(|source| {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            })?;
        let function = FunctionType::new(BaseType::new(
            name,
            visibility,
            source_file.clone(),
            module_id.clone(),
            self.attributes.to_attributes(catalog_name, cancellation)?,
        ))
        .with_callable_signature(callable);

        Ok(RustMethod::new(
            function,
            self.receiver
                .as_deref()
                .map(|receiver| {
                    Self::parse_receiver(receiver, &receiver_context, catalog_name, cancellation)
                })
                .transpose()?,
            self.default_body.unwrap_or(default_body),
            self.specialization_default,
            self.receiver_attributes
                .to_attributes(catalog_name, cancellation)?,
        ))
    }

    pub(super) fn parse_receiver(
        value: &str,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustReceiver, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let receiver = match value.trim() {
            "" | "none" | "static" => {
                return Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "static methods must omit receiver",
                ));
            }
            "ref" => "&self",
            "mut" | "ref_mut" => "&mut self",
            value => value,
        };
        let signature =
            syn::parse_str::<syn::Signature>(&format!("fn __contract_receiver({receiver})"))
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(
                        catalog_name,
                        format!("invalid Rust method receiver {receiver:?}: {source}"),
                    )
                })?;
        cancellation.checkpoint()?;
        let mut inputs = signature.inputs.into_iter();
        let Some(syn::FnArg::Receiver(receiver)) = inputs.next() else {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!("invalid Rust method receiver {receiver:?}"),
            ));
        };
        if inputs.next().is_some() || !receiver.attrs.is_empty() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "receiver must contain exactly one receiver form; use receiver_attributes for attributes",
            ));
        }

        let receiver_mutable = receiver.mutability.is_some();
        #[allow(unreachable_patterns)]
        match receiver.kind {
            syn::ReceiverKind::Value => Ok(RustReceiver::value(receiver_mutable)),
            syn::ReceiverKind::Reference(_, lifetime, mutability) => Ok(RustReceiver::reference(
                lifetime.map(|lifetime| lifetime.to_token_stream().to_string()),
                mutability.is_some(),
            )),
            syn::ReceiverKind::Typed(_, receiver_type) => {
                let receiver_type =
                    RustYamlTypeText::from_text(receiver_type.to_token_stream().to_string())
                        .parse(context, cancellation)
                        .map_err(|source| {
                            if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                                source
                            } else {
                                SignatureContractKitError::parse_failed(
                                    catalog_name,
                                    format!("invalid typed Rust method receiver: {source}"),
                                )
                            }
                        })?;
                Ok(RustReceiver::typed(receiver_mutable, receiver_type))
            }
            _ => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "unsupported future Rust method receiver",
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlShorthandImplementationInput {
    #[serde(rename = "trait", default)]
    pub(super) implemented_trait: RustYamlFieldPresence<Option<String>>,
    #[serde(default)]
    pub(super) impl_qualifiers:
        RustYamlFieldPresence<Option<Vec<RustYamlImplementationQualifierInput>>>,
    #[serde(default)]
    pub(super) generics: RustYamlFieldPresence<Option<RustYamlGenericParametersInput>>,
    #[serde(rename = "where", default)]
    pub(super) where_predicates: RustYamlFieldPresence<Option<Vec<String>>>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
    #[serde(default)]
    pub(super) items: Vec<RustYamlNestedItemInput>,
}

impl RustYamlShorthandImplementationInput {
    pub(super) fn into_implementation(
        self,
        owner: RustImplementationOwner,
        source_file: &CatalogPath,
        module_id: &RustModuleId,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<ImplementationType, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let qualifiers = self.implementation_qualifiers(catalog_name)?;
        let is_default = qualifiers.contains(&RustYamlImplementationQualifierInput::Default);
        let is_unsafe = qualifiers.contains(&RustYamlImplementationQualifierInput::Unsafe);
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.implementation_generics(catalog_name)?,
            self.implementation_where_predicates(catalog_name)?,
            catalog_name,
            cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, cancellation)?;
        let implemented_trait = self.implemented_trait(catalog_name)?;
        let attributes = self.attributes.to_attributes(catalog_name, cancellation)?;
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            cancellation.checkpoint()?;
            items.push(item.into_implementation_item(
                source_file,
                module_id,
                &context,
                catalog_name,
                cancellation,
            )?);
        }
        let mut implementation = ImplementationType::new(owner)
            .with_implemented_trait(implemented_trait)
            .with_qualifiers(is_default, is_unsafe)
            .with_generic_metadata(generics)
            .with_attributes(attributes)
            .with_items(items);
        cancellation.checkpoint()?;
        implementation.sort_associated_items(cancellation)?;
        cancellation.checkpoint()?;
        Ok(implementation)
    }

    pub(super) fn implemented_trait(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustImplementedTrait, SignatureContractKitError> {
        let value = match self.implemented_trait.as_ref() {
            None => None,
            Some(None) => {
                return Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "Rust YAML trait must not be null",
                ));
            }
            Some(Some(value)) if value.trim().is_empty() => {
                return Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "Rust YAML trait must not be empty",
                ));
            }
            Some(Some(value)) => Some(value.as_str()),
        };
        let Some(value) = value else {
            return Ok(RustImplementedTrait::Inherent);
        };
        let (polarity, path) = value
            .strip_prefix('!')
            .map(|path| (RustImplPolarity::Negative, path))
            .unwrap_or((RustImplPolarity::Positive, value));
        RustImplementedTrait::for_trait(path.to_owned(), polarity).map_err(|source| {
            SignatureContractKitError::parse_failed(catalog_name, source.to_string())
        })
    }

    pub(super) fn implementation_qualifiers(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<&[RustYamlImplementationQualifierInput], SignatureContractKitError> {
        match self.impl_qualifiers.as_ref() {
            None => Ok(&[]),
            Some(Some(value)) => Ok(value),
            Some(None) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML impl_qualifiers must be a list",
            )),
        }
    }

    pub(super) fn implementation_generics(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Option<&RustYamlGenericParametersInput>, SignatureContractKitError> {
        match self.generics.as_ref() {
            None => Ok(None),
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML generics must not be null",
            )),
        }
    }

    pub(super) fn implementation_where_predicates(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<&[String], SignatureContractKitError> {
        match self.where_predicates.as_ref() {
            None => Ok(&[]),
            Some(Some(value)) => Ok(value),
            Some(None) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML where must not be null",
            )),
        }
    }
}

pub(super) struct RustYamlFunctionParameterInput {
    pub(super) pattern: Option<String>,
    pub(super) type_text: String,
    pub(super) attributes: RustYamlAttributesValue,
}

impl<'de> serde::Deserialize<'de> for RustYamlFunctionParameterInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ParameterVisitor;

        impl<'de> de::Visitor<'de> for ParameterVisitor {
            type Value = RustYamlFunctionParameterInput;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a parameter mapping with explicit pattern and type fields")
            }

            fn visit_map<M>(self, mut mapping: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut pattern = None;
                let mut type_text = None;
                let mut attributes = None;
                let mut pattern_seen = false;
                while let Some(field) = mapping.next_key::<String>()? {
                    match field.as_str() {
                        "pattern" if !pattern_seen => {
                            pattern_seen = true;
                            pattern = mapping.next_value()?;
                        }
                        "type" if type_text.is_none() => type_text = Some(mapping.next_value()?),
                        "attributes" if attributes.is_none() => {
                            attributes = Some(mapping.next_value()?)
                        }
                        "pattern" | "type" | "attributes" => {
                            return Err(de::Error::custom(format!(
                                "parameter field {field} is duplicated"
                            )));
                        }
                        _ => {
                            return Err(de::Error::custom(format!(
                                "parameters must use explicit pattern and type fields; unknown field {field}"
                            )));
                        }
                    }
                }

                Ok(RustYamlFunctionParameterInput {
                    pattern,
                    type_text: type_text.ok_or_else(|| {
                        de::Error::custom(
                            "parameters must use explicit pattern and type fields; missing field type",
                        )
                    })?,
                    attributes: attributes.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_map(ParameterVisitor)
    }
}

impl RustYamlFunctionParameterInput {
    pub(super) fn to_parameter(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustFunctionParameter, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if self
            .pattern
            .as_ref()
            .is_some_and(|pattern| pattern.trim().is_empty())
        {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML parameter pattern must not be empty",
            ));
        }
        Ok(RustFunctionParameter::new(
            self.pattern
                .as_deref()
                .map(RustSyntaxText::parse_pattern)
                .transpose()
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                })?,
            RustYamlTypeText::from_text(self.type_text.clone()).parse(context, cancellation)?,
        )
        .with_attributes(self.attributes.to_attributes(catalog_name, cancellation)?))
    }
}
