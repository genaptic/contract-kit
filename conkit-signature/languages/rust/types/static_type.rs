use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::primitive_types::RustType;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.static_type.requires_capability_warning()
    }
}
