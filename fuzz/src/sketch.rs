use conkit_sketch::{
    CatalogPath, CheckMode, CheckRequest, CheckResponse, FileCatalog, GenerateMode,
    GenerateRequest, GenerateResponse, ReportRequest, SketchContractKit, SketchContractKitError,
    SketchOccurrence, SketchSeed,
};
use futures_executor::block_on;

std::thread_local! {
    static SKETCH_HARNESS: SketchFuzzHarness = SketchFuzzHarness::new();
}

pub struct SketchFuzzHarness {
    kit: SketchContractKit,
    source_file: CatalogPath,
    contract_file: CatalogPath,
}

impl SketchFuzzHarness {
    fn new() -> Self {
        Self {
            kit: SketchContractKit::builder()
                .build()
                .expect("sketch fuzz kit must initialize"),
            source_file: CatalogPath::new("lib.rs")
                .expect("static sketch source path must be valid"),
            contract_file: CatalogPath::new("fuzz.yml")
                .expect("static sketch contract path must be valid"),
        }
    }

    pub fn with<R>(run: impl FnOnce(&Self) -> R) -> R {
        SKETCH_HARNESS.with(run)
    }

    pub fn contract_catalog(&self, contract: Vec<u8>) -> FileCatalog {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(self.contract_file.clone(), contract)
            .expect("sketch fuzz contract catalog entry must be valid");
        catalog
    }

    pub fn check_contract(
        &self,
        source: Vec<u8>,
        contract: Vec<u8>,
        mode: CheckMode,
    ) -> Result<CheckResponse, SketchContractKitError> {
        self.check_catalog(source, self.contract_catalog(contract), mode)
    }

    pub fn check_catalog(
        &self,
        source: Vec<u8>,
        contract_files: FileCatalog,
        mode: CheckMode,
    ) -> Result<CheckResponse, SketchContractKitError> {
        block_on(self.kit.check(CheckRequest {
            source_files: self.source_catalog(source),
            contract_files,
            report: ReportRequest::None,
            mode,
        }))
    }

    pub fn generate(
        &self,
        contract_files: FileCatalog,
        seeds: Vec<SketchSeed>,
        mode: GenerateMode,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        block_on(self.kit.generate(GenerateRequest {
            contract_files,
            seeds,
            mode,
        }))
    }

    pub fn seed(&self, sketch_id: &str, signature_type: &str, code: String) -> SketchSeed {
        SketchSeed {
            contract_file: self.contract_file.clone(),
            document_index: 0,
            sketch_id: sketch_id.to_owned(),
            signature_type: signature_type.to_owned(),
            file: self.source_file.clone(),
            code,
        }
    }

    pub fn matching_contract(&self, occurrence: SketchOccurrence) -> Vec<u8> {
        let occurrence = match occurrence {
            SketchOccurrence::AtLeastOne => "at_least_one",
            SketchOccurrence::ExactlyOne => "exactly_one",
        };
        format!(
            concat!(
                "contract_version: 2\n",
                "root: ../src\n",
                "files: [lib.rs]\n",
                "extraction:\n",
                "  mode: rust_syntax_v2\n",
                "  profile: rust_api_v1\n",
                "  crates: [{{ id: fuzz, root: lib.rs, kind: library }}]\n",
                "signatures:\n",
                "  - matching_signature:\n",
                "      file: lib.rs\n",
                "      signature_type: function\n",
                "      sketch: matching_body\n",
                "sketches:\n",
                "  - matching_body:\n",
                "      file: lib.rs\n",
                "      signature: matching_signature\n",
                "      signature_type: function\n",
                "      matching: {{ normalization: exact_lines_v1, occurrence: {} }}\n",
                "      code: |-\n",
                "        needle\n",
                "        needle\n",
            ),
            occurrence,
        )
        .into_bytes()
    }

    fn source_catalog(&self, source: Vec<u8>) -> FileCatalog {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(self.source_file.clone(), source)
            .expect("sketch fuzz source catalog entry must be valid");
        catalog
    }
}
