use crate::error::SignatureContractKitError;
use crate::languages::rust::types::associated_item::RustAssociatedItem;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::primitive_types::{
    RustGenericMetadata, RustSyntaxCapabilityProbe,
};
use quote::ToTokens as _;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TraitType {
    base: BaseType,
    is_unsafe: bool,
    is_auto: bool,
    generics: RustGenericMetadata,
    supertraits: Vec<String>,
    items: Vec<RustAssociatedItem>,
}

impl TraitType {
    pub(crate) fn new(base: BaseType) -> Self {
        Self {
            base,
            is_unsafe: false,
            is_auto: false,
            generics: RustGenericMetadata::default(),
            supertraits: Vec::new(),
            items: Vec::new(),
        }
    }

    pub(crate) fn with_qualifiers(mut self, is_unsafe: bool, is_auto: bool) -> Self {
        self.is_unsafe = is_unsafe;
        self.is_auto = is_auto;
        self
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_supertraits(
        mut self,
        supertraits: Vec<String>,
    ) -> Result<Self, SignatureContractKitError> {
        self.supertraits = supertraits
            .into_iter()
            .map(Self::canonical_supertrait)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self)
    }

    fn canonical_supertrait(supertrait: String) -> Result<String, SignatureContractKitError> {
        if supertrait.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "trait supertrait cannot be empty",
            ));
        }
        if supertrait.trim() != supertrait {
            return Err(SignatureContractKitError::conversion_failed(
                "trait supertrait cannot have surrounding whitespace",
            ));
        }
        if supertrait.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "trait supertrait cannot contain control characters",
            ));
        }

        let parsed = syn::parse_str::<syn::TypeParamBound>(&supertrait).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid trait supertrait {supertrait:?}: {error}"
            ))
        })?;
        Ok(parsed.to_token_stream().to_string())
    }

    pub(crate) fn with_items(mut self, items: Vec<RustAssociatedItem>) -> Self {
        self.items = items;
        self
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn is_auto(&self) -> bool {
        self.is_auto
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn supertraits(&self) -> &[String] {
        &self.supertraits
    }

    pub(crate) fn items(&self) -> &[RustAssociatedItem] {
        &self.items
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.base.attributes().requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self
                .supertraits
                .iter()
                .any(|bound| RustSyntaxCapabilityProbe::bound_contains_macro(bound))
            || self
                .items
                .iter()
                .any(RustAssociatedItem::requires_capability_warning)
    }
}

#[cfg(test)]
mod tests {
    use super::TraitType;
    use crate::files::CatalogPath;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::base_type::BaseType;
    use crate::languages::rust::types::primitive_types::Visibility;

    fn trait_type() -> TraitType {
        TraitType::new(BaseType::new(
            "Service".to_owned(),
            Visibility::Public,
            CatalogPath::new("lib.rs").expect("fixture path"),
            RustModuleId::new(
                RustCrateId::new("fixture", &crate::work::CancellationProbe::new())
                    .expect("fixture crate"),
                RustModulePath::new(Vec::new()).expect("crate root module"),
            ),
            RustAttributes::default(),
        ))
    }

    #[test]
    fn supertraits_reject_empty_surrounding_whitespace_control_and_malformed_bounds() {
        for value in ["", " Send", "Send ", "Send\n", "Se\u{7}nd", "Send +"] {
            let error = trait_type()
                .with_supertraits(vec![value.to_owned()])
                .expect_err("invalid trait supertraits must fail");
            assert!(
                error.to_string().contains("trait supertrait"),
                "unexpected error for {value:?}: {error}"
            );
        }
    }

    #[test]
    fn supertraits_canonicalize_qualified_generic_and_higher_ranked_bounds() {
        let value = trait_type()
            .with_supertraits(vec![
                "::transport::Service<Request>".to_owned(),
                "for<'a> crate::BorrowingService<&'a Request>".to_owned(),
                "'static".to_owned(),
            ])
            .expect("valid supertraits");

        assert_eq!(
            value.supertraits(),
            [
                ":: transport :: Service < Request >",
                "for < 'a > crate :: BorrowingService < & 'a Request >",
                "'static",
            ]
        );
    }

    #[test]
    fn supertraits_preserve_source_order_and_duplicates_after_canonicalization() {
        let value = trait_type()
            .with_supertraits(vec![
                "Send".to_owned(),
                "crate::Service<T>".to_owned(),
                "Send".to_owned(),
            ])
            .expect("ordered duplicate supertraits");

        assert_eq!(
            value.supertraits(),
            ["Send", "crate :: Service < T >", "Send"]
        );
    }

    #[test]
    fn macro_bearing_supertraits_require_a_syntax_mode_capability_warning() {
        let value = trait_type()
            .with_supertraits(vec!["crate::Service<contract_type!()>".to_owned()])
            .expect("retained macro-bearing supertrait");

        assert!(value.requires_capability_warning());
    }
}
