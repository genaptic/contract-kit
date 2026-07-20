use crate::error::{RustSyntaxFamily, SignatureContractKitError};
use serde::Serialize;
use syn::parse::Parser as _;
use syn::visit::Visit as _;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustSyntaxText {
    canonical: String,
    contains_macro: bool,
}

impl RustSyntaxText {
    pub(crate) fn parse_expression(value: &str) -> Result<Self, SignatureContractKitError> {
        syn::parse_str::<syn::Expr>(value)
            .map(|expression| Self::from_expression(&expression))
            .map_err(|error| {
                SignatureContractKitError::invalid_rust_syntax_text(
                    RustSyntaxFamily::Expression,
                    value,
                    error.to_string(),
                )
            })
    }

    pub(crate) fn from_expression(value: &syn::Expr) -> Self {
        let mut probe = RustSyntaxMacroProbe::default();
        probe.visit_expr(value);
        Self::from_tokens(value, probe.found)
    }

    pub(crate) fn parse_type_bound(value: &str) -> Result<Self, SignatureContractKitError> {
        syn::parse_str::<syn::TypeParamBound>(value)
            .map(|bound| Self::from_type_bound(&bound))
            .map_err(|error| {
                SignatureContractKitError::invalid_rust_syntax_text(
                    RustSyntaxFamily::TypeParameterBound,
                    value,
                    error.to_string(),
                )
            })
    }

    pub(crate) fn from_type_bound(value: &syn::TypeParamBound) -> Self {
        let mut probe = RustSyntaxMacroProbe::default();
        probe.visit_type_param_bound(value);
        Self::from_tokens(value, probe.found)
    }

    pub(crate) fn parse_where_predicate(value: &str) -> Result<Self, SignatureContractKitError> {
        let predicate = syn::parse_str::<syn::WherePredicate>(value).map_err(|error| {
            SignatureContractKitError::invalid_rust_syntax_text(
                RustSyntaxFamily::WherePredicate,
                value,
                error.to_string(),
            )
        })?;
        let has_bounds = match &predicate {
            syn::WherePredicate::Lifetime(predicate) => !predicate.bounds.is_empty(),
            syn::WherePredicate::Type(predicate) => !predicate.bounds.is_empty(),
            _ => false,
        };
        if !has_bounds {
            return Err(SignatureContractKitError::invalid_rust_syntax_text(
                RustSyntaxFamily::WherePredicate,
                value,
                "where predicate must contain at least one bound",
            ));
        }

        Ok(Self::from_where_predicate(&predicate))
    }

    pub(crate) fn from_where_predicate(value: &syn::WherePredicate) -> Self {
        let mut probe = RustSyntaxMacroProbe::default();
        probe.visit_where_predicate(value);
        Self::from_tokens(value, probe.found)
    }

    pub(crate) fn parse_pattern(value: &str) -> Result<Self, SignatureContractKitError> {
        syn::Pat::parse_single
            .parse_str(value)
            .map(|pattern| Self::from_pattern(&pattern))
            .map_err(|error| {
                SignatureContractKitError::invalid_rust_syntax_text(
                    RustSyntaxFamily::Pattern,
                    value,
                    error.to_string(),
                )
            })
    }

    pub(crate) fn from_pattern(value: &syn::Pat) -> Self {
        let mut probe = RustSyntaxMacroProbe::default();
        probe.visit_pat(value);
        Self::from_tokens(value, probe.found)
    }

    pub(crate) fn from_identifier_pattern(value: &syn::Ident) -> Self {
        Self::from_tokens(value, false)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.canonical
    }

    pub(crate) fn contains_macro(&self) -> bool {
        self.contains_macro
    }

    fn from_tokens(value: &impl quote::ToTokens, contains_macro: bool) -> Self {
        Self {
            canonical: quote::ToTokens::to_token_stream(value).to_string(),
            contains_macro,
        }
    }
}

impl Serialize for RustSyntaxText {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Default)]
struct RustSyntaxMacroProbe {
    found: bool,
}

impl<'syntax> syn::visit::Visit<'syntax> for RustSyntaxMacroProbe {
    fn visit_macro(&mut self, _value: &'syntax syn::Macro) {
        self.found = true;
    }
}

#[cfg(test)]
mod tests {
    use super::RustSyntaxText;
    use syn::parse::Parser as _;

    #[test]
    fn expressions_canonicalize_formatting_and_retain_macro_evidence() {
        let compact =
            RustSyntaxText::parse_expression("1+contract_value!()").expect("compact expression");
        let spaced = RustSyntaxText::parse_expression("1 + contract_value ! ( )")
            .expect("spaced expression");
        let parsed: syn::Expr = syn::parse_quote!(1 + contract_value!());
        let direct = RustSyntaxText::from_expression(&parsed);

        assert_eq!(compact, spaced);
        assert_eq!(compact, direct);
        assert_eq!(compact.as_str(), "1 + contract_value ! ()");
        assert!(compact.contains_macro());
    }

    #[test]
    fn type_bounds_canonicalize_formatting_and_retain_macro_evidence() {
        let compact = RustSyntaxText::parse_type_bound(
            "for<'a> Service<&'a Request,Output=projected_type!()>",
        )
        .expect("compact type bound");
        let spaced = RustSyntaxText::parse_type_bound(
            "for < 'a > Service < & 'a Request , Output = projected_type ! ( ) >",
        )
        .expect("spaced type bound");
        let parsed: syn::TypeParamBound =
            syn::parse_quote!(for<'a> Service<&'a Request, Output = projected_type!()>);

        assert_eq!(compact, spaced);
        assert_eq!(compact, RustSyntaxText::from_type_bound(&parsed));
        assert!(compact.contains_macro());
    }

    #[test]
    fn where_predicates_canonicalize_formatting_and_retain_macro_evidence() {
        let compact = RustSyntaxText::parse_where_predicate(
            "for<'a>T:Service<&'a Request,Output=projected_type!()>",
        )
        .expect("compact where predicate");
        let spaced = RustSyntaxText::parse_where_predicate(
            "for < 'a > T : Service < & 'a Request , Output = projected_type ! ( ) >",
        )
        .expect("spaced where predicate");
        let parsed: syn::WherePredicate = syn::parse_quote!(
            for<'a> T: Service<&'a Request, Output = projected_type!()>
        );

        assert_eq!(compact, spaced);
        assert_eq!(compact, RustSyntaxText::from_where_predicate(&parsed));
        assert!(compact.contains_macro());
    }

    #[test]
    fn patterns_canonicalize_formatting_without_becoming_api_semantics() {
        let compact = RustSyntaxText::parse_pattern("(mut request,binding@Some(_))")
            .expect("compact pattern");
        let spaced = RustSyntaxText::parse_pattern("( mut request , binding @ Some ( _ ) )")
            .expect("spaced pattern");
        let parsed: syn::Pat = syn::Pat::parse_single
            .parse_str("(mut request, binding @ Some(_))")
            .expect("parsed pattern");

        assert_eq!(compact, spaced);
        assert_eq!(compact, RustSyntaxText::from_pattern(&parsed));
        assert!(!compact.contains_macro());
    }

    #[test]
    fn each_invalid_syntax_family_returns_its_typed_error() {
        for (family, error) in [
            (
                "expression",
                RustSyntaxText::parse_expression("1 +").expect_err("invalid expression"),
            ),
            (
                "type parameter bound",
                RustSyntaxText::parse_type_bound("Send + Sync")
                    .expect_err("one bound cannot contain a plus separator"),
            ),
            (
                "where predicate",
                RustSyntaxText::parse_where_predicate("T:").expect_err("invalid where predicate"),
            ),
            (
                "pattern",
                RustSyntaxText::parse_pattern("(").expect_err("invalid pattern"),
            ),
        ] {
            let rendered = error.to_string();
            assert!(rendered.contains("invalid Rust syntax text"), "{rendered}");
            assert!(rendered.contains(family), "{rendered}");
        }
    }

    #[test]
    fn incomplete_type_and_lifetime_where_predicates_are_rejected() {
        for predicate in ["T:", "'a:"] {
            let error = RustSyntaxText::parse_where_predicate(predicate)
                .expect_err("an incomplete where predicate must fail closed");
            let rendered = error.to_string();

            assert!(rendered.contains("where predicate"), "{rendered}");
            assert!(
                rendered.contains("must contain at least one bound"),
                "{rendered}"
            );
        }
    }
}
