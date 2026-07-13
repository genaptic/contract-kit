//! Runtime dependencies shared by command handlers.
//!
//! `CommandContext` is created after clap parsing and before command
//! execution. It keeps command handlers explicit about which domain adapters
//! and output policy they use.

use anyhow::Result;

use crate::contracts::SketchAdapter;
use crate::output::Output;

/// Initialized services available while a command executes.
pub(crate) struct CommandContext {
    signature: conkit_signature::SignatureContractKit,
    sketch: SketchAdapter,
    output: Output,
}

impl CommandContext {
    /// Initializes every CLI-owned runtime dependency.
    ///
    /// # Errors
    ///
    /// Returns an error if the signature or sketch contract adapter cannot be
    /// initialized.
    pub(crate) fn initialize() -> Result<Self> {
        Ok(Self {
            signature: conkit_signature::SignatureContractKit::builder().build()?,
            sketch: SketchAdapter::initialize()?,
            output: Output,
        })
    }

    /// Returns the signature contract adapter.
    pub(crate) fn signature(&self) -> &conkit_signature::SignatureContractKit {
        &self.signature
    }

    /// Returns the sketch contract adapter.
    pub(crate) fn sketch(&self) -> &SketchAdapter {
        &self.sketch
    }

    /// Returns the output sink for user-facing summaries.
    pub(crate) fn output(&self) -> &Output {
        &self.output
    }
}
