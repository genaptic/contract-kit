use crate::error::SignatureContractKitError;
use futures_channel::oneshot;
use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Arc;

/// CPU work scheduling options for [`SignatureContractKit`](crate::SignatureContractKit).
///
/// The public API remains async and runtime-neutral while CPU-heavy parsing and
/// rendering work runs on an internal Rayon pool.
///
/// # Examples
///
/// ```
/// use conkit_signature::{SignatureContractKit, WorkOptions, WorkParallelism};
/// use std::num::NonZeroUsize;
///
/// let kit = SignatureContractKit::builder()
///     .with_work_options(WorkOptions {
///         parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkOptions {
    /// Controls how many worker threads the internal pool may use.
    pub parallelism: WorkParallelism,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            parallelism: WorkParallelism::RuntimeDefault,
        }
    }
}

/// Parallelism policy for the internal CPU work pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkParallelism {
    /// Let Rayon choose its default worker count.
    RuntimeDefault,
    /// Use an explicit non-zero worker thread count.
    Fixed(NonZeroUsize),
}

#[derive(Clone)]
pub(crate) struct AsyncWorkPool {
    pool: Arc<ThreadPool>,
}

impl AsyncWorkPool {
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SignatureContractKitError> {
        let mut builder = ThreadPoolBuilder::new();

        if let WorkParallelism::Fixed(count) = options.parallelism {
            builder = builder.num_threads(count.get());
        }

        let pool = builder
            .build()
            .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?;

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

#[cfg(test)]
mod tests {
    use super::{AsyncWorkPool, WorkOptions, WorkParallelism};
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
}
