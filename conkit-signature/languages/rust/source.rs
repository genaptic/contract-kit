use crate::files::CatalogPath;

pub(super) struct RustSourceFile {
    path: CatalogPath,
    bytes: Vec<u8>,
}

impl RustSourceFile {
    pub(super) fn new(path: CatalogPath, bytes: Vec<u8>) -> Self {
        Self { path, bytes }
    }

    pub(super) fn into_parts(self) -> (CatalogPath, Vec<u8>) {
        (self.path, self.bytes)
    }

    pub(super) fn path(&self) -> &CatalogPath {
        &self.path
    }
}
