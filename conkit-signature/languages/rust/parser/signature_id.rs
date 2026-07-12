use crate::files::CatalogPath;
use crate::inventory::SignatureId;
use std::fmt;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RustItemId {
    file: CatalogPath,
    module_path: Vec<String>,
    kind: RustItemKind,
    name: String,
}

impl RustItemId {
    pub(crate) fn new(
        file: CatalogPath,
        module_path: Vec<String>,
        kind: RustItemKind,
        name: impl Into<String>,
    ) -> Self {
        Self {
            file,
            module_path,
            kind,
            name: name.into(),
        }
    }

    pub(crate) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(crate) fn module_path(&self) -> &[String] {
        &self.module_path
    }

    pub(crate) fn kind(&self) -> &RustItemKind {
        &self.kind
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn into_signature_id(self) -> SignatureId {
        SignatureId::new(self.render())
    }

    pub(crate) fn render(&self) -> String {
        let module = if self.module_path.is_empty() {
            String::new()
        } else {
            format!("::{}", self.module_path.join("::"))
        };

        format!("rust:{}{}:{}:{}", self.file, module, self.kind, self.name)
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum RustItemKind {
    Function,
    Struct,
    Enum,
    Trait,
    Implementation,
    Union,
    Static,
    Macro,
    TypeAlias,
}

impl fmt::Display for RustItemKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Function => formatter.write_str("function"),
            Self::Struct => formatter.write_str("struct"),
            Self::Enum => formatter.write_str("enum"),
            Self::Trait => formatter.write_str("trait"),
            Self::Implementation => formatter.write_str("impl"),
            Self::Union => formatter.write_str("union"),
            Self::Static => formatter.write_str("static"),
            Self::Macro => formatter.write_str("macro"),
            Self::TypeAlias => formatter.write_str("type_alias"),
        }
    }
}

pub(crate) struct RustImplementationId {
    owner_type: String,
    implemented_trait: Option<String>,
    polarity: crate::languages::rust::types::impl_type::RustImplPolarity,
}

impl RustImplementationId {
    pub(crate) fn inherent(owner_type: String) -> Self {
        Self {
            owner_type,
            implemented_trait: None,
            polarity: crate::languages::rust::types::impl_type::RustImplPolarity::Positive,
        }
    }

    pub(crate) fn trait_impl(
        owner_type: String,
        implemented_trait: String,
        polarity: crate::languages::rust::types::impl_type::RustImplPolarity,
    ) -> Self {
        Self {
            owner_type,
            implemented_trait: Some(implemented_trait),
            polarity,
        }
    }

    pub(crate) fn render(&self) -> String {
        match &self.implemented_trait {
            Some(trait_name) => {
                format!(
                    "{}:{} for {}",
                    self.polarity.as_str(),
                    trait_name,
                    self.owner_type
                )
            }
            None => format!("inherent:{}", self.owner_type),
        }
    }
}
