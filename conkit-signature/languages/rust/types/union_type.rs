use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::primitive_types::RustGenericMetadata;
use crate::languages::rust::types::struct_type::StructField;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
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

    pub(in crate::languages::rust) fn canonical_form(&self) -> UnionCanonical {
        UnionCanonical {
            base: self.base.canonical_form(),
            generics: self.generics.clone(),
            fields: self.fields.clone(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct UnionCanonical {
    base: BaseCanonical,
    generics: RustGenericMetadata,
    fields: Vec<StructField>,
}
