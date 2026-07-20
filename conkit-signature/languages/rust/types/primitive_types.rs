use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use proc_macro2::{Group, TokenStream, TokenTree};
use quote::ToTokens;
use serde::Serialize;
use syn::ext::IdentExt as _;
use syn::visit::Visit as _;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) enum Visibility {
    Public,
    Crate,
    Module(RustModuleId),
    #[default]
    Private,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustType {
    Bool,
    Char,
    Str,
    String,
    Never,
    SignedInteger(SignedIntegerType),
    UnsignedInteger(UnsignedIntegerType),
    Float(FloatType),
    Unit,
    Tuple(Vec<RustType>),
    Array(RustArrayType),
    Slice(Box<RustType>),
    Reference(RustReferenceType),
    RawPointer(RustRawPointerType),
    FunctionPointer(RustFunctionPointerType),
    TraitObject(RustTraitObjectType),
    ImplTrait(RustImplTraitType),
    TypePath(RustTypePath),
    GenericParameter(String),
    SelfType,
    Inferred,
    Parenthesized(Box<RustType>),
    MacroInvocation(String),
}

impl RustType {
    pub(crate) fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Tuple(elements) => elements.iter().any(Self::requires_capability_warning),
            Self::Array(array) => array.requires_capability_warning(),
            Self::Slice(element) | Self::Parenthesized(element) => {
                element.requires_capability_warning()
            }
            Self::Reference(reference) => reference.referenced_type().requires_capability_warning(),
            Self::RawPointer(pointer) => pointer.pointee_type().requires_capability_warning(),
            Self::FunctionPointer(function) => function.requires_capability_warning(),
            Self::TraitObject(trait_object) => trait_object.requires_capability_warning(),
            Self::ImplTrait(impl_trait) => impl_trait.requires_capability_warning(),
            Self::TypePath(path) => path.requires_capability_warning(),
            Self::MacroInvocation(_) => true,
            Self::Bool
            | Self::Char
            | Self::Str
            | Self::String
            | Self::Never
            | Self::SignedInteger(_)
            | Self::UnsignedInteger(_)
            | Self::Float(_)
            | Self::Unit
            | Self::GenericParameter(_)
            | Self::SelfType
            | Self::Inferred => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum SignedIntegerType {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum UnsignedIntegerType {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum FloatType {
    F32,
    F64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustArrayType {
    element_type: Box<RustType>,
    length: String,
}

impl RustArrayType {
    pub(crate) fn new(element_type: RustType, length: String) -> Self {
        Self {
            element_type: Box::new(element_type),
            length,
        }
    }

    pub(crate) fn element_type(&self) -> &RustType {
        &self.element_type
    }

    pub(crate) fn length(&self) -> &str {
        &self.length
    }

    fn requires_capability_warning(&self) -> bool {
        self.element_type.requires_capability_warning()
            || RustSyntaxCapabilityProbe::expression_contains_macro(&self.length)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustReferenceType {
    lifetime: Option<String>,
    mutable: bool,
    referenced_type: Box<RustType>,
}

impl RustReferenceType {
    pub(crate) fn new(lifetime: Option<String>, mutable: bool, referenced_type: RustType) -> Self {
        Self {
            lifetime,
            mutable,
            referenced_type: Box::new(referenced_type),
        }
    }

    pub(crate) fn lifetime(&self) -> Option<&str> {
        self.lifetime.as_deref()
    }

    pub(crate) fn mutable(&self) -> bool {
        self.mutable
    }

    pub(crate) fn referenced_type(&self) -> &RustType {
        &self.referenced_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustRawPointerType {
    mutable: bool,
    pointee_type: Box<RustType>,
}

impl RustRawPointerType {
    pub(crate) fn new(mutable: bool, pointee_type: RustType) -> Self {
        Self {
            mutable,
            pointee_type: Box::new(pointee_type),
        }
    }

    pub(crate) fn mutable(&self) -> bool {
        self.mutable
    }

    pub(crate) fn pointee_type(&self) -> &RustType {
        &self.pointee_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerType {
    lifetimes: Vec<String>,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    parameters: Vec<RustFunctionPointerParameter>,
    variadic: Option<RustFunctionPointerVariadic>,
    return_type: Option<Box<RustType>>,
}

impl RustFunctionPointerType {
    pub(crate) fn from_parts(
        parameters: Vec<RustFunctionPointerParameter>,
        return_type: Option<RustType>,
        is_unsafe: bool,
    ) -> Self {
        Self {
            lifetimes: Vec::new(),
            is_unsafe,
            abi: RustFunctionAbi::Rust,
            parameters,
            variadic: None,
            return_type: return_type.map(Box::new),
        }
    }

    pub(crate) fn with_lifetimes(mut self, lifetimes: Vec<String>) -> Self {
        self.lifetimes = lifetimes;
        self
    }

    pub(crate) fn with_abi(mut self, abi: RustFunctionAbi) -> Self {
        self.abi = abi;
        self
    }

    pub(crate) fn with_variadic(mut self, variadic: Option<RustFunctionPointerVariadic>) -> Self {
        self.variadic = variadic;
        self
    }

    pub(crate) fn lifetimes(&self) -> &[String] {
        &self.lifetimes
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn abi(&self) -> &RustFunctionAbi {
        &self.abi
    }

    pub(crate) fn parameters(&self) -> &[RustFunctionPointerParameter] {
        &self.parameters
    }

    pub(crate) fn variadic(&self) -> Option<&RustFunctionPointerVariadic> {
        self.variadic.as_ref()
    }

    pub(crate) fn return_type(&self) -> Option<&RustType> {
        self.return_type.as_deref()
    }

    fn requires_capability_warning(&self) -> bool {
        self.parameters
            .iter()
            .any(RustFunctionPointerParameter::requires_capability_warning)
            || self
                .variadic
                .as_ref()
                .is_some_and(|variadic| variadic.attributes.requires_capability_warning())
            || self
                .return_type
                .as_deref()
                .is_some_and(RustType::requires_capability_warning)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerParameter {
    attributes: RustAttributes,
    parameter_type: RustType,
}

impl RustFunctionPointerParameter {
    pub(crate) fn new(attributes: RustAttributes, parameter_type: RustType) -> Self {
        Self {
            attributes,
            parameter_type,
        }
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn parameter_type(&self) -> &RustType {
        &self.parameter_type
    }

    fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self.parameter_type.requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerVariadic {
    #[serde(skip)]
    pattern: Option<RustSyntaxText>,
    attributes: RustAttributes,
}

impl RustFunctionPointerVariadic {
    pub(crate) fn new(pattern: Option<RustSyntaxText>, attributes: RustAttributes) -> Self {
        Self {
            pattern,
            attributes,
        }
    }

    pub(crate) fn pattern(&self) -> Option<&str> {
        self.pattern.as_ref().map(RustSyntaxText::as_str)
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustTraitObjectType {
    bounds: Vec<String>,
}

impl RustTraitObjectType {
    pub(crate) fn new(bounds: Vec<String>) -> Self {
        Self { bounds }
    }

    pub(crate) fn bounds(&self) -> &[String] {
        &self.bounds
    }

    fn requires_capability_warning(&self) -> bool {
        self.bounds
            .iter()
            .any(|bound| RustSyntaxCapabilityProbe::bound_contains_macro(bound))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustImplTraitType {
    bounds: Vec<String>,
}

impl RustImplTraitType {
    pub(crate) fn new(bounds: Vec<String>) -> Self {
        Self { bounds }
    }

    pub(crate) fn bounds(&self) -> &[String] {
        &self.bounds
    }

    fn requires_capability_warning(&self) -> bool {
        self.bounds
            .iter()
            .any(|bound| RustSyntaxCapabilityProbe::bound_contains_macro(bound))
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RustTypePath {
    segments: Vec<String>,
    #[serde(skip)]
    source: String,
}

impl RustTypePath {
    pub(crate) fn new(segments: Vec<String>) -> Self {
        let source = segments
            .iter()
            .map(|segment| RustModulePath::source_ident(segment))
            .collect::<Vec<_>>()
            .join("::");
        Self { segments, source }
    }

    pub(crate) fn from_syn(path: &syn::TypePath) -> Self {
        if path.qself.is_some() {
            return Self::from_token_streams(vec![path.to_token_stream()]);
        }

        let mut converted = Self::from_token_streams(
            path.path
                .segments
                .iter()
                .map(|segment| segment.to_token_stream())
                .collect(),
        );
        if path.path.leading_colon.is_some() {
            converted.segments.insert(0, String::new());
            converted.source.insert_str(0, "::");
        }
        converted
    }

    pub(crate) fn from_path_segment(segment: &syn::PathSegment) -> Self {
        Self::from_token_streams(vec![segment.to_token_stream()])
    }

    pub(crate) fn source_syntax(&self) -> &str {
        &self.source
    }

    fn requires_capability_warning(&self) -> bool {
        RustSyntaxCapabilityProbe::type_contains_macro(self.source_syntax())
    }

    fn from_token_streams(streams: Vec<TokenStream>) -> Self {
        let source = streams
            .iter()
            .map(|tokens| tokens.to_string())
            .collect::<Vec<_>>()
            .join("::");
        let segments = streams
            .into_iter()
            .map(Self::canonicalize_tokens)
            .map(|tokens| tokens.to_string())
            .collect();

        Self { segments, source }
    }

    fn canonicalize_tokens(tokens: TokenStream) -> TokenStream {
        let mut macro_body_follows = false;
        tokens
            .into_iter()
            .map(|token| {
                let canonical = match token {
                    TokenTree::Group(group) if macro_body_follows => TokenTree::Group(group),
                    TokenTree::Group(group) => {
                        let mut canonical = Group::new(
                            group.delimiter(),
                            Self::canonicalize_tokens(group.stream()),
                        );
                        canonical.set_span(group.span());
                        TokenTree::Group(canonical)
                    }
                    TokenTree::Ident(ident) => TokenTree::Ident(ident.unraw()),
                    TokenTree::Punct(punctuation) => TokenTree::Punct(punctuation),
                    TokenTree::Literal(literal) => TokenTree::Literal(literal),
                };
                macro_body_follows = matches!(
                    &canonical,
                    TokenTree::Punct(punctuation) if punctuation.as_char() == '!'
                );
                canonical
            })
            .collect()
    }
}

impl PartialEq for RustTypePath {
    fn eq(&self, other: &Self) -> bool {
        self.segments == other.segments
    }
}

impl Eq for RustTypePath {}

#[derive(Default)]
pub(crate) struct RustSyntaxCapabilityProbe {
    found: bool,
}

impl RustSyntaxCapabilityProbe {
    pub(crate) fn type_contains_macro(source: &str) -> bool {
        let Ok(value) = syn::parse_str::<syn::Type>(source) else {
            return true;
        };
        let mut probe = Self::default();
        probe.visit_type(&value);
        probe.found
    }

    pub(crate) fn expression_contains_macro(source: &str) -> bool {
        let Ok(value) = syn::parse_str::<syn::Expr>(source) else {
            return true;
        };
        let mut probe = Self::default();
        probe.visit_expr(&value);
        probe.found
    }

    pub(crate) fn bound_contains_macro(source: &str) -> bool {
        let Ok(value) = syn::parse_str::<syn::TypeParamBound>(source) else {
            return true;
        };
        let mut probe = Self::default();
        probe.visit_type_param_bound(&value);
        probe.found
    }

    pub(crate) fn path_contains_macro(source: &str) -> bool {
        let Ok(value) = syn::parse_str::<syn::Path>(source) else {
            return true;
        };
        let mut probe = Self::default();
        probe.visit_path(&value);
        probe.found
    }
}

impl<'syntax> syn::visit::Visit<'syntax> for RustSyntaxCapabilityProbe {
    fn visit_macro(&mut self, _macro: &'syntax syn::Macro) {
        self.found = true;
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionParameter {
    #[serde(skip)]
    pattern: Option<RustSyntaxText>,
    parameter_type: RustType,
    attributes: RustAttributes,
}

impl RustFunctionParameter {
    pub(crate) fn new(pattern: Option<RustSyntaxText>, parameter_type: RustType) -> Self {
        Self {
            pattern,
            parameter_type,
            attributes: RustAttributes::default(),
        }
    }

    pub(crate) fn with_attributes(mut self, attributes: RustAttributes) -> Self {
        self.attributes = attributes;
        self
    }

    pub(crate) fn pattern(&self) -> Option<&str> {
        self.pattern.as_ref().map(RustSyntaxText::as_str)
    }

    pub(crate) fn parameter_type(&self) -> &RustType {
        &self.parameter_type
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self.parameter_type.requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustGenericMetadata {
    parameters: Vec<RustGenericParameter>,
    where_predicates: Vec<RustSyntaxText>,
}

impl RustGenericMetadata {
    pub(crate) fn new(parameters: Vec<RustGenericParameter>) -> Self {
        Self {
            parameters,
            where_predicates: Vec::new(),
        }
    }

    pub(crate) fn with_where_predicates(mut self, where_predicates: Vec<RustSyntaxText>) -> Self {
        self.where_predicates = where_predicates;
        self
    }

    pub(crate) fn parameters(&self) -> &[RustGenericParameter] {
        &self.parameters
    }

    pub(crate) fn where_predicates(&self) -> &[RustSyntaxText] {
        &self.where_predicates
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.parameters
            .iter()
            .any(RustGenericParameter::requires_capability_warning)
            || self
                .where_predicates
                .iter()
                .any(RustSyntaxText::contains_macro)
    }
}

impl From<Vec<RustGenericParameter>> for RustGenericMetadata {
    fn from(parameters: Vec<RustGenericParameter>) -> Self {
        Self::new(parameters)
    }
}

impl Default for RustGenericMetadata {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustGenericParameter {
    Type {
        name: String,
        bounds: Vec<String>,
        default: Option<String>,
        attributes: RustAttributes,
    },
    Lifetime {
        name: String,
        bounds: Vec<String>,
        attributes: RustAttributes,
    },
    Const {
        name: String,
        parameter_type: String,
        default: Option<String>,
        attributes: RustAttributes,
    },
}

impl RustGenericParameter {
    pub(crate) fn type_parameter(
        name: String,
        bounds: Vec<String>,
        default: Option<String>,
    ) -> Self {
        Self::Type {
            name,
            bounds,
            default,
            attributes: RustAttributes::default(),
        }
    }

    pub(crate) fn lifetime_parameter(name: String, bounds: Vec<String>) -> Self {
        Self::Lifetime {
            name,
            bounds,
            attributes: RustAttributes::default(),
        }
    }

    pub(crate) fn const_parameter(
        name: String,
        parameter_type: String,
        default: Option<String>,
    ) -> Self {
        Self::Const {
            name,
            parameter_type,
            default,
            attributes: RustAttributes::default(),
        }
    }

    pub(crate) fn with_attributes(mut self, attributes: RustAttributes) -> Self {
        match &mut self {
            Self::Type {
                attributes: stored, ..
            }
            | Self::Lifetime {
                attributes: stored, ..
            }
            | Self::Const {
                attributes: stored, ..
            } => *stored = attributes,
        }
        self
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        match self {
            Self::Type { attributes, .. }
            | Self::Lifetime { attributes, .. }
            | Self::Const { attributes, .. } => attributes,
        }
    }

    fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Type {
                bounds,
                default,
                attributes,
                ..
            } => {
                attributes.requires_capability_warning()
                    || bounds
                        .iter()
                        .any(|bound| RustSyntaxCapabilityProbe::bound_contains_macro(bound))
                    || default
                        .as_deref()
                        .is_some_and(RustSyntaxCapabilityProbe::type_contains_macro)
            }
            Self::Lifetime { attributes, .. } => attributes.requires_capability_warning(),
            Self::Const {
                parameter_type,
                default,
                attributes,
                ..
            } => {
                attributes.requires_capability_warning()
                    || RustSyntaxCapabilityProbe::type_contains_macro(parameter_type)
                    || default
                        .as_deref()
                        .is_some_and(RustSyntaxCapabilityProbe::expression_contains_macro)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RustArrayType, RustFunctionParameter, RustFunctionPointerParameter,
        RustFunctionPointerType, RustFunctionPointerVariadic, RustGenericMetadata,
        RustGenericParameter, RustImplTraitType, RustRawPointerType, RustReferenceType,
        RustSyntaxCapabilityProbe, RustTraitObjectType, RustType, RustTypePath,
        UnsignedIntegerType,
    };
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    #[test]
    fn function_pointer_variadic_pattern_is_render_metadata_not_api_semantics() {
        let left = RustType::FunctionPointer(
            super::RustFunctionPointerType::from_parts(Vec::new(), None, false).with_variadic(
                Some(RustFunctionPointerVariadic::new(
                    Some(RustSyntaxText::parse_pattern("left").expect("left pattern")),
                    RustAttributes::default(),
                )),
            ),
        );
        let right = RustType::FunctionPointer(
            super::RustFunctionPointerType::from_parts(Vec::new(), None, false).with_variadic(
                Some(RustFunctionPointerVariadic::new(
                    Some(RustSyntaxText::parse_pattern("right").expect("right pattern")),
                    RustAttributes::default(),
                )),
            ),
        );

        assert_ne!(left, right, "render metadata remains available");
        assert_eq!(
            serde_json::to_vec(&left).expect("left canonical type"),
            serde_json::to_vec(&right).expect("right canonical type"),
            "binding spelling must not enter the API digest"
        );
    }

    #[test]
    fn macro_capability_warning_recurses_through_every_nested_type_owner() {
        let macro_type = RustType::MacroInvocation("project_type ! ()".to_owned());
        let nested = [
            RustType::Tuple(vec![macro_type.clone()]),
            RustType::Array(RustArrayType::new(macro_type.clone(), "1".to_owned())),
            RustType::Slice(Box::new(macro_type.clone())),
            RustType::Reference(RustReferenceType::new(None, false, macro_type.clone())),
            RustType::RawPointer(RustRawPointerType::new(false, macro_type.clone())),
            RustType::Parenthesized(Box::new(macro_type.clone())),
            RustType::FunctionPointer(RustFunctionPointerType::from_parts(
                vec![RustFunctionPointerParameter::new(
                    RustAttributes::default(),
                    macro_type.clone(),
                )],
                Some(macro_type),
                false,
            )),
        ];

        for rust_type in nested {
            assert!(
                rust_type.requires_capability_warning(),
                "nested macro type must remain an honest syntax-mode warning: {rust_type:?}"
            );
        }
    }

    #[test]
    fn macro_capability_warning_detects_macros_inside_retained_type_syntax() {
        let retained = [
            RustType::Array(RustArrayType::new(
                RustType::UnsignedInteger(UnsignedIntegerType::U8),
                "array_length ! ()".to_owned(),
            )),
            RustType::TypePath(RustTypePath::new(vec![
                "Container < projected_type ! () >".to_owned(),
            ])),
            RustType::TraitObject(RustTraitObjectType::new(vec![
                "Iterator < Item = projected_type ! () >".to_owned(),
            ])),
            RustType::ImplTrait(RustImplTraitType::new(vec![
                "Iterator < Item = projected_type ! () >".to_owned(),
            ])),
        ];

        for rust_type in retained {
            assert!(
                rust_type.requires_capability_warning(),
                "macro syntax retained inside a type must warn: {rust_type:?}"
            );
        }
    }

    #[test]
    fn capability_warning_is_false_for_macro_free_nested_types() {
        let rust_type = RustType::FunctionPointer(RustFunctionPointerType::from_parts(
            vec![RustFunctionPointerParameter::new(
                RustAttributes::default(),
                RustType::Reference(RustReferenceType::new(
                    None,
                    false,
                    RustType::UnsignedInteger(UnsignedIntegerType::U8),
                )),
            )],
            Some(RustType::Tuple(vec![RustType::UnsignedInteger(
                UnsignedIntegerType::Usize,
            )])),
            false,
        ));

        assert!(!rust_type.requires_capability_warning());
    }

    #[test]
    fn callable_parameter_warning_includes_its_nested_type() {
        let parameter = RustFunctionParameter::new(
            Some(RustSyntaxText::parse_pattern("value").expect("parameter pattern")),
            RustType::Reference(RustReferenceType::new(
                None,
                false,
                RustType::MacroInvocation("projected_type ! ()".to_owned()),
            )),
        );

        assert!(parameter.requires_capability_warning());
    }

    #[test]
    fn macro_capability_warning_covers_generic_defaults_bounds_and_where_clauses() {
        let metadata = super::RustGenericMetadata::new(vec![
            super::RustGenericParameter::type_parameter(
                "T".to_owned(),
                vec!["Iterator < Item = projected_type ! () >".to_owned()],
                Some("default_type ! ()".to_owned()),
            ),
            super::RustGenericParameter::const_parameter(
                "N".to_owned(),
                "projected_type ! ()".to_owned(),
                Some("array_length ! ()".to_owned()),
            ),
        ])
        .with_where_predicates(vec![
            RustSyntaxText::parse_where_predicate("T : Trait < Item = where_type ! () >")
                .expect("where predicate"),
        ]);

        assert!(metadata.requires_capability_warning());
        assert!(!super::RustGenericMetadata::default().requires_capability_warning());
    }

    #[test]
    fn one_syntax_probe_detects_macros_in_each_string_backed_syntax_family() {
        assert!(RustSyntaxCapabilityProbe::expression_contains_macro(
            "contract_value!()"
        ));
        assert!(RustSyntaxCapabilityProbe::bound_contains_macro(
            "Service<Item = contract_type!()>"
        ));
        assert!(RustSyntaxCapabilityProbe::path_contains_macro(
            "crate::Service<contract_type!()>"
        ));
        assert!(!RustSyntaxCapabilityProbe::expression_contains_macro("4"));
        assert!(!RustSyntaxCapabilityProbe::bound_contains_macro("Send"));
        assert!(!RustSyntaxCapabilityProbe::path_contains_macro(
            "crate::Service<Request>"
        ));
    }

    #[test]
    fn lifetime_and_const_generic_attribute_placements_require_warnings() {
        let attribute: syn::Attribute = syn::parse_quote!(#[contract_semantic]);
        let warning =
            RustAttributes::from_syn(&[attribute], &crate::work::CancellationProbe::new())
                .expect("retained semantic attribute");
        let lifetime = RustGenericMetadata::new(vec![
            RustGenericParameter::lifetime_parameter("'a".to_owned(), Vec::new())
                .with_attributes(warning.clone()),
        ]);
        let constant = RustGenericMetadata::new(vec![
            RustGenericParameter::const_parameter("N".to_owned(), "usize".to_owned(), None)
                .with_attributes(warning),
        ]);

        assert!(lifetime.requires_capability_warning());
        assert!(constant.requires_capability_warning());
    }

    #[test]
    fn type_path_equality_and_serialization_use_canonical_identifier_spelling() {
        let raw: syn::TypePath =
            syn::parse_str("::r#model::r#Widget<r#Item, nested::r#Output>").expect("raw type path");
        let ordinary: syn::TypePath =
            syn::parse_str("::model::Widget<Item, nested::Output>").expect("ordinary type path");
        let raw = RustTypePath::from_syn(&raw);
        let ordinary = RustTypePath::from_syn(&ordinary);

        assert_eq!(raw, ordinary);
        assert_eq!(raw.segments, ordinary.segments);
        assert_eq!(
            serde_json::to_vec(&raw).expect("raw canonical path"),
            serde_json::to_vec(&ordinary).expect("ordinary canonical path")
        );
        assert_ne!(raw.source_syntax(), ordinary.source_syntax());
    }

    #[test]
    fn canonical_keyword_path_constructor_derives_valid_source_syntax() {
        let path = RustTypePath::new(vec!["type".to_owned()]);

        assert_eq!(path.segments, ["type".to_owned()]);
        assert_eq!(path.source_syntax(), "r#type");
        assert!(!path.requires_capability_warning());
    }

    #[test]
    fn type_path_canonicalization_does_not_reinterpret_macro_input_tokens() {
        let raw: syn::TypePath =
            syn::parse_str("Container<project!(r#type)>").expect("raw macro-input type path");
        let ordinary: syn::TypePath =
            syn::parse_str("Container<project!(type)>").expect("ordinary macro-input type path");
        let raw = RustTypePath::from_syn(&raw);
        let ordinary = RustTypePath::from_syn(&ordinary);

        assert_ne!(raw, ordinary, "macro input token spelling remains semantic");
        assert_ne!(
            serde_json::to_vec(&raw).expect("raw macro-input path"),
            serde_json::to_vec(&ordinary).expect("ordinary macro-input path")
        );
        assert!(raw.requires_capability_warning());
        assert!(ordinary.requires_capability_warning());
    }
}
