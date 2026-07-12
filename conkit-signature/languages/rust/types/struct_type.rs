use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::callable_type::{RustMethod, RustMethodCanonical};
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType, Visibility};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct StructField {
    name: Option<String>,
    visibility: Visibility,
    field_type: RustType,
}

impl StructField {
    pub(crate) fn new(name: Option<String>, visibility: Visibility, field_type: RustType) -> Self {
        Self {
            name,
            visibility,
            field_type,
        }
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructType {
    base: BaseType,
    generics: RustGenericMetadata,
    fields: Vec<StructField>,
    methods: Vec<RustMethod>,
}

impl StructType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            generics: RustGenericMetadata::default(),
            fields: Vec::new(),
            methods: Vec::new(),
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

    pub(crate) fn methods(&self) -> &[RustMethod] {
        &self.methods
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> StructCanonical {
        StructCanonical {
            base: self.base.canonical_form(),
            generics: self.generics.clone(),
            fields: self.fields.clone(),
            methods: self
                .methods
                .iter()
                .map(RustMethod::canonical_form)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct StructCanonical {
    base: BaseCanonical,
    generics: RustGenericMetadata,
    fields: Vec<StructField>,
    methods: Vec<RustMethodCanonical>,
}
