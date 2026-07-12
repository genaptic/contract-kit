use crate::error::SketchContractKitError;
use futures_channel::oneshot;
use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Arc;

/// CPU work-pool configuration for [`SketchContractKit`](crate::SketchContractKit).
///
/// Public operations return runtime-neutral futures: `conkit-sketch` does not select
/// or depend on the async executor that polls them. CPU-heavy parsing,
/// matching, semantic diffing, rendering, and generation run on a crate-owned
/// Rayon pool.
/// [`WorkOptions::default`] selects [`WorkParallelism::RuntimeDefault`], which
/// leaves the worker count to Rayon.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{SketchContractKit, WorkOptions, WorkParallelism};
/// use std::num::NonZeroUsize;
///
/// let kit = SketchContractKit::builder()
///     .with_work_options(WorkOptions {
///         parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkOptions {
    /// Selects the worker count for the crate-owned Rayon pool.
    pub parallelism: WorkParallelism,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            parallelism: WorkParallelism::RuntimeDefault,
        }
    }
}

/// Worker-count policy for the crate-owned CPU work pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkParallelism {
    /// Let Rayon choose its default worker count.
    ///
    /// This variant does not select an async runtime.
    RuntimeDefault,
    /// Use an explicit non-zero worker thread count.
    Fixed(NonZeroUsize),
}

#[derive(Clone)]
pub(crate) struct AsyncWorkPool {
    pool: Arc<ThreadPool>,
}

impl AsyncWorkPool {
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SketchContractKitError> {
        let mut builder = ThreadPoolBuilder::new();

        if let WorkParallelism::Fixed(count) = options.parallelism {
            builder = builder.num_threads(count.get());
        }

        let pool = builder
            .build()
            .map_err(|source| SketchContractKitError::worker_failed(source.to_string()))?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub(crate) fn submit<T, F>(&self, job: F) -> WorkHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.pool.spawn(move || {
            let _ = sender.send(job());
        });
        WorkHandle::new(receiver)
    }
}

pub(crate) struct WorkHandle<T> {
    receiver: oneshot::Receiver<T>,
}

impl<T> WorkHandle<T> {
    fn new(receiver: oneshot::Receiver<T>) -> Self {
        Self { receiver }
    }

    pub(crate) async fn into_result(self) -> Result<T, AsyncWorkError> {
        self.receiver
            .await
            .map_err(|_| AsyncWorkError::WorkerDropped)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AsyncWorkError {
    #[error("background work did not complete")]
    WorkerDropped,
}

impl From<AsyncWorkError> for SketchContractKitError {
    fn from(error: AsyncWorkError) -> Self {
        SketchContractKitError::worker_failed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncWorkError, AsyncWorkPool, WorkHandle, WorkOptions, WorkParallelism};
    use futures_channel::oneshot;
    use std::num::NonZeroUsize;

    #[test]
    fn default_options_build_pool() {
        AsyncWorkPool::new(WorkOptions::default()).expect("pool should build");
    }

    #[test]
    fn fixed_parallelism_builds_pool() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::new(2).expect("nonzero")),
        };

        AsyncWorkPool::new(options).expect("pool should build");
    }

    #[test]
    fn submitted_job_returns_value() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let handle = pool.submit(|| 42);

        let actual = futures_executor::block_on(handle.into_result()).expect("work result");

        assert_eq!(actual, 42);
    }

    #[test]
    fn dropped_worker_sender_returns_worker_dropped_error() {
        let (sender, receiver) = oneshot::channel::<()>();
        drop(sender);

        let error = futures_executor::block_on(WorkHandle::new(receiver).into_result())
            .expect_err("dropped sender should fail");

        assert!(matches!(error, AsyncWorkError::WorkerDropped));
    }
}
