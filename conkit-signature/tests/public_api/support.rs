use conkit_signature::{
    CatalogPath, CheckDiagnostic, CheckMode, CheckRequest, CheckResponse, ContractScope,
    DiffRequest, DiffResponse, FileCatalog, GenerateDocument, GenerateRequest, GenerateResponse,
    GenerateTarget, ReportRequest, RustCrateKind, RustCrateRoot, RustExtractionInput,
    SignatureContractKit, SignatureContractKitBuilder,
};

pub(super) struct PublicFixture {
    pub(super) kit: SignatureContractKit,
}

impl PublicFixture {
    pub(super) fn new() -> Self {
        Self {
            kit: SignatureContractKitBuilder::default().build().expect("kit"),
        }
    }

    pub(super) fn catalog<const N: usize>(entries: [(&str, &[u8]); N]) -> FileCatalog {
        let mut catalog = FileCatalog::new();
        for (path, bytes) in entries {
            catalog
                .insert(
                    CatalogPath::new(path).expect("fixture catalog path"),
                    bytes.to_vec(),
                )
                .expect("fixture catalog insert");
        }
        catalog
    }

    pub(super) fn crate_root(id: &str, root: &str, kind: RustCrateKind) -> RustCrateRoot {
        RustCrateRoot {
            id: id.to_owned(),
            root: CatalogPath::new(root).expect("crate root path"),
            kind,
        }
    }

    pub(super) fn target(files: &[&str], crates: Vec<RustCrateRoot>) -> GenerateTarget {
        GenerateTarget::New(GenerateDocument {
            contract_file: CatalogPath::new("main.yml").expect("contract path"),
            root: "../src".to_owned(),
            files: files
                .iter()
                .map(|file| CatalogPath::new(*file).expect("allowlisted source path"))
                .collect(),
            crates,
        })
    }

    pub(super) fn single_target(source_file: &str) -> GenerateTarget {
        Self::target(
            &[source_file],
            vec![Self::crate_root(
                "sample",
                source_file,
                RustCrateKind::Library,
            )],
        )
    }

    pub(super) fn generate(
        &self,
        source_files: FileCatalog,
        files: &[&str],
        crates: Vec<RustCrateRoot>,
    ) -> GenerateResponse {
        futures_executor::block_on(self.kit.generate(GenerateRequest {
            extraction: RustExtractionInput::Syntax,
            source_files,
            target: Self::target(files, crates),
            scope: ContractScope::Signatures,
        }))
        .expect("syntax generation")
    }

    pub(super) fn check(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        mode: CheckMode,
    ) -> CheckResponse {
        futures_executor::block_on(self.kit.check(CheckRequest {
            extraction: RustExtractionInput::Syntax,
            source_files,
            contract_files,
            report: ReportRequest::None,
            mode,
        }))
        .expect("syntax check")
    }

    pub(super) fn diff(
        &self,
        current_contract_files: FileCatalog,
        previous_contract_files: FileCatalog,
    ) -> DiffResponse {
        futures_executor::block_on(self.kit.diff(DiffRequest {
            current_contract_files,
            previous_contract_files,
        }))
        .expect("contract diff")
    }

    pub(super) fn generated_yaml(response: &GenerateResponse) -> &str {
        std::str::from_utf8(
            response
                .contract_files
                .get(&CatalogPath::new("main.yml").expect("contract path"))
                .expect("generated contract"),
        )
        .expect("generated YAML")
    }

    pub(super) fn digest(fill: char) -> String {
        std::iter::repeat_n(fill, 64).collect()
    }

    pub(super) fn assert_digest(digest: &str) {
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    pub(super) fn has_cfg_capability_warning(response: &CheckResponse) -> bool {
        response.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                CheckDiagnostic::Warning { message }
                    if message.contains("cfg") && message.contains("rust_syntax_v2")
            )
        })
    }
}
