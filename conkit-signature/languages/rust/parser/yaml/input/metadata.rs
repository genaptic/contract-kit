use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::attributes::{
    RustAttribute, RustAttributes, RustCfgPredicate, RustConditional, RustDeprecation,
    RustExportAttribute, RustLinkageAttribute, RustNativeLibrary, RustPath, RustRawAttribute,
    RustRawAttributeArguments, RustRepr, RustReprHint, RustTokenSyntax,
};
use crate::languages::rust::types::callable_type::RustVariadicParameter;
use crate::languages::rust::types::primitive_types::{
    RustGenericMetadata, RustGenericParameter, Visibility,
};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::work::CancellationProbe;
use quote::ToTokens;
use serde::{Deserialize, Serialize, de};

pub(super) struct RustYamlShorthandCatalogPath {
    pub(super) value: CatalogPath,
}

impl RustYamlShorthandCatalogPath {
    pub(super) fn to_catalog_path(&self) -> CatalogPath {
        self.value.clone()
    }
}

impl<'de> Deserialize<'de> for RustYamlShorthandCatalogPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| de::Error::custom("Rust YAML catalog paths must use shorthand text"))?;
        let path = CatalogPath::new(value).map_err(de::Error::custom)?;

        Ok(Self { value: path })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(in crate::languages::rust::parser::yaml) struct RustYamlAttributesValue(
    Vec<RustYamlAttributeValue>,
);

impl RustYamlAttributesValue {
    pub(in crate::languages::rust::parser::yaml) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn to_attributes(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAttributes, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let values = self.to_values(catalog_name, cancellation)?;
        RustAttributes::new(values, cancellation).map_err(|source| {
            if source.limit_exceeded().is_some() || source.is_operation_canceled() {
                source
            } else {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            }
        })
    }

    pub(super) fn to_values(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustAttribute>, SignatureContractKitError> {
        let mut attributes = Vec::with_capacity(self.0.len());
        for value in &self.0 {
            cancellation.checkpoint()?;
            attributes.push(value.to_attribute(catalog_name, cancellation)?);
        }
        Ok(attributes)
    }

    pub(in crate::languages::rust::parser::yaml) fn from_attributes(
        attributes: &RustAttributes,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut values = Vec::with_capacity(attributes.values().len());
        for attribute in attributes.values() {
            cancellation.checkpoint()?;
            values.push(RustYamlAttributeValue::from_attribute(
                attribute,
                cancellation,
            )?);
        }
        Ok(Self(values))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RustYamlAttributeValue {
    Derive(Vec<String>),
    Repr(Vec<String>),
    NonExhaustive,
    Cfg(String),
    CfgAttr {
        predicate: String,
        attributes: Vec<RustYamlAttributeValue>,
    },
    Deprecated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    MustUse(Option<String>),
    DocHidden,
    NoMangle,
    ExportName(String),
    LinkSection(String),
    LinkName(String),
    Link(RustYamlNativeLibraryValue),
    Unresolved {
        path: String,
        arguments: RustYamlRawAttributeArgumentsValue,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "style", content = "value", rename_all = "snake_case")]
pub(super) enum RustYamlRawAttributeArgumentsValue {
    Path,
    List(String),
    NameValue(String),
}

impl RustYamlAttributeValue {
    pub(super) fn to_attribute(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustAttribute, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Derive(paths) => {
                let mut parsed = Vec::with_capacity(paths.len());
                for path in paths {
                    cancellation.checkpoint()?;
                    parsed.push(RustPath::new(path.clone()).map_err(|source| {
                        SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                    })?);
                }
                Ok(RustAttribute::Derive(parsed))
            }
            Self::Repr(hints) => {
                let mut parsed = Vec::with_capacity(hints.len());
                for hint in hints {
                    cancellation.checkpoint()?;
                    parsed.push(Self::repr_hint(hint, catalog_name)?);
                }
                Ok(RustAttribute::Repr(RustRepr::new(parsed)?))
            }
            Self::NonExhaustive => Ok(RustAttribute::NonExhaustive),
            Self::Cfg(predicate) => Ok(RustAttribute::Conditional(RustConditional::Cfg(
                RustCfgPredicate::new(predicate.clone(), cancellation).map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                })?,
            ))),
            Self::CfgAttr {
                predicate,
                attributes,
            } => {
                if attributes.is_empty() {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        "Rust YAML cfg_attr requires at least one nested semantic attribute",
                    ));
                }
                let attributes = Self::nested_attributes(attributes, catalog_name, cancellation)?;
                Ok(RustAttribute::Conditional(RustConditional::CfgAttr {
                    predicate: RustCfgPredicate::new(predicate.clone(), cancellation).map_err(
                        |source| {
                            SignatureContractKitError::parse_failed(
                                catalog_name,
                                source.to_string(),
                            )
                        },
                    )?,
                    attributes,
                }))
            }
            Self::Deprecated { since, note } => Ok(RustAttribute::Deprecated(
                RustDeprecation::new(since.clone(), note.clone()),
            )),
            Self::MustUse(message) => Ok(RustAttribute::MustUse(message.clone())),
            Self::DocHidden => Ok(RustAttribute::DocHidden),
            Self::NoMangle => Ok(RustAttribute::Export(RustExportAttribute::NoMangle)),
            Self::ExportName(name) => Ok(RustAttribute::Export(RustExportAttribute::Name(
                name.clone(),
            ))),
            Self::LinkSection(section) => Ok(RustAttribute::Linkage(
                RustLinkageAttribute::Section(section.clone()),
            )),
            Self::LinkName(name) => Ok(RustAttribute::Linkage(RustLinkageAttribute::Name(
                name.clone(),
            ))),
            Self::Link(library) => Ok(RustAttribute::Linkage(RustLinkageAttribute::Library(
                library.to_library(catalog_name)?,
            ))),
            Self::Unresolved { path, arguments } => {
                let path = RustPath::new(path.clone()).map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                })?;
                let arguments = arguments.to_arguments(catalog_name, cancellation)?;
                RustRawAttribute::new(path, arguments)
                    .map(RustAttribute::Unresolved)
                    .map_err(|source| {
                        SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                    })
            }
        }
    }

    pub(super) fn nested_attributes(
        values: &[Self],
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<RustAttribute>, SignatureContractKitError> {
        let mut attributes = Vec::with_capacity(values.len());
        for value in values {
            cancellation.checkpoint()?;
            attributes.push(value.to_attribute(catalog_name, cancellation)?);
        }
        Ok(attributes)
    }

    pub(super) fn repr_hint(
        value: &str,
        catalog_name: &CatalogPath,
    ) -> Result<RustReprHint, SignatureContractKitError> {
        let integer = |name: &str| {
            value
                .strip_prefix(name)
                .and_then(|value| value.strip_prefix('('))
                .and_then(|value| value.strip_suffix(')'))
                .and_then(|value| value.parse::<u32>().ok())
        };
        let hint = match value {
            "C" => RustReprHint::C,
            "transparent" => RustReprHint::Transparent,
            "simd" => RustReprHint::Simd,
            "i8" => RustReprHint::I8,
            "i16" => RustReprHint::I16,
            "i32" => RustReprHint::I32,
            "i64" => RustReprHint::I64,
            "i128" => RustReprHint::I128,
            "isize" => RustReprHint::Isize,
            "u8" => RustReprHint::U8,
            "u16" => RustReprHint::U16,
            "u32" => RustReprHint::U32,
            "u64" => RustReprHint::U64,
            "u128" => RustReprHint::U128,
            "usize" => RustReprHint::Usize,
            "packed" => RustReprHint::Packed(None),
            _ => match (integer("align"), integer("packed")) {
                (Some(alignment), _) => RustReprHint::Align(alignment),
                (_, Some(packing)) => RustReprHint::Packed(Some(packing)),
                _ => {
                    return Err(SignatureContractKitError::parse_failed(
                        catalog_name,
                        format!("unsupported Rust YAML repr hint {value}"),
                    ));
                }
            },
        };
        Ok(hint)
    }

    pub(super) fn from_attribute(
        attribute: &RustAttribute,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(match attribute {
            RustAttribute::Derive(paths) => {
                let mut values = Vec::with_capacity(paths.len());
                for path in paths {
                    cancellation.checkpoint()?;
                    values.push(path.as_str().to_owned());
                }
                Self::Derive(values)
            }
            RustAttribute::Repr(repr) => {
                let mut values = Vec::with_capacity(repr.hints().len());
                for hint in repr.hints() {
                    cancellation.checkpoint()?;
                    values.push(hint.as_str());
                }
                Self::Repr(values)
            }
            RustAttribute::NonExhaustive => Self::NonExhaustive,
            RustAttribute::Conditional(RustConditional::Cfg(predicate)) => {
                Self::Cfg(predicate.as_str().to_owned())
            }
            RustAttribute::Conditional(RustConditional::CfgAttr {
                predicate,
                attributes,
            }) => Self::CfgAttr {
                predicate: predicate.as_str().to_owned(),
                attributes: {
                    let mut nested = Vec::with_capacity(attributes.len());
                    for attribute in attributes {
                        cancellation.checkpoint()?;
                        nested.push(Self::from_attribute(attribute, cancellation)?);
                    }
                    nested
                },
            },
            RustAttribute::Deprecated(value) => Self::Deprecated {
                since: value.since().map(str::to_owned),
                note: value.note().map(str::to_owned),
            },
            RustAttribute::MustUse(message) => Self::MustUse(message.clone()),
            RustAttribute::DocHidden => Self::DocHidden,
            RustAttribute::Export(RustExportAttribute::NoMangle) => Self::NoMangle,
            RustAttribute::Export(RustExportAttribute::Name(name)) => {
                Self::ExportName(name.clone())
            }
            RustAttribute::Linkage(RustLinkageAttribute::Section(section)) => {
                Self::LinkSection(section.clone())
            }
            RustAttribute::Linkage(RustLinkageAttribute::Name(name)) => {
                Self::LinkName(name.clone())
            }
            RustAttribute::Linkage(RustLinkageAttribute::Library(library)) => {
                Self::Link(RustYamlNativeLibraryValue::from_library(library))
            }
            RustAttribute::Unresolved(raw) => Self::Unresolved {
                path: raw.path().to_owned(),
                arguments: RustYamlRawAttributeArgumentsValue::from_arguments(raw.arguments()),
            },
        })
    }
}

impl RustYamlRawAttributeArgumentsValue {
    pub(super) fn to_arguments(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustRawAttributeArguments, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let syntax = |value: &str| {
            RustTokenSyntax::new(value.to_owned()).map_err(|source| {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            })
        };
        match self {
            Self::Path => Ok(RustRawAttributeArguments::Path),
            Self::List(value) => syntax(value).map(RustRawAttributeArguments::List),
            Self::NameValue(value) => RustSyntaxText::parse_expression(value)
                .map(RustRawAttributeArguments::NameValue)
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                }),
        }
    }

    pub(super) fn from_arguments(arguments: &RustRawAttributeArguments) -> Self {
        match arguments {
            RustRawAttributeArguments::Path => Self::Path,
            RustRawAttributeArguments::List(value) => Self::List(value.as_str().to_owned()),
            RustRawAttributeArguments::NameValue(value) => {
                Self::NameValue(value.as_str().to_owned())
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlNativeLibraryValue {
    pub(super) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) modifiers: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RustYamlCallableQualifierInput {
    Const,
    Async,
    Unsafe,
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RustYamlImplementationQualifierInput {
    Default,
    Unsafe,
}

#[derive(Default)]
pub(super) struct RustYamlCallableQualifiers {
    pub(super) is_const: bool,
    pub(super) is_async: bool,
    pub(super) is_unsafe: bool,
}

impl RustYamlCallableQualifiers {
    pub(super) fn from_inputs(
        inputs: &[RustYamlCallableQualifierInput],
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut qualifiers = Self::default();

        for input in inputs {
            cancellation.checkpoint()?;
            match input {
                RustYamlCallableQualifierInput::Const => qualifiers.is_const = true,
                RustYamlCallableQualifierInput::Async => qualifiers.is_async = true,
                RustYamlCallableQualifierInput::Unsafe => qualifiers.is_unsafe = true,
            }
        }

        Ok(qualifiers)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlShorthandVariadicInput {
    Present(bool),
    Details(RustYamlShorthandVariadicDetailsInput),
}

impl RustYamlShorthandVariadicInput {
    pub(super) fn to_variadic(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Option<RustVariadicParameter>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Present(false) => Ok(None),
            Self::Present(true) => Ok(Some(RustVariadicParameter::new(
                None,
                RustAttributes::default(),
            ))),
            Self::Details(details) => details.to_variadic(catalog_name, cancellation).map(Some),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlShorthandVariadicDetailsInput {
    pub(super) pattern: Option<String>,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

impl RustYamlShorthandVariadicDetailsInput {
    pub(super) fn to_variadic(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustVariadicParameter, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if let Some(pattern) = &self.pattern
            && pattern.trim().is_empty()
        {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML variadic pattern cannot be empty",
            ));
        }

        Ok(RustVariadicParameter::new(
            self.pattern
                .as_deref()
                .map(RustSyntaxText::parse_pattern)
                .transpose()
                .map_err(|source| {
                    SignatureContractKitError::parse_failed(catalog_name, source.to_string())
                })?,
            self.attributes.to_attributes(catalog_name, cancellation)?,
        ))
    }
}

pub(super) struct RustYamlVisibilityInput {
    pub(super) value: String,
}

impl RustYamlVisibilityInput {
    pub(super) fn to_visibility(
        &self,
        current_module: &RustModuleId,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        if self.value.trim() != self.value {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                "Rust YAML visibility must not contain surrounding whitespace",
            ));
        }
        match self.value.as_str() {
            "public" => Ok(Visibility::Public),
            "crate" => Ok(Visibility::Crate),
            "private" => Ok(Visibility::Private),
            value => self.module_visibility(value, current_module, catalog_name),
        }
    }

    pub(super) fn module_visibility(
        &self,
        value: &str,
        current_module: &RustModuleId,
        catalog_name: &CatalogPath,
    ) -> Result<Visibility, SignatureContractKitError> {
        let module = value
            .strip_prefix("module(")
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| {
                SignatureContractKitError::parse_failed(
                    catalog_name,
                    "Rust YAML visibility must be public, crate, private, or module(CRATE::ANCESTOR)",
                )
            })?;
        let mut segments = module.split("::");
        let crate_id = segments.next().unwrap_or_default();
        if crate_id != current_module.crate_id().as_str() {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "Rust YAML visibility module {module} is outside current crate {}",
                    current_module.crate_id()
                ),
            ));
        }
        let path =
            RustModulePath::new(segments.map(str::to_owned).collect()).map_err(|source| {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            })?;
        let target = RustModuleId::new(current_module.crate_id().clone(), path);
        if !target.is_strict_ancestor_of(current_module) {
            return Err(SignatureContractKitError::parse_failed(
                catalog_name,
                format!(
                    "Rust YAML visibility module {target} is not an ancestor of {current_module}"
                ),
            ));
        }
        Ok(Visibility::Module(target))
    }
}

impl<'de> Deserialize<'de> for RustYamlVisibilityInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)
            .map_err(|_| de::Error::custom("Rust YAML visibility must use shorthand text"))?;

        Ok(Self { value })
    }
}

pub(super) struct RustYamlGenericParametersInput {
    pub(super) values: Vec<RustYamlGenericParameterInput>,
}

impl RustYamlGenericParametersInput {
    pub(super) fn to_metadata_from(
        input: Option<&Self>,
        where_predicates: &[String],
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustGenericMetadata, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let parameters_input = input
            .map(|value| value.values.as_slice())
            .unwrap_or_default();
        let mut parameters = Vec::with_capacity(parameters_input.len());
        for value in parameters_input {
            cancellation.checkpoint()?;
            parameters.push(value.to_parameter(catalog_name, cancellation)?);
        }
        let mut parsed_predicates = Vec::with_capacity(where_predicates.len());
        for predicate in where_predicates {
            cancellation.checkpoint()?;
            parsed_predicates.push(RustSyntaxText::parse_where_predicate(predicate).map_err(
                |source| SignatureContractKitError::parse_failed(catalog_name, source.to_string()),
            )?);
        }
        Ok(RustGenericMetadata::new(parameters).with_where_predicates(parsed_predicates))
    }
}

impl<'de> Deserialize<'de> for RustYamlGenericParametersInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Vec::<RustYamlGenericParameterInput>::deserialize(deserializer)
            .map(|values| Self { values })
            .map_err(|_| de::Error::custom("Rust YAML generics must be a shorthand list"))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RustYamlGenericParameterInput {
    Shorthand(String),
    Details(RustYamlGenericParameterDetailsInput),
}

impl RustYamlGenericParameterInput {
    pub(super) fn to_parameter(
        &self,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<RustGenericParameter, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let (declaration, attributes) = match self {
            Self::Shorthand(declaration) => (declaration.as_str(), RustAttributes::default()),
            Self::Details(details) => (
                details.declaration.as_str(),
                details
                    .attributes
                    .to_attributes(catalog_name, cancellation)?,
            ),
        };
        RustYamlParsedGenericParameter::parse(declaration, catalog_name, cancellation)
            .and_then(|parameter| parameter.into_parameter(cancellation))
            .map(|parameter| parameter.with_attributes(attributes))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RustYamlGenericParameterDetailsInput {
    pub(super) declaration: String,
    #[serde(default)]
    pub(super) attributes: RustYamlAttributesValue,
}

pub(super) struct RustYamlParsedGenericParameter {
    pub(super) value: syn::GenericParam,
}

impl RustYamlParsedGenericParameter {
    pub(super) fn parse(
        value: &str,
        catalog_name: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let parsed = syn::parse_str::<syn::GenericParam>(value.trim())
            .map(|value| Self { value })
            .map_err(|source| {
                SignatureContractKitError::parse_failed(
                    catalog_name,
                    format!(
                        "Rust YAML generic parameter must be valid shorthand Rust syntax: {source}"
                    ),
                )
            })?;
        cancellation.checkpoint()?;
        Ok(parsed)
    }

    pub(super) fn into_parameter(
        self,
        cancellation: &CancellationProbe,
    ) -> Result<RustGenericParameter, SignatureContractKitError> {
        cancellation.checkpoint()?;
        Ok(match self.value {
            syn::GenericParam::Type(parameter) => {
                let mut bounds = Vec::with_capacity(parameter.bounds.len());
                for bound in &parameter.bounds {
                    cancellation.checkpoint()?;
                    bounds.push(RustYamlTokenText::new(bound).render());
                }
                RustGenericParameter::type_parameter(
                    RustModulePath::semantic_ident(&parameter.ident),
                    bounds,
                    parameter
                        .default
                        .as_ref()
                        .map(|value| RustYamlTokenText::new(value).render()),
                )
            }
            syn::GenericParam::Lifetime(parameter) => {
                let mut bounds = Vec::with_capacity(parameter.bounds.len());
                for bound in &parameter.bounds {
                    cancellation.checkpoint()?;
                    bounds.push(RustYamlTokenText::new(bound).render());
                }
                RustGenericParameter::lifetime_parameter(parameter.lifetime.to_string(), bounds)
            }
            syn::GenericParam::Const(parameter) => RustGenericParameter::const_parameter(
                RustModulePath::semantic_ident(&parameter.ident),
                RustYamlTokenText::new(&parameter.ty).render(),
                parameter
                    .default
                    .as_ref()
                    .map(|value| RustYamlTokenText::new(value).render()),
            ),
        })
    }
}

pub(super) struct RustYamlTokenText {
    pub(super) value: String,
}

impl RustYamlTokenText {
    pub(super) fn new(value: &impl ToTokens) -> Self {
        Self {
            value: value.to_token_stream().to_string(),
        }
    }

    pub(super) fn render(self) -> String {
        self.value
    }
}

impl RustYamlNativeLibraryValue {
    pub(super) fn to_library(
        &self,
        catalog_name: &CatalogPath,
    ) -> Result<RustNativeLibrary, SignatureContractKitError> {
        RustNativeLibrary::new(self.name.clone(), self.kind.clone(), self.modifiers.clone())
            .map_err(|source| {
                SignatureContractKitError::parse_failed(catalog_name, source.to_string())
            })
    }

    pub(super) fn from_library(library: &RustNativeLibrary) -> Self {
        Self {
            name: library.name().to_owned(),
            kind: library.kind().map(str::to_owned),
            modifiers: library.modifiers().map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RustYamlAttributeValue, RustYamlAttributesValue};
    use crate::files::CatalogPath;

    #[test]
    fn nested_attribute_materialization_stops_when_canceled() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let input = RustYamlAttributesValue(vec![RustYamlAttributeValue::CfgAttr {
            predicate: "all()".to_owned(),
            attributes: (0..4_096)
                .map(|index| RustYamlAttributeValue::MustUse(Some(format!("message-{index}"))))
                .collect(),
        }]);
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = input
            .to_attributes(&catalog_name, &cancellation)
            .expect_err("canceled nested attribute materialization must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn large_nested_attribute_materialization_preserves_every_attribute() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let nested_count = 4_096;
        let input = RustYamlAttributesValue(vec![RustYamlAttributeValue::CfgAttr {
            predicate: "all()".to_owned(),
            attributes: (0..nested_count)
                .map(|index| RustYamlAttributeValue::MustUse(Some(format!("message-{index}"))))
                .collect(),
        }]);
        let cancellation = crate::work::CancellationProbe::new();

        let attributes = input
            .to_attributes(&catalog_name, &cancellation)
            .expect("large nested attributes must materialize");
        let crate::languages::rust::types::attributes::RustAttribute::Conditional(
            crate::languages::rust::types::attributes::RustConditional::CfgAttr {
                attributes, ..
            },
        ) = &attributes.values()[0]
        else {
            panic!("expected cfg_attr semantic attribute");
        };

        assert_eq!(attributes.len(), nested_count);
    }

    #[test]
    fn yaml_cfg_predicates_use_the_rust_reference_grammar() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let cancellation = crate::work::CancellationProbe::new();
        let valid = RustYamlAttributesValue(vec![RustYamlAttributeValue::Cfg(
            "any(all(), feature = r#\"fast\"#, not(windows))".to_owned(),
        )]);
        valid
            .to_attributes(&catalog_name, &cancellation)
            .expect("valid YAML cfg predicate");

        for predicate in [
            "crate::unix",
            "crate",
            "true = \"value\"",
            "feature = 1",
            "target_os(\"linux\")",
            "not(unix,)",
            "all(,)",
        ] {
            let invalid =
                RustYamlAttributesValue(vec![RustYamlAttributeValue::Cfg(predicate.to_owned())]);
            let error = invalid
                .to_attributes(&catalog_name, &cancellation)
                .expect_err("balanced invalid YAML cfg predicate must fail");
            assert!(error.to_string().contains("main.yml"), "{error}");
        }
    }

    #[test]
    fn yaml_cfg_attr_keeps_canonical_nonempty_nested_attributes() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let cancellation = crate::work::CancellationProbe::new();
        let input = RustYamlAttributesValue(vec![RustYamlAttributeValue::CfgAttr {
            predicate: "true".to_owned(),
            attributes: vec![RustYamlAttributeValue::MustUse(None)],
        }]);

        input
            .to_attributes(&catalog_name, &cancellation)
            .expect("canonical nonempty YAML cfg_attr");

        let empty = RustYamlAttributesValue(vec![RustYamlAttributeValue::CfgAttr {
            predicate: "true".to_owned(),
            attributes: Vec::new(),
        }]);
        empty
            .to_attributes(&catalog_name, &cancellation)
            .expect_err("canonical YAML must not encode a source-only no-op cfg_attr");

        let invalid = RustYamlAttributesValue(vec![RustYamlAttributeValue::CfgAttr {
            predicate: "feature = 1".to_owned(),
            attributes: vec![RustYamlAttributeValue::MustUse(None)],
        }]);
        invalid
            .to_attributes(&catalog_name, &cancellation)
            .expect_err("canonical YAML cfg_attr predicates use Rust cfg grammar");
    }

    #[test]
    fn yaml_repr_rejects_the_noncanonical_rust_default_hint() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let input =
            RustYamlAttributesValue(vec![RustYamlAttributeValue::Repr(vec!["Rust".to_owned()])]);

        let error = input
            .to_attributes(&catalog_name, &crate::work::CancellationProbe::new())
            .expect_err("canonical YAML must omit the default Rust representation");

        assert!(error.to_string().contains("main.yml"), "{error}");
        assert!(error.to_string().contains("Rust"), "{error}");
    }
}
