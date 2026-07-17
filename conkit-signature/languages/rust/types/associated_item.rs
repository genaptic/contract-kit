use crate::error::SignatureContractKitError;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::callable_type::RustMethod;
use crate::languages::rust::types::declaration::RustIdentifier;
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType, Visibility};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use serde::Serialize;

/// Closed semantic family for items owned by a trait or implementation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustAssociatedItem {
    Method(Box<RustMethod>),
    Constant(RustAssociatedConstant),
    Type(RustAssociatedType),
}

impl RustAssociatedItem {
    pub(crate) fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Method(method) => method.requires_capability_warning(),
            Self::Constant(constant) => constant.requires_capability_warning(),
            Self::Type(associated_type) => associated_type.requires_capability_warning(),
        }
    }

    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, SignatureContractKitError> {
        serde_json::to_vec(self).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to encode canonical Rust associated item: {error}"
            ))
        })
    }

    pub(crate) fn normalize_implementation_member(
        &mut self,
        context: &crate::languages::rust::types::base_type::RustImplementationContext,
        trait_owned: bool,
    ) {
        match self {
            Self::Method(method) => method.normalize_implementation_member(context, trait_owned),
            Self::Constant(constant) => {
                context.normalize_visibility(&mut constant.visibility, trait_owned)
            }
            Self::Type(associated_type) => {
                context.normalize_visibility(&mut associated_type.visibility, trait_owned)
            }
        }
    }
}

/// A trait or implementation associated constant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustAssociatedConstant {
    name: RustIdentifier,
    visibility: Visibility,
    constant_type: RustType,
    default_value: Option<RustSyntaxText>,
    is_specialization_default: bool,
    attributes: RustAttributes,
}

impl RustAssociatedConstant {
    pub(crate) fn new(
        name: String,
        visibility: Visibility,
        constant_type: RustType,
        default_value: Option<RustSyntaxText>,
        is_specialization_default: bool,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            name: RustIdentifier::new(name, "associated constant name")?,
            visibility,
            constant_type,
            default_value,
            is_specialization_default,
            attributes,
        })
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn constant_type(&self) -> &RustType {
        &self.constant_type
    }

    pub(crate) fn default_value(&self) -> Option<&str> {
        self.default_value.as_ref().map(RustSyntaxText::as_str)
    }

    pub(crate) fn is_specialization_default(&self) -> bool {
        self.is_specialization_default
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self.constant_type.requires_capability_warning()
            || self
                .default_value
                .as_ref()
                .is_some_and(RustSyntaxText::contains_macro)
    }
}

/// A trait or implementation associated type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustAssociatedType {
    name: RustIdentifier,
    visibility: Visibility,
    generics: RustGenericMetadata,
    bounds: Vec<RustSyntaxText>,
    default_type: Option<RustType>,
    is_specialization_default: bool,
    attributes: RustAttributes,
}

impl RustAssociatedType {
    pub(crate) fn new(
        name: String,
        visibility: Visibility,
        generics: RustGenericMetadata,
        bounds: Vec<RustSyntaxText>,
        default_type: Option<RustType>,
        is_specialization_default: bool,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            name: RustIdentifier::new(name, "associated type name")?,
            visibility,
            generics,
            bounds,
            default_type,
            is_specialization_default,
            attributes,
        })
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn bounds(&self) -> &[RustSyntaxText] {
        &self.bounds
    }

    pub(crate) fn default_type(&self) -> Option<&RustType> {
        self.default_type.as_ref()
    }

    pub(crate) fn is_specialization_default(&self) -> bool {
        self.is_specialization_default
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self.bounds.iter().any(RustSyntaxText::contains_macro)
            || self
                .default_type
                .as_ref()
                .is_some_and(RustType::requires_capability_warning)
    }
}

#[cfg(test)]
mod tests {
    use super::{RustAssociatedConstant, RustAssociatedItem, RustAssociatedType};
    use crate::files::CatalogPath;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::attributes::{
        RustAttribute, RustAttributes, RustPath, RustRawAttribute, RustRawAttributeArguments,
    };
    use crate::languages::rust::types::base_type::BaseType;
    use crate::languages::rust::types::callable_type::{
        RustCallableSignature, RustMethod, RustReceiver,
    };
    use crate::languages::rust::types::function_type::FunctionType;
    use crate::languages::rust::types::primitive_types::{
        RustFunctionParameter, RustGenericMetadata, RustGenericParameter, RustType,
        UnsignedIntegerType, Visibility,
    };
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    struct AssociatedFixture {
        file: CatalogPath,
        module: RustModuleId,
    }

    impl AssociatedFixture {
        fn new() -> Self {
            Self {
                file: CatalogPath::new("lib.rs").expect("fixture path"),
                module: RustModuleId::new(
                    RustCrateId::new("fixture", &crate::work::CancellationProbe::new())
                        .expect("fixture crate"),
                    RustModulePath::new(Vec::new()).expect("crate root module"),
                ),
            }
        }

        fn method(
            &self,
            name: &str,
            has_default_body: bool,
            is_specialization_default: bool,
            attributes: RustAttributes,
            receiver_attributes: RustAttributes,
        ) -> RustAssociatedItem {
            let function = FunctionType::new(BaseType::new(
                name.to_owned(),
                Visibility::Public,
                self.file.clone(),
                self.module.clone(),
                attributes,
            ))
            .with_callable_signature(RustCallableSignature::empty());

            RustAssociatedItem::Method(Box::new(RustMethod::new(
                function,
                Some(RustReceiver::reference(None, false)),
                has_default_body,
                is_specialization_default,
                receiver_attributes,
            )))
        }

        fn base(&self, name: &str, attributes: RustAttributes) -> BaseType {
            BaseType::new(
                name.to_owned(),
                Visibility::Public,
                self.file.clone(),
                self.module.clone(),
                attributes,
            )
        }

        fn warning_attributes(&self) -> RustAttributes {
            RustAttributes::new(
                vec![RustAttribute::Unresolved(
                    RustRawAttribute::new(
                        RustPath::new("contract_semantic".to_owned()).expect("attribute path"),
                        RustRawAttributeArguments::Path,
                    )
                    .expect("raw attribute"),
                )],
                &crate::work::CancellationProbe::new(),
            )
            .expect("warning attributes")
        }

        fn expression(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_expression(value).expect("expression fixture")
        }

        fn bound(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_type_bound(value).expect("bound fixture")
        }

        fn predicate(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_where_predicate(value).expect("where fixture")
        }

        fn pattern(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_pattern(value).expect("pattern fixture")
        }
    }

    #[test]
    fn associated_items_retain_defaults_specialization_attributes_and_order() {
        let fixture = AssociatedFixture::new();
        let items = [
            RustAssociatedItem::Constant(
                RustAssociatedConstant::new(
                    "REQUIRED".to_owned(),
                    Visibility::Public,
                    RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                    None,
                    false,
                    RustAttributes::default(),
                )
                .expect("required constant"),
            ),
            RustAssociatedItem::Constant(
                RustAssociatedConstant::new(
                    "PROVIDED".to_owned(),
                    Visibility::Public,
                    RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                    Some(fixture.expression("4")),
                    true,
                    fixture.warning_attributes(),
                )
                .expect("provided constant"),
            ),
            RustAssociatedItem::Type(
                RustAssociatedType::new(
                    "Output".to_owned(),
                    Visibility::Public,
                    RustGenericMetadata::new(vec![RustGenericParameter::type_parameter(
                        "T".to_owned(),
                        vec!["Send".to_owned()],
                        None,
                    )])
                    .with_where_predicates(vec![fixture.predicate("T: 'static")]),
                    vec![fixture.bound("Clone"), fixture.bound("Send")],
                    Some(RustType::UnsignedInteger(UnsignedIntegerType::U8)),
                    true,
                    RustAttributes::default(),
                )
                .expect("associated type"),
            ),
            fixture.method(
                "execute",
                true,
                true,
                RustAttributes::default(),
                fixture.warning_attributes(),
            ),
        ];

        let RustAssociatedItem::Constant(required) = &items[0] else {
            panic!("first item must remain the required constant");
        };
        assert_eq!(required.default_value(), None);
        let RustAssociatedItem::Constant(provided) = &items[1] else {
            panic!("second item must remain the provided constant");
        };
        assert_eq!(provided.default_value(), Some("4"));
        assert!(provided.is_specialization_default());
        assert!(provided.requires_capability_warning());
        let RustAssociatedItem::Type(associated_type) = &items[2] else {
            panic!("third item must remain the associated type");
        };
        assert_eq!(
            associated_type.generics().where_predicates(),
            &[fixture.predicate("T: 'static")]
        );
        assert_eq!(
            associated_type
                .bounds()
                .iter()
                .map(RustSyntaxText::as_str)
                .collect::<Vec<_>>(),
            vec!["Clone", "Send"]
        );
        assert!(matches!(
            associated_type.default_type(),
            Some(RustType::UnsignedInteger(UnsignedIntegerType::U8))
        ));
        let RustAssociatedItem::Method(method) = &items[3] else {
            panic!("fourth item must remain the method");
        };
        assert!(method.has_default_body());
        assert!(method.is_specialization_default());
        assert!(items[3].requires_capability_warning());

        let canonical = items
            .iter()
            .map(|item| item.canonical_bytes().expect("canonical item"))
            .collect::<Vec<_>>();
        let reversed = items
            .iter()
            .rev()
            .map(|item| item.canonical_bytes().expect("canonical item"))
            .collect::<Vec<_>>();
        assert_ne!(
            canonical, reversed,
            "associated declaration order is semantic"
        );
    }

    #[test]
    fn inherent_associated_visibility_and_generic_type_metadata_are_semantic() {
        let fixture = AssociatedFixture::new();
        let public_constant = RustAssociatedItem::Constant(
            RustAssociatedConstant::new(
                "LIMIT".to_owned(),
                Visibility::Public,
                RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                Some(fixture.expression("4")),
                false,
                RustAttributes::default(),
            )
            .expect("public associated constant"),
        );
        let private_constant = RustAssociatedItem::Constant(
            RustAssociatedConstant::new(
                "LIMIT".to_owned(),
                Visibility::Private,
                RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                Some(fixture.expression("4")),
                false,
                RustAttributes::default(),
            )
            .expect("private associated constant"),
        );
        let public_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Output".to_owned(),
                Visibility::Public,
                RustGenericMetadata::default(),
                vec![fixture.bound("Send")],
                None,
                false,
                RustAttributes::default(),
            )
            .expect("public associated type"),
        );
        let private_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Output".to_owned(),
                Visibility::Private,
                RustGenericMetadata::default(),
                vec![fixture.bound("Send")],
                None,
                false,
                RustAttributes::default(),
            )
            .expect("private associated type"),
        );
        let generic_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Output".to_owned(),
                Visibility::Public,
                RustGenericMetadata::new(vec![
                    RustGenericParameter::type_parameter("T".to_owned(), Vec::new(), None)
                        .with_attributes(fixture.warning_attributes()),
                ]),
                vec![fixture.bound("Send")],
                None,
                false,
                RustAttributes::default(),
            )
            .expect("generic associated type"),
        );

        assert_ne!(
            public_constant
                .canonical_bytes()
                .expect("public constant canonical bytes"),
            private_constant
                .canonical_bytes()
                .expect("private constant canonical bytes"),
        );
        assert_ne!(
            public_type
                .canonical_bytes()
                .expect("public type canonical bytes"),
            private_type
                .canonical_bytes()
                .expect("private type canonical bytes"),
        );
        assert_ne!(
            public_type
                .canonical_bytes()
                .expect("nongeneric type canonical bytes"),
            generic_type
                .canonical_bytes()
                .expect("generic type canonical bytes"),
        );
        assert!(!public_constant.requires_capability_warning());
        assert!(!public_type.requires_capability_warning());
        assert!(generic_type.requires_capability_warning());

        let RustAssociatedItem::Constant(public_constant) = public_constant else {
            unreachable!("fixture is an associated constant")
        };
        let RustAssociatedItem::Type(private_type) = private_type else {
            unreachable!("fixture is an associated type")
        };
        assert_eq!(public_constant.visibility(), &Visibility::Public);
        assert_eq!(private_type.visibility(), &Visibility::Private);
    }

    #[test]
    fn associated_capability_warnings_recurse_through_types_and_generics() {
        let fixture = AssociatedFixture::new();
        let macro_type = RustType::MacroInvocation("contract_type!()".to_owned());
        let constant = RustAssociatedItem::Constant(
            RustAssociatedConstant::new(
                "LIMIT".to_owned(),
                Visibility::Public,
                macro_type.clone(),
                None,
                false,
                RustAttributes::default(),
            )
            .expect("associated constant with retained macro type"),
        );
        let generic_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Output".to_owned(),
                Visibility::Public,
                RustGenericMetadata::default().with_where_predicates(vec![
                    fixture.predicate("T: Trait<Item = contract_type!()>"),
                ]),
                Vec::new(),
                None,
                false,
                RustAttributes::default(),
            )
            .expect("associated type with retained macro generic"),
        );
        let defaulted_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Defaulted".to_owned(),
                Visibility::Public,
                RustGenericMetadata::default(),
                Vec::new(),
                Some(macro_type.clone()),
                false,
                RustAttributes::default(),
            )
            .expect("associated type with retained macro default"),
        );
        let parameter_method = RustAssociatedItem::Method(Box::new(RustMethod::new(
            FunctionType::new(fixture.base("parameter", RustAttributes::default()))
                .with_callable_signature(
                    RustCallableSignature::builder()
                        .with_parameters(vec![RustFunctionParameter::new(
                            Some(fixture.pattern("value")),
                            macro_type.clone(),
                        )])
                        .build(),
                ),
            Some(RustReceiver::reference(None, false)),
            false,
            false,
            RustAttributes::default(),
        )));
        let return_method = RustAssociatedItem::Method(Box::new(RustMethod::new(
            FunctionType::new(fixture.base("result", RustAttributes::default()))
                .with_callable_signature(
                    RustCallableSignature::builder()
                        .with_return_type(Some(macro_type))
                        .build(),
                ),
            Some(RustReceiver::reference(None, false)),
            false,
            false,
            RustAttributes::default(),
        )));
        let generic_method = RustAssociatedItem::Method(Box::new(RustMethod::new(
            FunctionType::new(fixture.base("generic", RustAttributes::default()))
                .with_callable_signature(
                    RustCallableSignature::builder()
                        .with_generics(RustGenericMetadata::default().with_where_predicates(vec![
                            fixture.predicate("T: Trait<Item = contract_type!()>"),
                        ]))
                        .build(),
                ),
            Some(RustReceiver::reference(None, false)),
            false,
            false,
            RustAttributes::default(),
        )));

        for item in [
            constant,
            generic_type,
            defaulted_type,
            parameter_method,
            return_method,
            generic_method,
        ] {
            assert!(
                item.requires_capability_warning(),
                "every retained nested macro fact must produce a syntax-mode capability warning"
            );
        }
    }

    #[test]
    fn retained_defaults_bounds_receivers_and_attributes_all_warn() {
        let fixture = AssociatedFixture::new();
        let defaulted_constant = RustAssociatedItem::Constant(
            RustAssociatedConstant::new(
                "LIMIT".to_owned(),
                Visibility::Public,
                RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                Some(fixture.expression("contract_value!()")),
                false,
                RustAttributes::default(),
            )
            .expect("macro-valued associated constant"),
        );
        let bounded_type = RustAssociatedItem::Type(
            RustAssociatedType::new(
                "Output".to_owned(),
                Visibility::Public,
                RustGenericMetadata::default(),
                vec![fixture.bound("Service<Item = contract_type!()>")],
                None,
                false,
                RustAttributes::default(),
            )
            .expect("macro-bearing associated type bound"),
        );
        let receiver = RustAssociatedItem::Method(Box::new(RustMethod::new(
            FunctionType::new(fixture.base("receive", RustAttributes::default()))
                .with_callable_signature(RustCallableSignature::empty()),
            Some(RustReceiver::typed(
                false,
                RustType::MacroInvocation("contract_receiver!()".to_owned()),
            )),
            false,
            false,
            RustAttributes::default(),
        )));

        for item in [defaulted_constant, bounded_type, receiver] {
            assert!(item.requires_capability_warning());
        }
    }

    #[test]
    fn constructors_reject_invalid_identifiers_defaults_and_bounds() {
        let invalid_name = RustAssociatedConstant::new(
            " VALUE".to_owned(),
            Visibility::Public,
            RustType::UnsignedInteger(UnsignedIntegerType::Usize),
            None,
            false,
            RustAttributes::default(),
        )
        .expect_err("surrounding identifier whitespace must fail");
        let invalid_default = RustSyntaxText::parse_expression("let value")
            .expect_err("invalid expression must fail");
        let invalid_bound =
            RustSyntaxText::parse_type_bound("Clone +").expect_err("invalid type bound must fail");

        assert!(invalid_name.to_string().contains("name"));
        assert!(invalid_default.to_string().contains("expression"));
        assert!(invalid_bound.to_string().contains("type parameter bound"));
    }
}
