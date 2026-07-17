use crate::support::PublicFixture;
use conkit_signature::{
    CheckMode, CheckRequest, ContractScope, DiffRequest, FileCatalog, GenerateRequest,
    ResolveSketchesRequest, RustCrateKind, RustExtractionInput, SignatureContractKit,
    SignatureContractKitBuilder,
};
use std::future::Future;
use std::sync::Arc;

struct SpawnContract;

impl SpawnContract {
    fn assert_send<F>(future: F)
    where
        F: Future + Send,
    {
        drop(future);
    }

    fn assert_spawn_compatible<F>(future: F)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        drop(future);
    }

    fn assert_send_static<T: Send + 'static>() {}

    fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    fn check_request() -> CheckRequest {
        CheckRequest {
            extraction: RustExtractionInput::Syntax,
            source_files: FileCatalog::new(),
            contract_files: FileCatalog::new(),
            report: conkit_signature::ReportRequest::None,
            mode: CheckMode::Default,
        }
    }

    fn generate_request() -> GenerateRequest {
        GenerateRequest {
            extraction: RustExtractionInput::Syntax,
            source_files: FileCatalog::new(),
            target: PublicFixture::target(
                &["lib.rs"],
                vec![PublicFixture::crate_root(
                    "sample",
                    "lib.rs",
                    RustCrateKind::Library,
                )],
            ),
            scope: ContractScope::Signatures,
        }
    }

    fn resolve_sketches_request() -> ResolveSketchesRequest {
        ResolveSketchesRequest {
            extraction: RustExtractionInput::Syntax,
            source_files: FileCatalog::new(),
            contract_files: FileCatalog::new(),
        }
    }

    fn diff_request() -> DiffRequest {
        DiffRequest {
            current_contract_files: FileCatalog::new(),
            previous_contract_files: FileCatalog::new(),
        }
    }

    fn assert_public_contracts() {
        Self::assert_send_static::<SignatureContractKitBuilder>();
        Self::assert_send_sync_static::<SignatureContractKit>();

        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        Self::assert_send(kit.check(Self::check_request()));
        Self::assert_send(kit.generate(Self::generate_request()));
        Self::assert_send(kit.resolve_sketches(Self::resolve_sketches_request()));
        Self::assert_send(kit.diff(Self::diff_request()));

        let kit = Arc::new(kit);

        let task_kit = Arc::clone(&kit);
        Self::assert_spawn_compatible(async move { task_kit.check(Self::check_request()).await });

        let task_kit = Arc::clone(&kit);
        Self::assert_spawn_compatible(
            async move { task_kit.generate(Self::generate_request()).await },
        );

        let task_kit = Arc::clone(&kit);
        Self::assert_spawn_compatible(async move {
            task_kit
                .resolve_sketches(Self::resolve_sketches_request())
                .await
        });

        let task_kit = Arc::clone(&kit);
        Self::assert_spawn_compatible(async move { task_kit.diff(Self::diff_request()).await });
    }
}

#[test]
fn public_operations_support_send_and_owning_spawn_contracts() {
    SpawnContract::assert_public_contracts();
}
