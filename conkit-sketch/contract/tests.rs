use super::model::SketchContracts as ParsedSketchContracts;
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::limits::SketchLimits;
use crate::work::CancellationProbe;

pub(super) struct SketchContracts;

impl SketchContracts {
    pub(super) fn from_catalog(
        catalog: FileCatalog,
    ) -> Result<ParsedSketchContracts, SketchContractKitError> {
        Self::from_catalog_with_limits(catalog, &SketchLimits::default())
    }

    pub(super) fn from_catalog_with_limits(
        catalog: FileCatalog,
        limits: &SketchLimits,
    ) -> Result<ParsedSketchContracts, SketchContractKitError> {
        let mut yaml_budget = limits.yaml_budget();
        ParsedSketchContracts::from_catalog(
            catalog,
            limits,
            &mut yaml_budget,
            &CancellationProbe::new(),
        )
    }

    pub(super) fn diff(
        current: &ParsedSketchContracts,
        previous: &ParsedSketchContracts,
    ) -> crate::DiffResponse {
        current
            .diff_against(previous, &CancellationProbe::new())
            .expect("semantic diff")
    }
}

pub(super) struct TestCatalog {
    catalog: FileCatalog,
}

impl TestCatalog {
    pub(super) fn new() -> Self {
        Self {
            catalog: FileCatalog::new(),
        }
    }

    pub(super) fn with_file(mut self, path: &str, yaml: &str) -> Self {
        self.catalog
            .insert(
                CatalogPath::new(path).expect("test path"),
                yaml.as_bytes().to_vec(),
            )
            .expect("insert test yaml");
        self
    }

    pub(super) fn into_catalog(self) -> FileCatalog {
        self.catalog
    }
}

pub(super) struct ContractYaml;

impl ContractYaml {
    pub(super) fn linked(label: &str, sketch_id: &str, signature_type: &str, code: &str) -> String {
        format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: lib.rs, kind: library }}] }}\nsignatures:\n  - {label}:\n      file: lib.rs\n      signature_type: {signature_type}\n      name: answer\n      sketch: {sketch_id}\nsketches:\n  - {sketch_id}:\n      file: lib.rs\n      signature: {label}\n      signature_type: {signature_type}\n      matching: {{ normalization: exact_lines_v1, occurrence: at_least_one }}\n      code: '{code}'\n"
        )
    }
}
