//! Cross-domain conformance without a shared core crate.

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use conkit_signature::{
    CatalogPath as SignatureCatalogPath, DiffRequest as SignatureDiffRequest,
    FileCatalog as SignatureFileCatalog, SignatureContractKit, WorkOptions as SignatureWorkOptions,
    WorkerPool as SignatureWorkerPool,
};
use conkit_sketch::{
    CatalogPath as SketchCatalogPath, CheckMode as SketchCheckMode,
    CheckRequest as SketchCheckRequest, FileCatalog as SketchFileCatalog,
    ReportRequest as SketchReportRequest, SketchContractKit, WorkOptions as SketchWorkOptions,
    WorkerPool as SketchWorkerPool,
};
use futures_executor::block_on;
use rayon::{ThreadPool, ThreadPoolBuilder};

struct BlockedSharedPool {
    pool: Arc<ThreadPool>,
    release: Option<mpsc::SyncSender<()>>,
}

struct FutureRequirements;

impl FutureRequirements {
    fn require_send_static<F>(future: F)
    where
        F: Future + Send + 'static,
    {
        drop(future);
    }
}

impl BlockedSharedPool {
    fn new() -> Self {
        let pool = Arc::new(
            ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("one-worker shared pool"),
        );
        let (started_sender, started_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        pool.spawn(move || {
            started_sender.send(()).expect("announce blocked worker");
            release_receiver.recv().expect("release blocked worker");
        });
        started_receiver.recv().expect("worker reached barrier");

        Self {
            pool,
            release: Some(release_sender),
        }
    }

    fn shared(&self) -> Arc<ThreadPool> {
        Arc::clone(&self.pool)
    }

    fn poll_once<F>(future: Pin<&mut F>) -> Poll<F::Output>
    where
        F: Future,
    {
        let mut context = Context::from_waker(Waker::noop());
        future.poll(&mut context)
    }

    fn release(&mut self) {
        self.release
            .take()
            .expect("worker release is single-use")
            .send(())
            .expect("release blocked worker");
    }
}

impl Drop for BlockedSharedPool {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

#[test]
fn nominal_catalog_paths_and_ordering_remain_conformant() {
    let cases = [
        ("main.yml", true),
        ("nested/source.rs", true),
        ("space name/雪.rs", true),
        ("", false),
        ("/absolute.rs", false),
        ("../escape.rs", false),
        ("nested//source.rs", false),
        ("nested/./source.rs", false),
        ("nested\\source.rs", false),
        ("C:/source.rs", false),
    ];

    for (value, expected) in cases {
        assert_eq!(
            conkit_signature::CatalogPath::new(value).is_ok(),
            expected,
            "signature path result for {value:?}"
        );
        assert_eq!(
            conkit_sketch::CatalogPath::new(value).is_ok(),
            expected,
            "sketch path result for {value:?}"
        );
    }

    let mut signatures = conkit_signature::FileCatalog::new();
    signatures
        .insert(
            conkit_signature::CatalogPath::new("z.rs").expect("z path"),
            b"z".to_vec(),
        )
        .expect("z signature entry");
    signatures
        .insert(
            conkit_signature::CatalogPath::new("a.rs").expect("a path"),
            b"a".to_vec(),
        )
        .expect("a signature entry");
    let signature_duplicate = signatures
        .insert(
            conkit_signature::CatalogPath::new("a.rs").expect("duplicate path"),
            Vec::new(),
        )
        .expect_err("signature duplicate");

    let mut sketches = conkit_sketch::FileCatalog::new();
    sketches
        .insert(
            conkit_sketch::CatalogPath::new("z.rs").expect("z path"),
            b"z".to_vec(),
        )
        .expect("z sketch entry");
    sketches
        .insert(
            conkit_sketch::CatalogPath::new("a.rs").expect("a path"),
            b"a".to_vec(),
        )
        .expect("a sketch entry");
    let sketch_duplicate = sketches
        .insert(
            conkit_sketch::CatalogPath::new("a.rs").expect("duplicate path"),
            Vec::new(),
        )
        .expect_err("sketch duplicate");

    assert_eq!(
        signatures
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        sketches
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        signature_duplicate.to_string(),
        sketch_duplicate.to_string()
    );
}

#[test]
fn encoded_duplicate_catalog_paths_are_rejected_by_both_domains() {
    let json = r#"{"files":{"src/lib.rs":[1],"src/lib.rs":[2]}}"#;
    let signature_json = serde_json::from_str::<conkit_signature::FileCatalog>(json)
        .expect_err("signature catalog must reject a duplicate encoded JSON path");
    let sketch_json = serde_json::from_str::<conkit_sketch::FileCatalog>(json)
        .expect_err("sketch catalog must reject a duplicate encoded JSON path");

    assert!(
        signature_json
            .to_string()
            .contains("duplicate catalog path: src/lib.rs")
    );
    assert!(
        sketch_json
            .to_string()
            .contains("duplicate catalog path: src/lib.rs")
    );

    let yaml = "files:\n  src/lib.rs: [1]\n  src/lib.rs: [2]\n";
    serde_saphyr::from_str::<conkit_signature::FileCatalog>(yaml)
        .expect_err("signature catalog must reject a duplicate encoded YAML path");
    serde_saphyr::from_str::<conkit_sketch::FileCatalog>(yaml)
        .expect_err("sketch catalog must reject a duplicate encoded YAML path");
}

#[test]
fn catalog_limits_precede_malformed_yaml_in_both_domains() {
    let mut signature_limits = conkit_signature::SignatureLimits::default();
    signature_limits.catalog.per_file_bytes = 0;
    let signature = conkit_signature::SignatureContractKit::builder()
        .with_limits(signature_limits)
        .build()
        .expect("signature kit");
    let signature_path = conkit_signature::CatalogPath::new("main.yml").expect("contract path");
    let mut signature_current = conkit_signature::FileCatalog::new();
    signature_current
        .insert(signature_path.clone(), b": [malformed".to_vec())
        .expect("signature contract");
    let signature_error =
        futures_executor::block_on(signature.diff(conkit_signature::DiffRequest {
            current_contract_files: signature_current,
            previous_contract_files: conkit_signature::FileCatalog::new(),
        }))
        .expect_err("catalog limit must precede signature YAML parsing");
    let signature_limit = signature_error
        .limit_exceeded()
        .expect("typed signature limit");
    assert_eq!(
        signature_limit.resource,
        conkit_signature::LimitResource::CatalogFileBytes
    );
    assert_eq!(signature_limit.file.as_ref(), Some(&signature_path));

    let mut sketch_limits = conkit_sketch::SketchLimits::default();
    sketch_limits.catalog.per_file_bytes = 0;
    let sketch = conkit_sketch::SketchContractKit::builder()
        .with_limits(sketch_limits)
        .build()
        .expect("sketch kit");
    let sketch_path = conkit_sketch::CatalogPath::new("main.yml").expect("contract path");
    let mut sketch_current = conkit_sketch::FileCatalog::new();
    sketch_current
        .insert(sketch_path.clone(), b": [malformed".to_vec())
        .expect("sketch contract");
    let sketch_error = futures_executor::block_on(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: sketch_current,
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }))
    .expect_err("catalog limit must precede sketch YAML parsing");
    let sketch_limit = sketch_error.limit_exceeded().expect("typed sketch limit");
    assert_eq!(
        sketch_limit.resource,
        conkit_sketch::LimitResource::CatalogFileBytes
    );
    assert_eq!(sketch_limit.file.as_ref(), Some(&sketch_path));
}

#[test]
fn one_shared_worker_has_independent_domain_admission_and_queued_cancellation() {
    let mut blocked = BlockedSharedPool::new();
    let signature = conkit_signature::SignatureContractKit::builder()
        .with_work_options(conkit_signature::WorkOptions {
            pool: conkit_signature::WorkerPool::Shared {
                pool: blocked.shared(),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        })
        .build()
        .expect("signature kit");
    let sketch = conkit_sketch::SketchContractKit::builder()
        .with_work_options(conkit_sketch::WorkOptions {
            pool: conkit_sketch::WorkerPool::Shared {
                pool: blocked.shared(),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        })
        .build()
        .expect("sketch kit");

    let mut signature_active = Box::pin(signature.diff(conkit_signature::DiffRequest {
        current_contract_files: conkit_signature::FileCatalog::new(),
        previous_contract_files: conkit_signature::FileCatalog::new(),
    }));
    let mut sketch_active = Box::pin(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }));
    assert!(BlockedSharedPool::poll_once(signature_active.as_mut()).is_pending());
    assert!(BlockedSharedPool::poll_once(sketch_active.as_mut()).is_pending());

    let mut signature_queued = Box::pin(signature.diff(conkit_signature::DiffRequest {
        current_contract_files: conkit_signature::FileCatalog::new(),
        previous_contract_files: conkit_signature::FileCatalog::new(),
    }));
    let mut sketch_queued = Box::pin(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }));
    assert!(BlockedSharedPool::poll_once(signature_queued.as_mut()).is_pending());
    assert!(BlockedSharedPool::poll_once(sketch_queued.as_mut()).is_pending());

    let signature_full =
        futures_executor::block_on(signature.diff(conkit_signature::DiffRequest {
            current_contract_files: conkit_signature::FileCatalog::new(),
            previous_contract_files: conkit_signature::FileCatalog::new(),
        }))
        .expect_err("signature queue must be full");
    let sketch_full = futures_executor::block_on(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }))
    .expect_err("sketch queue must be full");
    assert!(signature_full.is_queue_full());
    assert!(sketch_full.is_queue_full());

    drop(signature_queued);
    drop(sketch_queued);
    let mut signature_replacement = Box::pin(signature.diff(conkit_signature::DiffRequest {
        current_contract_files: conkit_signature::FileCatalog::new(),
        previous_contract_files: conkit_signature::FileCatalog::new(),
    }));
    let mut sketch_replacement = Box::pin(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }));
    assert!(BlockedSharedPool::poll_once(signature_replacement.as_mut()).is_pending());
    assert!(BlockedSharedPool::poll_once(sketch_replacement.as_mut()).is_pending());
    drop(signature_replacement);
    drop(sketch_replacement);

    blocked.release();
    futures_executor::block_on(signature_active).expect("signature active operation");
    futures_executor::block_on(sketch_active).expect("sketch active operation");
}

#[test]
fn dropping_running_futures_cancels_before_following_shared_pool_work() {
    let mut blocked = BlockedSharedPool::new();
    let signature = conkit_signature::SignatureContractKit::builder()
        .with_work_options(conkit_signature::WorkOptions {
            pool: conkit_signature::WorkerPool::Shared {
                pool: blocked.shared(),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        })
        .build()
        .expect("signature kit");
    let sketch = conkit_sketch::SketchContractKit::builder()
        .with_work_options(conkit_sketch::WorkOptions {
            pool: conkit_sketch::WorkerPool::Shared {
                pool: blocked.shared(),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        })
        .build()
        .expect("sketch kit");

    let mut signature_canceled = Box::pin(signature.diff(conkit_signature::DiffRequest {
        current_contract_files: conkit_signature::FileCatalog::new(),
        previous_contract_files: conkit_signature::FileCatalog::new(),
    }));
    let mut sketch_canceled = Box::pin(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }));
    assert!(BlockedSharedPool::poll_once(signature_canceled.as_mut()).is_pending());
    assert!(BlockedSharedPool::poll_once(sketch_canceled.as_mut()).is_pending());
    drop(signature_canceled);
    drop(sketch_canceled);
    blocked.release();

    futures_executor::block_on(signature.diff(conkit_signature::DiffRequest {
        current_contract_files: conkit_signature::FileCatalog::new(),
        previous_contract_files: conkit_signature::FileCatalog::new(),
    }))
    .expect("signature work after cancellation");
    futures_executor::block_on(sketch.diff(conkit_sketch::DiffRequest {
        current_contract_files: conkit_sketch::FileCatalog::new(),
        previous_contract_files: conkit_sketch::FileCatalog::new(),
    }))
    .expect("sketch work after cancellation");
}

#[test]
fn owning_operation_futures_remain_send_and_static_in_both_domains() {
    let signature = Arc::new(
        conkit_signature::SignatureContractKit::builder()
            .build()
            .expect("signature kit"),
    );
    let signature_task = {
        let signature = Arc::clone(&signature);
        async move {
            signature
                .diff(conkit_signature::DiffRequest {
                    current_contract_files: conkit_signature::FileCatalog::new(),
                    previous_contract_files: conkit_signature::FileCatalog::new(),
                })
                .await
        }
    };
    FutureRequirements::require_send_static(signature_task);

    let sketch = Arc::new(
        conkit_sketch::SketchContractKit::builder()
            .build()
            .expect("sketch kit"),
    );
    let sketch_task = {
        let sketch = Arc::clone(&sketch);
        async move {
            sketch
                .diff(conkit_sketch::DiffRequest {
                    current_contract_files: conkit_sketch::FileCatalog::new(),
                    previous_contract_files: conkit_sketch::FileCatalog::new(),
                })
                .await
        }
    };
    FutureRequirements::require_send_static(sketch_task);
}

#[test]
fn nonempty_domain_results_and_diagnostics_are_deterministic_across_worker_counts() {
    let signature_kit = |worker_threads| {
        SignatureContractKit::builder()
            .with_work_options(SignatureWorkOptions {
                pool: SignatureWorkerPool::Dedicated { worker_threads },
                max_in_flight_operations: NonZeroUsize::MIN,
                max_pending_operations: 0,
            })
            .build()
            .expect("signature kit")
    };
    let signature_contract = |return_type: &str| {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles:\n  - lib.rs\nextraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n    - id: example\n      root: lib.rs\n      kind: library\nsignatures:\n  - answer:\n      file: lib.rs\n      signature_type: function\n      name: answer\n      module_path: []\n      visibility: public\n      parameters: []\n      return_type: {return_type}\nsketches: []\n"
        );
        let mut catalog = SignatureFileCatalog::new();
        catalog
            .insert(
                SignatureCatalogPath::new("main.yml").expect("signature contract path"),
                yaml.into_bytes(),
            )
            .expect("signature contract insert");
        catalog
    };

    let signature_one = block_on(signature_kit(NonZeroUsize::MIN).diff(SignatureDiffRequest {
        current_contract_files: signature_contract("u16"),
        previous_contract_files: signature_contract("u8"),
    }))
    .expect("one-worker signature diff");
    let signature_many = block_on(
        signature_kit(NonZeroUsize::new(4).expect("four workers")).diff(SignatureDiffRequest {
            current_contract_files: signature_contract("u16"),
            previous_contract_files: signature_contract("u8"),
        }),
    )
    .expect("four-worker signature diff");

    assert_eq!(signature_one, signature_many);
    assert!(signature_one.changed());
    assert!(!signature_one.entries.is_empty());

    let sketch_kit = |worker_threads| {
        SketchContractKit::builder()
            .with_work_options(SketchWorkOptions {
                pool: SketchWorkerPool::Dedicated { worker_threads },
                max_in_flight_operations: NonZeroUsize::MIN,
                max_pending_operations: 0,
            })
            .build()
            .expect("sketch kit")
    };
    let sketch_request = || {
        let mut source_files = SketchFileCatalog::new();
        source_files
            .insert(
                SketchCatalogPath::new("first.rs").expect("first source path"),
                b"actual first\n".to_vec(),
            )
            .expect("first source insert");
        source_files
            .insert(
                SketchCatalogPath::new("second.rs").expect("second source path"),
                b"actual second\n".to_vec(),
            )
            .expect("second source insert");

        let mut contract_files = SketchFileCatalog::new();
        contract_files
            .insert(
                SketchCatalogPath::new("main.yml").expect("sketch contract path"),
                b"contract_version: 2\nroot: ../src\nfiles:\n  - first.rs\n  - second.rs\nextraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n    - id: example\n      root: first.rs\n      kind: library\nsignatures:\n  - first_signature:\n      file: first.rs\n      signature_type: function\n      name: first\n      module_path: []\n      visibility: public\n      parameters: []\n      sketch: first_sketch\n  - second_signature:\n      file: second.rs\n      signature_type: function\n      name: second\n      module_path: []\n      visibility: public\n      parameters: []\n      sketch: second_sketch\nsketches:\n  - first_sketch:\n      file: first.rs\n      signature: first_signature\n      signature_type: function\n      matching:\n        normalization: exact_lines_v1\n        occurrence: at_least_one\n      code: |\n        expected first\n  - second_sketch:\n      file: second.rs\n      signature: second_signature\n      signature_type: function\n      matching:\n        normalization: exact_lines_v1\n        occurrence: at_least_one\n      code: |\n        expected second\n"
                    .to_vec(),
            )
            .expect("sketch contract insert");

        SketchCheckRequest {
            source_files,
            contract_files,
            report: SketchReportRequest::None,
            mode: SketchCheckMode::Enforce,
        }
    };

    let sketch_one = block_on(sketch_kit(NonZeroUsize::MIN).check(sketch_request()))
        .expect("one-worker sketch check");
    let sketch_many =
        block_on(sketch_kit(NonZeroUsize::new(4).expect("four workers")).check(sketch_request()))
            .expect("four-worker sketch check");

    assert_eq!(sketch_one, sketch_many);
    assert!(!sketch_one.passed);
    assert_eq!(sketch_one.diagnostics.len(), 2);
}
