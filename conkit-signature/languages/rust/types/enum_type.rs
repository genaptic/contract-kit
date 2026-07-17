use crate::SignatureContractKitError;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::declaration::RustIdentifier;
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EnumVariantField {
    name: Option<String>,
    field_type: RustType,
    attributes: RustAttributes,
}

impl EnumVariantField {
    pub(crate) fn new(
        name: Option<String>,
        field_type: RustType,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        let name = name
            .map(|name| {
                RustIdentifier::new(name, "enum variant field name")
                    .map(|name| name.as_str().to_owned())
            })
            .transpose()?;

        Ok(Self {
            name,
            field_type,
            attributes,
        })
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn field_type(&self) -> &RustType {
        &self.field_type
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self.field_type.requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EnumVariant {
    name: String,
    fields: Vec<EnumVariantField>,
    discriminant: Option<RustSyntaxText>,
    attributes: RustAttributes,
}

impl EnumVariant {
    pub(crate) fn new(
        name: String,
        fields: Vec<EnumVariantField>,
        discriminant: Option<RustSyntaxText>,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        let name = RustIdentifier::new(name, "enum variant name")?
            .as_str()
            .to_owned();
        Ok(Self {
            name,
            fields,
            discriminant,
            attributes,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn fields(&self) -> &[EnumVariantField] {
        &self.fields
    }

    pub(crate) fn discriminant(&self) -> Option<&str> {
        self.discriminant.as_ref().map(RustSyntaxText::as_str)
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self
                .fields
                .iter()
                .any(EnumVariantField::requires_capability_warning)
            || self
                .discriminant
                .as_ref()
                .is_some_and(RustSyntaxText::contains_macro)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EnumType {
    base: BaseType,
    generics: RustGenericMetadata,
    variants: Vec<EnumVariant>,
}

impl EnumType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            generics: RustGenericMetadata::default(),
            variants: Vec::new(),
        }
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_variants(mut self, variants: Vec<EnumVariant>) -> Self {
        self.variants = variants;
        self
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn variants(&self) -> &[EnumVariant] {
        &self.variants
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self
                .variants
                .iter()
                .any(EnumVariant::requires_capability_warning)
    }
}

#[cfg(test)]
mod tests {
    use super::{EnumVariant, EnumVariantField};
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::primitive_types::RustType;
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    struct VariantFixture;

    impl VariantFixture {
        fn field(name: Option<&str>) -> Result<EnumVariantField, crate::SignatureContractKitError> {
            EnumVariantField::new(
                name.map(ToOwned::to_owned),
                RustType::Bool,
                RustAttributes::default(),
            )
        }

        fn variant(
            name: &str,
            discriminant: Option<&str>,
        ) -> Result<EnumVariant, crate::SignatureContractKitError> {
            EnumVariant::new(
                name.to_owned(),
                Vec::new(),
                discriminant
                    .map(RustSyntaxText::parse_expression)
                    .transpose()?,
                RustAttributes::default(),
            )
        }
    }

    #[test]
    fn enum_variants_and_fields_accept_valid_canonical_syntax() {
        let named = VariantFixture::field(Some("type")).expect("canonical named field");
        let unnamed = VariantFixture::field(None).expect("tuple variant field");
        let variant = VariantFixture::variant("Ready", Some("1 << 2"))
            .expect("valid discriminant expression");

        assert_eq!(named.name(), Some("type"));
        assert_eq!(unnamed.name(), None);
        assert_eq!(variant.name(), "Ready");
        assert_eq!(variant.discriminant(), Some("1 << 2"));
    }

    #[test]
    fn enum_variant_fields_reject_invalid_names() {
        for name in [
            "",
            " value",
            "value ",
            "r#type",
            "not-an-ident",
            "line\nbreak",
        ] {
            let error = VariantFixture::field(Some(name))
                .expect_err("invalid variant field name must fail");
            assert!(error.to_string().contains("enum variant field name"));
        }
    }

    #[test]
    fn enum_variants_reject_invalid_names_and_canonicalize_discriminants() {
        for name in ["", " Ready", "Ready ", "not-a-variant", "line\nbreak"] {
            let error =
                VariantFixture::variant(name, None).expect_err("invalid variant name must fail");
            assert!(error.to_string().contains("enum variant name"));
        }

        for discriminant in ["", "line\nbreak", "1 +"] {
            let error = VariantFixture::variant("Ready", Some(discriminant))
                .expect_err("invalid discriminant must fail");
            assert!(error.to_string().contains("expression"));
        }

        assert_eq!(
            VariantFixture::variant("Ready", Some(" 1+2 "))
                .expect("formatting-only discriminant")
                .discriminant(),
            Some("1 + 2")
        );
    }

    #[test]
    fn discriminant_and_field_attribute_capabilities_are_not_lost() {
        let attribute: syn::Attribute = syn::parse_quote!(#[cfg(unix)]);
        let attributes =
            RustAttributes::from_syn(&[attribute], &crate::work::CancellationProbe::new())
                .expect("conditional field attribute");
        let field = EnumVariantField::new(None, RustType::Bool, attributes)
            .expect("attributed variant field");
        let attributed = EnumVariant::new(
            "Attributed".to_owned(),
            vec![field],
            None,
            RustAttributes::default(),
        )
        .expect("attributed enum field");
        let discriminated = EnumVariant::new(
            "Ready".to_owned(),
            Vec::new(),
            Some(RustSyntaxText::parse_expression("contract_value!()").expect("macro expression")),
            RustAttributes::default(),
        )
        .expect("macro-valued enum discriminant");

        assert!(attributed.requires_capability_warning());
        assert!(discriminated.requires_capability_warning());
    }
}
