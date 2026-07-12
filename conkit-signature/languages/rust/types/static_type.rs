use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::primitive_types::RustType;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticType {
    base: BaseType,
    mutable: bool,
    static_type: RustType,
}

impl StaticType {
    pub(crate) fn new(base: BaseType, mutable: bool, static_type: RustType) -> Self {
        Self {
            base,
            mutable,
            static_type,
        }
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn mutable(&self) -> bool {
        self.mutable
    }

    pub(crate) fn static_type(&self) -> &RustType {
        &self.static_type
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> StaticCanonical {
        StaticCanonical {
            base: self.base.canonical_form(),
            mutable: self.mutable,
            static_type: self.static_type.clone(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct StaticCanonical {
    base: BaseCanonical,
    mutable: bool,
    static_type: RustType,
}
