use conkit_signature::{
    CatalogPath, CheckMode, CheckRequest, CheckResponse, ContractScope, FileCatalog,
    GenerateDocument, GenerateRequest, GenerateResponse, GenerateTarget, ReportRequest,
    RustCrateKind, RustCrateRoot, RustExtractionInput, SignatureContractKit,
    SignatureContractKitError,
};
use futures_executor::block_on;

std::thread_local! {
    static SIGNATURE_HARNESS: SignatureFuzzHarness = SignatureFuzzHarness::new();
}

pub struct SignatureFuzzHarness {
    kit: SignatureContractKit,
    source_file: CatalogPath,
    contract_file: CatalogPath,
}

impl SignatureFuzzHarness {
    fn new() -> Self {
        Self {
            kit: SignatureContractKit::builder()
                .build()
                .expect("signature fuzz kit must initialize"),
            source_file: CatalogPath::new("lib.rs")
                .expect("static signature source path must be valid"),
            contract_file: CatalogPath::new("fuzz.yml")
                .expect("static signature contract path must be valid"),
        }
    }

    pub fn with<R>(run: impl FnOnce(&Self) -> R) -> R {
        SIGNATURE_HARNESS.with(run)
    }

    pub fn source_catalog(&self, source: Vec<u8>) -> FileCatalog {
        self.source_files([(self.source_file.clone(), source)])
    }

    pub fn source_files(
        &self,
        entries: impl IntoIterator<Item = (CatalogPath, Vec<u8>)>,
    ) -> FileCatalog {
        let mut catalog = FileCatalog::new();
        for (path, bytes) in entries {
            catalog
                .insert(path, bytes)
                .expect("signature fuzz source catalog entries must be unique and valid");
        }
        catalog
    }

    pub fn check_contract(
        &self,
        source: Vec<u8>,
        contract: Vec<u8>,
        mode: CheckMode,
    ) -> Result<CheckResponse, SignatureContractKitError> {
        self.check_catalogs(
            self.source_catalog(source),
            self.contract_catalog(contract),
            mode,
        )
    }

    pub fn check_catalogs(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        mode: CheckMode,
    ) -> Result<CheckResponse, SignatureContractKitError> {
        block_on(self.kit.check(CheckRequest {
            extraction: RustExtractionInput::Syntax,
            source_files,
            contract_files,
            report: ReportRequest::None,
            mode,
        }))
    }

    pub fn generate_single_source(
        &self,
        source: Vec<u8>,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let source_file = self.source_file.clone();
        self.generate_new(
            self.source_catalog(source),
            vec![source_file.clone()],
            vec![RustCrateRoot {
                id: "fuzz".to_owned(),
                root: source_file,
                kind: RustCrateKind::Library,
            }],
        )
    }

    pub fn generate_new(
        &self,
        source_files: FileCatalog,
        files: Vec<CatalogPath>,
        crates: Vec<RustCrateRoot>,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        self.generate(
            source_files,
            GenerateTarget::New(GenerateDocument {
                contract_file: self.contract_file.clone(),
                root: "../src".to_owned(),
                files,
                crates,
            }),
        )
    }

    pub fn generate_existing(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        self.generate(source_files, GenerateTarget::Existing(contract_files))
    }

    fn contract_catalog(&self, contract: Vec<u8>) -> FileCatalog {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(self.contract_file.clone(), contract)
            .expect("signature fuzz contract catalog entry must be valid");
        catalog
    }

    fn generate(
        &self,
        source_files: FileCatalog,
        target: GenerateTarget,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        block_on(self.kit.generate(GenerateRequest {
            extraction: RustExtractionInput::Syntax,
            source_files,
            target,
            scope: ContractScope::Signatures,
        }))
    }
}
