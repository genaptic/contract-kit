use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::callable_type::{RustMethod, RustMethodCanonical};
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EnumVariantField {
    name: Option<String>,
    field_type: RustType,
}

impl EnumVariantField {
    pub(crate) fn new(name: Option<String>, field_type: RustType) -> Self {
        Self { name, field_type }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn field_type(&self) -> &RustType {
        &self.field_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EnumVariant {
    name: String,
    fields: Vec<EnumVariantField>,
    discriminant: Option<String>,
}

impl EnumVariant {
    pub(crate) fn new(
        name: String,
        fields: Vec<EnumVariantField>,
        discriminant: Option<String>,
    ) -> Self {
        Self {
            name,
            fields,
            discriminant,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn fields(&self) -> &[EnumVariantField] {
        &self.fields
    }

    pub(crate) fn discriminant(&self) -> Option<&str> {
        self.discriminant.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EnumType {
    base: BaseType,
    generics: RustGenericMetadata,
    variants: Vec<EnumVariant>,
    methods: Vec<RustMethod>,
}

impl EnumType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            generics: RustGenericMetadata::default(),
            variants: Vec::new(),
            methods: Vec::new(),
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

    pub(crate) fn methods(&self) -> &[RustMethod] {
        &self.methods
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> EnumCanonical {
        EnumCanonical {
            base: self.base.canonical_form(),
            generics: self.generics.clone(),
            variants: self.variants.clone(),
            methods: self
                .methods
                .iter()
                .map(RustMethod::canonical_form)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct EnumCanonical {
    base: BaseCanonical,
    generics: RustGenericMetadata,
    variants: Vec<EnumVariant>,
    methods: Vec<RustMethodCanonical>,
}
