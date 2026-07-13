use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::item_converter::RustItemConverter;
use crate::languages::rust::parser::{RustParsedEntry, RustParsedInventory};

pub(super) struct RustInventoryBuilder {
    file: CatalogPath,
    module_path: Vec<String>,
    entries: Vec<RustParsedEntry>,
    converter: RustItemConverter,
}

impl RustInventoryBuilder {
    pub(super) fn new(file: CatalogPath) -> Self {
        Self {
            file,
            module_path: Vec::new(),
            entries: Vec::new(),
            converter: RustItemConverter::default(),
        }
    }

    pub(super) fn collect_file(
        mut self,
        syntax: syn::File,
    ) -> Result<RustParsedInventory, SignatureContractKitError> {
        self.collect_items(syntax.items)?;

        Ok(RustParsedInventory::new(self.file, self.entries))
    }

    fn collect_items(&mut self, items: Vec<syn::Item>) -> Result<(), SignatureContractKitError> {
        for item in items {
            self.collect_item(item)?;
        }
        Ok(())
    }

    fn collect_item(&mut self, item: syn::Item) -> Result<(), SignatureContractKitError> {
        let entry = match item {
            syn::Item::Fn(item) => Some(self.converter().convert_function(&self.context(), item)?),
            syn::Item::Struct(item) => {
                Some(self.converter().convert_struct(&self.context(), item)?)
            }
            syn::Item::Enum(item) => Some(self.converter().convert_enum(&self.context(), item)?),
            syn::Item::Trait(item) => Some(self.converter().convert_trait(&self.context(), item)?),
            syn::Item::Impl(item) => Some(self.converter().convert_impl(&self.context(), item)?),
            syn::Item::Union(item) => Some(self.converter().convert_union(&self.context(), item)?),
            syn::Item::Mod(item) => return self.collect_module(item),
            syn::Item::Static(item) => {
                Some(self.converter().convert_static(&self.context(), item)?)
            }
            syn::Item::Macro(item) => {
                let context = self.context();
                Some(self.converter.convert_macro(&context, item)?)
            }
            syn::Item::Type(item) => {
                Some(self.converter().convert_type_alias(&self.context(), item)?)
            }
            _ => None,
        };

        if let Some(entry) = entry {
            self.entries.push(entry);
        }

        Ok(())
    }

    fn collect_module(&mut self, item: syn::ItemMod) -> Result<(), SignatureContractKitError> {
        if let Some((_, items)) = item.content {
            self.module_path.push(item.ident.to_string());
            self.collect_items(items)?;
            self.module_path.pop();
        }

        Ok(())
    }

    fn context(&self) -> RustItemContext {
        RustItemContext::new(self.file.clone(), self.module_path.clone())
    }

    fn converter(&self) -> &RustItemConverter {
        &self.converter
    }
}

pub(super) struct RustItemContext {
    file: CatalogPath,
    module_path: Vec<String>,
}

impl RustItemContext {
    pub(super) fn new(file: CatalogPath, module_path: Vec<String>) -> Self {
        Self { file, module_path }
    }

    pub(super) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(super) fn module_path(&self) -> &[String] {
        &self.module_path
    }
}
