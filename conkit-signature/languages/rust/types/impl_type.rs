use crate::files::CatalogPath;
use crate::languages::rust::types::callable_type::{RustMethod, RustMethodCanonical};
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, Visibility};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImplementationType {
    owner_type: String,
    implemented_trait: RustImplementedTrait,
    is_default: bool,
    is_unsafe: bool,
    generics: RustGenericMetadata,
    methods: Vec<RustMethod>,
}

impl ImplementationType {
    pub(crate) fn new(owner_type: String) -> Self {
        Self {
            owner_type,
            implemented_trait: RustImplementedTrait::Inherent,
            is_default: false,
            is_unsafe: false,
            generics: RustGenericMetadata::default(),
            methods: Vec::new(),
        }
    }

    pub(crate) fn with_implemented_trait(
        mut self,
        implemented_trait: RustImplementedTrait,
    ) -> Self {
        self.implemented_trait = implemented_trait;
        self
    }

    pub(crate) fn with_qualifiers(mut self, is_default: bool, is_unsafe: bool) -> Self {
        self.is_default = is_default;
        self.is_unsafe = is_unsafe;
        self
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_methods(mut self, methods: Vec<RustMethod>) -> Self {
        self.methods = methods;
        self
    }

    pub(crate) fn owner_type(&self) -> &str {
        &self.owner_type
    }

    pub(crate) fn implemented_trait(&self) -> &RustImplementedTrait {
        &self.implemented_trait
    }

    pub(crate) fn is_default(&self) -> bool {
        self.is_default
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn methods(&self) -> &[RustMethod] {
        &self.methods
    }

    pub(crate) fn descriptor_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&(
            &self.owner_type,
            &self.implemented_trait,
            self.is_default,
            self.is_unsafe,
            &self.generics,
        ))
    }

    pub(crate) fn append_methods(&mut self, mut incoming: Self) {
        self.methods.append(&mut incoming.methods);
    }

    pub(crate) fn sort_methods(&mut self) -> Result<(), serde_json::Error> {
        let canonical = self
            .methods
            .iter()
            .map(|method| serde_json::to_vec(&method.canonical_form()))
            .collect::<Result<Vec<_>, _>>()?;
        let methods = std::mem::take(&mut self.methods);
        let mut methods = canonical.into_iter().zip(methods).collect::<Vec<_>>();
        methods.sort_by(|left, right| left.0.cmp(&right.0));
        self.methods = methods.into_iter().map(|(_, method)| method).collect();
        Ok(())
    }

    pub(crate) fn into_owner_context(
        mut self,
        owner_name: String,
        owner_file: CatalogPath,
        owner_module_path: Vec<String>,
        owner_visibility: Visibility,
    ) -> Self {
        let trait_implementation =
            matches!(self.implemented_trait, RustImplementedTrait::Trait { .. });
        self.owner_type = owner_name;
        self.methods = self
            .methods
            .into_iter()
            .map(|method| {
                let visibility = if trait_implementation {
                    owner_visibility.clone()
                } else {
                    method.visibility().clone()
                };
                method.into_owner_context(owner_file.clone(), owner_module_path.clone(), visibility)
            })
            .collect();
        self
    }

    pub(in crate::languages::rust) fn canonical_form(&self) -> ImplementationCanonical {
        ImplementationCanonical {
            owner_type: self.owner_type.clone(),
            implemented_trait: self.implemented_trait.clone(),
            is_default: self.is_default,
            is_unsafe: self.is_unsafe,
            generics: self.generics.clone(),
            methods: self
                .methods
                .iter()
                .map(RustMethod::canonical_form)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustImplementedTrait {
    Inherent,
    Trait {
        name: String,
        polarity: RustImplPolarity,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub(crate) enum RustImplPolarity {
    Positive,
    Negative,
}

impl RustImplPolarity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }
}

#[derive(Serialize)]
pub(in crate::languages::rust) struct ImplementationCanonical {
    owner_type: String,
    implemented_trait: RustImplementedTrait,
    is_default: bool,
    is_unsafe: bool,
    generics: RustGenericMetadata,
    methods: Vec<RustMethodCanonical>,
}
