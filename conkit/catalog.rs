//! Filesystem-to-catalog adapters.
//!
//! Source reads, contracts persistence, path security, persisted ownership,
//! and runtime reconciliation each have one concrete owner below this facade.

mod ownership;
mod path;
mod reconciliation;
mod source;
mod store;

pub(crate) use path::{PathRole, PortableCatalogPathKey, ResolvedPath};
pub(crate) use source::SourceTree;
pub(crate) use store::{
    ContractsStore, ExistingOutputPolicy, GeneratedContracts, GenerationReceipt,
};
