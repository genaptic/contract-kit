use crate::error::SignatureContractKitError;
use futures_channel::oneshot;
use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::sync::Arc;

/// CPU work scheduling options for [`SignatureContractKit`](crate::SignatureContractKit).
///
/// Public operation futures are executor-neutral, and direct `.await` is the
/// normal integration. A spawned task must own its request and the kit, usually
/// through an [`Arc`](std::sync::Arc) clone in an `async move` block. That
/// owning task future and its output satisfy `Send + 'static`; a direct method
/// future continues to borrow the kit.
///
/// Each kit owns a Rayon pool for its complete CPU workflows. [`Fixed(n)`]
/// configures both `n` worker threads and at most `n` admitted root operations.
/// [`RuntimeDefault`] uses the worker count selected when Rayon builds the pool
/// for both limits. Nested Rayon work belongs to its admitted root operation
/// and does not acquire another root permit.
///
/// Waiting for admission is asynchronous and does not block the caller's
/// executor thread. Dropping a future before admission prevents submission.
/// If it is dropped after submission but before execution, the worker checks
/// for cancellation and normally skips the queued job; that check is best
/// effort. Once finite CPU work starts, it runs to completion and its result is
/// discarded when the caller is gone. A caller deadline or timeout therefore
/// limits how long the caller waits, not how long already-started CPU work
/// executes.
///
/// Admission bounds work owned by this kit, but it does not bound how many
/// pending task futures and owned catalogs a host creates. High-concurrency
/// hosts must apply their own request-level limit. Neither admission nor Rayon
/// scheduling promises FIFO order.
///
/// Completion crosses the worker boundary through a runtime-neutral one-shot
/// channel. A panic from the CPU workflow is captured in the worker and resumes
/// unwinding on the polling thread; recoverable failures remain typed errors.
/// Operations transform owned in-memory catalogs without external side
/// effects, so discarding a completed result cannot leave partial external
/// state.
///
/// [`Fixed(n)`]: WorkParallelism::Fixed
/// [`RuntimeDefault`]: WorkParallelism::RuntimeDefault
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
    /// Controls the worker-thread and admitted-root-operation budgets.
    pub parallelism: WorkParallelism,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            parallelism: WorkParallelism::RuntimeDefault,
        }
    }
}

/// Worker parallelism and root-operation admission policy for the internal CPU
/// work pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkParallelism {
    /// Let Rayon choose the worker count and use that exact count as the
    /// admitted-root-operation limit.
    RuntimeDefault,
    /// Use the non-zero value for both worker threads and admitted root
    /// operations.
    Fixed(NonZeroUsize),
}

#[derive(Clone)]
pub(crate) struct AsyncWorkPool {
    pool: Arc<ThreadPool>,
    admission: Arc<async_lock::Semaphore>,
}

impl AsyncWorkPool {
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SignatureContractKitError> {
        let mut builder = ThreadPoolBuilder::new();

        if let WorkParallelism::Fixed(count) = options.parallelism {
            builder = builder.num_threads(count.get());
        }

        let pool = Arc::new(
            builder
                .build()
                .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?,
        );
        let admission = Arc::new(async_lock::Semaphore::new(pool.current_num_threads()));

        Ok(Self { pool, admission })
    }

    pub(crate) async fn execute<T, F>(&self, job: F) -> Result<T, AsyncWorkError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self.admission.acquire_arc().await;
        let (sender, receiver) = oneshot::channel();
        self.pool.spawn(move || {
            if sender.is_canceled() {
                return;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            // Release admission before making completion observable.
            drop(permit);
            let _ = sender.send(outcome);
        });

        let outcome = receiver.await.map_err(|_| AsyncWorkError::WorkerDropped)?;
        Ok(outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AsyncWorkError {
    #[error("background work did not complete")]
    WorkerDropped,
}

impl From<AsyncWorkError> for SignatureContractKitError {
    fn from(error: AsyncWorkError) -> Self {
        Self::worker_failed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncWorkError, AsyncWorkPool, WorkOptions, WorkParallelism};
    use futures_channel::oneshot;
    use std::future::Future;
    use std::num::NonZeroUsize;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;
    use std::time::Duration;

    struct FutureProbe<F> {
        future: Pin<Box<F>>,
    }

    impl<F> FutureProbe<F>
    where
        F: Future,
    {
        fn new(future: F) -> Self {
            Self {
                future: Box::pin(future),
            }
        }

        fn poll_once(&mut self) -> Poll<F::Output> {
            let mut context = Context::from_waker(Waker::noop());
            self.future.as_mut().poll(&mut context)
        }

        fn finish_bounded(mut self) -> F::Output {
            let (wake_tx, wake_rx) = mpsc::sync_channel(1);
            let waker = Waker::from(Arc::new(CompletionWake::new(wake_tx)));
            let mut context = Context::from_waker(&waker);

            loop {
                match self.future.as_mut().poll(&mut context) {
                    Poll::Ready(output) => return output,
                    Poll::Pending => {
                        wake_rx
                            .recv_timeout(Duration::from_secs(5))
                            .expect("future should wake before the completion guard expires");
                    }
                }
            }
        }
    }

    struct CompletionWake {
        wake_tx: mpsc::SyncSender<()>,
    }

    impl CompletionWake {
        fn new(wake_tx: mpsc::SyncSender<()>) -> Self {
            Self { wake_tx }
        }

        fn notify(&self) {
            let _ = self.wake_tx.try_send(());
        }
    }

    impl Wake for CompletionWake {
        fn wake(self: Arc<Self>) {
            self.notify();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.notify();
        }
    }

    struct OperationThread<T> {
        result_rx: mpsc::Receiver<T>,
        handle: thread::JoinHandle<()>,
    }

    impl<T> OperationThread<T>
    where
        T: Send + 'static,
    {
        fn spawn<F>(operation: F) -> Self
        where
            F: FnOnce() -> T + Send + 'static,
        {
            let (result_tx, result_rx) = mpsc::sync_channel(1);
            let handle = thread::spawn(move || {
                let result = operation();
                result_tx.send(result).expect("send operation result");
            });

            Self { result_rx, handle }
        }

        fn finish_bounded(self) -> T {
            let result = self
                .result_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("operation should finish before the completion guard expires");
            self.handle
                .join()
                .expect("operation thread should not panic");
            result
        }
    }

    struct ReceiverProbe<T> {
        receiver: oneshot::Receiver<T>,
    }

    impl<T> ReceiverProbe<T> {
        fn new(receiver: oneshot::Receiver<T>) -> Self {
            Self { receiver }
        }

        async fn receive(self) -> Result<T, AsyncWorkError> {
            self.receiver
                .await
                .map_err(|_| AsyncWorkError::WorkerDropped)
        }
    }

    #[derive(Default)]
    struct ActiveJobs {
        current: AtomicUsize,
        maximum: AtomicUsize,
    }

    impl ActiveJobs {
        fn enter(self: &Arc<Self>) -> ActiveJob {
            let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(current, Ordering::SeqCst);
            ActiveJob {
                jobs: Arc::clone(self),
            }
        }

        fn current(&self) -> usize {
            self.current.load(Ordering::SeqCst)
        }

        fn maximum(&self) -> usize {
            self.maximum.load(Ordering::SeqCst)
        }
    }

    struct ActiveJob {
        jobs: Arc<ActiveJobs>,
    }

    impl Drop for ActiveJob {
        fn drop(&mut self) {
            self.jobs.current.fetch_sub(1, Ordering::SeqCst);
        }
    }

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
    fn execute_returns_value() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let actual = FutureProbe::new(pool.execute(|| 42))
            .finish_bounded()
            .expect("work result");

        assert_eq!(actual, 42);
    }

    #[test]
    fn execute_runs_on_a_worker_thread() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let polling_thread = thread::current().id();

        let worker_thread = FutureProbe::new(pool.execute(|| thread::current().id()))
            .finish_bounded()
            .expect("work result");

        assert_ne!(worker_thread, polling_thread);
    }

    #[test]
    fn fixed_one_bounds_admission_until_running_work_finishes() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::sync_channel(0);
        let first_pool = pool.clone();
        let first_driver = OperationThread::spawn(move || {
            FutureProbe::new(first_pool.execute(move || {
                first_started_tx.send(()).expect("signal first start");
                first_release_rx.recv().expect("release first job");
                1
            }))
            .finish_bounded()
        });
        first_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first job should start");
        assert!(
            pool.admission.try_acquire_arc().is_none(),
            "running root work should hold the only admission permit"
        );

        let second_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&second_started);
        let mut second = FutureProbe::new(pool.execute(move || {
            task_started.store(true, Ordering::SeqCst);
            2
        }));

        assert!(matches!(second.poll_once(), Poll::Pending));
        assert!(!second_started.load(Ordering::SeqCst));

        first_release_tx.send(()).expect("release first job");
        assert_eq!(first_driver.finish_bounded().expect("first work result"), 1);
        assert_eq!(second.finish_bounded().expect("second work result"), 2);
        assert!(second_started.load(Ordering::SeqCst));
        assert!(
            pool.admission.try_acquire_arc().is_some(),
            "completed root work should release the admission permit"
        );
    }

    #[test]
    fn dropping_an_admission_waiter_prevents_submission() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::sync_channel(0);
        let first_pool = pool.clone();
        let first_driver = OperationThread::spawn(move || {
            FutureProbe::new(first_pool.execute(move || {
                first_started_tx.send(()).expect("signal first start");
                first_release_rx.recv().expect("release first job");
            }))
            .finish_bounded()
        });
        first_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first job should start");

        let canceled_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&canceled_started);
        let mut canceled = FutureProbe::new(pool.execute(move || {
            task_started.store(true, Ordering::SeqCst);
        }));

        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);
        first_release_tx.send(()).expect("release first job");
        first_driver.finish_bounded().expect("first work result");

        assert_eq!(
            FutureProbe::new(pool.execute(|| 7))
                .finish_bounded()
                .expect("later work result"),
            7
        );
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_queued_work_skips_its_body_and_releases_admission() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (blocker_release_tx, blocker_release_rx) = mpsc::sync_channel(0);
        pool.pool.spawn(move || {
            blocker_started_tx.send(()).expect("signal blocker start");
            blocker_release_rx.recv().expect("release raw blocker");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("raw blocker should start");

        let canceled_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&canceled_started);
        let mut canceled = FutureProbe::new(pool.execute(move || {
            task_started.store(true, Ordering::SeqCst);
        }));

        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);
        blocker_release_tx.send(()).expect("release raw blocker");

        assert_eq!(
            FutureProbe::new(pool.execute(|| 11))
                .finish_bounded()
                .expect("later work result"),
            11
        );
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_running_work_holds_admission_until_the_job_finishes() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (running_started_tx, running_started_rx) = mpsc::channel();
        let (running_release_tx, running_release_rx) = mpsc::sync_channel(0);
        let (running_finished_tx, running_finished_rx) = mpsc::channel();
        let mut running = FutureProbe::new(pool.execute(move || {
            running_started_tx.send(()).expect("signal running start");
            running_release_rx.recv().expect("release running job");
            running_finished_tx.send(()).expect("signal running finish");
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("running job should start");
        drop(running);

        let next_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&next_started);
        let mut next = FutureProbe::new(pool.execute(move || {
            task_started.store(true, Ordering::SeqCst);
            13
        }));
        assert!(matches!(next.poll_once(), Poll::Pending));
        assert!(!next_started.load(Ordering::SeqCst));

        running_release_tx.send(()).expect("release running job");
        running_finished_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("running job should finish");

        assert_eq!(next.finish_bounded().expect("next work result"), 13);
        assert!(next_started.load(Ordering::SeqCst));
    }

    #[test]
    fn fixed_two_limits_active_root_jobs() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::new(2).expect("nonzero")),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let jobs = Arc::new(ActiveJobs::default());
        let (started_tx, started_rx) = mpsc::channel();
        let mut releases = Vec::new();
        let mut drivers = Vec::new();

        for index in 0..4 {
            let (release_tx, release_rx) = mpsc::sync_channel(0);
            releases.push(Some(release_tx));
            let task_pool = pool.clone();
            let task_jobs = Arc::clone(&jobs);
            let task_started_tx = started_tx.clone();
            drivers.push(OperationThread::spawn(move || {
                FutureProbe::new(task_pool.execute(move || {
                    let _active = task_jobs.enter();
                    task_started_tx.send(index).expect("signal job start");
                    release_rx.recv().expect("release active job");
                    index
                }))
                .finish_bounded()
            }));
        }
        drop(started_tx);

        let first = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first job should start");
        let second = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second job should start");
        assert_eq!(jobs.maximum(), 2);

        for index in [first, second] {
            releases[index]
                .take()
                .expect("first-wave release")
                .send(())
                .expect("release first-wave job");
        }

        let third = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("third job should start");
        let fourth = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("fourth job should start");

        for index in [third, fourth] {
            releases[index]
                .take()
                .expect("second-wave release")
                .send(())
                .expect("release second-wave job");
        }

        for (index, driver) in drivers.into_iter().enumerate() {
            assert_eq!(driver.finish_bounded().expect("work result"), index);
        }

        assert_eq!(jobs.current(), 0);
        assert_eq!(jobs.maximum(), 2);
    }

    #[test]
    fn worker_panics_resume_on_the_polling_thread_and_leave_the_pool_usable() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let polling_thread = thread::current().id();
        let (worker_thread_tx, worker_thread_rx) = mpsc::channel();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            FutureProbe::new(pool.execute(move || {
                worker_thread_tx
                    .send(thread::current().id())
                    .expect("record worker thread");
                panic!("worker panic");
            }))
            .finish_bounded()
        }));

        assert!(panic.is_err());
        assert_ne!(
            worker_thread_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker thread should be recorded"),
            polling_thread
        );
        assert_eq!(
            FutureProbe::new(pool.execute(|| 17))
                .finish_bounded()
                .expect("pool remains usable"),
            17
        );
    }

    #[test]
    fn dropped_sender_maps_to_worker_dropped() {
        let (sender, receiver) = oneshot::channel::<()>();
        drop(sender);

        let error = FutureProbe::new(ReceiverProbe::new(receiver).receive())
            .finish_bounded()
            .expect_err("dropped sender should fail");

        assert!(matches!(error, AsyncWorkError::WorkerDropped));
    }
}
