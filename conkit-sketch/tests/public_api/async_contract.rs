use conkit_sketch::{
    CheckMode, CheckRequest, DiffRequest, FileCatalog, GenerateMode, GenerateRequest,
    ReportRequest, SketchContractKit, SketchContractKitBuilder,
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
            source_files: FileCatalog::new(),
            contract_files: FileCatalog::new(),
            report: ReportRequest::None,
            mode: CheckMode::Enforce,
        }
    }

    fn generate_request() -> GenerateRequest {
        GenerateRequest {
            contract_files: FileCatalog::new(),
            seeds: Vec::new(),
            mode: GenerateMode::FullRefresh,
        }
    }

    fn diff_request() -> DiffRequest {
        DiffRequest {
            current_contract_files: FileCatalog::new(),
            previous_contract_files: FileCatalog::new(),
        }
    }
}

#[test]
fn public_types_satisfy_send_static_contracts() {
    SpawnContract::assert_send_static::<SketchContractKitBuilder>();
    SpawnContract::assert_send_sync_static::<SketchContractKit>();
}

#[test]
fn directly_borrowed_operation_futures_are_send() {
    let kit = SketchContractKit::builder().build().expect("kit");

    SpawnContract::assert_send(kit.check(SpawnContract::check_request()));
    SpawnContract::assert_send(kit.generate(SpawnContract::generate_request()));
    SpawnContract::assert_send(kit.diff(SpawnContract::diff_request()));
}

#[test]
fn owning_operation_tasks_are_spawn_compatible() {
    let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
    let task_kit = Arc::clone(&kit);
    SpawnContract::assert_spawn_compatible(async move {
        task_kit.check(SpawnContract::check_request()).await
    });

    let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
    let task_kit = Arc::clone(&kit);
    SpawnContract::assert_spawn_compatible(async move {
        task_kit.generate(SpawnContract::generate_request()).await
    });

    let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
    let task_kit = Arc::clone(&kit);
    SpawnContract::assert_spawn_compatible(async move {
        task_kit.diff(SpawnContract::diff_request()).await
    });
}
