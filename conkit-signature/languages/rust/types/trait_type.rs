use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::callable_type::{RustMethod, RustMethodCanonical};
use crate::languages::rust::types::primitive_types::RustGenericMetadata;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TraitType {
    base: BaseType,
    generics: RustGenericMetadata,
    supertraits: Vec<String>,
    methods: Vec<RustMethod>,
}

impl TraitType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            generics: RustGenericMetadata::default(),
            supertraits: Vec::new(),
            methods: Vec::new(),
        }
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_supertraits(mut self, supertraits: Vec<String>) -> Self {
        self.supertraits = supertraits;
        self
    }

    pub(crate) fn with_methods(mut self, methods: Vec<RustMethod>) -> Self {
        self.methods = methods;
        self
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn supertraits(&self) -> &[String] {
        &self.supertraits
    }

    pub(crate) fn methods(&self) -> &[RustMethod] {
        &self.methods
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> TraitCanonical {
        TraitCanonical {
            base: self.base.canonical_form(),
            generics: self.generics.clone(),
            supertraits: self.supertraits.clone(),
            methods: self
                .methods
                .iter()
                .map(RustMethod::canonical_form)
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct TraitCanonical {
    base: BaseCanonical,
    generics: RustGenericMetadata,
    supertraits: Vec<String>,
    methods: Vec<RustMethodCanonical>,
}
