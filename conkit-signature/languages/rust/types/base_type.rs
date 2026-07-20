use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::RustModuleId;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::primitive_types::Visibility;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BaseType {
    name: String,
    visibility: Visibility,
    #[serde(skip)]
    file_path: CatalogPath,
    module_id: RustModuleId,
    attributes: RustAttributes,
}

#[derive(Clone)]
pub(crate) struct RustImplementationContext {
    file_path: CatalogPath,
    module_id: RustModuleId,
    visibility: Visibility,
}

impl RustImplementationContext {
    pub(crate) fn new(
        owner_id: &RustItemId,
        owner: &BaseType,
    ) -> Result<Self, SignatureContractKitError> {
        if owner.module_id != *owner_id.module_id() {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "Rust owner declaration {} carries module {} instead of its canonical module {}",
                owner_id.render(),
                owner.module_id,
                owner_id.module_id()
            )));
        }
        Ok(Self {
            file_path: owner.file_path.clone(),
            module_id: owner.module_id.clone(),
            visibility: owner.visibility.clone(),
        })
    }

    pub(crate) fn normalize_base(&self, base: &mut BaseType, trait_owned: bool) {
        base.file_path = self.file_path.clone();
        base.module_id = self.module_id.clone();
        self.normalize_visibility(&mut base.visibility, trait_owned);
    }

    pub(crate) fn normalize_visibility(&self, visibility: &mut Visibility, trait_owned: bool) {
        if trait_owned {
            *visibility = self.visibility.clone();
        }
    }
}

impl BaseType {
    pub(crate) fn new(
        name: String,
        visibility: Visibility,
        file_path: CatalogPath,
        module_id: RustModuleId,
        attributes: RustAttributes,
    ) -> Self {
        Self {
            name,
            visibility,
            file_path,
            module_id,
            attributes,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn file_path(&self) -> &CatalogPath {
        &self.file_path
    }

    pub(crate) fn module_id(&self) -> &RustModuleId {
        &self.module_id
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }
}
