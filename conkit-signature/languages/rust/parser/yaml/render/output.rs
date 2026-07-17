use super::super::input::{
    RustYamlAttributesValue, RustYamlDocument, RustYamlDocumentLocation, RustYamlSignatureType,
};
use super::super::type_text::RustTypeTextRenderer;
use super::lossless::RustYamlLosslessEditor;
use super::proposal::{RustYamlDocumentOrigin, RustYamlGeneratedDocument, RustYamlSourceGroup};
use crate::api::SignatureGenerationCounts;
use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::RustParsedEntry;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::{RustCrateId, RustModulePath};
use crate::languages::rust::types::associated_item::RustAssociatedItem;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{
    RustCallableSignature, RustFunctionAbi, RustMethod, RustReceiver, RustVariadicParameter,
};
use crate::languages::rust::types::declaration::{
    ForeignModuleType, RustDeclaration, RustForeignItem, RustItemKind,
};
use crate::languages::rust::types::enum_type::{EnumVariant, EnumVariantField};
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementedTrait,
};
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustGenericParameter, RustType, Visibility,
};
use crate::languages::rust::types::struct_type::StructField;
use crate::limits::{GeneratedOutputMeter, YamlUsage};
use crate::work::CancellationProbe;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) enum RustYamlOutput {
    New(Box<RustYamlGeneratedDocument>),
    Existing {
        catalog_name: CatalogPath,
        original_bytes: Arc<[u8]>,
        documents: Vec<RustYamlGeneratedDocument>,
    },
}

#[cfg(test)]
mod tests {
    use super::{RustYamlAttributesValue, RustYamlSanitizedLabel};
    use crate::languages::rust::types::attributes::{RustAttribute, RustAttributes};

    #[test]
    fn label_sanitization_stops_when_generation_is_canceled() {
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = RustYamlSanitizedLabel::new("Example", &cancellation)
            .err()
            .expect("canceled label sanitization must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn output_attribute_materialization_stops_when_canceled() {
        let construction = crate::work::CancellationProbe::new();
        let attributes = RustAttributes::new(vec![RustAttribute::DocHidden; 4_096], &construction)
            .expect("attribute fixture");
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = RustYamlAttributesValue::from_attributes(&attributes, &cancellation)
            .expect_err("canceled output attribute materialization must stop");

        assert!(error.is_operation_canceled());
    }
}

pub(super) struct RustYamlLabelPlanner {
    retained: BTreeMap<RustItemId, String>,
    reserved: BTreeSet<String>,
}

impl RustYamlLabelPlanner {
    pub(super) fn new() -> Self {
        Self {
            retained: BTreeMap::new(),
            reserved: BTreeSet::new(),
        }
    }

    pub(super) fn from_existing(
        location: &RustYamlDocumentLocation,
        document: &RustYamlDocument,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut planner = Self::new();

        for signature in &document.signatures {
            cancellation.checkpoint()?;
            planner.reserved.insert(signature.label.clone());

            let Some(primary) = signature.entries.first() else {
                continue;
            };
            let structural_key = primary.id().clone();
            if let Some(previous) = planner
                .retained
                .insert(structural_key.clone(), signature.label.clone())
                && previous != signature.label
            {
                return Err(SignatureContractKitError::write_failed(
                    location,
                    format!(
                        "structural signature {} uses both {previous} and {}",
                        structural_key.render(),
                        signature.label
                    ),
                ));
            }
        }

        Ok(planner)
    }

    pub(super) fn plan(
        &self,
        groups: &[RustYamlSourceGroup<'_>],
        cancellation: &CancellationProbe,
    ) -> Result<BTreeMap<RustItemId, String>, SignatureContractKitError> {
        let mut used = self.reserved.clone();
        let mut next_ordinals = BTreeMap::new();
        let mut labels = BTreeMap::new();
        for group in groups {
            cancellation.checkpoint()?;
            let structural_key = group.structural_key();
            if let Some(retained) = self.retained.get(structural_key) {
                labels.insert(structural_key.clone(), retained.clone());
                continue;
            }

            let base = Self::base(group, cancellation)?;
            let qualified = self.qualified(group, &base, cancellation)?;
            let label =
                self.allocate(base, qualified, &mut used, &mut next_ordinals, cancellation)?;
            labels.insert(structural_key.clone(), label);
        }
        Ok(labels)
    }

    fn qualified(
        &self,
        group: &RustYamlSourceGroup<'_>,
        base: &str,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        let mut components = vec![group.primary.id().module_id().crate_id().to_string()];
        for segment in group.primary.id().module_id().module_path().segments() {
            cancellation.checkpoint()?;
            components.push(segment.clone());
        }
        components.push(base.to_owned());
        let prefix = components.join("_");
        RustYamlSanitizedLabel::new(&prefix, cancellation).map(RustYamlSanitizedLabel::into_string)
    }

    fn allocate(
        &self,
        base: String,
        qualified: String,
        used: &mut BTreeSet<String>,
        next_ordinals: &mut BTreeMap<String, usize>,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        if used.insert(base.clone()) {
            return Ok(base);
        }
        if used.insert(qualified.clone()) {
            return Ok(qualified);
        }

        let ordinal = next_ordinals.entry(qualified.clone()).or_insert(2);
        loop {
            cancellation.checkpoint()?;
            let candidate = format!("{qualified}_{ordinal}");
            *ordinal = ordinal.checked_add(1).ok_or_else(|| {
                SignatureContractKitError::conversion_failed(
                    "generated signature label space is exhausted",
                )
            })?;
            if used.insert(candidate.clone()) {
                return Ok(candidate);
            }
        }
    }

    fn base(
        group: &RustYamlSourceGroup<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        let name =
            RustYamlSanitizedLabel::new(group.primary.id().name(), cancellation)?.into_string();
        Ok(match group.primary.id().kind() {
            RustItemKind::Constant => format!("{name}_constant"),
            RustItemKind::Function => format!("{name}_function"),
            RustItemKind::Struct => format!("{name}_struct"),
            RustItemKind::Enum => format!("{name}_enum"),
            RustItemKind::ExternCrate => format!("{name}_extern_crate"),
            RustItemKind::ForeignModule => format!("{name}_foreign_module"),
            RustItemKind::Trait => format!("{name}_trait"),
            RustItemKind::TraitAlias => format!("{name}_trait_alias"),
            RustItemKind::Union => format!("{name}_union"),
            RustItemKind::Static => format!("{name}_static"),
            RustItemKind::Macro => format!("{name}_macro"),
            RustItemKind::Module => format!("{name}_module"),
            RustItemKind::TypeAlias => format!("{name}_type_alias"),
            RustItemKind::Reexport => format!("{name}_reexport"),
            RustItemKind::Implementation => name,
        })
    }
}

pub(super) struct RustYamlSanitizedLabel {
    value: String,
}

impl RustYamlSanitizedLabel {
    pub(super) fn new(
        value: &str,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut output = String::new();
        for (index, character) in value.chars().enumerate() {
            if index % 1024 == 0 {
                cancellation.checkpoint()?;
            }
            if character.is_ascii_alphanumeric() {
                output.push(character.to_ascii_lowercase());
            } else if !output.ends_with('_') {
                output.push('_');
            }
        }
        cancellation.checkpoint()?;
        Ok(Self {
            value: output.trim_matches('_').to_owned(),
        })
    }

    pub(super) fn into_string(self) -> String {
        self.value
    }
}

#[derive(Serialize)]
pub(super) struct RustYamlRenderedSignature {
    crate_id: RustCrateId,
    file: String,
    #[serde(flatten)]
    declaration: RustYamlRenderedDeclaration,
    #[serde(skip_serializing_if = "Option::is_none")]
    sketch: Option<String>,
}

#[derive(Serialize)]
struct RustYamlRenderedMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    module_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
}

#[derive(Serialize)]
struct RustYamlRenderedGenerics {
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<RustYamlGenericParameterOutput>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
}

#[derive(Serialize)]
struct RustYamlRenderedCallable {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variadic: Option<RustYamlShorthandVariadicOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<RustYamlFunctionParameterOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
}

struct RustYamlDeclarationRenderer<'entry, 'operation> {
    entry: &'entry RustParsedEntry,
    label: &'entry str,
    cancellation: &'operation CancellationProbe,
}

#[derive(Serialize)]
#[serde(tag = "signature_type", rename_all = "snake_case")]
enum RustYamlRenderedDeclaration {
    Constant {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(rename = "type")]
        type_text: String,
        value: String,
    },
    Function {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(flatten)]
        callable: RustYamlRenderedCallable,
    },
    #[serde(rename = "struct")]
    Struct {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<RustYamlShorthandFieldsOutput>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        implementations: Vec<RustYamlShorthandImplementationOutput>,
    },
    #[serde(rename = "enum")]
    Enum {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        variants: Vec<RustYamlShorthandVariantOutput>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        implementations: Vec<RustYamlShorthandImplementationOutput>,
    },
    ExternCrate {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },
    ForeignModule {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        items: Option<Vec<RustYamlNestedItemOutput>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        abi: Option<String>,
        #[serde(rename = "unsafe", skip_serializing_if = "std::ops::Not::not")]
        is_unsafe: bool,
    },
    Trait {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Option::is_none")]
        items: Option<Vec<RustYamlNestedItemOutput>>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        supertraits: Vec<String>,
        #[serde(rename = "unsafe", skip_serializing_if = "std::ops::Not::not")]
        is_unsafe: bool,
        #[serde(rename = "auto", skip_serializing_if = "std::ops::Not::not")]
        is_auto: bool,
    },
    TraitAlias {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        supertraits: Vec<String>,
    },
    Union {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<RustYamlShorthandFieldsOutput>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        implementations: Vec<RustYamlShorthandImplementationOutput>,
    },
    Static {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(rename = "type")]
        type_text: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        mutable: bool,
    },
    Macro {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        tokens: String,
    },
    Module {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(rename = "inline", skip_serializing_if = "std::ops::Not::not")]
        is_inline: bool,
    },
    TypeAlias {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(flatten)]
        generics: RustYamlRenderedGenerics,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        implementations: Vec<RustYamlShorthandImplementationOutput>,
        target_type: String,
    },
    Reexport {
        #[serde(flatten)]
        metadata: RustYamlRenderedMetadata,
        #[serde(skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        path: String,
    },
}

impl RustYamlRenderedSignature {
    pub(super) fn from_group(
        group: &RustYamlSourceGroup<'_>,
        sketch: Option<String>,
        label: &str,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut implementations = Vec::with_capacity(group.implementations.len());
        for entry in &group.implementations {
            cancellation.checkpoint()?;
            let RustDeclaration::Implementation(value) = entry.declaration() else {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation group contains a non-implementation signature",
                ));
            };
            implementations.push(RustYamlShorthandImplementationOutput::from_implementation(
                value,
                cancellation,
            )?);
        }

        Ok(Self {
            crate_id: group.primary.id().module_id().crate_id().clone(),
            file: group.primary.file().as_str().to_owned(),
            declaration: RustYamlDeclarationRenderer::new(group.primary, label, cancellation)
                .render(implementations)?,
            sketch,
        })
    }

    pub(super) fn signature_type(&self) -> RustYamlSignatureType {
        self.declaration.signature_type()
    }

    pub(super) fn sketch(&self) -> Option<&str> {
        self.sketch.as_deref()
    }

    fn type_text(
        value: &RustType,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let rendered = RustTypeTextRenderer.render_type(value);
        cancellation.checkpoint()?;
        Ok(rendered)
    }
}

impl<'entry, 'operation> RustYamlDeclarationRenderer<'entry, 'operation> {
    fn new(
        entry: &'entry RustParsedEntry,
        label: &'entry str,
        cancellation: &'operation CancellationProbe,
    ) -> Self {
        Self {
            entry,
            label,
            cancellation,
        }
    }

    fn render(
        self,
        implementations: Vec<RustYamlShorthandImplementationOutput>,
    ) -> Result<RustYamlRenderedDeclaration, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        let supports_implementations = matches!(
            self.entry.declaration(),
            RustDeclaration::Structure(_)
                | RustDeclaration::Enumeration(_)
                | RustDeclaration::Union(_)
                | RustDeclaration::TypeAlias(_)
        );
        if !supports_implementations && !implementations.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation group has a declaration that cannot own implementations",
            ));
        }

        match self.entry.declaration() {
            RustDeclaration::Constant(value) => Ok(RustYamlRenderedDeclaration::Constant {
                metadata: self.metadata(value.base())?,
                type_text: self.type_text(value.constant_type())?,
                value: value.value().to_owned(),
            }),
            RustDeclaration::Function(value) => Ok(RustYamlRenderedDeclaration::Function {
                metadata: self.metadata(value.base())?,
                generics: self.generics(value.signature().generics())?,
                callable: RustYamlRenderedCallable::from_signature(
                    value.signature(),
                    self.cancellation,
                )?,
            }),
            RustDeclaration::Structure(value) => Ok(RustYamlRenderedDeclaration::Struct {
                metadata: self.metadata(value.base())?,
                generics: self.generics(value.generics())?,
                fields: RustYamlShorthandFieldsOutput::from_fields(
                    value.fields(),
                    self.cancellation,
                )?,
                implementations,
            }),
            RustDeclaration::Enumeration(value) => {
                let mut variants = Vec::with_capacity(value.variants().len());
                for variant in value.variants() {
                    self.cancellation.checkpoint()?;
                    variants.push(RustYamlShorthandVariantOutput::from_variant(
                        variant,
                        self.cancellation,
                    )?);
                }
                Ok(RustYamlRenderedDeclaration::Enum {
                    metadata: self.metadata(value.base())?,
                    generics: self.generics(value.generics())?,
                    variants,
                    implementations,
                })
            }
            RustDeclaration::ExternCrate(value) => Ok(RustYamlRenderedDeclaration::ExternCrate {
                metadata: self.metadata(value.base())?,
                alias: value.alias().map(str::to_owned),
            }),
            RustDeclaration::ForeignModule(value) => {
                Ok(RustYamlRenderedDeclaration::ForeignModule {
                    metadata: self.foreign_metadata(value)?,
                    items: RustYamlNestedItemOutput::from_foreign_items(
                        value.items(),
                        self.cancellation,
                    )?,
                    abi: RustYamlRenderedCallable::abi(value.abi()),
                    is_unsafe: value.is_unsafe(),
                })
            }
            RustDeclaration::Trait(value) => {
                let mut supertraits = Vec::with_capacity(value.supertraits().len());
                for supertrait in value.supertraits() {
                    self.cancellation.checkpoint()?;
                    supertraits.push(supertrait.clone());
                }
                Ok(RustYamlRenderedDeclaration::Trait {
                    metadata: self.metadata(value.base())?,
                    generics: self.generics(value.generics())?,
                    items: RustYamlNestedItemOutput::from_associated_items(
                        value.items(),
                        self.cancellation,
                    )?,
                    supertraits,
                    is_unsafe: value.is_unsafe(),
                    is_auto: value.is_auto(),
                })
            }
            RustDeclaration::TraitAlias(value) => {
                let mut supertraits = Vec::with_capacity(value.supertraits().len());
                for bound in value.supertraits() {
                    self.cancellation.checkpoint()?;
                    supertraits.push(bound.as_str().to_owned());
                }
                Ok(RustYamlRenderedDeclaration::TraitAlias {
                    metadata: self.metadata(value.base())?,
                    generics: self.generics(value.generics())?,
                    supertraits,
                })
            }
            RustDeclaration::Union(value) => Ok(RustYamlRenderedDeclaration::Union {
                metadata: self.metadata(value.base())?,
                generics: self.generics(value.generics())?,
                fields: RustYamlShorthandFieldsOutput::from_fields(
                    value.fields(),
                    self.cancellation,
                )?,
                implementations,
            }),
            RustDeclaration::Static(value) => Ok(RustYamlRenderedDeclaration::Static {
                metadata: self.metadata(value.base())?,
                type_text: self.type_text(value.static_type())?,
                mutable: value.mutable(),
            }),
            RustDeclaration::Macro(value) => Ok(RustYamlRenderedDeclaration::Macro {
                metadata: self.metadata(value.base())?,
                tokens: value.tokens().to_owned(),
            }),
            RustDeclaration::Module(value) => Ok(RustYamlRenderedDeclaration::Module {
                metadata: self.metadata(value.base())?,
                path: value.path_override().map(str::to_owned),
                is_inline: value.is_inline(),
            }),
            RustDeclaration::TypeAlias(value) => Ok(RustYamlRenderedDeclaration::TypeAlias {
                metadata: self.metadata(value.base())?,
                generics: self.generics(value.generics())?,
                implementations,
                target_type: self.type_text(value.target_type())?,
            }),
            RustDeclaration::Reexport(value) => Ok(RustYamlRenderedDeclaration::Reexport {
                metadata: self.metadata(value.base())?,
                alias: value.alias().map(str::to_owned),
                path: value.path().to_owned(),
            }),
            RustDeclaration::Implementation(_) => {
                Err(SignatureContractKitError::conversion_failed(
                    "implementation entries cannot be top-level contract signatures",
                ))
            }
        }
    }
    fn metadata(
        &self,
        base: &BaseType,
    ) -> Result<RustYamlRenderedMetadata, SignatureContractKitError> {
        RustYamlRenderedMetadata::new(
            self.entry,
            self.label,
            Some(base.visibility()),
            base.attributes(),
            self.cancellation,
        )
    }

    fn foreign_metadata(
        &self,
        value: &ForeignModuleType,
    ) -> Result<RustYamlRenderedMetadata, SignatureContractKitError> {
        RustYamlRenderedMetadata::new(
            self.entry,
            self.label,
            None,
            value.attributes(),
            self.cancellation,
        )
    }

    fn generics(
        &self,
        metadata: &RustGenericMetadata,
    ) -> Result<RustYamlRenderedGenerics, SignatureContractKitError> {
        RustYamlRenderedGenerics::from_metadata(metadata, self.cancellation)
    }

    fn type_text(&self, value: &RustType) -> Result<String, SignatureContractKitError> {
        RustYamlRenderedSignature::type_text(value, self.cancellation)
    }
}

impl RustYamlRenderedDeclaration {
    fn signature_type(&self) -> RustYamlSignatureType {
        match self {
            Self::Constant { .. } => RustYamlSignatureType::Constant,
            Self::Function { .. } => RustYamlSignatureType::Function,
            Self::Struct { .. } => RustYamlSignatureType::Struct,
            Self::Enum { .. } => RustYamlSignatureType::Enum,
            Self::ExternCrate { .. } => RustYamlSignatureType::ExternCrate,
            Self::ForeignModule { .. } => RustYamlSignatureType::ForeignModule,
            Self::Trait { .. } => RustYamlSignatureType::Trait,
            Self::TraitAlias { .. } => RustYamlSignatureType::TraitAlias,
            Self::Union { .. } => RustYamlSignatureType::Union,
            Self::Static { .. } => RustYamlSignatureType::Static,
            Self::Macro { .. } => RustYamlSignatureType::Macro,
            Self::Module { .. } => RustYamlSignatureType::Module,
            Self::TypeAlias { .. } => RustYamlSignatureType::TypeAlias,
            Self::Reexport { .. } => RustYamlSignatureType::Reexport,
        }
    }
}

impl RustYamlRenderedMetadata {
    fn new(
        entry: &RustParsedEntry,
        label: &str,
        visibility: Option<&Visibility>,
        attributes: &RustAttributes,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let id = entry.id();
        let mut module_path = Vec::with_capacity(id.module_id().module_path().segments().len());
        for segment in id.module_id().module_path().segments() {
            cancellation.checkpoint()?;
            module_path.push(segment.clone());
        }
        Ok(Self {
            name: (label != id.name()).then(|| id.name().to_owned()),
            module_path,
            visibility: visibility.map(Self::visibility),
            attributes: RustYamlAttributesValue::from_attributes(attributes, cancellation)?,
        })
    }

    fn visibility(visibility: &Visibility) -> String {
        match visibility {
            Visibility::Public => "public".to_owned(),
            Visibility::Crate => "crate".to_owned(),
            Visibility::Module(module) => format!("module({module})"),
            Visibility::Private => "private".to_owned(),
        }
    }
}

impl RustYamlRenderedGenerics {
    fn from_metadata(
        metadata: &RustGenericMetadata,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut parameters = Vec::with_capacity(metadata.parameters().len());
        for parameter in metadata.parameters() {
            cancellation.checkpoint()?;
            parameters.push(Self::generic_parameter(parameter, cancellation)?);
        }
        let mut predicates = Vec::with_capacity(metadata.where_predicates().len());
        for predicate in metadata.where_predicates() {
            cancellation.checkpoint()?;
            predicates.push(predicate.as_str().to_owned());
        }
        Ok(Self {
            generics: (!parameters.is_empty()).then_some(parameters),
            where_predicates: (!predicates.is_empty()).then_some(predicates),
        })
    }

    fn generic_parameter(
        parameter: &RustGenericParameter,
        cancellation: &CancellationProbe,
    ) -> Result<RustYamlGenericParameterOutput, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let declaration = match parameter {
            RustGenericParameter::Type {
                name,
                bounds,
                default,
                ..
            } => Self::bounded_parameter(
                RustModulePath::source_ident(name),
                bounds,
                default.as_deref(),
                cancellation,
            ),
            RustGenericParameter::Lifetime { name, bounds, .. } => {
                Self::bounded_parameter(name.clone(), bounds, None, cancellation)
            }
            RustGenericParameter::Const {
                name,
                parameter_type,
                default,
                ..
            } => Self::bounded_parameter(
                format!(
                    "const {}: {parameter_type}",
                    RustModulePath::source_ident(name)
                ),
                &[],
                default.as_deref(),
                cancellation,
            ),
        }?;
        let attributes =
            RustYamlAttributesValue::from_attributes(parameter.attributes(), cancellation)?;
        if attributes.is_empty() {
            Ok(RustYamlGenericParameterOutput::Shorthand(declaration))
        } else {
            Ok(RustYamlGenericParameterOutput::Details {
                declaration,
                attributes,
            })
        }
    }

    fn bounded_parameter(
        mut value: String,
        bounds: &[String],
        default: Option<&str>,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        if !bounds.is_empty() {
            value.push_str(": ");
            for (index, bound) in bounds.iter().enumerate() {
                cancellation.checkpoint()?;
                if index > 0 {
                    value.push_str(" + ");
                }
                value.push_str(bound);
            }
        }

        if let Some(default) = default {
            value.push_str(" = ");
            value.push_str(default);
        }

        Ok(value)
    }
}

impl RustYamlRenderedCallable {
    fn from_signature(
        callable: &RustCallableSignature,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut parameters = Vec::with_capacity(callable.parameters().len());
        for parameter in callable.parameters() {
            cancellation.checkpoint()?;
            parameters.push(RustYamlFunctionParameterOutput::from_parameter(
                parameter,
                cancellation,
            )?);
        }
        Ok(Self {
            qualifiers: Self::qualifiers(callable),
            abi: Self::abi(callable.abi()),
            variadic: callable
                .variadic()
                .map(|value| RustYamlShorthandVariadicOutput::from_variadic(value, cancellation))
                .transpose()?,
            parameters: (!parameters.is_empty()).then_some(parameters),
            return_type: callable
                .return_type()
                .map(|value| RustYamlRenderedSignature::type_text(value, cancellation))
                .transpose()?,
        })
    }

    fn qualifiers(callable: &RustCallableSignature) -> Vec<&'static str> {
        let mut qualifiers = Vec::new();
        if callable.is_const() {
            qualifiers.push("const");
        }
        if callable.is_async() {
            qualifiers.push("async");
        }
        if callable.is_unsafe() {
            qualifiers.push("unsafe");
        }
        qualifiers
    }

    fn abi(abi: &RustFunctionAbi) -> Option<String> {
        match abi {
            RustFunctionAbi::Rust => None,
            RustFunctionAbi::Extern { name } => {
                Some(name.clone().unwrap_or_else(|| "extern".to_owned()))
            }
        }
    }
}

#[derive(Serialize)]
struct RustYamlFunctionParameterOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<serde_saphyr::DoubleQuoted<String>>,
    #[serde(rename = "type")]
    type_text: String,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
}

impl RustYamlFunctionParameterOutput {
    fn from_parameter(
        parameter: &RustFunctionParameter,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(Self {
            // `yaml-edit` 0.2.x tokenizes commas as flow delimiters even in a
            // block plain scalar. Rust patterns commonly contain commas, so
            // use the semantic serializer's typed scalar-style wrapper rather
            // than teaching the lossless editor a textual YAML exception.
            pattern: parameter
                .pattern()
                .map(str::to_owned)
                .map(serde_saphyr::DoubleQuoted),
            type_text: RustYamlRenderedSignature::type_text(
                parameter.parameter_type(),
                cancellation,
            )?,
            attributes: RustYamlAttributesValue::from_attributes(
                parameter.attributes(),
                cancellation,
            )?,
        })
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlGenericParameterOutput {
    Shorthand(String),
    Details {
        declaration: String,
        attributes: RustYamlAttributesValue,
    },
}

#[derive(Serialize)]
struct RustYamlCallableBodyOutput {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variadic: Option<RustYamlShorthandVariadicOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<RustYamlGenericParameterOutput>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<RustYamlFunctionParameterOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
}

impl RustYamlCallableBodyOutput {
    fn from_callable(
        callable: &RustCallableSignature,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let generics = RustYamlRenderedGenerics::from_metadata(callable.generics(), cancellation)?;
        let rendered = RustYamlRenderedCallable::from_signature(callable, cancellation)?;
        Ok(Self {
            qualifiers: rendered.qualifiers,
            abi: rendered.abi,
            variadic: rendered.variadic,
            generics: generics.generics,
            where_predicates: generics.where_predicates,
            parameters: rendered.parameters,
            return_type: rendered.return_type,
        })
    }
}

enum RustYamlShorthandFieldsOutput {
    Named(Vec<RustYamlShorthandNamedFieldOutput>),
    Unnamed(Vec<RustYamlShorthandFieldValueOutput>),
}

struct RustYamlShorthandNamedFieldOutput {
    name: String,
    value: RustYamlShorthandFieldValueOutput,
}

impl Serialize for RustYamlShorthandFieldsOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Named(fields) => {
                use serde::ser::SerializeMap;
                let mut mapping = serializer.serialize_map(Some(fields.len()))?;
                for field in fields {
                    mapping.serialize_entry(&field.name, &field.value)?;
                }
                mapping.end()
            }
            Self::Unnamed(fields) => fields.serialize(serializer),
        }
    }
}

impl RustYamlShorthandFieldsOutput {
    fn from_fields(
        fields: &[StructField],
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if fields.is_empty() {
            return Ok(None);
        }
        let mut named = Vec::with_capacity(fields.len());
        let mut all_named = true;
        for field in fields {
            cancellation.checkpoint()?;
            let Some(name) = field.name() else {
                all_named = false;
                break;
            };
            named.push(RustYamlShorthandNamedFieldOutput {
                name: name.to_owned(),
                value: RustYamlShorthandFieldValueOutput::from_field(field, cancellation)?,
            });
        }
        if all_named {
            return Ok(Some(Self::Named(named)));
        }
        let mut unnamed = Vec::with_capacity(fields.len());
        for field in fields {
            cancellation.checkpoint()?;
            unnamed.push(RustYamlShorthandFieldValueOutput::from_field(
                field,
                cancellation,
            )?);
        }
        Ok(Some(Self::Unnamed(unnamed)))
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandFieldValueOutput {
    Type(String),
    Details(RustYamlShorthandFieldDetailsOutput),
}

impl RustYamlShorthandFieldValueOutput {
    fn from_field(
        field: &StructField,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let attributes =
            RustYamlAttributesValue::from_attributes(field.attributes(), cancellation)?;
        if matches!(field.visibility(), Visibility::Private) && attributes.is_empty() {
            return Ok(Self::Type(RustYamlRenderedSignature::type_text(
                field.field_type(),
                cancellation,
            )?));
        }

        Ok(Self::Details(RustYamlShorthandFieldDetailsOutput {
            field_type: RustYamlRenderedSignature::type_text(field.field_type(), cancellation)?,
            visibility: RustYamlRenderedMetadata::visibility(field.visibility()),
            attributes,
        }))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandFieldDetailsOutput {
    #[serde(rename = "type")]
    field_type: String,
    visibility: String,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariadicOutput {
    Present(bool),
    Details(RustYamlShorthandVariadicDetailsOutput),
}

impl RustYamlShorthandVariadicOutput {
    fn from_variadic(
        value: &RustVariadicParameter,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if value.pattern().is_none() && value.attributes().values().is_empty() {
            return Ok(Self::Present(true));
        }

        Ok(Self::Details(RustYamlShorthandVariadicDetailsOutput {
            pattern: value
                .pattern()
                .map(str::to_owned)
                .map(serde_saphyr::DoubleQuoted),
            attributes: RustYamlAttributesValue::from_attributes(value.attributes(), cancellation)?,
        }))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandVariadicDetailsOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<serde_saphyr::DoubleQuoted<String>>,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariantOutput {
    Unit(String),
    Tuple(BTreeMap<String, Vec<String>>),
    Details(BTreeMap<String, RustYamlShorthandVariantDetailsOutput>),
}

impl RustYamlShorthandVariantOutput {
    fn from_variant(
        variant: &EnumVariant,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let fields =
            RustYamlShorthandVariantFieldsOutput::from_fields(variant.fields(), cancellation)?;
        let discriminant = variant.discriminant().map(str::to_owned);
        let attributes =
            RustYamlAttributesValue::from_attributes(variant.attributes(), cancellation)?;

        if fields.is_none() && discriminant.is_none() && attributes.is_empty() {
            return Ok(Self::Unit(variant.name().to_owned()));
        }

        if let Some(RustYamlShorthandVariantFieldsOutput::Tuple(fields)) = fields.as_ref()
            && discriminant.is_none()
            && attributes.is_empty()
        {
            let mut types = Vec::with_capacity(fields.len());
            let mut shorthand = true;
            for field in fields {
                cancellation.checkpoint()?;
                match field.type_text() {
                    Some(field_type) => types.push(field_type.to_owned()),
                    None => {
                        shorthand = false;
                        break;
                    }
                }
            }
            if shorthand {
                return Ok(Self::Tuple(BTreeMap::from([(
                    variant.name().to_owned(),
                    types,
                )])));
            }
        }

        Ok(Self::Details(BTreeMap::from([(
            variant.name().to_owned(),
            RustYamlShorthandVariantDetailsOutput {
                fields,
                discriminant,
                attributes,
            },
        )])))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandVariantDetailsOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<RustYamlShorthandVariantFieldsOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discriminant: Option<String>,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariantFieldsOutput {
    Tuple(Vec<RustYamlVariantFieldValueOutput>),
    Named(Vec<BTreeMap<String, RustYamlVariantFieldValueOutput>>),
}

impl RustYamlShorthandVariantFieldsOutput {
    fn from_fields(
        fields: &[EnumVariantField],
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if fields.is_empty() {
            return Ok(None);
        }

        let mut all_unnamed = true;
        for field in fields {
            cancellation.checkpoint()?;
            if field.name().is_some() {
                all_unnamed = false;
                break;
            }
        }
        if all_unnamed {
            let mut tuple = Vec::with_capacity(fields.len());
            for field in fields {
                cancellation.checkpoint()?;
                tuple.push(RustYamlVariantFieldValueOutput::from_field(
                    field,
                    cancellation,
                )?);
            }
            return Ok(Some(Self::Tuple(tuple)));
        }

        let mut named = Vec::with_capacity(fields.len());
        for field in fields {
            cancellation.checkpoint()?;
            if let Some(name) = field.name() {
                named.push(BTreeMap::from([(
                    name.to_owned(),
                    RustYamlVariantFieldValueOutput::from_field(field, cancellation)?,
                )]));
            }
        }
        Ok(Some(Self::Named(named)))
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlVariantFieldValueOutput {
    Type(String),
    Details(RustYamlVariantFieldDetailsOutput),
}

impl RustYamlVariantFieldValueOutput {
    fn from_field(
        field: &EnumVariantField,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let field_type = RustYamlRenderedSignature::type_text(field.field_type(), cancellation)?;
        let attributes =
            RustYamlAttributesValue::from_attributes(field.attributes(), cancellation)?;
        if attributes.is_empty() {
            Ok(Self::Type(field_type))
        } else {
            Ok(Self::Details(RustYamlVariantFieldDetailsOutput {
                field_type,
                attributes,
            }))
        }
    }

    fn type_text(&self) -> Option<&str> {
        match self {
            Self::Type(value) => Some(value),
            Self::Details(_) => None,
        }
    }
}

#[derive(Serialize)]
struct RustYamlVariantFieldDetailsOutput {
    #[serde(rename = "type")]
    field_type: String,
    attributes: RustYamlAttributesValue,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlNestedItemOutput {
    Associated(RustYamlAssociatedItemOutput),
    Foreign(RustYamlForeignItemOutput),
}

impl RustYamlNestedItemOutput {
    fn from_associated_items(
        items: &[RustAssociatedItem],
        cancellation: &CancellationProbe,
    ) -> Result<Option<Vec<Self>>, SignatureContractKitError> {
        let mut output = Vec::with_capacity(items.len());
        for item in items {
            cancellation.checkpoint()?;
            output.push(Self::Associated(RustYamlAssociatedItemOutput::from_item(
                item,
                true,
                cancellation,
            )?));
        }
        Ok((!output.is_empty()).then_some(output))
    }

    fn from_foreign_items(
        items: &[RustForeignItem],
        cancellation: &CancellationProbe,
    ) -> Result<Option<Vec<Self>>, SignatureContractKitError> {
        let mut output = Vec::with_capacity(items.len());
        for item in items {
            cancellation.checkpoint()?;
            output.push(Self::Foreign(RustYamlForeignItemOutput::from_item(
                item,
                cancellation,
            )?));
        }
        Ok((!output.is_empty()).then_some(output))
    }
}

#[derive(Serialize)]
#[serde(tag = "signature_type", rename_all = "snake_case")]
enum RustYamlForeignItemOutput {
    #[serde(rename = "foreign_function")]
    Function {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
        #[serde(flatten)]
        callable: RustYamlCallableBodyOutput,
    },
    #[serde(rename = "foreign_static")]
    Static {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mutable: Option<bool>,
        #[serde(rename = "type")]
        type_text: String,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
    },
    #[serde(rename = "foreign_type")]
    Type {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
    },
    #[serde(rename = "foreign_macro")]
    Macro {
        tokens: String,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
    },
}

impl RustYamlForeignItemOutput {
    fn from_item(
        item: &RustForeignItem,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(match item {
            RustForeignItem::Function(function) => {
                let function = function.function();
                Self::Function {
                    name: function.base().name().to_owned(),
                    visibility: Self::explicit_visibility(function.base().visibility()),
                    attributes: RustYamlAttributesValue::from_attributes(
                        function.base().attributes(),
                        cancellation,
                    )?,
                    callable: RustYamlCallableBodyOutput::from_callable(
                        function.signature(),
                        cancellation,
                    )?,
                }
            }
            RustForeignItem::Static(value) => {
                let value = value.value();
                Self::Static {
                    name: value.base().name().to_owned(),
                    visibility: Self::explicit_visibility(value.base().visibility()),
                    mutable: value.mutable().then_some(true),
                    type_text: RustYamlRenderedSignature::type_text(
                        value.static_type(),
                        cancellation,
                    )?,
                    attributes: RustYamlAttributesValue::from_attributes(
                        value.base().attributes(),
                        cancellation,
                    )?,
                }
            }
            RustForeignItem::Type(value) => Self::Type {
                name: value.base().name().to_owned(),
                visibility: Self::explicit_visibility(value.base().visibility()),
                attributes: RustYamlAttributesValue::from_attributes(
                    value.base().attributes(),
                    cancellation,
                )?,
            },
            RustForeignItem::Macro(value) => Self::Macro {
                tokens: value.tokens().to_owned(),
                attributes: RustYamlAttributesValue::from_attributes(
                    value.attributes(),
                    cancellation,
                )?,
            },
        })
    }

    fn explicit_visibility(visibility: &Visibility) -> Option<String> {
        (!matches!(visibility, Visibility::Private))
            .then(|| RustYamlRenderedMetadata::visibility(visibility))
    }
}

#[derive(Serialize)]
#[serde(tag = "signature_type", rename_all = "snake_case")]
enum RustYamlAssociatedItemOutput {
    Method {
        name: String,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        receiver: Option<String>,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        receiver_attributes: RustYamlAttributesValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default_body: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        specialization_default: Option<bool>,
        #[serde(flatten)]
        callable: RustYamlCallableBodyOutput,
    },
    AssociatedConstant {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(rename = "type")]
        type_text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        default_value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        specialization_default: Option<bool>,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
    },
    AssociatedType {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        generics: Option<Vec<RustYamlGenericParameterOutput>>,
        #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
        where_predicates: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        bounds: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        specialization_default: Option<bool>,
        #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
        attributes: RustYamlAttributesValue,
    },
}

impl RustYamlAssociatedItemOutput {
    fn from_items(
        items: &[RustAssociatedItem],
        trait_items: bool,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Vec<Self>>, SignatureContractKitError> {
        let mut output = Vec::with_capacity(items.len());
        for item in items {
            cancellation.checkpoint()?;
            output.push(Self::from_item(item, trait_items, cancellation)?);
        }
        Ok((!output.is_empty()).then_some(output))
    }

    fn from_item(
        item: &RustAssociatedItem,
        trait_item: bool,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(match item {
            RustAssociatedItem::Method(method) => {
                Self::from_method(method, trait_item, cancellation)?
            }
            RustAssociatedItem::Constant(constant) => Self::AssociatedConstant {
                name: constant.name().to_owned(),
                visibility: Self::explicit_item_visibility(constant.visibility(), trait_item),
                type_text: RustYamlRenderedSignature::type_text(
                    constant.constant_type(),
                    cancellation,
                )?,
                default_value: constant.default_value().map(str::to_owned),
                specialization_default: constant.is_specialization_default().then_some(true),
                attributes: RustYamlAttributesValue::from_attributes(
                    constant.attributes(),
                    cancellation,
                )?,
            },
            RustAssociatedItem::Type(associated_type) => {
                let generics = RustYamlRenderedGenerics::from_metadata(
                    associated_type.generics(),
                    cancellation,
                )?;
                let mut bounds = Vec::with_capacity(associated_type.bounds().len());
                for bound in associated_type.bounds() {
                    cancellation.checkpoint()?;
                    bounds.push(bound.as_str().to_owned());
                }
                Self::AssociatedType {
                    name: associated_type.name().to_owned(),
                    visibility: Self::explicit_item_visibility(
                        associated_type.visibility(),
                        trait_item,
                    ),
                    generics: generics.generics,
                    where_predicates: generics.where_predicates,
                    bounds,
                    default_type: associated_type
                        .default_type()
                        .map(|value| RustYamlRenderedSignature::type_text(value, cancellation))
                        .transpose()?,
                    specialization_default: associated_type
                        .is_specialization_default()
                        .then_some(true),
                    attributes: RustYamlAttributesValue::from_attributes(
                        associated_type.attributes(),
                        cancellation,
                    )?,
                }
            }
        })
    }

    fn from_method(
        method: &RustMethod,
        trait_item: bool,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let function = method.function();
        let callable = function.signature();

        Ok(Self::Method {
            name: function.base().name().to_owned(),
            attributes: RustYamlAttributesValue::from_attributes(
                function.base().attributes(),
                cancellation,
            )?,
            receiver: method
                .receiver()
                .map(|value| Self::receiver(value, cancellation))
                .transpose()?,
            receiver_attributes: RustYamlAttributesValue::from_attributes(
                method.receiver_attributes(),
                cancellation,
            )?,
            visibility: Self::explicit_item_visibility(method.visibility(), trait_item),
            default_body: (trait_item && method.has_default_body()).then_some(true),
            specialization_default: method.is_specialization_default().then_some(true),
            callable: RustYamlCallableBodyOutput::from_callable(callable, cancellation)?,
        })
    }

    fn receiver(
        value: &RustReceiver,
        cancellation: &CancellationProbe,
    ) -> Result<String, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(match value {
            RustReceiver::Value { mutable: false } => "self".to_owned(),
            RustReceiver::Value { mutable: true } => "mut self".to_owned(),
            RustReceiver::Reference {
                lifetime: None,
                mutable: false,
            } => "ref".to_owned(),
            RustReceiver::Reference {
                lifetime: None,
                mutable: true,
            } => "mut".to_owned(),
            RustReceiver::Reference {
                lifetime: Some(lifetime),
                mutable,
            } => format!("&{lifetime} {}self", if *mutable { "mut " } else { "" }),
            RustReceiver::Typed {
                mutable,
                receiver_type,
            } => format!(
                "{}self: {}",
                if *mutable { "mut " } else { "" },
                RustYamlRenderedSignature::type_text(receiver_type, cancellation,)?
            ),
        })
    }

    fn explicit_item_visibility(visibility: &Visibility, trait_item: bool) -> Option<String> {
        let is_default = matches!(visibility, Visibility::Public) && trait_item
            || matches!(visibility, Visibility::Private) && !trait_item;
        (!is_default).then(|| RustYamlRenderedMetadata::visibility(visibility))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandImplementationOutput {
    #[serde(rename = "trait", skip_serializing_if = "Option::is_none")]
    implemented_trait: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    impl_qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<RustYamlGenericParameterOutput>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "RustYamlAttributesValue::is_empty")]
    attributes: RustYamlAttributesValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Vec<RustYamlAssociatedItemOutput>>,
}

impl RustYamlShorthandImplementationOutput {
    fn from_implementation(
        value: &ImplementationType,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let generics = RustYamlRenderedGenerics::from_metadata(value.generics(), cancellation)?;
        Ok(Self {
            implemented_trait: Self::implemented_trait(value),
            impl_qualifiers: Self::qualifiers(value),
            generics: generics.generics,
            where_predicates: generics.where_predicates,
            attributes: RustYamlAttributesValue::from_attributes(value.attributes(), cancellation)?,
            items: RustYamlAssociatedItemOutput::from_items(value.items(), false, cancellation)?,
        })
    }

    fn implemented_trait(value: &ImplementationType) -> Option<String> {
        match value.implemented_trait() {
            RustImplementedTrait::Inherent => None,
            RustImplementedTrait::Trait { path, polarity } => Some(match polarity {
                RustImplPolarity::Positive => path.as_str().to_owned(),
                RustImplPolarity::Negative => format!("!{}", path.as_str()),
            }),
        }
    }

    fn qualifiers(value: &ImplementationType) -> Vec<&'static str> {
        let mut qualifiers = Vec::new();
        if value.is_unsafe() {
            qualifiers.push("unsafe");
        }
        if value.is_default() {
            qualifiers.push("default");
        }
        qualifiers
    }
}

impl RustYamlOutput {
    pub(super) fn new(
        catalog_name: &CatalogPath,
        documents: Vec<RustYamlGeneratedDocument>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let Some(first) = documents.first() else {
            return Err(SignatureContractKitError::write_failed(
                catalog_name,
                "cannot render an empty YAML document set",
            ));
        };
        match &first.origin {
            RustYamlDocumentOrigin::New => {
                if documents.len() != 1 {
                    return Err(SignatureContractKitError::write_failed(
                        catalog_name,
                        "new generation supports exactly one semantic YAML document per file",
                    ));
                }
                let mut documents = documents.into_iter();
                let document = documents.next().ok_or_else(|| {
                    SignatureContractKitError::write_failed(
                        catalog_name,
                        "cannot render an empty YAML document set",
                    )
                })?;
                Ok(Self::New(Box::new(document)))
            }
            RustYamlDocumentOrigin::Existing { bytes, .. } => {
                let original_bytes = Arc::clone(bytes);
                for document in &documents {
                    cancellation.checkpoint()?;
                    match &document.origin {
                        RustYamlDocumentOrigin::Existing { bytes, .. }
                            if bytes.as_ref() == original_bytes.as_ref() => {}
                        RustYamlDocumentOrigin::New | RustYamlDocumentOrigin::Existing { .. } => {
                            return Err(SignatureContractKitError::write_failed(
                                catalog_name,
                                "semantic documents from one YAML file do not share the same original bytes",
                            ));
                        }
                    }
                }
                Ok(Self::Existing {
                    catalog_name: catalog_name.clone(),
                    original_bytes,
                    documents,
                })
            }
        }
    }

    pub(super) fn render(
        self,
        counts: &mut SignatureGenerationCounts,
        output: &mut GeneratedOutputMeter<'_>,
        yaml_usage: &mut YamlUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<u8>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::New(document) => {
                counts.semantically_changed_document_count += 1;
                counts.byte_changed_document_count += 1;
                document.as_ref().serialize(output)
            }
            Self::Existing {
                catalog_name,
                original_bytes,
                documents,
            } => {
                let mut semantically_changed_document_count = 0;
                for document in &documents {
                    cancellation.checkpoint()?;
                    if !document.unchanged(cancellation)? {
                        semantically_changed_document_count += 1;
                    }
                }
                if semantically_changed_document_count == 0 {
                    output.record(&catalog_name, original_bytes.len())?;
                    let mut preserved = Vec::with_capacity(original_bytes.len());
                    for chunk in original_bytes.chunks(64 * 1024) {
                        cancellation.checkpoint()?;
                        preserved.extend_from_slice(chunk);
                    }
                    return Ok(preserved);
                }
                let edited = RustYamlLosslessEditor::new(catalog_name, original_bytes)?.apply(
                    documents,
                    output,
                    yaml_usage,
                    cancellation,
                )?;
                counts.semantically_changed_document_count += semantically_changed_document_count;
                counts.byte_changed_document_count += semantically_changed_document_count;
                Ok(edited)
            }
        }
    }
}
