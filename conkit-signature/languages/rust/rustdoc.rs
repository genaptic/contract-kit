use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::limits::{RustExtractionLimits, RustExtractionUsage};
use crate::work::CancellationProbe;
use std::collections::BTreeSet;

mod artifact;
mod declarations;
mod index;
mod modules;
mod provenance;
mod types;

use index::{CompilerInventory, RustdocIndex};

pub use artifact::{
    CompilerSourcePath, CompilerSourceProvenance, RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
    RUSTDOC_FORMAT_VERSION, RustCompilerArtifact, RustCompilerArtifactFailure, RustCompilerCrate,
};
pub(crate) use artifact::{RustCompilerExtraction, RustCompilerExtractionContext};

impl RustCompilerArtifact {
    pub(crate) fn extract<'limits>(
        mut self,
        source_files: FileCatalog,
        allowed_files: &BTreeSet<CatalogPath>,
        limits: &'limits RustExtractionLimits,
        usage: &mut RustExtractionUsage<'limits>,
        cancellation: &CancellationProbe,
    ) -> Result<RustCompilerExtraction, SignatureContractKitError> {
        cancellation.checkpoint()?;
        limits.validate_compiler_artifact_bytes(self.rustdoc_json.len())?;
        limits.validate_source_count(allowed_files.len())?;
        let context = self.validate_metadata(cancellation)?;
        if !allowed_files.contains(&context.crate_metadata.root) {
            return Err(RustCompilerArtifactFailure::source_map(
                Some(context.crate_metadata.root_item_id),
                format!(
                    "compiler crate root {} is absent from the contract file allowlist",
                    context.crate_metadata.root
                ),
            ));
        }
        let document = serde_json::from_slice::<rustdoc_types::Crate>(&self.rustdoc_json).map_err(
            |error| RustCompilerArtifactFailure::InvalidJson {
                message: error.to_string(),
            },
        )?;
        self.validate_document(&context, &document, limits, cancellation)?;
        let source_map = self.validate_source_map(
            &context,
            &document,
            allowed_files,
            &source_files,
            limits,
            cancellation,
        )?;
        let sources =
            self.parse_sources(&context, &source_map, source_files, limits, cancellation)?;
        let index = RustdocIndex {
            context,
            document,
            source_map,
        };
        CompilerInventory::new(sources, limits, usage, cancellation).extract(index)
    }
}

#[cfg(test)]
mod tests;
