use crate::languages::rust::types::base_type::{BaseCanonical, BaseType};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MacroType {
    base: BaseType,
    tokens: String,
}

impl MacroType {
    pub(crate) fn new(base: BaseType, tokens: String) -> Self {
        Self { base, tokens }
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn tokens(&self) -> &str {
        &self.tokens
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> MacroCanonical {
        MacroCanonical {
            base: self.base.canonical_form(),
            tokens: self.tokens.clone(),
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct MacroCanonical {
    base: BaseCanonical,
    tokens: String,
}
