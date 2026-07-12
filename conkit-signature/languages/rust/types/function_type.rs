use crate::files::CatalogPath;
use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use crate::languages::rust::types::callable_type::RustCallableSignature;
use crate::languages::rust::types::primitive_types::Visibility;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionType {
    base: BaseType,
    signature: RustCallableSignature,
}

impl FunctionType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            signature: RustCallableSignature::default(),
        }
    }

    pub(crate) fn with_callable_signature(mut self, signature: RustCallableSignature) -> Self {
        self.signature = signature;
        self
    }

    pub(crate) fn into_method_context(
        mut self,
        file: CatalogPath,
        module_path: Vec<String>,
        visibility: Visibility,
    ) -> Self {
        self.base = self.base.into_context(file, module_path, visibility);
        self
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn signature(&self) -> &RustCallableSignature {
        &self.signature
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> FunctionCanonical {
        FunctionCanonical {
            base: self.base.canonical_form(),
            signature: self.signature.clone(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct FunctionCanonical {
    base: BaseCanonical,
    signature: RustCallableSignature,
}
