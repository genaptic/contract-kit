use crate::SignatureContractKitError;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::declaration::RustIdentifier;
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType, Visibility};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StructField {
    name: Option<String>,
    visibility: Visibility,
    field_type: RustType,
    attributes: RustAttributes,
}

impl StructField {
    pub(crate) fn new(
        name: Option<String>,
        visibility: Visibility,
        field_type: RustType,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        let name = name
            .map(|name| {
                RustIdentifier::new(name, "struct field name").map(|name| name.as_str().to_owned())
            })
            .transpose()?;

        Ok(Self {
            name,
            visibility,
            field_type,
            attributes,
        })
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn field_type(&self) -> &RustType {
        &self.field_type
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StructType {
    base: BaseType,
    generics: RustGenericMetadata,
    fields: Vec<StructField>,
}

impl StructType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            generics: RustGenericMetadata::default(),
            fields: Vec::new(),
        }
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_fields(mut self, fields: Vec<StructField>) -> Self {
        self.fields = fields;
        self
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn fields(&self) -> &[StructField] {
        &self.fields
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self.fields.iter().any(|field| {
                field.attributes().requires_capability_warning()
                    || field.field_type().requires_capability_warning()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::StructField;
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::primitive_types::{RustType, Visibility};

    struct FieldFixture;

    impl FieldFixture {
        fn named(name: &str) -> Result<StructField, crate::SignatureContractKitError> {
            StructField::new(
                Some(name.to_owned()),
                Visibility::Private,
                RustType::Bool,
                RustAttributes::default(),
            )
        }
    }

    #[test]
    fn struct_fields_accept_canonical_named_and_unnamed_forms() {
        let named = FieldFixture::named("type").expect("canonical semantic identifier");
        let unnamed = StructField::new(
            None,
            Visibility::Private,
            RustType::Bool,
            RustAttributes::default(),
        )
        .expect("tuple field");

        assert_eq!(named.name(), Some("type"));
        assert_eq!(unnamed.name(), None);
    }

    #[test]
    fn struct_fields_reject_invalid_names_before_entering_the_model() {
        for name in [
            "",
            " value",
            "value ",
            "r#type",
            "not-an-ident",
            "line\nbreak",
        ] {
            let error = FieldFixture::named(name).expect_err("invalid field name must fail");
            assert!(error.to_string().contains("struct field name"));
        }
    }
}
