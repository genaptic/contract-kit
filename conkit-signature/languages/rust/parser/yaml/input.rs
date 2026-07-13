use super::type_text::{RustYamlGenericContext, RustYamlTypeText};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::inventory::SignatureInventory;
use crate::languages::rust::parser::signature_id::{
    RustImplementationId, RustItemId, RustItemIdAllocator, RustItemKind,
};
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
use serde::de;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) struct RustYamlDocument {
    pub(super) root: String,
    pub(super) files: Vec<CatalogPath>,
    pub(super) signatures: Vec<RustYamlNamedSignature>,
    pub(super) sketches: Vec<RustYamlSketch>,
}

impl RustYamlDocument {
    pub(super) fn parse(
        catalog_name: &CatalogPath,
        bytes: &[u8],
    ) -> Result<Self, SignatureContractKitError> {
        RustYamlDocumentInput::parse(catalog_name, bytes)?.into_document(catalog_name)
    }

    pub(super) fn into_inventory(
        self,
        catalog_name: &CatalogPath,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        let mut inventory = SignatureInventory::default();
        let document_context = RustYamlDocumentContext::new(self.root, self.files);

        for signature in self.signatures {
            if signature.label.trim().is_empty() {
                return Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "signature label must not be empty",
                ));
            }
            let group_id = crate::inventory::SignatureId::new(signature.label.clone());
            let context = document_context.signature_bytes(&signature)?;
            for entry in signature.entries {
                inventory.insert(entry.into_signature_entry(group_id.clone())?)?;
            }
            inventory.set_group_context(group_id, context)?;
        }

        Ok(inventory)
    }
}

#[derive(Serialize)]
struct RustYamlDocumentContext {
    root: String,
    files: Vec<CatalogPath>,
}

impl RustYamlDocumentContext {
    fn new(root: String, files: Vec<CatalogPath>) -> Self {
        Self { root, files }
    }

    fn signature_bytes(
        &self,
        signature: &RustYamlNamedSignature,
    ) -> Result<Vec<u8>, SignatureContractKitError> {
        serde_json::to_vec(&(self, &signature.sketch, signature.signature_type.as_str()))
            .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlDocumentInput {
    root: String,
    files: Vec<RustYamlShorthandCatalogPath>,
    signatures: Vec<RustYamlShorthandEntry>,
    #[serde(default)]
    sketches: Vec<RustYamlSketch>,
}

impl RustYamlDocumentInput {
    fn parse(catalog_name: &CatalogPath, bytes: &[u8]) -> Result<Self, SignatureContractKitError> {
        serde_yaml::from_slice::<Self>(bytes).map_err(|source| {
            SignatureContractKitError::parse_failed(catalog_name, source.to_string())
        })
    }

    fn into_document(
        self,
        catalog_name: &CatalogPath,
    ) -> Result<RustYamlDocument, SignatureContractKitError> {
        let files = self
            .files
            .into_iter()
            .map(|file| file.to_catalog_path())
            .collect::<Vec<_>>();
        let mut signatures = Vec::new();
        let mut item_ids = RustItemIdAllocator::default();

        for signature in self.signatures {
            signatures.push(signature.into_named(catalog_name, &mut item_ids)?);
        }
        RustYamlDocumentValidator::new(catalog_name, &files, &signatures, &self.sketches)
            .validate()?;

        Ok(RustYamlDocument {
            root: self.root,
            files,
            signatures,
            sketches: self.sketches,
        })
    }
}

struct RustYamlShorthandEntry(BTreeMap<String, RustYamlShorthandSignature>);

impl<'de> Deserialize<'de> for RustYamlShorthandEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let is_long_form = value.as_mapping().is_some_and(|mapping| {
            mapping.contains_key(serde_yaml::Value::String("id".to_owned()))
                && mapping.contains_key(serde_yaml::Value::String("signature".to_owned()))
        });

        if is_long_form {
            return Err(de::Error::custom(
                "Rust YAML longhand signature entries are internal only; use shorthand signature_type entries",
            ));
        }

        BTreeMap::<String, RustYamlShorthandSignature>::deserialize(value)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

impl RustYamlShorthandEntry {
    fn into_named(
        self,
        catalog_name: &CatalogPath,
        item_ids: &mut RustItemIdAllocator,
    ) -> Result<RustYamlNamedSignature, SignatureContractKitError> {
        if self.0.len() != 1 {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML shorthand signature entries must contain exactly one named entry",
            ));
        }

        let Some((label, signature)) = self.0.into_iter().next() else {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML shorthand signature entry is empty",
            ));
        };

        signature.into_named(label, catalog_name, item_ids)
    }
}

struct RustYamlShorthandCatalogPath {
    value: CatalogPath,
}

impl RustYamlShorthandCatalogPath {
    fn to_catalog_path(&self) -> CatalogPath {
        self.value.clone()
    }
}

impl<'de> Deserialize<'de> for RustYamlShorthandCatalogPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let value = value
            .as_str()
            .ok_or_else(|| de::Error::custom("Rust YAML catalog paths must use shorthand text"))?;
        let path = CatalogPath::new(value.to_owned()).map_err(de::Error::custom)?;

        Ok(Self { value: path })
    }
}

struct RustYamlShorthandSignature {
    common: RustYamlSignatureCommonInput,
    body: RustYamlSignatureBodyInput,
}

struct RustYamlSignatureCommonInput {
    file: RustYamlShorthandCatalogPath,
    module_path: Vec<String>,
    name: Option<String>,
    visibility: Option<RustYamlVisibilityInput>,
    derives: Vec<String>,
    sketch: Option<String>,
}

enum RustYamlSignatureBodyInput {
    MainMethod(RustYamlCallableInput),
    Function(RustYamlCallableInput),
    Struct(RustYamlStructInput),
    Enum(RustYamlEnumInput),
    Trait(RustYamlTraitInput),
    Union(RustYamlUnionInput),
    Static(RustYamlStaticInput),
    Macro(RustYamlMacroInput),
    TypeAlias(RustYamlTypeAliasInput),
}

struct RustYamlCallableInput {
    qualifiers: Vec<RustYamlCallableQualifierInput>,
    abi: Option<String>,
    variadic: Option<RustYamlShorthandVariadicInput>,
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    parameters: Vec<RustYamlFunctionParameterInput>,
    return_type: Option<String>,
}

struct RustYamlStructInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    fields: Option<RustYamlShorthandFields>,
    methods: Vec<RustYamlShorthandMethodEntry>,
    implementations: Vec<RustYamlShorthandImplementationInput>,
}

struct RustYamlEnumInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    variants: Option<Vec<RustYamlShorthandVariant>>,
    methods: Vec<RustYamlShorthandMethodEntry>,
    implementations: Vec<RustYamlShorthandImplementationInput>,
}

struct RustYamlTraitInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    methods: Vec<RustYamlShorthandMethodEntry>,
    supertraits: Vec<String>,
}

struct RustYamlUnionInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    fields: Option<RustYamlShorthandFields>,
    methods: Vec<RustYamlShorthandMethodEntry>,
    implementations: Vec<RustYamlShorthandImplementationInput>,
}

struct RustYamlStaticInput {
    type_text: String,
    mutable: bool,
}

struct RustYamlMacroInput {
    tokens: Option<String>,
}

struct RustYamlTypeAliasInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    target_type: String,
    methods: Vec<RustYamlShorthandMethodEntry>,
    implementations: Vec<RustYamlShorthandImplementationInput>,
}

#[derive(Default)]
enum RustYamlFieldPresence<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T> Deserialize<'de> for RustYamlFieldPresence<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

impl<T> RustYamlFieldPresence<T> {
    fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }

    fn take(&mut self) -> Option<T> {
        match std::mem::take(self) {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlRawSignature {
    file: RustYamlShorthandCatalogPath,
    module_path: Option<Vec<String>>,
    #[serde(rename = "signature_type")]
    signature_type: RustYamlSignatureType,
    name: Option<String>,
    #[serde(default)]
    visibility: RustYamlFieldPresence<RustYamlVisibilityInput>,
    derives: Option<Vec<String>>,
    #[serde(default)]
    qualifiers: RustYamlFieldPresence<Vec<RustYamlCallableQualifierInput>>,
    #[serde(default)]
    abi: RustYamlFieldPresence<String>,
    #[serde(default)]
    variadic: RustYamlFieldPresence<RustYamlShorthandVariadicInput>,
    #[serde(default)]
    generics: RustYamlFieldPresence<RustYamlGenericParametersInput>,
    #[serde(rename = "where", default)]
    where_predicates: RustYamlFieldPresence<Vec<String>>,
    #[serde(default)]
    fields: RustYamlFieldPresence<RustYamlShorthandFields>,
    #[serde(default)]
    variants: RustYamlFieldPresence<Vec<RustYamlShorthandVariant>>,
    #[serde(default)]
    methods: RustYamlFieldPresence<Vec<RustYamlShorthandMethodEntry>>,
    #[serde(default)]
    implementations: RustYamlFieldPresence<Vec<RustYamlShorthandImplementationInput>>,
    #[serde(default)]
    parameters: RustYamlFieldPresence<Vec<RustYamlFunctionParameterInput>>,
    #[serde(default)]
    return_type: RustYamlFieldPresence<String>,
    #[serde(default)]
    supertraits: RustYamlFieldPresence<Vec<String>>,
    #[serde(rename = "type", default)]
    type_text: RustYamlFieldPresence<String>,
    #[serde(default)]
    target_type: RustYamlFieldPresence<String>,
    #[serde(default)]
    mutable: RustYamlFieldPresence<bool>,
    #[serde(default)]
    tokens: RustYamlFieldPresence<String>,
    sketch: Option<String>,
}

impl<'de> Deserialize<'de> for RustYamlShorthandSignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        RustYamlRawSignature::deserialize(deserializer)?
            .into_typed()
            .map_err(de::Error::custom)
    }
}

impl RustYamlShorthandSignature {
    fn into_named(
        self,
        label: String,
        catalog_name: &CatalogPath,
        item_ids: &mut RustItemIdAllocator,
    ) -> Result<RustYamlNamedSignature, SignatureContractKitError> {
        let Self { common, body } = self;
        let file = common.file.to_catalog_path();
        let signature_type = body.signature_type();
        let sketch = common.sketch.clone();
        let entries = body.into_entries(&common, label.clone(), catalog_name, item_ids)?;

        Ok(RustYamlNamedSignature {
            label,
            file,
            signature_type,
            sketch,
            entries,
        })
    }
}

impl RustYamlSignatureBodyInput {
    fn signature_type(&self) -> RustYamlSignatureType {
        match self {
            Self::MainMethod(_) => RustYamlSignatureType::MainMethod,
            Self::Function(_) => RustYamlSignatureType::Function,
            Self::Struct(_) => RustYamlSignatureType::Struct,
            Self::Enum(_) => RustYamlSignatureType::Enum,
            Self::Trait(_) => RustYamlSignatureType::Trait,
            Self::Union(_) => RustYamlSignatureType::Union,
            Self::Static(_) => RustYamlSignatureType::Static,
            Self::Macro(_) => RustYamlSignatureType::Macro,
            Self::TypeAlias(_) => RustYamlSignatureType::TypeAlias,
        }
    }

    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
        item_ids: &mut RustItemIdAllocator,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        match self {
            Self::MainMethod(value) => {
                if common.name.as_deref().is_some_and(|name| name != "main") {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "main_method name must be main when explicitly provided",
                    ));
                }
                Ok(vec![value.into_entry(
                    common,
                    "main".to_owned(),
                    catalog_name,
                )?])
            }
            Self::Function(value) => Ok(vec![value.into_entry(common, label, catalog_name)?]),
            Self::Struct(value) => value.into_entries(common, label, catalog_name),
            Self::Enum(value) => value.into_entries(common, label, catalog_name),
            Self::Trait(value) => value.into_entries(common, label, catalog_name),
            Self::Union(value) => value.into_entries(common, label, catalog_name),
            Self::Static(value) => Ok(vec![value.into_entry(common, label, catalog_name)?]),
            Self::Macro(value) => Ok(vec![value.into_entry(
                common,
                label,
                catalog_name,
                item_ids,
            )?]),
            Self::TypeAlias(value) => value.into_entries(common, label, catalog_name),
        }
    }
}

impl RustYamlSignatureCommonInput {
    fn item_name(&self, label: String) -> String {
        self.name.clone().unwrap_or(label)
    }

    fn visibility(
        &self,
        default: Visibility,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        self.visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(catalog_name))
            .unwrap_or(Ok(default))
    }

    fn base(
        &self,
        name: String,
        default_visibility: Visibility,
        catalog_name: &CatalogPath,
    ) -> Result<BaseType, SignatureContractKitError> {
        Ok(BaseType::new(
            name,
            self.visibility(default_visibility, catalog_name)?,
            self.file.to_catalog_path(),
        )
        .with_module_path(self.module_path.clone())
        .with_derives(self.derives.clone()))
    }

    fn entry(&self, kind: RustItemKind, name: String, signature: RustSignature) -> RustParsedEntry {
        RustParsedEntry::new(self.id(kind, name), signature)
    }

    fn id(&self, kind: RustItemKind, name: String) -> RustItemId {
        RustItemId::new(
            self.file.to_catalog_path(),
            self.module_path.clone(),
            kind,
            name,
        )
    }

    fn entries_with_implementations(
        &self,
        item_entry: RustParsedEntry,
        owner_type: String,
        methods: &[RustYamlShorthandMethodEntry],
        implementations: &[RustYamlShorthandImplementationInput],
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        let mut grouped = BTreeMap::<Vec<u8>, (ImplementationType, Vec<RustMethod>)>::new();
        let source_file = self.file.to_catalog_path();
        for method in methods {
            let implementation = method.implementation(&owner_type, catalog_name)?;
            let key = implementation.descriptor_bytes().map_err(|source| {
                SignatureContractKitError::conversion_failed(source.to_string())
            })?;
            let context = RustYamlGenericContext::from_metadata(implementation.generics());
            let converted = method.to_method(
                &source_file,
                self.module_path.clone(),
                Visibility::Private,
                &context,
                catalog_name,
            )?;
            grouped
                .entry(key)
                .or_insert_with(|| (implementation, Vec::new()))
                .1
                .push(converted);
        }
        for marker in implementations {
            let implementation = marker.implementation(&owner_type, catalog_name)?;
            let key = implementation.descriptor_bytes().map_err(|source| {
                SignatureContractKitError::conversion_failed(source.to_string())
            })?;
            if grouped.contains_key(&key) {
                return Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "methodless implementation duplicates a method implementation descriptor",
                ));
            }
            grouped.insert(key, (implementation, Vec::new()));
        }

        let mut entries = vec![item_entry];
        let mut implementation_entries = Vec::with_capacity(grouped.len());
        for (descriptor, (implementation, methods)) in grouped {
            let mut implementation = implementation.with_methods(methods);
            implementation.sort_methods().map_err(|source| {
                SignatureContractKitError::conversion_failed(source.to_string())
            })?;
            let implementation_id = match implementation.implemented_trait() {
                RustImplementedTrait::Inherent => {
                    RustImplementationId::inherent(owner_type.clone())
                }
                RustImplementedTrait::Trait { name, polarity } => {
                    RustImplementationId::trait_impl(owner_type.clone(), name.clone(), *polarity)
                }
            };
            let base_name = implementation_id.render();
            implementation_entries.push((
                descriptor,
                base_name.clone(),
                self.entry(
                    RustItemKind::Implementation,
                    base_name,
                    RustSignature::Implementation(implementation),
                ),
            ));
        }
        let mut collisions = BTreeMap::new();
        for (_, base_name, _) in &implementation_entries {
            *collisions.entry(base_name.clone()).or_insert(0_usize) += 1;
        }
        for (descriptor, base_name, mut entry) in implementation_entries {
            if collisions.get(&base_name).copied().unwrap_or_default() > 1 {
                entry.disambiguate_implementation(&descriptor);
            }
            entries.push(entry);
        }

        Ok(entries)
    }
}

impl RustYamlCallableInput {
    fn into_entry(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = common.item_name(label);
        let signature = RustSignature::Function(
            FunctionType::new(common.base(name.clone(), Visibility::Private, catalog_name)?)
                .with_callable_signature(
                    self.to_signature(&RustYamlGenericContext::default(), catalog_name)?,
                ),
        );

        Ok(common.entry(RustItemKind::Function, name, signature))
    }

    fn to_signature(
        &self,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<RustCallableSignature, SignatureContractKitError> {
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = owner_context.with_metadata(&generics);
        let qualifiers = RustYamlCallableQualifiers::from_inputs(&self.qualifiers);
        let parameters = self
            .parameters
            .iter()
            .map(|parameter| parameter.to_parameter(&context, catalog_name))
            .collect::<Result<Vec<_>, _>>()?;
        let return_type = self
            .return_type
            .as_ref()
            .map(|value| RustYamlTypeText::from_text(value.clone()).parse(&context))
            .transpose()?;

        Ok(RustCallableSignature::builder()
            .with_const(qualifiers.is_const)
            .with_async(qualifiers.is_async)
            .with_unsafe(qualifiers.is_unsafe)
            .with_abi(RustYamlFunctionAbiInput::new(self.abi.as_deref()).to_abi(catalog_name)?)
            .with_variadic(
                self.variadic
                    .as_ref()
                    .map(|variadic| variadic.to_variadic(catalog_name))
                    .transpose()?
                    .flatten(),
            )
            .with_generics(generics)
            .with_parameters(parameters)
            .with_return_type(return_type)
            .build())
    }
}

impl RustYamlStructInput {
    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        let name = common.item_name(label);
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = RustYamlGenericContext::from_metadata(&generics);
        let fields = self
            .fields
            .as_ref()
            .map(|fields| fields.to_struct_fields(&context, catalog_name))
            .unwrap_or_else(|| Ok(Vec::new()))?;
        let entry = common.entry(
            RustItemKind::Struct,
            name.clone(),
            RustSignature::Struct(
                StructType::new(common.base(name.clone(), Visibility::Private, catalog_name)?)
                    .with_generic_metadata(generics)
                    .with_fields(fields),
            ),
        );

        common.entries_with_implementations(
            entry,
            name,
            &self.methods,
            &self.implementations,
            catalog_name,
        )
    }
}

impl RustYamlEnumInput {
    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        let name = common.item_name(label);
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = RustYamlGenericContext::from_metadata(&generics);
        let variants = self
            .variants
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|variant| variant.to_variant(&context, catalog_name))
            .collect::<Result<Vec<_>, _>>()?;
        let entry = common.entry(
            RustItemKind::Enum,
            name.clone(),
            RustSignature::Enum(
                EnumType::new(common.base(name.clone(), Visibility::Private, catalog_name)?)
                    .with_generic_metadata(generics)
                    .with_variants(variants),
            ),
        );

        common.entries_with_implementations(
            entry,
            name,
            &self.methods,
            &self.implementations,
            catalog_name,
        )
    }
}

impl RustYamlTraitInput {
    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        if self.methods.iter().any(|method| {
            method.implemented_trait.is_present()
                || method.impl_qualifiers.is_present()
                || method.impl_generics.is_present()
                || method.impl_where_predicates.is_present()
        }) {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "trait declaration methods cannot carry implementation fields",
            ));
        }
        let name = common.item_name(label);
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = RustYamlGenericContext::from_metadata(&generics);
        let source_file = common.file.to_catalog_path();
        let methods = self
            .methods
            .iter()
            .map(|method| {
                method.to_method(
                    &source_file,
                    common.module_path.clone(),
                    Visibility::Public,
                    &context,
                    catalog_name,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry = common.entry(
            RustItemKind::Trait,
            name.clone(),
            RustSignature::Trait(
                TraitType::new(common.base(name, Visibility::Public, catalog_name)?)
                    .with_generic_metadata(generics)
                    .with_supertraits(self.supertraits)
                    .with_methods(methods),
            ),
        );

        Ok(vec![entry])
    }
}

impl RustYamlUnionInput {
    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        let name = common.item_name(label);
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = RustYamlGenericContext::from_metadata(&generics);
        let fields = self
            .fields
            .as_ref()
            .map(|fields| fields.to_struct_fields(&context, catalog_name))
            .unwrap_or_else(|| Ok(Vec::new()))?;
        let entry = common.entry(
            RustItemKind::Union,
            name.clone(),
            RustSignature::Union(
                UnionType::new(common.base(name.clone(), Visibility::Private, catalog_name)?)
                    .with_generic_metadata(generics)
                    .with_fields(fields),
            ),
        );

        common.entries_with_implementations(
            entry,
            name,
            &self.methods,
            &self.implementations,
            catalog_name,
        )
    }
}

impl RustYamlStaticInput {
    fn into_entry(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = common.item_name(label);
        Ok(common.entry(
            RustItemKind::Static,
            name.clone(),
            RustSignature::Static(StaticType::new(
                common.base(name, Visibility::Private, catalog_name)?,
                self.mutable,
                RustYamlTypeText::from_text(self.type_text)
                    .parse(&RustYamlGenericContext::default())?,
            )),
        ))
    }
}

impl RustYamlMacroInput {
    fn into_entry(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
        item_ids: &mut RustItemIdAllocator,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let name = common.item_name(label);
        let id = common.id(RustItemKind::Macro, name.clone());
        let signature = RustSignature::Macro(MacroType::new(
            common.base(name, Visibility::Private, catalog_name)?,
            self.tokens.unwrap_or_default(),
        ));
        Ok(RustParsedEntry::new(item_ids.allocate(id)?, signature))
    }
}

impl RustYamlTypeAliasInput {
    fn into_entries(
        self,
        common: &RustYamlSignatureCommonInput,
        label: String,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        let name = common.item_name(label);
        let generics =
            RustYamlShorthandGenericsInput::new(self.generics.as_ref(), &self.where_predicates)
                .to_metadata(catalog_name)?;
        let context = RustYamlGenericContext::from_metadata(&generics);
        let entry = common.entry(
            RustItemKind::TypeAlias,
            name.clone(),
            RustSignature::TypeAlias(TypeAliasType::new(
                common.base(name.clone(), Visibility::Private, catalog_name)?,
                generics,
                RustYamlTypeText::from_text(self.target_type).parse(&context)?,
            )),
        );

        common.entries_with_implementations(
            entry,
            name,
            &self.methods,
            &self.implementations,
            catalog_name,
        )
    }
}

impl RustYamlRawSignature {
    fn into_typed(mut self) -> Result<RustYamlShorthandSignature, RustYamlShapeError> {
        self.validate_shape()?;
        let body = match self.signature_type {
            RustYamlSignatureType::MainMethod => {
                RustYamlSignatureBodyInput::MainMethod(self.take_callable())
            }
            RustYamlSignatureType::Function => {
                RustYamlSignatureBodyInput::Function(self.take_callable())
            }
            RustYamlSignatureType::Struct => {
                RustYamlSignatureBodyInput::Struct(RustYamlStructInput {
                    generics: self.generics.take(),
                    where_predicates: self.where_predicates.take().unwrap_or_default(),
                    fields: self.fields.take(),
                    methods: self.methods.take().unwrap_or_default(),
                    implementations: self.implementations.take().unwrap_or_default(),
                })
            }
            RustYamlSignatureType::Enum => RustYamlSignatureBodyInput::Enum(RustYamlEnumInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                variants: self.variants.take(),
                methods: self.methods.take().unwrap_or_default(),
                implementations: self.implementations.take().unwrap_or_default(),
            }),
            RustYamlSignatureType::Trait => RustYamlSignatureBodyInput::Trait(RustYamlTraitInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                methods: self.methods.take().unwrap_or_default(),
                supertraits: self.supertraits.take().unwrap_or_default(),
            }),
            RustYamlSignatureType::Union => RustYamlSignatureBodyInput::Union(RustYamlUnionInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                fields: self.fields.take(),
                methods: self.methods.take().unwrap_or_default(),
                implementations: self.implementations.take().unwrap_or_default(),
            }),
            RustYamlSignatureType::Static => {
                RustYamlSignatureBodyInput::Static(RustYamlStaticInput {
                    type_text: self
                        .type_text
                        .take()
                        .ok_or(RustYamlShapeError::MissingField {
                            signature_type: "static",
                            field: "type",
                        })?,
                    mutable: self.mutable.take().unwrap_or(false),
                })
            }
            RustYamlSignatureType::Macro => RustYamlSignatureBodyInput::Macro(RustYamlMacroInput {
                tokens: self.tokens.take(),
            }),
            RustYamlSignatureType::TypeAlias => {
                RustYamlSignatureBodyInput::TypeAlias(RustYamlTypeAliasInput {
                    generics: self.generics.take(),
                    where_predicates: self.where_predicates.take().unwrap_or_default(),
                    target_type: self.target_type.take().ok_or(
                        RustYamlShapeError::MissingField {
                            signature_type: "type_alias",
                            field: "target_type",
                        },
                    )?,
                    methods: self.methods.take().unwrap_or_default(),
                    implementations: self.implementations.take().unwrap_or_default(),
                })
            }
        };
        Ok(RustYamlShorthandSignature {
            common: RustYamlSignatureCommonInput {
                file: self.file,
                module_path: self.module_path.unwrap_or_default(),
                name: self.name,
                visibility: self.visibility.take(),
                derives: self.derives.unwrap_or_default(),
                sketch: self.sketch,
            },
            body,
        })
    }

    fn take_callable(&mut self) -> RustYamlCallableInput {
        RustYamlCallableInput {
            qualifiers: self.qualifiers.take().unwrap_or_default(),
            abi: self.abi.take(),
            variadic: self.variadic.take(),
            generics: self.generics.take(),
            where_predicates: self.where_predicates.take().unwrap_or_default(),
            parameters: self.parameters.take().unwrap_or_default(),
            return_type: self.return_type.take(),
        }
    }

    fn validate_shape(&self) -> Result<(), RustYamlShapeError> {
        match self.signature_type {
            RustYamlSignatureType::MainMethod => {
                self.reject_present(self.visibility.is_present(), "visibility")?;
                self.reject_non_callable_fields()?;
            }
            RustYamlSignatureType::Function => self.reject_non_callable_fields()?,
            RustYamlSignatureType::Struct => {
                self.reject_callable_fields()?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Enum => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Trait => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.implementations.is_present(), "implementations")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Union => {
                self.reject_callable_fields()?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Static => {
                self.reject_non_simple_fields()?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Macro => {
                self.reject_non_simple_fields()?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
            }
            RustYamlSignatureType::TypeAlias => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
        }
        Ok(())
    }

    fn reject_non_callable_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_present(self.fields.is_present(), "fields")?;
        self.reject_present(self.variants.is_present(), "variants")?;
        self.reject_present(self.methods.is_present(), "methods")?;
        self.reject_present(self.implementations.is_present(), "implementations")?;
        self.reject_present(self.supertraits.is_present(), "supertraits")?;
        self.reject_present(self.type_text.is_present(), "type")?;
        self.reject_present(self.target_type.is_present(), "target_type")?;
        self.reject_present(self.mutable.is_present(), "mutable")?;
        self.reject_present(self.tokens.is_present(), "tokens")
    }

    fn reject_callable_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_present(self.qualifiers.is_present(), "qualifiers")?;
        self.reject_present(self.abi.is_present(), "abi")?;
        self.reject_present(self.variadic.is_present(), "variadic")?;
        self.reject_present(self.parameters.is_present(), "parameters")?;
        self.reject_present(self.return_type.is_present(), "return_type")
    }

    fn reject_non_simple_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_callable_fields()?;
        self.reject_present(self.generics.is_present(), "generics")?;
        self.reject_present(self.where_predicates.is_present(), "where")?;
        self.reject_present(self.fields.is_present(), "fields")?;
        self.reject_present(self.variants.is_present(), "variants")?;
        self.reject_present(self.methods.is_present(), "methods")?;
        self.reject_present(self.implementations.is_present(), "implementations")?;
        self.reject_present(self.supertraits.is_present(), "supertraits")
    }

    fn reject_present(&self, present: bool, field: &'static str) -> Result<(), RustYamlShapeError> {
        if !present {
            return Ok(());
        }
        Err(RustYamlShapeError::FieldNotAllowed {
            signature_type: self.signature_type.as_str(),
            field,
        })
    }
}

enum RustYamlShapeError {
    FieldNotAllowed {
        signature_type: &'static str,
        field: &'static str,
    },
    MissingField {
        signature_type: &'static str,
        field: &'static str,
    },
}

impl std::fmt::Display for RustYamlShapeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FieldNotAllowed {
                signature_type,
                field,
            } => write!(
                formatter,
                "field {field} is not allowed for signature_type {signature_type}"
            ),
            Self::MissingField {
                signature_type,
                field,
            } => write!(
                formatter,
                "signature_type {signature_type} requires field {field}"
            ),
        }
    }
}

pub(super) struct RustYamlNamedSignature {
    pub(super) label: String,
    pub(super) file: CatalogPath,
    pub(super) signature_type: RustYamlSignatureType,
    pub(super) sketch: Option<String>,
    pub(super) entries: Vec<RustParsedEntry>,
}

pub(super) struct RustYamlSketch {
    pub(super) id: String,
    pub(super) signature_type: String,
    pub(super) code: String,
}

impl<'de> Deserialize<'de> for RustYamlSketch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let mapping = value
            .as_mapping()
            .ok_or_else(|| de::Error::custom("sketch entry must be a mapping"))?;
        let mut id = None;
        let mut signature_type = None;
        let mut code = None;

        for (key, value) in mapping {
            let key = key
                .as_str()
                .ok_or_else(|| de::Error::custom("sketch keys must be strings"))?;
            match key {
                "signature_type" => {
                    signature_type = value.as_str().map(str::to_owned);
                    if signature_type.is_none() {
                        return Err(de::Error::custom("sketch signature_type must be a string"));
                    }
                }
                "code" => {
                    code = value.as_str().map(str::to_owned);
                    if code.is_none() {
                        return Err(de::Error::custom("sketch code must be a string"));
                    }
                }
                candidate if value.is_null() && id.is_none() => id = Some(candidate.to_owned()),
                candidate if value.is_null() => {
                    return Err(de::Error::custom(format!(
                        "sketch entry has more than one identifier key, including {candidate}"
                    )));
                }
                unknown => {
                    return Err(de::Error::custom(format!("unknown sketch field {unknown}")));
                }
            }
        }

        Ok(Self {
            id: id.ok_or_else(|| de::Error::custom("sketch entry is missing its identifier"))?,
            signature_type: signature_type
                .ok_or_else(|| de::Error::custom("sketch entry is missing signature_type"))?,
            code: code.ok_or_else(|| de::Error::custom("sketch entry is missing code"))?,
        })
    }
}

impl Serialize for RustYamlSketch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut mapping = serializer.serialize_map(Some(3))?;
        mapping.serialize_entry(&self.id, &Option::<()>::None)?;
        mapping.serialize_entry("signature_type", &self.signature_type)?;
        mapping.serialize_entry("code", &self.code)?;
        mapping.end()
    }
}

struct RustYamlDocumentValidator<'a> {
    catalog_name: &'a CatalogPath,
    files: &'a [CatalogPath],
    signatures: &'a [RustYamlNamedSignature],
    sketches: &'a [RustYamlSketch],
}

impl<'a> RustYamlDocumentValidator<'a> {
    fn new(
        catalog_name: &'a CatalogPath,
        files: &'a [CatalogPath],
        signatures: &'a [RustYamlNamedSignature],
        sketches: &'a [RustYamlSketch],
    ) -> Self {
        Self {
            catalog_name,
            files,
            signatures,
            sketches,
        }
    }

    fn validate(&self) -> Result<(), SignatureContractKitError> {
        let mut files = std::collections::BTreeSet::new();
        for file in self.files {
            if !file.has_extension("rs") {
                return self.fail(format!(
                    "listed source file {file} must have a .rs extension"
                ));
            }
            if !files.insert(file) {
                return self.fail(format!("duplicate listed source file {file}"));
            }
        }

        let mut labels = std::collections::BTreeSet::new();
        let mut references = BTreeMap::<&str, usize>::new();
        let sketches = self
            .sketches
            .iter()
            .map(|sketch| (sketch.id.as_str(), sketch))
            .collect::<BTreeMap<_, _>>();
        if sketches.len() != self.sketches.len() {
            return self.fail("duplicate sketch identifier in document".to_owned());
        }

        for signature in self.signatures {
            if !labels.insert(signature.label.as_str()) {
                return self.fail(format!("duplicate signature label {}", signature.label));
            }
            if !files.contains(&signature.file) {
                return self.fail(format!(
                    "signature {} references unlisted source file {}",
                    signature.label, signature.file
                ));
            }
            if let Some(sketch_id) = signature.sketch.as_deref() {
                let sketch = sketches.get(sketch_id).ok_or_else(|| {
                    SignatureContractKitError::parse_failed(
                        self.catalog_name,
                        format!(
                            "signature {} links missing sketch {sketch_id}",
                            signature.label
                        ),
                    )
                })?;
                if sketch.signature_type != signature.signature_type.as_str() {
                    return self.fail(format!(
                        "sketch {sketch_id} signature_type {} does not match linked signature type {}",
                        sketch.signature_type,
                        signature.signature_type.as_str()
                    ));
                }
                *references.entry(sketch_id).or_default() += 1;
            }
        }

        for sketch in self.sketches {
            if sketch.id.trim().is_empty() {
                return self.fail("sketch id must not be empty".to_owned());
            }
            match references
                .get(sketch.id.as_str())
                .copied()
                .unwrap_or_default()
            {
                1 => {}
                0 => return self.fail(format!("orphan sketch {}", sketch.id)),
                count => {
                    return self.fail(format!(
                        "sketch {} is referenced by {count} signatures",
                        sketch.id
                    ));
                }
            }
        }

        Ok(())
    }

    fn fail<T>(&self, message: String) -> Result<T, SignatureContractKitError> {
        Err(SignatureContractKitError::parse_failed(
            self.catalog_name,
            message,
        ))
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RustYamlSignatureType {
    MainMethod,
    Function,
    Struct,
    Enum,
    Trait,
    Union,
    Static,
    Macro,
    TypeAlias,
}

impl RustYamlSignatureType {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MainMethod => "main_method",
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Union => "union",
            Self::Static => "static",
            Self::Macro => "macro",
            Self::TypeAlias => "type_alias",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RustYamlCallableQualifierInput {
    Const,
    Async,
    Unsafe,
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RustYamlImplementationQualifierInput {
    Default,
    Unsafe,
}

#[derive(Default)]
struct RustYamlCallableQualifiers {
    is_const: bool,
    is_async: bool,
    is_unsafe: bool,
}

impl RustYamlCallableQualifiers {
    fn from_inputs(inputs: &[RustYamlCallableQualifierInput]) -> Self {
        let mut qualifiers = Self::default();

        for input in inputs {
            match input {
                RustYamlCallableQualifierInput::Const => qualifiers.is_const = true,
                RustYamlCallableQualifierInput::Async => qualifiers.is_async = true,
                RustYamlCallableQualifierInput::Unsafe => qualifiers.is_unsafe = true,
            }
        }

        qualifiers
    }
}

struct RustYamlImplementedTraitInput<'a> {
    value: Option<&'a str>,
}

impl<'a> RustYamlImplementedTraitInput<'a> {
    fn new(value: Option<&'a str>) -> Self {
        Self { value }
    }

    fn to_implemented_trait(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustImplementedTrait, SignatureContractKitError> {
        let Some(value) = self.value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(RustImplementedTrait::Inherent);
        };

        let (polarity, name) = value
            .strip_prefix('!')
            .map(|name| (RustImplPolarity::Negative, name.trim()))
            .unwrap_or((RustImplPolarity::Positive, value));

        if name.is_empty() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML implementation trait cannot be empty",
            ));
        }

        Ok(RustImplementedTrait::Trait {
            name: name.to_owned(),
            polarity,
        })
    }
}

struct RustYamlFunctionAbiInput<'a> {
    value: Option<&'a str>,
}

impl<'a> RustYamlFunctionAbiInput<'a> {
    fn new(value: Option<&'a str>) -> Self {
        Self { value }
    }

    fn to_abi(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustFunctionAbi, SignatureContractKitError> {
        let Some(value) = self.value else {
            return Ok(RustFunctionAbi::Rust);
        };
        if value.is_empty() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML abi must be `extern` or a nonempty ABI name",
            ));
        }
        let canonical_name = value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            && value
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic())
            && value
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric());
        if !canonical_name {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML abi must use the canonical ABI name without Rust syntax",
            ));
        }

        Ok(match value {
            "extern" => RustFunctionAbi::Extern { name: None },
            value => RustFunctionAbi::Extern {
                name: Some(value.to_owned()),
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlShorthandVariadicInput {
    Present(bool),
    Details(RustYamlShorthandVariadicDetailsInput),
}

impl RustYamlShorthandVariadicInput {
    fn to_variadic(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Option<RustVariadicParameter>, SignatureContractKitError> {
        match self {
            Self::Present(false) => Ok(None),
            Self::Present(true) => Ok(Some(RustVariadicParameter::new(None, Vec::new()))),
            Self::Details(details) => details.to_variadic(catalog_name).map(Some),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlShorthandVariadicDetailsInput {
    pattern: Option<String>,
    attributes: Option<Vec<String>>,
}

impl RustYamlShorthandVariadicDetailsInput {
    fn to_variadic(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustVariadicParameter, SignatureContractKitError> {
        if let Some(pattern) = &self.pattern
            && pattern.trim().is_empty()
        {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML variadic pattern cannot be empty",
            ));
        }

        Ok(RustVariadicParameter::new(
            self.pattern.clone(),
            self.attributes.clone().unwrap_or_default(),
        ))
    }
}

struct RustYamlVisibilityInput {
    value: String,
}

impl RustYamlVisibilityInput {
    fn to_visibility(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        RustYamlVisibilityText::new(&self.value).to_visibility(catalog_name)
    }
}

impl<'de> Deserialize<'de> for RustYamlVisibilityInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let Some(value) = value.as_str() else {
            return Err(de::Error::custom(
                "Rust YAML visibility must use shorthand text",
            ));
        };

        Ok(Self {
            value: value.to_owned(),
        })
    }
}

struct RustYamlVisibilityText<'a> {
    value: &'a str,
}

impl<'a> RustYamlVisibilityText<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }

    fn to_visibility(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        match self.value.trim() {
            "public" | "pub" => Ok(Visibility::Public),
            "public(crate)" | "pub(crate)" | "public_crate" => Ok(Visibility::PublicCrate),
            "private" => Ok(Visibility::Private),
            "Public" | "PublicCrate" | "Private" | "Restricted" => {
                Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "Rust YAML visibility uses shorthand values like public, public(crate), private, or pub(...)",
                ))
            }
            value if value.starts_with("Restricted") => {
                Err(SignatureContractKitError::parse_failed(
                    catalog_name,
                    "Rust YAML visibility uses shorthand values like public, public(crate), private, or pub(...)",
                ))
            }
            value => Ok(Visibility::Restricted(value.to_owned())),
        }
    }
}

struct RustYamlGenericParametersInput {
    values: Vec<String>,
}

impl RustYamlGenericParametersInput {
    fn as_slice(&self) -> &[String] {
        &self.values
    }
}

impl<'de> Deserialize<'de> for RustYamlGenericParametersInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;

        match value {
            serde_yaml::Value::Sequence(_) => Vec::<String>::deserialize(value)
                .map(|values| Self { values })
                .map_err(de::Error::custom),
            serde_yaml::Value::Mapping(mapping)
                if mapping.contains_key(serde_yaml::Value::String("parameters".to_owned()))
                    || mapping
                        .contains_key(serde_yaml::Value::String("where_predicates".to_owned())) =>
            {
                Err(de::Error::custom(
                    "Rust YAML longhand generics are internal only; use shorthand generics: [...] and where: [...]",
                ))
            }
            _ => Err(de::Error::custom(
                "Rust YAML generics must be a shorthand list",
            )),
        }
    }
}

struct RustYamlShorthandGenericsInput<'a> {
    parameters: &'a [String],
    where_predicates: &'a [String],
}

impl<'a> RustYamlShorthandGenericsInput<'a> {
    fn new(
        parameters: Option<&'a RustYamlGenericParametersInput>,
        where_predicates: &'a [String],
    ) -> Self {
        Self {
            parameters: parameters
                .map(RustYamlGenericParametersInput::as_slice)
                .unwrap_or(&[]),
            where_predicates,
        }
    }

    fn to_metadata(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustGenericMetadata, SignatureContractKitError> {
        Ok(RustGenericMetadata::new(
            self.parameters
                .iter()
                .map(|value| RustYamlGenericParameterText::new(value).to_parameter(catalog_name))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .with_where_predicates(self.where_predicates.to_vec()))
    }
}

struct RustYamlGenericParameterText<'a> {
    value: &'a str,
}

impl<'a> RustYamlGenericParameterText<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }

    fn to_parameter(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustGenericParameter, SignatureContractKitError> {
        RustYamlGenericParameterInput::parse(self.value, catalog_name)
            .map(|input| input.into_parameter())
    }
}

struct RustYamlGenericParameterInput {
    value: syn::GenericParam,
}

impl RustYamlGenericParameterInput {
    fn parse(value: &str, catalog_name: &CatalogPath) -> Result<Self, SignatureContractKitError> {
        syn::parse_str::<syn::GenericParam>(value.trim())
            .map(|value| Self { value })
            .map_err(|source| {
                SignatureContractKitError::parse_failed(
                    catalog_name,
                    format!(
                        "Rust YAML generic parameter must be valid shorthand Rust syntax: {source}"
                    ),
                )
            })
    }

    fn into_parameter(self) -> RustGenericParameter {
        match self.value {
            syn::GenericParam::Type(parameter) => RustGenericParameter::type_parameter(
                parameter.ident.to_string(),
                parameter
                    .bounds
                    .iter()
                    .map(|bound| RustYamlTokenText::new(bound).render())
                    .collect(),
                parameter
                    .default
                    .as_ref()
                    .map(|value| RustYamlTokenText::new(value).render()),
            ),
            syn::GenericParam::Lifetime(parameter) => RustGenericParameter::lifetime_parameter(
                parameter.lifetime.to_string(),
                parameter
                    .bounds
                    .iter()
                    .map(|bound| RustYamlTokenText::new(bound).render())
                    .collect(),
            ),
            syn::GenericParam::Const(parameter) => RustGenericParameter::const_parameter(
                parameter.ident.to_string(),
                RustYamlTokenText::new(&parameter.ty).render(),
                parameter
                    .default
                    .as_ref()
                    .map(|value| RustYamlTokenText::new(value).render()),
            ),
        }
    }
}

struct RustYamlTokenText {
    value: String,
}

impl RustYamlTokenText {
    fn new(value: &impl ToTokens) -> Self {
        Self {
            value: value.to_token_stream().to_string(),
        }
    }

    fn render(self) -> String {
        self.value
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlShorthandFields {
    List(Vec<RustYamlStructFieldInput>),
    Map(serde_yaml::Mapping),
}

impl RustYamlShorthandFields {
    fn to_struct_fields(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<StructField>, SignatureContractKitError> {
        match self {
            Self::List(fields) => fields
                .iter()
                .map(|field| field.to_struct_field(context, catalog_name))
                .collect(),
            Self::Map(fields) => fields
                .iter()
                .map(|(name, field)| {
                    RustYamlMapField::new(name, field).to_struct_field(context, catalog_name)
                })
                .collect(),
        }
    }
}

struct RustYamlMapField<'a> {
    name: &'a serde_yaml::Value,
    field: &'a serde_yaml::Value,
}

impl<'a> RustYamlMapField<'a> {
    fn new(name: &'a serde_yaml::Value, field: &'a serde_yaml::Value) -> Self {
        Self { name, field }
    }

    fn to_struct_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<StructField, SignatureContractKitError> {
        let field = RustYamlShorthandFieldValue::from_value(self.field, catalog_name)?;

        Ok(StructField::new(
            Some(self.string(self.name, "field name", catalog_name)?),
            field.visibility,
            RustYamlTypeText::from_text(field.field_type).parse(context)?,
        ))
    }

    fn string(
        &self,
        value: &serde_yaml::Value,
        label: &str,
        catalog_name: &CatalogPath,
    ) -> Result<String, SignatureContractKitError> {
        value.as_str().map(str::to_owned).ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                catalog_name,
                format!("Rust YAML shorthand {label} must be a string"),
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlStructFieldInput {
    UnnamedType(String),
    UnnamedDetails(RustYamlShorthandFieldDetailsInput),
    Named(BTreeMap<String, RustYamlShorthandFieldValueInput>),
}

impl RustYamlStructFieldInput {
    fn to_struct_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<StructField, SignatureContractKitError> {
        match self {
            Self::UnnamedType(field_type) => Ok(StructField::new(
                None,
                Visibility::Private,
                RustYamlTypeText::from_text(field_type.clone()).parse(context)?,
            )),
            Self::UnnamedDetails(details) => Ok(StructField::new(
                None,
                details.visibility(catalog_name)?,
                RustYamlTypeText::from_text(details.field_type.clone()).parse(context)?,
            )),
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

                Ok(StructField::new(
                    Some(name.clone()),
                    field.visibility(catalog_name)?,
                    RustYamlTypeText::from_text(field.field_type()).parse(context)?,
                ))
            }
        }
    }
}

struct RustYamlShorthandFieldValue {
    field_type: String,
    visibility: Visibility,
}

impl RustYamlShorthandFieldValue {
    fn from_value(
        value: &serde_yaml::Value,
        catalog_name: &CatalogPath,
    ) -> Result<Self, SignatureContractKitError> {
        match value {
            serde_yaml::Value::String(field_type) => Ok(Self {
                field_type: field_type.clone(),
                visibility: Visibility::Private,
            }),
            value => {
                let input =
                    serde_yaml::from_value::<RustYamlShorthandFieldDetailsInput>(value.clone())
                        .map_err(|source| {
                            SignatureContractKitError::parse_failed(
                                catalog_name,
                                source.to_string(),
                            )
                        })?;
                Ok(Self {
                    field_type: input.field_type,
                    visibility: input
                        .visibility
                        .map(|visibility| visibility.to_visibility(catalog_name))
                        .unwrap_or(Ok(Visibility::Private))?,
                })
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlShorthandFieldValueInput {
    Type(String),
    Details(RustYamlShorthandFieldDetailsInput),
}

impl RustYamlShorthandFieldValueInput {
    fn field_type(&self) -> String {
        match self {
            Self::Type(value) => value.clone(),
            Self::Details(value) => value.field_type.clone(),
        }
    }

    fn visibility(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        match self {
            Self::Type(_) => Ok(Visibility::Private),
            Self::Details(value) => value
                .visibility
                .as_ref()
                .map(|visibility| visibility.to_visibility(catalog_name))
                .unwrap_or(Ok(Visibility::Private)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlShorthandFieldDetailsInput {
    #[serde(rename = "type")]
    field_type: String,
    visibility: Option<RustYamlVisibilityInput>,
}

impl RustYamlShorthandFieldDetailsInput {
    fn visibility(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        self.visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(catalog_name))
            .unwrap_or(Ok(Visibility::Private))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlShorthandVariant {
    Unit(String),
    Tuple(BTreeMap<String, Vec<String>>),
    Details(BTreeMap<String, RustYamlShorthandVariantDetailsInput>),
}

impl RustYamlShorthandVariant {
    fn to_variant(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<EnumVariant, SignatureContractKitError> {
        match self {
            Self::Unit(name) => Ok(EnumVariant::new(name.clone(), Vec::new(), None)),
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

                Ok(EnumVariant::new(
                    name.clone(),
                    fields
                        .iter()
                        .map(|field_type| {
                            Ok(EnumVariantField::new(
                                None,
                                RustYamlTypeText::from_text(field_type.clone()).parse(context)?,
                            ))
                        })
                        .collect::<Result<Vec<_>, SignatureContractKitError>>()?,
                    None,
                ))
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

                details.to_variant(name, context, catalog_name)
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlShorthandVariantDetailsInput {
    fields: Option<RustYamlShorthandVariantFieldsInput>,
    discriminant: Option<RustYamlDiscriminantInput>,
}

impl RustYamlShorthandVariantDetailsInput {
    fn to_variant(
        &self,
        name: &str,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<EnumVariant, SignatureContractKitError> {
        Ok(EnumVariant::new(
            name.to_owned(),
            self.fields
                .as_ref()
                .map(|fields| fields.to_fields(context, catalog_name))
                .unwrap_or_else(|| Ok(Vec::new()))?,
            self.discriminant
                .as_ref()
                .map(RustYamlDiscriminantInput::to_discriminant),
        ))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlShorthandVariantFieldsInput {
    List(Vec<RustYamlEnumVariantFieldInput>),
    Map(serde_yaml::Mapping),
}

impl RustYamlShorthandVariantFieldsInput {
    fn to_fields(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<EnumVariantField>, SignatureContractKitError> {
        match self {
            Self::List(fields) => fields
                .iter()
                .map(|field| field.to_variant_field(context, catalog_name))
                .collect(),
            Self::Map(fields) => fields
                .iter()
                .map(|(name, field_type)| {
                    RustYamlMapVariantField::new(name, field_type)
                        .to_variant_field(context, catalog_name)
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RustYamlEnumVariantFieldInput {
    UnnamedType(String),
    Named(BTreeMap<String, String>),
}

impl RustYamlEnumVariantFieldInput {
    fn to_variant_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<EnumVariantField, SignatureContractKitError> {
        match self {
            Self::UnnamedType(field_type) => Ok(EnumVariantField::new(
                None,
                RustYamlTypeText::from_text(field_type.clone()).parse(context)?,
            )),
            Self::Named(field) => {
                if field.len() != 1 {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand enum variant field entries must contain exactly one field",
                    ));
                }

                let Some((name, field_type)) = field.iter().next() else {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML shorthand enum variant field entry is empty",
                    ));
                };

                Ok(EnumVariantField::new(
                    Some(name.clone()),
                    RustYamlTypeText::from_text(field_type.clone()).parse(context)?,
                ))
            }
        }
    }
}

struct RustYamlMapVariantField<'a> {
    name: &'a serde_yaml::Value,
    field_type: &'a serde_yaml::Value,
}

impl<'a> RustYamlMapVariantField<'a> {
    fn new(name: &'a serde_yaml::Value, field_type: &'a serde_yaml::Value) -> Self {
        Self { name, field_type }
    }

    fn to_variant_field(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<EnumVariantField, SignatureContractKitError> {
        Ok(EnumVariantField::new(
            Some(self.string(self.name, "variant field name", catalog_name)?),
            RustYamlTypeText::from_text(self.string(
                self.field_type,
                "variant field type",
                catalog_name,
            )?)
            .parse(context)?,
        ))
    }

    fn string(
        &self,
        value: &serde_yaml::Value,
        label: &str,
        catalog_name: &CatalogPath,
    ) -> Result<String, SignatureContractKitError> {
        value.as_str().map(str::to_owned).ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                catalog_name,
                format!("Rust YAML shorthand {label} must be a string"),
            )
        })
    }
}

struct RustYamlDiscriminantInput {
    value: String,
}

impl RustYamlDiscriminantInput {
    fn to_discriminant(&self) -> String {
        self.value.clone()
    }
}

impl<'de> Deserialize<'de> for RustYamlDiscriminantInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let value = match value {
            serde_yaml::Value::String(value) => value,
            serde_yaml::Value::Number(value) => value.to_string(),
            _ => {
                return Err(de::Error::custom(
                    "Rust YAML enum discriminant must be shorthand text or a number",
                ));
            }
        };

        Ok(Self { value })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlShorthandMethodEntry {
    #[serde(rename = "signature_type")]
    signature_type: String,
    name: String,
    visibility: Option<RustYamlVisibilityInput>,
    derives: Option<Vec<String>>,
    receiver: Option<String>,
    #[serde(rename = "trait", default)]
    implemented_trait: RustYamlFieldPresence<Option<String>>,
    #[serde(default)]
    impl_qualifiers: RustYamlFieldPresence<Option<Vec<RustYamlImplementationQualifierInput>>>,
    #[serde(default)]
    impl_generics: RustYamlFieldPresence<Option<RustYamlGenericParametersInput>>,
    #[serde(rename = "impl_where", default)]
    impl_where_predicates: RustYamlFieldPresence<Option<Vec<String>>>,
    #[serde(default)]
    qualifiers: Vec<RustYamlCallableQualifierInput>,
    #[serde(default)]
    abi: RustYamlFieldPresence<String>,
    variadic: Option<RustYamlShorthandVariadicInput>,
    generics: Option<RustYamlGenericParametersInput>,
    #[serde(rename = "where")]
    where_predicates: Option<Vec<String>>,
    parameters: Option<Vec<RustYamlFunctionParameterInput>>,
    return_type: Option<String>,
}

impl RustYamlShorthandMethodEntry {
    fn to_method(
        &self,
        source_file: &CatalogPath,
        module_path: Vec<String>,
        default_visibility: Visibility,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<RustMethod, SignatureContractKitError> {
        if self.signature_type != "method" {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "nested signature {} must use signature_type: method",
                    self.name
                ),
            ));
        }
        let visibility = self
            .visibility
            .as_ref()
            .map(|visibility| visibility.to_visibility(catalog_name))
            .unwrap_or(Ok(default_visibility))?;
        let generics = self.generic_metadata(catalog_name)?;
        let context = owner_context.with_metadata(&generics);
        let qualifiers = RustYamlCallableQualifiers::from_inputs(&self.qualifiers);
        let callable = RustCallableSignature::builder()
            .with_const(qualifiers.is_const)
            .with_async(qualifiers.is_async)
            .with_unsafe(qualifiers.is_unsafe)
            .with_abi(
                RustYamlFunctionAbiInput::new(self.abi.as_ref().map(String::as_str))
                    .to_abi(catalog_name)?,
            )
            .with_variadic(
                self.variadic
                    .as_ref()
                    .map(|variadic| variadic.to_variadic(catalog_name))
                    .transpose()?
                    .flatten(),
            )
            .with_generics(generics)
            .with_parameters(self.parameters(&context, catalog_name)?)
            .with_return_type(
                self.return_type
                    .as_ref()
                    .map(|value| RustYamlTypeText::from_text(value.clone()).parse(&context))
                    .transpose()?,
            )
            .build();
        let function = FunctionType::new(
            BaseType::new(self.name.clone(), visibility.clone(), source_file.clone())
                .with_module_path(module_path)
                .with_derives(self.derives.clone().unwrap_or_default()),
        )
        .with_callable_signature(callable);

        Ok(RustMethod::new(
            function,
            self.receiver
                .as_ref()
                .map(|receiver| RustYamlReceiverText::new(receiver).to_receiver(catalog_name))
                .transpose()?,
            visibility,
        ))
    }

    fn implementation(
        &self,
        owner_type: &str,
        catalog_name: &CatalogPath,
    ) -> Result<ImplementationType, SignatureContractKitError> {
        let qualifiers = self.implementation_qualifiers(catalog_name)?;
        Ok(ImplementationType::new(owner_type.to_owned())
            .with_implemented_trait(self.implemented_trait(catalog_name)?)
            .with_qualifiers(
                qualifiers.contains(&RustYamlImplementationQualifierInput::Default),
                qualifiers.contains(&RustYamlImplementationQualifierInput::Unsafe),
            )
            .with_generic_metadata(
                RustYamlShorthandGenericsInput::new(
                    self.implementation_generics(catalog_name)?,
                    self.implementation_where_predicates(catalog_name)?,
                )
                .to_metadata(catalog_name)?,
            ))
    }

    fn generic_metadata(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustGenericMetadata, SignatureContractKitError> {
        RustYamlShorthandGenericsInput::new(
            self.generics.as_ref(),
            self.where_predicates.as_deref().unwrap_or(&[]),
        )
        .to_metadata(catalog_name)
    }

    fn parameters(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<Vec<RustFunctionParameter>, SignatureContractKitError> {
        self.parameters
            .as_ref()
            .map(|parameters| {
                parameters
                    .iter()
                    .map(|parameter| parameter.to_parameter(context, catalog_name))
                    .collect()
            })
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn implemented_trait(
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
        RustYamlImplementedTraitInput::new(value).to_implemented_trait(catalog_name)
    }

    fn implementation_qualifiers(
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

    fn implementation_generics(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<Option<&RustYamlGenericParametersInput>, SignatureContractKitError> {
        match self.impl_generics.as_ref() {
            None => Ok(None),
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML impl_generics must not be null",
            )),
        }
    }

    fn implementation_where_predicates(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<&[String], SignatureContractKitError> {
        match self.impl_where_predicates.as_ref() {
            None => Ok(&[]),
            Some(Some(value)) => Ok(value),
            Some(None) => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML impl_where must not be null",
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustYamlShorthandImplementationInput {
    #[serde(rename = "trait", default)]
    implemented_trait: RustYamlFieldPresence<Option<String>>,
    #[serde(default)]
    impl_qualifiers: RustYamlFieldPresence<Option<Vec<RustYamlImplementationQualifierInput>>>,
    #[serde(default)]
    generics: RustYamlFieldPresence<Option<RustYamlGenericParametersInput>>,
    #[serde(rename = "where", default)]
    where_predicates: RustYamlFieldPresence<Option<Vec<String>>>,
}

impl RustYamlShorthandImplementationInput {
    fn implementation(
        &self,
        owner_type: &str,
        catalog_name: &CatalogPath,
    ) -> Result<ImplementationType, SignatureContractKitError> {
        let qualifiers = self.implementation_qualifiers(catalog_name)?;
        Ok(ImplementationType::new(owner_type.to_owned())
            .with_implemented_trait(self.implemented_trait(catalog_name)?)
            .with_qualifiers(
                qualifiers.contains(&RustYamlImplementationQualifierInput::Default),
                qualifiers.contains(&RustYamlImplementationQualifierInput::Unsafe),
            )
            .with_generic_metadata(
                RustYamlShorthandGenericsInput::new(
                    self.implementation_generics(catalog_name)?,
                    self.implementation_where_predicates(catalog_name)?,
                )
                .to_metadata(catalog_name)?,
            ))
    }

    fn implemented_trait(
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
        RustYamlImplementedTraitInput::new(value).to_implemented_trait(catalog_name)
    }

    fn implementation_qualifiers(
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

    fn implementation_generics(
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

    fn implementation_where_predicates(
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

struct RustYamlReceiverText<'a> {
    value: &'a str,
}

impl<'a> RustYamlReceiverText<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }

    fn to_receiver(&self, catalog_name: &CatalogPath) -> Result<String, SignatureContractKitError> {
        match self.value.trim() {
            "" | "none" | "static" => Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "static methods must omit receiver",
            )),
            "ref" | "&self" | "& self" => Ok("& self".to_owned()),
            "mut" | "ref_mut" | "&mut self" | "& mut self" => Ok("& mut self".to_owned()),
            "self" => Ok("self".to_owned()),
            value => Ok(value.to_owned()),
        }
    }
}

#[derive(Deserialize)]
struct RustYamlFunctionParameterInput(BTreeMap<String, String>);

impl RustYamlFunctionParameterInput {
    fn to_parameter(
        &self,
        context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
    ) -> Result<RustFunctionParameter, SignatureContractKitError> {
        if self.0.len() != 1 {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML shorthand parameter entries must contain exactly one parameter",
            ));
        }

        let Some((name, parameter_type)) = self.0.iter().next() else {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML shorthand parameter entry is empty",
            ));
        };

        Ok(RustFunctionParameter::new(
            RustYamlParameterNameInput::new(name).to_name(),
            RustYamlTypeText::from_text(parameter_type.clone()).parse(context)?,
        ))
    }
}

struct RustYamlParameterNameInput<'a> {
    value: &'a str,
}

impl<'a> RustYamlParameterNameInput<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }

    fn to_name(&self) -> Option<String> {
        match self.value.trim() {
            "_" => None,
            value => Some(value.to_owned()),
        }
    }
}
