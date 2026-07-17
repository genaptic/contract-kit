use super::super::type_text::{RustYamlGenericContext, RustYamlTypeText};
use super::contract::{RustYamlExtraction, RustYamlNamedSignature};
use super::member::{
    RustYamlFunctionParameterInput, RustYamlNestedItemInput, RustYamlShorthandFields,
    RustYamlShorthandImplementationInput, RustYamlShorthandVariant,
};
use super::metadata::{
    RustYamlAttributesValue, RustYamlCallableQualifierInput, RustYamlCallableQualifiers,
    RustYamlGenericParametersInput, RustYamlShorthandCatalogPath, RustYamlShorthandVariadicInput,
    RustYamlVisibilityInput,
};
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::RustParsedEntry;
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemIdAllocator};
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{RustCallableSignature, RustFunctionAbi};
use crate::languages::rust::types::declaration::{
    ConstantType, ExternCrateType, ForeignModuleType, ModuleDeclarationType, ReexportType,
    RustDeclaration, RustIdentifier, TraitAliasType,
};
use crate::languages::rust::types::enum_type::EnumType;
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{ImplementationType, RustImplementationOwner};
use crate::languages::rust::types::macro_type::MacroType;
use crate::languages::rust::types::primitive_types::Visibility;
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::StructType;
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use crate::work::CancellationProbe;
use serde::{Deserialize, de};
use std::collections::BTreeMap;

pub(super) struct RustYamlRawEntry(pub(super) Vec<(String, RustYamlRawSignature)>);

impl<'de> Deserialize<'de> for RustYamlRawEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RawEntryVisitor;

        impl<'de> de::Visitor<'de> for RawEntryVisitor {
            type Value = RustYamlRawEntry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Rust YAML shorthand signature entry")
            }

            fn visit_map<M>(self, mut mapping: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut entries = Vec::with_capacity(mapping.size_hint().unwrap_or_default());
                while let Some(entry) = mapping.next_entry()? {
                    entries.push(entry);
                }
                Ok(RustYamlRawEntry(entries))
            }
        }

        deserializer.deserialize_map(RawEntryVisitor)
    }
}

struct RustYamlConstantInput {
    type_text: String,
    value: String,
}

pub(super) struct RustYamlCallableInput {
    pub(super) qualifiers: Vec<RustYamlCallableQualifierInput>,
    pub(super) abi: Option<String>,
    pub(super) variadic: Option<RustYamlShorthandVariadicInput>,
    pub(super) generics: Option<RustYamlGenericParametersInput>,
    pub(super) where_predicates: Vec<String>,
    pub(super) parameters: Vec<RustYamlFunctionParameterInput>,
    pub(super) return_type: Option<String>,
}

struct RustYamlStructInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    fields: Option<RustYamlShorthandFields>,
}

struct RustYamlEnumInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    variants: Option<Vec<RustYamlShorthandVariant>>,
}

struct RustYamlTraitInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    is_unsafe: bool,
    is_auto: bool,
    items: Vec<RustYamlNestedItemInput>,
    supertraits: Vec<String>,
}

struct RustYamlExternCrateInput {
    alias: Option<String>,
}

struct RustYamlForeignModuleInput {
    abi: Option<String>,
    is_unsafe: bool,
    items: Vec<RustYamlNestedItemInput>,
}

struct RustYamlTraitAliasInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    supertraits: Vec<String>,
}

struct RustYamlUnionInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    fields: Option<RustYamlShorthandFields>,
}

struct RustYamlStaticInput {
    type_text: String,
    mutable: bool,
}

struct RustYamlMacroInput {
    tokens: String,
}

struct RustYamlModuleInput {
    is_inline: bool,
    path_override: Option<String>,
}

struct RustYamlTypeAliasInput {
    generics: Option<RustYamlGenericParametersInput>,
    where_predicates: Vec<String>,
    target_type: String,
}

struct RustYamlReexportInput {
    path: String,
    alias: Option<String>,
}

#[derive(Default)]
pub(super) enum RustYamlFieldPresence<T> {
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

    pub(super) fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }

    pub(super) fn take(&mut self) -> Option<T> {
        match std::mem::take(self) {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }

    fn take_required(
        &mut self,
        signature_type: RustYamlSignatureType,
        field: &'static str,
        catalog_name: &CatalogPath,
    ) -> Result<T, SignatureContractKitError> {
        self.take().ok_or_else(|| {
            SignatureContractKitError::parse_failed(
                catalog_name,
                RustYamlShapeError::MissingField {
                    signature_type: signature_type.as_str(),
                    field,
                }
                .to_string(),
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlRawSignature {
    pub(super) crate_id: Option<String>,
    #[serde(default)]
    pub(super) file: RustYamlFieldPresence<RustYamlShorthandCatalogPath>,
    pub(super) module_path: Option<Vec<String>>,
    #[serde(rename = "signature_type")]
    pub(super) signature_type: RustYamlSignatureType,
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) visibility: RustYamlFieldPresence<RustYamlVisibilityInput>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
    #[serde(default)]
    pub(super) qualifiers: RustYamlFieldPresence<Vec<RustYamlCallableQualifierInput>>,
    #[serde(default)]
    pub(super) abi: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) variadic: RustYamlFieldPresence<RustYamlShorthandVariadicInput>,
    #[serde(default)]
    pub(super) generics: RustYamlFieldPresence<RustYamlGenericParametersInput>,
    #[serde(rename = "where", default)]
    pub(super) where_predicates: RustYamlFieldPresence<Vec<String>>,
    #[serde(default)]
    pub(super) fields: RustYamlFieldPresence<RustYamlShorthandFields>,
    #[serde(default)]
    pub(super) variants: RustYamlFieldPresence<Vec<RustYamlShorthandVariant>>,
    #[serde(default)]
    pub(super) items: RustYamlFieldPresence<Vec<RustYamlNestedItemInput>>,
    #[serde(default)]
    pub(super) implementations: RustYamlFieldPresence<Vec<RustYamlShorthandImplementationInput>>,
    #[serde(default)]
    pub(super) parameters: RustYamlFieldPresence<Vec<RustYamlFunctionParameterInput>>,
    #[serde(default)]
    pub(super) return_type: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) supertraits: RustYamlFieldPresence<Vec<String>>,
    #[serde(rename = "type", default)]
    pub(super) type_text: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) value: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) target_type: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) alias: RustYamlFieldPresence<String>,
    #[serde(default)]
    pub(super) path: RustYamlFieldPresence<String>,
    #[serde(rename = "unsafe", default)]
    pub(super) is_unsafe: RustYamlFieldPresence<bool>,
    #[serde(rename = "auto", default)]
    pub(super) is_auto: RustYamlFieldPresence<bool>,
    #[serde(default)]
    pub(super) mutable: RustYamlFieldPresence<bool>,
    #[serde(default)]
    pub(super) tokens: RustYamlFieldPresence<String>,
    #[serde(rename = "inline", default)]
    pub(super) is_inline: RustYamlFieldPresence<bool>,
    pub(super) sketch: Option<String>,
}

pub(super) struct RustYamlDocumentDecoder<'document, 'operation> {
    pub(super) catalog_name: &'document CatalogPath,
    pub(super) extraction: &'document RustYamlExtraction,
    pub(super) item_ids: &'document mut RustItemIdAllocator,
    pub(super) cancellation: &'operation CancellationProbe,
}

impl RustYamlDocumentDecoder<'_, '_> {
    pub(super) fn decode_entry(
        &mut self,
        entry: RustYamlRawEntry,
    ) -> Result<RustYamlNamedSignature, SignatureContractKitError> {
        for (_, raw) in &entry.0 {
            self.cancellation.checkpoint()?;
            if !raw.file.is_present() {
                return Err(SignatureContractKitError::parse_failed(
                    self.catalog_name,
                    "missing field `file`",
                ));
            }
            raw.validate_shape().map_err(|source| {
                SignatureContractKitError::parse_failed(self.catalog_name, source.to_string())
            })?;
        }
        if entry.0.len() != 1 {
            return Err(SignatureContractKitError::parse_failed(
                self.catalog_name,
                "Rust YAML shorthand signature entries must contain exactly one named entry",
            ));
        }

        let Some((label, raw)) = entry.0.into_iter().next() else {
            return Err(SignatureContractKitError::parse_failed(
                self.catalog_name,
                "Rust YAML shorthand signature entry is empty",
            ));
        };
        self.decode(label, raw)
    }

    fn decode(
        &mut self,
        label: String,
        mut raw: RustYamlRawSignature,
    ) -> Result<RustYamlNamedSignature, SignatureContractKitError> {
        self.cancellation.checkpoint()?;

        let signature_type = raw.signature_type;
        let crate_id = self.extraction.signature_crate_id(
            raw.crate_id.as_deref(),
            &label,
            self.catalog_name,
            self.cancellation,
        )?;
        let module_path =
            RustModulePath::new(raw.module_path.take().unwrap_or_default()).map_err(|source| {
                SignatureContractKitError::parse_failed(self.catalog_name, source.to_string())
            })?;
        let module_id = RustModuleId::new(crate_id.clone(), module_path);
        let attributes = raw
            .attributes
            .to_attributes(self.catalog_name, self.cancellation)?;
        let name = raw.signature_name(&label, self.catalog_name)?;
        let file = raw
            .file
            .take_required(signature_type, "file", self.catalog_name)?
            .to_catalog_path();
        if signature_type == RustYamlSignatureType::ForeignModule && raw.visibility.is_present() {
            return Err(SignatureContractKitError::parse_failed(
                self.catalog_name,
                "foreign modules do not carry top-level visibility",
            ));
        }
        let default_visibility = if signature_type == RustYamlSignatureType::Reexport {
            Visibility::Public
        } else {
            Visibility::Private
        };
        let visibility = raw
            .visibility
            .take()
            .map(|value| value.to_visibility(&module_id, self.catalog_name))
            .unwrap_or(Ok(default_visibility))?;
        let base = BaseType::new(
            name.clone(),
            visibility,
            file.clone(),
            module_id.clone(),
            attributes,
        );
        let sketch = raw.sketch.take();

        let (declaration, implementations) = raw.into_declaration(base, self)?;
        let mut entries = if implementations.is_empty() {
            vec![self.allocate_entry(name, module_id, declaration)?]
        } else {
            self.entries_with_implementations(name, module_id, declaration, implementations)?
        };
        if let Some((_, implementation_entries)) = entries.split_first_mut() {
            self.cancellation.checkpoint()?;
            implementation_entries.sort_by(|left, right| left.id().cmp(right.id()));
            self.cancellation.checkpoint()?;
        }

        Ok(RustYamlNamedSignature {
            label,
            crate_id,
            file,
            signature_type,
            sketch,
            entries,
        })
    }

    fn allocate_entry(
        &mut self,
        name: String,
        module_id: RustModuleId,
        declaration: RustDeclaration,
    ) -> Result<RustParsedEntry, SignatureContractKitError> {
        let kind = declaration.kind();
        RustParsedEntry::from_contract(
            RustItemId::new(module_id, kind, name),
            declaration,
            self.catalog_name.clone(),
        )
        .allocate_id(self.item_ids)
    }

    fn entries_with_implementations(
        &mut self,
        owner_spelling: String,
        module_id: RustModuleId,
        declaration: RustDeclaration,
        implementations: Vec<RustYamlShorthandImplementationInput>,
    ) -> Result<Vec<RustParsedEntry>, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        let item_entry = self.allocate_entry(owner_spelling.clone(), module_id, declaration)?;
        let owner_base = item_entry
            .declaration()
            .implementation_owner_base()
            .ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    self.catalog_name,
                    "Rust YAML implementations require a local struct, enum, union, or type-alias owner",
                )
            })?;
        let file = owner_base.file_path().clone();
        let module_id = owner_base.module_id().clone();
        let owner_context =
            crate::languages::rust::types::base_type::RustImplementationContext::new(
                item_entry.id(),
                owner_base,
            )?;
        let owner = RustImplementationOwner::new(
            item_entry.id().clone(),
            RustModulePath::source_ident(&owner_spelling),
        )
        .map_err(|source| {
            SignatureContractKitError::parse_failed(self.catalog_name, source.to_string())
        })?;
        let mut grouped = BTreeMap::<Vec<u8>, ImplementationType>::new();
        for marker in implementations {
            self.cancellation.checkpoint()?;
            let mut implementation = marker.into_implementation(
                owner.clone(),
                &file,
                &module_id,
                self.catalog_name,
                self.cancellation,
            )?;
            implementation.normalize_for_owner(&owner_context, self.cancellation)?;
            let key = implementation.descriptor_bytes().map_err(|source| {
                SignatureContractKitError::conversion_failed(source.to_string())
            })?;
            if grouped.insert(key, implementation).is_some() {
                return Err(SignatureContractKitError::parse_failed(
                    self.catalog_name,
                    "implementation descriptors must be unique within one signature",
                ));
            }
        }

        let mut entries = vec![item_entry];
        for (_, implementation) in grouped {
            self.cancellation.checkpoint()?;
            let owner_id = implementation.owner().id().render();
            entries.push(self.allocate_entry(
                owner_id,
                module_id.clone(),
                RustDeclaration::Implementation(implementation),
            )?);
        }

        Ok(entries)
    }
}

impl RustYamlCallableInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        Ok(RustDeclaration::Function(
            FunctionType::new(base).with_callable_signature(self.into_signature(
                &RustYamlGenericContext::default(),
                decoder.catalog_name,
                decoder.cancellation,
            )?),
        ))
    }

    pub(super) fn into_signature(
        self,
        owner_context: &RustYamlGenericContext,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustCallableSignature, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            catalog_name,
            cancellation,
        )?;
        let context = owner_context.with_metadata(&generics, cancellation)?;
        let qualifiers = RustYamlCallableQualifiers::from_inputs(&self.qualifiers, cancellation)?;
        let mut parameters = Vec::with_capacity(self.parameters.len());
        for parameter in &self.parameters {
            cancellation.checkpoint()?;
            parameters.push(parameter.to_parameter(&context, catalog_name, cancellation)?);
        }
        let return_type = self
            .return_type
            .map(|value| RustYamlTypeText::from_text(value).parse(&context, cancellation))
            .transpose()?;

        Ok(RustCallableSignature::builder()
            .with_const(qualifiers.is_const)
            .with_async(qualifiers.is_async)
            .with_unsafe(qualifiers.is_unsafe)
            .with_abi(Self::parse_abi(self.abi.as_deref(), catalog_name)?)
            .with_variadic(
                self.variadic
                    .map(|variadic| variadic.to_variadic(catalog_name, cancellation))
                    .transpose()?
                    .flatten(),
            )
            .with_generics(generics)
            .with_parameters(parameters)
            .with_return_type(return_type)
            .build())
    }

    pub(super) fn parse_abi(
        value: Option<&str>,
        catalog_name: &CatalogPath,
    ) -> Result<RustFunctionAbi, SignatureContractKitError> {
        let Some(value) = value else {
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

impl RustYamlConstantInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let value = ConstantType::new(
            base,
            RustYamlTypeText::from_text(self.type_text)
                .parse(&RustYamlGenericContext::default(), decoder.cancellation)?,
            RustSyntaxText::parse_expression(&self.value).map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })?,
        )
        .map_err(|source| {
            SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
        })?;
        Ok(RustDeclaration::Constant(value))
    }
}

impl RustYamlStructInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, decoder.cancellation)?;
        let fields = self
            .fields
            .as_ref()
            .map(|fields| {
                fields.to_struct_fields(
                    &context,
                    base.module_id(),
                    decoder.catalog_name,
                    decoder.cancellation,
                )
            })
            .unwrap_or_else(|| Ok(Vec::new()))?;
        Ok(RustDeclaration::Structure(
            StructType::new(base)
                .with_generic_metadata(generics)
                .with_fields(fields),
        ))
    }
}

impl RustYamlEnumInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, decoder.cancellation)?;
        let mut variants = Vec::new();
        for variant in self.variants.as_deref().unwrap_or_default() {
            decoder.cancellation.checkpoint()?;
            variants.push(variant.to_variant(
                &context,
                decoder.catalog_name,
                decoder.cancellation,
            )?);
        }
        Ok(RustDeclaration::Enumeration(
            EnumType::new(base)
                .with_generic_metadata(generics)
                .with_variants(variants),
        ))
    }
}

impl RustYamlTraitInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, decoder.cancellation)?;
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            decoder.cancellation.checkpoint()?;
            items.push(item.into_trait_item(
                base.file_path(),
                base.module_id(),
                &context,
                decoder.catalog_name,
                decoder.cancellation,
            )?);
        }
        for _ in &self.supertraits {
            decoder.cancellation.checkpoint()?;
        }
        Ok(RustDeclaration::Trait(
            TraitType::new(base)
                .with_qualifiers(self.is_unsafe, self.is_auto)
                .with_generic_metadata(generics)
                .with_supertraits(self.supertraits)?
                .with_items(items),
        ))
    }
}

impl RustYamlExternCrateInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        ExternCrateType::new(base, self.alias)
            .map(RustDeclaration::ExternCrate)
            .map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })
    }
}

impl RustYamlForeignModuleInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            decoder.cancellation.checkpoint()?;
            items.push(item.into_foreign_item(
                base.file_path(),
                base.module_id(),
                decoder.catalog_name,
                decoder.cancellation,
            )?);
        }
        let value = ForeignModuleType::new(
            base.file_path().clone(),
            base.module_id().clone(),
            self.abi
                .as_deref()
                .map_or(Ok(RustFunctionAbi::Extern { name: None }), |abi| {
                    RustYamlCallableInput::parse_abi(Some(abi), decoder.catalog_name)
                })?,
            self.is_unsafe,
            base.attributes().clone(),
            items,
        )
        .map_err(|source| {
            SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
        })?;
        Ok(RustDeclaration::ForeignModule(value))
    }
}

impl RustYamlTraitAliasInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let mut supertraits = Vec::with_capacity(self.supertraits.len());
        for bound in &self.supertraits {
            decoder.cancellation.checkpoint()?;
            supertraits.push(RustSyntaxText::parse_type_bound(bound).map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })?);
        }
        TraitAliasType::new(base, generics, supertraits)
            .map(RustDeclaration::TraitAlias)
            .map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })
    }
}

impl RustYamlUnionInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, decoder.cancellation)?;
        let fields = self
            .fields
            .as_ref()
            .map(|fields| {
                fields.to_struct_fields(
                    &context,
                    base.module_id(),
                    decoder.catalog_name,
                    decoder.cancellation,
                )
            })
            .unwrap_or_else(|| Ok(Vec::new()))?;
        Ok(RustDeclaration::Union(
            UnionType::new(base)
                .with_generic_metadata(generics)
                .with_fields(fields),
        ))
    }
}

impl RustYamlStaticInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        Ok(RustDeclaration::Static(StaticType::new(
            base,
            self.mutable,
            RustYamlTypeText::from_text(self.type_text)
                .parse(&RustYamlGenericContext::default(), decoder.cancellation)?,
        )))
    }
}

impl RustYamlMacroInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        MacroType::new(base, self.tokens)
            .map(RustDeclaration::Macro)
            .map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })
    }
}

impl RustYamlModuleInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        ModuleDeclarationType::new(base, self.is_inline, self.path_override)
            .map(RustDeclaration::Module)
            .map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })
    }
}

impl RustYamlTypeAliasInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        decoder.cancellation.checkpoint()?;
        let generics = RustYamlGenericParametersInput::to_metadata_from(
            self.generics.as_ref(),
            &self.where_predicates,
            decoder.catalog_name,
            decoder.cancellation,
        )?;
        let context = RustYamlGenericContext::from_metadata(&generics, decoder.cancellation)?;
        Ok(RustDeclaration::TypeAlias(TypeAliasType::new(
            base,
            generics,
            RustYamlTypeText::from_text(self.target_type).parse(&context, decoder.cancellation)?,
        )))
    }
}

impl RustYamlReexportInput {
    pub(super) fn into_declaration(
        self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<RustDeclaration, SignatureContractKitError> {
        if matches!(base.visibility(), Visibility::Private) {
            return Err(SignatureContractKitError::parse_failed(
                decoder.catalog_name,
                "contract re-exports must use non-private visibility; private imports are resolution-only",
            ));
        }
        ReexportType::new(base, self.path, self.alias)
            .map(RustDeclaration::Reexport)
            .map_err(|source| {
                SignatureContractKitError::parse_failed(decoder.catalog_name, source.to_string())
            })
    }
}

impl RustYamlRawSignature {
    pub(super) fn signature_name(
        &mut self,
        label: &str,
        catalog_name: &CatalogPath,
    ) -> Result<String, SignatureContractKitError> {
        let name = if self.signature_type == RustYamlSignatureType::Reexport {
            self.name.take().map_or_else(
                || {
                    ReexportType::visible_name(
                        self.path.as_ref().ok_or_else(|| {
                            SignatureContractKitError::parse_failed(
                                catalog_name,
                                "signature_type reexport requires field path",
                            )
                        })?,
                        self.alias.as_ref().map(String::as_str),
                    )
                    .map(str::to_owned)
                    .map_err(|source| {
                        SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                    })
                },
                Ok,
            )?
        } else {
            self.name.take().unwrap_or_else(|| label.to_owned())
        };
        let role = match self.signature_type {
            RustYamlSignatureType::Function => Some("function name"),
            RustYamlSignatureType::Struct => Some("struct name"),
            RustYamlSignatureType::Enum => Some("enum name"),
            RustYamlSignatureType::Trait => Some("trait name"),
            RustYamlSignatureType::Union => Some("union name"),
            RustYamlSignatureType::Static => Some("static name"),
            RustYamlSignatureType::TypeAlias => Some("type alias name"),
            RustYamlSignatureType::Constant
            | RustYamlSignatureType::ExternCrate
            | RustYamlSignatureType::ForeignModule
            | RustYamlSignatureType::TraitAlias
            | RustYamlSignatureType::Macro
            | RustYamlSignatureType::Module
            | RustYamlSignatureType::Reexport => None,
        };
        match role {
            Some(role) => RustIdentifier::new(name, role)
                .map(|identifier| identifier.as_str().to_owned())
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                }),
            None => Ok(name),
        }
    }

    pub(super) fn into_declaration(
        mut self,
        base: BaseType,
        decoder: &RustYamlDocumentDecoder<'_, '_>,
    ) -> Result<
        (RustDeclaration, Vec<RustYamlShorthandImplementationInput>),
        SignatureContractKitError,
    > {
        decoder.cancellation.checkpoint()?;
        let implementations = self.implementations.take().unwrap_or_default();
        let signature_type = self.signature_type;
        let declaration = match signature_type {
            RustYamlSignatureType::Constant => RustYamlConstantInput {
                type_text: self.type_text.take_required(
                    signature_type,
                    "type",
                    decoder.catalog_name,
                )?,
                value: self
                    .value
                    .take_required(signature_type, "value", decoder.catalog_name)?,
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Function => RustYamlCallableInput {
                qualifiers: self.qualifiers.take().unwrap_or_default(),
                abi: self.abi.take(),
                variadic: self.variadic.take(),
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                parameters: self.parameters.take().unwrap_or_default(),
                return_type: self.return_type.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Struct => RustYamlStructInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                fields: self.fields.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Enum => RustYamlEnumInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                variants: self.variants.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::ExternCrate => RustYamlExternCrateInput {
                alias: self.alias.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::ForeignModule => RustYamlForeignModuleInput {
                abi: self.abi.take(),
                is_unsafe: self.is_unsafe.take().unwrap_or(false),
                items: self.items.take().unwrap_or_default(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Trait => RustYamlTraitInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                is_unsafe: self.is_unsafe.take().unwrap_or(false),
                is_auto: self.is_auto.take().unwrap_or(false),
                items: self.items.take().unwrap_or_default(),
                supertraits: self.supertraits.take().unwrap_or_default(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::TraitAlias => RustYamlTraitAliasInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                supertraits: self.supertraits.take().unwrap_or_default(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Union => RustYamlUnionInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                fields: self.fields.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Static => RustYamlStaticInput {
                type_text: self.type_text.take_required(
                    signature_type,
                    "type",
                    decoder.catalog_name,
                )?,
                mutable: self.mutable.take().unwrap_or(false),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Macro => RustYamlMacroInput {
                tokens: self.tokens.take_required(
                    signature_type,
                    "tokens",
                    decoder.catalog_name,
                )?,
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Module => RustYamlModuleInput {
                is_inline: self.is_inline.take().unwrap_or(false),
                path_override: self.path.take(),
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::TypeAlias => RustYamlTypeAliasInput {
                generics: self.generics.take(),
                where_predicates: self.where_predicates.take().unwrap_or_default(),
                target_type: self.target_type.take_required(
                    signature_type,
                    "target_type",
                    decoder.catalog_name,
                )?,
            }
            .into_declaration(base, decoder)?,
            RustYamlSignatureType::Reexport => RustYamlReexportInput {
                path: self
                    .path
                    .take_required(signature_type, "path", decoder.catalog_name)?,
                alias: self.alias.take(),
            }
            .into_declaration(base, decoder)?,
        };
        Ok((declaration, implementations))
    }

    pub(super) fn validate_shape(&self) -> Result<(), RustYamlShapeError> {
        if self.signature_type != RustYamlSignatureType::Module {
            self.reject_present(self.is_inline.is_present(), "inline")?;
        }
        match self.signature_type {
            RustYamlSignatureType::Constant => {
                self.reject_structural_fields()?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.alias.is_present(), "alias")?;
                self.reject_present(self.path.is_present(), "path")?;
                self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.require_present(self.type_text.is_present(), "type")?;
                self.require_present(self.value.is_present(), "value")?;
            }
            RustYamlSignatureType::Function => {
                self.reject_non_callable_fields()?;
                self.reject_extended_fields()?;
            }
            RustYamlSignatureType::Struct => {
                self.reject_callable_fields()?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.items.is_present(), "items")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.reject_extended_fields()?;
            }
            RustYamlSignatureType::Enum => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.items.is_present(), "items")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.reject_extended_fields()?;
            }
            RustYamlSignatureType::ExternCrate => {
                self.reject_structural_fields()?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.path.is_present(), "path")?;
                self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::ForeignModule => {
                self.reject_present(self.qualifiers.is_present(), "qualifiers")?;
                self.reject_present(self.variadic.is_present(), "variadic")?;
                self.reject_present(self.generics.is_present(), "generics")?;
                self.reject_present(self.where_predicates.is_present(), "where")?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.implementations.is_present(), "implementations")?;
                self.reject_present(self.parameters.is_present(), "parameters")?;
                self.reject_present(self.return_type.is_present(), "return_type")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.alias.is_present(), "alias")?;
                self.reject_present(self.path.is_present(), "path")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
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
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.alias.is_present(), "alias")?;
                self.reject_present(self.path.is_present(), "path")?;
            }
            RustYamlSignatureType::TraitAlias => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.items.is_present(), "items")?;
                self.reject_present(self.implementations.is_present(), "implementations")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.alias.is_present(), "alias")?;
                self.reject_present(self.path.is_present(), "path")?;
                self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Union => {
                self.reject_callable_fields()?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.items.is_present(), "items")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.reject_extended_fields()?;
            }
            RustYamlSignatureType::Static => {
                self.reject_structural_fields()?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.reject_extended_fields()?;
                self.require_present(self.type_text.is_present(), "type")?;
            }
            RustYamlSignatureType::Macro => {
                self.reject_structural_fields()?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_extended_fields()?;
                self.require_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::Module => {
                self.reject_structural_fields()?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.alias.is_present(), "alias")?;
                self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
            }
            RustYamlSignatureType::TypeAlias => {
                self.reject_callable_fields()?;
                self.reject_present(self.fields.is_present(), "fields")?;
                self.reject_present(self.variants.is_present(), "variants")?;
                self.reject_present(self.items.is_present(), "items")?;
                self.reject_present(self.supertraits.is_present(), "supertraits")?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.reject_extended_fields()?;
                self.require_present(self.target_type.is_present(), "target_type")?;
            }
            RustYamlSignatureType::Reexport => {
                self.reject_structural_fields()?;
                self.reject_present(self.type_text.is_present(), "type")?;
                self.reject_present(self.value.is_present(), "value")?;
                self.reject_present(self.target_type.is_present(), "target_type")?;
                self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
                self.reject_present(self.is_auto.is_present(), "auto")?;
                self.reject_present(self.mutable.is_present(), "mutable")?;
                self.reject_present(self.tokens.is_present(), "tokens")?;
                self.require_present(self.path.is_present(), "path")?;
            }
        }
        Ok(())
    }

    pub(super) fn reject_non_callable_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_present(self.fields.is_present(), "fields")?;
        self.reject_present(self.variants.is_present(), "variants")?;
        self.reject_present(self.items.is_present(), "items")?;
        self.reject_present(self.implementations.is_present(), "implementations")?;
        self.reject_present(self.supertraits.is_present(), "supertraits")?;
        self.reject_present(self.type_text.is_present(), "type")?;
        self.reject_present(self.target_type.is_present(), "target_type")?;
        self.reject_present(self.mutable.is_present(), "mutable")?;
        self.reject_present(self.tokens.is_present(), "tokens")
    }

    pub(super) fn reject_callable_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_present(self.qualifiers.is_present(), "qualifiers")?;
        self.reject_present(self.abi.is_present(), "abi")?;
        self.reject_present(self.variadic.is_present(), "variadic")?;
        self.reject_present(self.parameters.is_present(), "parameters")?;
        self.reject_present(self.return_type.is_present(), "return_type")
    }

    pub(super) fn reject_structural_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_callable_fields()?;
        self.reject_present(self.generics.is_present(), "generics")?;
        self.reject_present(self.where_predicates.is_present(), "where")?;
        self.reject_present(self.fields.is_present(), "fields")?;
        self.reject_present(self.variants.is_present(), "variants")?;
        self.reject_present(self.items.is_present(), "items")?;
        self.reject_present(self.implementations.is_present(), "implementations")?;
        self.reject_present(self.supertraits.is_present(), "supertraits")
    }

    pub(super) fn reject_extended_fields(&self) -> Result<(), RustYamlShapeError> {
        self.reject_present(self.value.is_present(), "value")?;
        self.reject_present(self.alias.is_present(), "alias")?;
        self.reject_present(self.path.is_present(), "path")?;
        self.reject_present(self.is_unsafe.is_present(), "unsafe")?;
        self.reject_present(self.is_auto.is_present(), "auto")
    }

    pub(super) fn reject_present(
        &self,
        present: bool,
        field: &'static str,
    ) -> Result<(), RustYamlShapeError> {
        if !present {
            return Ok(());
        }
        Err(RustYamlShapeError::FieldNotAllowed {
            signature_type: self.signature_type.as_str(),
            field,
        })
    }

    pub(super) fn require_present(
        &self,
        present: bool,
        field: &'static str,
    ) -> Result<(), RustYamlShapeError> {
        if present {
            return Ok(());
        }
        Err(RustYamlShapeError::MissingField {
            signature_type: self.signature_type.as_str(),
            field,
        })
    }
}

pub(super) enum RustYamlShapeError {
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

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(in crate::languages::rust::parser::yaml) enum RustYamlSignatureType {
    Constant,
    Function,
    Struct,
    Enum,
    ExternCrate,
    ForeignModule,
    Trait,
    TraitAlias,
    Union,
    Static,
    Macro,
    Module,
    TypeAlias,
    Reexport,
}

impl RustYamlSignatureType {
    pub(in crate::languages::rust::parser::yaml) fn as_str(self) -> &'static str {
        match self {
            Self::Constant => "constant",
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::ExternCrate => "extern_crate",
            Self::ForeignModule => "foreign_module",
            Self::Trait => "trait",
            Self::TraitAlias => "trait_alias",
            Self::Union => "union",
            Self::Static => "static",
            Self::Macro => "macro",
            Self::Module => "module",
            Self::TypeAlias => "type_alias",
            Self::Reexport => "reexport",
        }
    }
}
