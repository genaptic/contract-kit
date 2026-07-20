mod diff;
mod document;
mod edit;
mod model;
mod resolve;
#[cfg(test)]
mod tests;

pub(crate) use document::SketchContractDocuments;
pub(crate) use model::{SketchContract, SketchContracts};
pub use model::{SketchMatchPolicy, SketchNormalization, SketchOccurrence};
