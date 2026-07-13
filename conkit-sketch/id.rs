use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SketchId {
    value: String,
}

impl SketchId {
    pub(crate) fn from_contract(
        value: impl Into<String>,
        catalog_name: &CatalogPath,
    ) -> Result<Self, SketchContractKitError> {
        Self::from_user_text(value).ok_or_else(|| {
            SketchContractKitError::parse_failed(catalog_name, "sketch id must not be empty")
        })
    }

    pub(crate) fn from_seed(value: impl Into<String>) -> Result<Self, SketchContractKitError> {
        Self::from_user_text(value).ok_or_else(|| {
            SketchContractKitError::conversion_failed("sketch seed id must not be empty")
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }

    fn from_user_text(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let value = value.trim();

        if value.is_empty() {
            None
        } else {
            Some(Self {
                value: value.to_owned(),
            })
        }
    }
}

impl fmt::Display for SketchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::SketchId;
    use crate::files::CatalogPath;

    #[test]
    fn contract_and_seed_ids_keep_trimmed_nonempty_semantics() {
        let contract = CatalogPath::new("contracts/main.yml").expect("contract path");

        assert_eq!(
            SketchId::from_contract("  answer_body  ", &contract)
                .expect("contract ID")
                .as_str(),
            "answer_body"
        );
        assert_eq!(
            SketchId::from_seed("  answer_body  ")
                .expect("seed ID")
                .as_str(),
            "answer_body"
        );
        assert!(SketchId::from_contract(" \u{00a0} ", &contract).is_err());
        assert!(SketchId::from_seed(" \u{00a0} ").is_err());
    }
}
