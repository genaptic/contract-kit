use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TypeAliasType {
    base: BaseType,
    generics: RustGenericMetadata,
    target_type: RustType,
}

impl TypeAliasType {
    pub(crate) fn new(
        base: BaseType,
        generics: RustGenericMetadata,
        target_type: RustType,
    ) -> Self {
        Self {
            base,
            generics,
            target_type,
        }
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn target_type(&self) -> &RustType {
        &self.target_type
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self.target_type.requires_capability_warning()
    }
}
