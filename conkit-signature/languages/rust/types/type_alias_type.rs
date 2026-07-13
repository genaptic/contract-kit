use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
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

    pub(in crate::languages::rust) fn canonical_form(&self) -> TypeAliasCanonical {
        TypeAliasCanonical {
            base: self.base.canonical_form(),
            generics: self.generics.clone(),
            target_type: self.target_type.clone(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct TypeAliasCanonical {
    base: BaseCanonical,
    generics: RustGenericMetadata,
    target_type: RustType,
}
