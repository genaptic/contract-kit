use crate::languages::rust::types::base_type::{BaseType, RustImplementationContext};
use crate::languages::rust::types::callable_type::RustCallableSignature;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn signature(&self) -> &RustCallableSignature {
        &self.signature
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.signature.requires_capability_warning()
    }

    pub(crate) fn normalize_implementation_member(
        &mut self,
        context: &RustImplementationContext,
        trait_owned: bool,
    ) {
        context.normalize_base(&mut self.base, trait_owned);
    }
}
