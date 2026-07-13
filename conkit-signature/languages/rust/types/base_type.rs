use crate::files::CatalogPath;
use crate::languages::rust::types::primitive_types::Visibility;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BaseType {
    name: String,
    visibility: Visibility,
    file_path: CatalogPath,
    module_path: Vec<String>,
    derives: Vec<String>,
}

impl BaseType {
    pub(crate) fn new(name: String, visibility: Visibility, file_path: CatalogPath) -> Self {
        Self {
            name,
            visibility,
            file_path,
            module_path: Vec::new(),
            derives: Vec::new(),
        }
    }

    pub(crate) fn with_module_path(mut self, module_path: Vec<String>) -> Self {
        self.module_path = module_path;
        self
    }

    pub(crate) fn into_context(
        mut self,
        file_path: CatalogPath,
        module_path: Vec<String>,
        visibility: Visibility,
    ) -> Self {
        self.file_path = file_path;
        self.module_path = module_path;
        self.visibility = visibility;
        self
    }

    pub(crate) fn with_derives(mut self, derives: Vec<String>) -> Self {
        self.derives = derives;
        self
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn derives(&self) -> &[String] {
        &self.derives
    }

    pub(super) fn canonical_form(&self) -> BaseCanonical {
        BaseCanonical {
            name: self.name.clone(),
            visibility: self.visibility.clone(),
            module_path: self.module_path.clone(),
            derives: self.derives.clone(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct BaseCanonical {
    name: String,
    visibility: Visibility,
    module_path: Vec<String>,
    derives: Vec<String>,
}
