use crate::error::SignatureContractKitError;
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::primitive_types::Visibility;
use quote::ToTokens as _;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct RustMacroTokens(String);

impl RustMacroTokens {
    pub(crate) fn new(value: String) -> Result<Self, SignatureContractKitError> {
        if value.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "macro tokens cannot be empty",
            ));
        }
        let canonical = syn::parse_str::<syn::Macro>(&value)
            .map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid macro tokens {value:?}: {error}"
                ))
            })?
            .to_token_stream()
            .to_string();
        if canonical.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "macro tokens cannot be empty",
            ));
        }

        Ok(Self(canonical))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct MacroType {
    base: BaseType,
    tokens: RustMacroTokens,
}

impl MacroType {
    pub(crate) fn new(base: BaseType, tokens: String) -> Result<Self, SignatureContractKitError> {
        if !matches!(base.visibility(), Visibility::Private) {
            return Err(SignatureContractKitError::conversion_failed(
                "top-level macro visibility must be private because Rust item macros have no visibility syntax",
            ));
        }

        Ok(Self {
            base,
            tokens: RustMacroTokens::new(tokens)?,
        })
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn tokens(&self) -> &str {
        self.tokens.as_str()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::MacroType;
    use crate::files::CatalogPath;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::base_type::BaseType;
    use crate::languages::rust::types::primitive_types::Visibility;

    struct MacroFixture {
        file: CatalogPath,
        module: RustModuleId,
    }

    impl MacroFixture {
        fn new() -> Self {
            Self {
                file: CatalogPath::new("lib.rs").expect("fixture source path"),
                module: RustModuleId::new(
                    RustCrateId::new("fixture", &crate::work::CancellationProbe::new())
                        .expect("fixture crate"),
                    RustModulePath::new(Vec::new()).expect("fixture module"),
                ),
            }
        }

        fn base(&self, visibility: Visibility) -> BaseType {
            BaseType::new(
                "contract_item".to_owned(),
                visibility,
                self.file.clone(),
                self.module.clone(),
                RustAttributes::default(),
            )
        }
    }

    #[test]
    fn macro_tokens_are_required_valid_and_format_canonical() {
        let fixture = MacroFixture::new();
        let compact = MacroType::new(
            fixture.base(Visibility::Private),
            "contract_item!()".to_owned(),
        )
        .expect("compact macro tokens");
        let spaced = MacroType::new(
            fixture.base(Visibility::Private),
            "  contract_item ! ( )  ".to_owned(),
        )
        .expect("spaced macro tokens");

        assert_eq!(compact, spaced);
        assert_eq!(compact.tokens(), "contract_item ! ()");

        for invalid in ["", "   ", "plain_ident", "contract_item ! ("] {
            let error = MacroType::new(fixture.base(Visibility::Private), invalid.to_owned())
                .expect_err("invalid macro tokens must fail");
            assert!(error.to_string().contains("macro tokens"), "{error}");
        }
    }

    #[test]
    fn top_level_macro_visibility_is_always_private() {
        let fixture = MacroFixture::new();

        MacroType::new(
            fixture.base(Visibility::Private),
            "contract_item ! ()".to_owned(),
        )
        .expect("private top-level macro");

        for visibility in [Visibility::Public, Visibility::Crate] {
            let error = MacroType::new(fixture.base(visibility), "contract_item ! ()".to_owned())
                .expect_err("Rust item macros cannot carry explicit visibility");
            assert!(
                error.to_string().contains("top-level macro visibility"),
                "{error}"
            );
        }
    }
}
