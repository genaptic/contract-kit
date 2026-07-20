use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::primitive_types::RustGenericMetadata;
use crate::languages::rust::types::struct_type::StructField;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct UnionType {
    base: BaseType,
    generics: RustGenericMetadata,
    fields: Vec<StructField>,
}

impl UnionType {
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
