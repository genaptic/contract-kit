//! Concrete adapter for the independent sketch contract domain.

use crate::context::ApplicationCancellation;
use crate::error::CliError;

/// Initialized sketch service used by command handlers.
pub(crate) struct SketchAdapter {
    kit: conkit_sketch::SketchContractKit,
    cancellation: ApplicationCancellation,
}

impl SketchAdapter {
    /// Builds the sketch kit over application-owned work and resource budgets.
    ///
    /// # Errors
    ///
    /// Returns an error if the sketch domain cannot initialize its CPU pool.
    pub(crate) fn initialize(
        work: conkit_sketch::WorkOptions,
        limits: conkit_sketch::SketchLimits,
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        Ok(Self {
            kit: conkit_sketch::SketchContractKit::builder()
                .with_work_options(work)
                .with_limits(limits)
                .build()?,
            cancellation: cancellation.clone(),
        })
    }

    /// Checks sketches after converting CLI catalogs to sketch catalogs.
    ///
    /// # Errors
    ///
    /// Returns an error when catalog conversion or sketch checking fails.
    pub(crate) async fn check(
        &self,
        request: SketchCheckRequest,
    ) -> Result<conkit_sketch::CheckResponse, CliError> {
        Ok(self
            .kit
            .check(request.into_sketch_request(&self.cancellation)?)
            .await?)
    }

    /// Refreshes explicitly linked sketches and converts the result back.
    ///
    /// # Errors
    ///
    /// Returns an error when request conversion, generation, or response
    /// catalog conversion fails.
    pub(crate) async fn generate(
        &self,
        request: SketchGenerateRequest,
    ) -> Result<SketchGenerateResponse, CliError> {
        let response = self
            .kit
            .generate(request.into_sketch_request(&self.cancellation)?)
            .await?;

        Ok(SketchGenerateResponse {
            contract_files: SketchCatalogs::into_signature_catalog(
                response.contract_files,
                &self.cancellation,
            )?,
            counts: response.counts,
        })
    }

    /// Diffs current and previous contracts through the sketch domain.
    ///
    /// # Errors
    ///
    /// Returns an error when either catalog cannot cross the sketch boundary
    /// or the sketch domain cannot compare it.
    pub(crate) async fn diff(
        &self,
        current_contract_files: conkit_signature::FileCatalog,
        previous_contract_files: conkit_signature::FileCatalog,
    ) -> Result<conkit_sketch::DiffResponse, CliError> {
        Ok(self
            .kit
            .diff(conkit_sketch::DiffRequest {
                current_contract_files: SketchCatalogs::from_signature_catalog(
                    current_contract_files,
                    &self.cancellation,
                )?,
                previous_contract_files: SketchCatalogs::from_signature_catalog(
                    previous_contract_files,
                    &self.cancellation,
                )?,
            })
            .await?)
    }
}

/// Complete sketch-check values selected by the CLI.
#[derive(Debug)]
pub(crate) struct SketchCheckRequest {
    source_files: conkit_signature::FileCatalog,
    contract_files: conkit_signature::FileCatalog,
    report: conkit_sketch::ReportRequest,
    mode: conkit_sketch::CheckMode,
}

impl SketchCheckRequest {
    /// Builds a complete sketch-check request.
    pub(crate) fn new(
        source_files: conkit_signature::FileCatalog,
        contract_files: conkit_signature::FileCatalog,
        report: conkit_sketch::ReportRequest,
        mode: conkit_sketch::CheckMode,
    ) -> Self {
        Self {
            source_files,
            contract_files,
            report,
            mode,
        }
    }

    fn into_sketch_request(
        self,
        cancellation: &ApplicationCancellation,
    ) -> Result<conkit_sketch::CheckRequest, CliError> {
        Ok(conkit_sketch::CheckRequest {
            source_files: SketchCatalogs::from_signature_catalog(self.source_files, cancellation)?,
            contract_files: SketchCatalogs::from_signature_catalog(
                self.contract_files,
                cancellation,
            )?,
            report: self.report,
            mode: self.mode,
        })
    }
}

/// Combined documents and exact signature-resolved sketch seeds.
#[derive(Debug)]
pub(crate) struct SketchGenerateRequest {
    contract_files: conkit_signature::FileCatalog,
    seeds: Vec<conkit_signature::ResolvedSketchSeed>,
}

impl SketchGenerateRequest {
    /// Builds a complete sketch-generation request.
    pub(crate) fn new(
        contract_files: conkit_signature::FileCatalog,
        seeds: Vec<conkit_signature::ResolvedSketchSeed>,
    ) -> Self {
        Self {
            contract_files,
            seeds,
        }
    }

    fn into_sketch_request(
        self,
        cancellation: &ApplicationCancellation,
    ) -> Result<conkit_sketch::GenerateRequest, CliError> {
        let mut seeds = Vec::with_capacity(self.seeds.len());
        for seed in self.seeds {
            cancellation.checkpoint()?;
            seeds.push(SketchCatalogs::from_resolved_seed(seed)?);
        }
        Ok(conkit_sketch::GenerateRequest {
            contract_files: SketchCatalogs::from_signature_catalog(
                self.contract_files,
                cancellation,
            )?,
            seeds,
            mode: conkit_sketch::GenerateMode::FullRefresh,
        })
    }
}

/// Completed sketch generation in the CLI's signature catalog representation.
#[derive(Debug)]
pub(crate) struct SketchGenerateResponse {
    contract_files: conkit_signature::FileCatalog,
    counts: conkit_sketch::SketchGenerationCounts,
}

impl SketchGenerateResponse {
    /// Consumes the response into its completed documents and cohesive counts.
    pub(crate) fn into_parts(
        self,
    ) -> (
        conkit_signature::FileCatalog,
        conkit_sketch::SketchGenerationCounts,
    ) {
        (self.contract_files, self.counts)
    }
}

struct SketchCatalogs;

impl SketchCatalogs {
    fn from_signature_catalog(
        catalog: conkit_signature::FileCatalog,
        cancellation: &ApplicationCancellation,
    ) -> Result<conkit_sketch::FileCatalog, CliError> {
        let mut converted = conkit_sketch::FileCatalog::new();

        for (path, bytes) in catalog.into_entries() {
            cancellation.checkpoint()?;
            converted.insert(conkit_sketch::CatalogPath::new(path.as_str())?, bytes)?;
        }

        Ok(converted)
    }

    fn into_signature_catalog(
        catalog: conkit_sketch::FileCatalog,
        cancellation: &ApplicationCancellation,
    ) -> Result<conkit_signature::FileCatalog, CliError> {
        let mut converted = conkit_signature::FileCatalog::new();

        for (path, bytes) in catalog.into_entries() {
            cancellation.checkpoint()?;
            converted.insert(conkit_signature::CatalogPath::new(path.as_str())?, bytes)?;
        }

        Ok(converted)
    }

    fn from_resolved_seed(
        seed: conkit_signature::ResolvedSketchSeed,
    ) -> Result<conkit_sketch::SketchSeed, CliError> {
        Ok(conkit_sketch::SketchSeed {
            contract_file: conkit_sketch::CatalogPath::new(seed.contract_file.as_str())?,
            document_index: seed.document_index,
            sketch_id: seed.sketch_id,
            signature_type: seed.signature_type,
            file: conkit_sketch::CatalogPath::new(seed.file.as_str())?,
            code: seed.code,
        })
    }
}

#[cfg(test)]
mod tests {
    use conkit_signature::{CatalogPath, FileCatalog};

    use super::{
        SketchCatalogs, SketchCheckRequest, SketchGenerateRequest, SketchGenerateResponse,
    };

    #[test]
    fn check_request_preserves_selected_catalogs_report_and_mode() {
        let source_path = CatalogPath::new("src/lib.rs").expect("source path");
        let contract_path = CatalogPath::new("main.yml").expect("contract path");
        let mut source_files = FileCatalog::new();
        source_files
            .insert(source_path, b"source\n".to_vec())
            .expect("source entry");
        let mut contract_files = FileCatalog::new();
        contract_files
            .insert(contract_path, b"contract\n".to_vec())
            .expect("contract entry");
        let output_file = conkit_sketch::CatalogPath::new("report.json").expect("report path");
        let report = conkit_sketch::ReportRequest::Generate {
            format: conkit_sketch::ReportFormat::Json,
            output_file: output_file.clone(),
        };

        let request = SketchCheckRequest::new(
            source_files,
            contract_files,
            report,
            conkit_sketch::CheckMode::Warning,
        )
        .into_sketch_request(&crate::context::ApplicationCancellation::new())
        .expect("converted request");

        assert_eq!(request.source_files.len(), 1);
        assert_eq!(request.contract_files.len(), 1);
        assert_eq!(request.mode, conkit_sketch::CheckMode::Warning);
        assert_eq!(
            request.report,
            conkit_sketch::ReportRequest::Generate {
                format: conkit_sketch::ReportFormat::Json,
                output_file,
            },
        );
    }

    #[test]
    fn generation_request_converts_catalogs_and_resolved_seeds() {
        let contract_path = CatalogPath::new("main.yml").expect("contract path");
        let source_path = CatalogPath::new("src/lib.rs").expect("source path");
        let mut contract_files = FileCatalog::new();
        contract_files
            .insert(contract_path.clone(), b"contract\n".to_vec())
            .expect("contract entry");
        let seed = conkit_signature::ResolvedSketchSeed {
            contract_file: contract_path,
            document_index: 2,
            sketch_id: "answer_path".to_owned(),
            signature_type: "function".to_owned(),
            file: source_path,
            code: "pub fn answer() -> u8 { 42 }".to_owned(),
        };

        let request = SketchGenerateRequest::new(contract_files, vec![seed])
            .into_sketch_request(&crate::context::ApplicationCancellation::new())
            .expect("converted request");

        assert_eq!(request.contract_files.len(), 1);
        assert_eq!(request.seeds.len(), 1);
        assert_eq!(request.seeds[0].contract_file.as_str(), "main.yml");
        assert_eq!(request.seeds[0].document_index, 2);
        assert_eq!(request.seeds[0].file.as_str(), "src/lib.rs");
        assert_eq!(request.seeds[0].sketch_id, "answer_path");
        assert_eq!(request.mode, conkit_sketch::GenerateMode::FullRefresh);
    }

    #[test]
    fn catalog_conversion_roundtrips_paths_bytes_and_cohesive_response_parts() {
        let path = CatalogPath::new("main.yml").expect("contract path");
        let mut signature_catalog = FileCatalog::new();
        signature_catalog
            .insert(path.clone(), b"contract\n".to_vec())
            .expect("contract entry");

        let cancellation = crate::context::ApplicationCancellation::new();
        let sketch_catalog =
            SketchCatalogs::from_signature_catalog(signature_catalog, &cancellation)
                .expect("sketch catalog");
        let signature_catalog =
            SketchCatalogs::into_signature_catalog(sketch_catalog, &cancellation)
                .expect("signature catalog");
        let (contract_files, counts) = SketchGenerateResponse {
            contract_files: signature_catalog,
            counts: conkit_sketch::SketchGenerationCounts {
                linked_sketch_count: 3,
                refreshed_sketch_count: 3,
                changed_sketch_count: 2,
                changed_document_count: 1,
            },
        }
        .into_parts();

        assert_eq!(contract_files.get(&path), Some(b"contract\n".as_slice()));
        assert_eq!(counts.linked_sketch_count, 3);
        assert_eq!(counts.refreshed_sketch_count, 3);
        assert_eq!(counts.changed_sketch_count, 2);
        assert_eq!(counts.changed_document_count, 1);
    }

    #[test]
    fn canceled_catalog_conversion_stops_before_domain_work() {
        let mut signature_catalog = FileCatalog::new();
        signature_catalog
            .insert(
                CatalogPath::new("main.yml").expect("contract path"),
                b"contract\n".to_vec(),
            )
            .expect("contract entry");
        let cancellation = crate::context::ApplicationCancellation::new();
        cancellation.request();

        let error = SketchCatalogs::from_signature_catalog(signature_catalog, &cancellation)
            .expect_err("pre-canceled conversion must stop before domain dispatch");

        assert!(matches!(error, crate::error::CliError::OperationCanceled));
    }
}
