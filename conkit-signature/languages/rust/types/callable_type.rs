use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustType, Visibility,
};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use serde::Serialize;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) enum RustFunctionAbi {
    #[default]
    Rust,
    Extern {
        name: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustCallableSignature {
    is_const: bool,
    is_async: bool,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    variadic: Option<RustVariadicParameter>,
    generics: RustGenericMetadata,
    parameters: Vec<RustFunctionParameter>,
    return_type: Option<RustType>,
}

impl RustCallableSignature {
    pub(crate) fn builder() -> RustCallableSignatureBuilder {
        RustCallableSignatureBuilder::default()
    }

    pub(crate) fn empty() -> Self {
        Self::builder().build()
    }

    pub(crate) fn is_const(&self) -> bool {
        self.is_const
    }

    pub(crate) fn is_async(&self) -> bool {
        self.is_async
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn abi(&self) -> &RustFunctionAbi {
        &self.abi
    }

    pub(crate) fn variadic(&self) -> Option<&RustVariadicParameter> {
        self.variadic.as_ref()
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn parameters(&self) -> &[RustFunctionParameter] {
        &self.parameters
    }

    pub(crate) fn return_type(&self) -> Option<&RustType> {
        self.return_type.as_ref()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.parameters
            .iter()
            .any(RustFunctionParameter::requires_capability_warning)
            || self
                .variadic
                .as_ref()
                .is_some_and(RustVariadicParameter::requires_capability_warning)
            || self.generics.requires_capability_warning()
            || self
                .return_type
                .as_ref()
                .is_some_and(RustType::requires_capability_warning)
    }
}

impl Default for RustCallableSignature {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustCallableSignatureBuilder {
    is_const: bool,
    is_async: bool,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    variadic: Option<RustVariadicParameter>,
    generics: RustGenericMetadata,
    parameters: Vec<RustFunctionParameter>,
    return_type: Option<RustType>,
}

impl RustCallableSignatureBuilder {
    pub(crate) fn with_const(mut self, is_const: bool) -> Self {
        self.is_const = is_const;
        self
    }

    pub(crate) fn with_async(mut self, is_async: bool) -> Self {
        self.is_async = is_async;
        self
    }

    pub(crate) fn with_unsafe(mut self, is_unsafe: bool) -> Self {
        self.is_unsafe = is_unsafe;
        self
    }

    pub(crate) fn with_abi(mut self, abi: RustFunctionAbi) -> Self {
        self.abi = abi;
        self
    }

    pub(crate) fn with_variadic(mut self, variadic: Option<RustVariadicParameter>) -> Self {
        self.variadic = variadic;
        self
    }

    pub(crate) fn with_generics(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_parameters(mut self, parameters: Vec<RustFunctionParameter>) -> Self {
        self.parameters = parameters;
        self
    }

    pub(crate) fn with_return_type(mut self, return_type: Option<RustType>) -> Self {
        self.return_type = return_type;
        self
    }

    pub(crate) fn build(self) -> RustCallableSignature {
        RustCallableSignature {
            is_const: self.is_const,
            is_async: self.is_async,
            is_unsafe: self.is_unsafe,
            abi: self.abi,
            variadic: self.variadic,
            generics: self.generics,
            parameters: self.parameters,
            return_type: self.return_type,
        }
    }
}

impl Default for RustCallableSignatureBuilder {
    fn default() -> Self {
        Self {
            is_const: false,
            is_async: false,
            is_unsafe: false,
            abi: RustFunctionAbi::Rust,
            variadic: None,
            generics: RustGenericMetadata::default(),
            parameters: Vec::new(),
            return_type: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustVariadicParameter {
    #[serde(skip)]
    pattern: Option<RustSyntaxText>,
    attributes: RustAttributes,
}

impl RustVariadicParameter {
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

    fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
    }
}

/// Canonical receiver semantics for one associated method.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub(crate) enum RustReceiver {
    Value {
        mutable: bool,
    },
    Reference {
        lifetime: Option<String>,
        mutable: bool,
    },
    Typed {
        mutable: bool,
        receiver_type: RustType,
    },
}

impl RustReceiver {
    pub(crate) fn value(mutable: bool) -> Self {
        Self::Value { mutable }
    }

    pub(crate) fn reference(lifetime: Option<String>, mutable: bool) -> Self {
        Self::Reference { lifetime, mutable }
    }

    pub(crate) fn typed(mutable: bool, receiver_type: RustType) -> Self {
        Self::Typed {
            mutable,
            receiver_type,
        }
    }

    pub(crate) fn receiver_type(&self) -> Option<&RustType> {
        match self {
            Self::Typed { receiver_type, .. } => Some(receiver_type),
            Self::Value { .. } | Self::Reference { .. } => None,
        }
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.receiver_type()
            .is_some_and(RustType::requires_capability_warning)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustMethod {
    function: FunctionType,
    receiver: Option<RustReceiver>,
    has_default_body: bool,
    is_specialization_default: bool,
    receiver_attributes: RustAttributes,
}

impl RustMethod {
    pub(crate) fn new(
        function: FunctionType,
        receiver: Option<RustReceiver>,
        has_default_body: bool,
        is_specialization_default: bool,
        receiver_attributes: RustAttributes,
    ) -> Self {
        Self {
            function,
            receiver,
            has_default_body,
            is_specialization_default,
            receiver_attributes,
        }
    }

    pub(crate) fn function(&self) -> &FunctionType {
        &self.function
    }

    pub(crate) fn receiver(&self) -> Option<&RustReceiver> {
        self.receiver.as_ref()
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        self.function.base().visibility()
    }

    pub(crate) fn has_default_body(&self) -> bool {
        self.has_default_body
    }

    pub(crate) fn is_specialization_default(&self) -> bool {
        self.is_specialization_default
    }

    pub(crate) fn receiver_attributes(&self) -> &RustAttributes {
        &self.receiver_attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.function.requires_capability_warning()
            || self
                .receiver
                .as_ref()
                .is_some_and(RustReceiver::requires_capability_warning)
            || self.receiver_attributes.requires_capability_warning()
    }

    pub(crate) fn normalize_implementation_member(
        &mut self,
        context: &crate::languages::rust::types::base_type::RustImplementationContext,
        trait_owned: bool,
    ) {
        self.function
            .normalize_implementation_member(context, trait_owned);
    }
}

#[cfg(test)]
mod tests {
    use super::{RustCallableSignature, RustReceiver, RustVariadicParameter};
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::primitive_types::RustType;
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    #[test]
    fn receiver_forms_are_closed_semantic_values() {
        let receivers = [
            RustReceiver::value(false),
            RustReceiver::value(true),
            RustReceiver::reference(None, false),
            RustReceiver::reference(None, true),
            RustReceiver::reference(Some("'request".to_owned()), false),
            RustReceiver::typed(false, RustType::SelfType),
        ];
        let canonical = receivers
            .iter()
            .map(|receiver| serde_json::to_vec(receiver).expect("canonical receiver"))
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(canonical.len(), receivers.len());
    }

    #[test]
    fn typed_receiver_type_participates_in_capability_evidence() {
        let receiver = RustReceiver::typed(
            false,
            RustType::MacroInvocation("generated_receiver!()".to_owned()),
        );

        assert!(receiver.requires_capability_warning());
        assert_eq!(
            receiver.receiver_type(),
            Some(&RustType::MacroInvocation(
                "generated_receiver!()".to_owned(),
            ))
        );
    }

    #[test]
    fn variadic_binding_name_is_render_metadata_not_api_semantics() {
        let left = RustCallableSignature::builder()
            .with_variadic(Some(RustVariadicParameter::new(
                Some(RustSyntaxText::parse_pattern("left").expect("left pattern")),
                RustAttributes::default(),
            )))
            .build();
        let right = RustCallableSignature::builder()
            .with_variadic(Some(RustVariadicParameter::new(
                Some(RustSyntaxText::parse_pattern("right").expect("right pattern")),
                RustAttributes::default(),
            )))
            .build();

        assert_ne!(left, right, "render metadata remains available");
        assert_eq!(
            serde_json::to_vec(&left).expect("left canonical callable"),
            serde_json::to_vec(&right).expect("right canonical callable"),
            "variadic binding spelling must not enter rust_api_v1 semantics"
        );
    }

    #[test]
    fn callable_serialization_preserves_the_v2_canonical_field_order() {
        let signature = RustCallableSignature::builder()
            .with_parameters(vec![
                crate::languages::rust::types::primitive_types::RustFunctionParameter::new(
                    Some(RustSyntaxText::parse_pattern("value").expect("parameter pattern")),
                    RustType::Bool,
                ),
            ])
            .build();

        assert_eq!(
            serde_json::to_string(&signature).expect("canonical callable"),
            r#"{"is_const":false,"is_async":false,"is_unsafe":false,"abi":"Rust","variadic":null,"generics":{"parameters":[],"where_predicates":[]},"parameters":[{"parameter_type":"Bool","attributes":{"values":[]}}],"return_type":null}"#,
        );
    }

    #[test]
    fn callable_serialization_keeps_parameter_type_and_order_semantic() {
        let first = RustCallableSignature::builder()
            .with_parameters(vec![
                crate::languages::rust::types::primitive_types::RustFunctionParameter::new(
                    Some(RustSyntaxText::parse_pattern("left").expect("left pattern")),
                    RustType::Bool,
                ),
                crate::languages::rust::types::primitive_types::RustFunctionParameter::new(
                    Some(RustSyntaxText::parse_pattern("right").expect("right pattern")),
                    RustType::Char,
                ),
            ])
            .build();
        let reordered = RustCallableSignature::builder()
            .with_parameters(vec![
                crate::languages::rust::types::primitive_types::RustFunctionParameter::new(
                    Some(RustSyntaxText::parse_pattern("left").expect("left pattern")),
                    RustType::Char,
                ),
                crate::languages::rust::types::primitive_types::RustFunctionParameter::new(
                    Some(RustSyntaxText::parse_pattern("right").expect("right pattern")),
                    RustType::Bool,
                ),
            ])
            .build();

        assert_ne!(
            serde_json::to_vec(&first).expect("first canonical callable"),
            serde_json::to_vec(&reordered).expect("reordered canonical callable"),
            "parameter types and declaration order remain API semantics",
        );
    }

    #[test]
    fn variadic_attributes_require_a_capability_warning() {
        let attribute: syn::Attribute = syn::parse_quote!(#[cfg(unix)]);
        let attributes =
            RustAttributes::from_syn(&[attribute], &crate::work::CancellationProbe::new())
                .expect("conditional variadic attribute");
        let signature = RustCallableSignature::builder()
            .with_variadic(Some(RustVariadicParameter::new(None, attributes)))
            .build();

        assert!(signature.requires_capability_warning());
    }
}
