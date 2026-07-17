use crate::error::SignatureContractKitError;
use futures_channel::oneshot;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// CPU work scheduling options for [`SignatureContractKit`](crate::SignatureContractKit).
///
/// Public operation futures are executor-neutral, and direct `.await` is the
/// normal integration. A spawned task must own its request and the kit, usually
/// through an [`Arc`](std::sync::Arc) clone in an `async move` block. That
/// owning task future and its output satisfy `Send + 'static`; a direct method
/// future continues to borrow the kit.
///
/// Worker threads, active root operations, and pending admitted operations are
/// independent budgets. [`WorkerPool::RuntimeDefault`] and
/// [`WorkerPool::Dedicated`] create a kit-local Rayon pool, while
/// [`WorkerPool::Shared`] lets a host reuse one pool across contract domains.
/// One root operation may use nested Rayon work without consuming another root
/// permit.
///
/// Admission is immediate: when all active and pending slots are occupied, the
/// operation returns a queue-full error instead of growing an unbounded waiter
/// list. An admitted operation asynchronously waits for an active slot without
/// blocking its executor. Dropping a queued future releases admission. Dropping
/// a running future marks its cooperative cancellation probe; finite parsing
/// and rendering code observes that flag at group boundaries. Cancellation is
/// cooperative and never terminates a worker thread.
///
/// Completion crosses the worker boundary through a runtime-neutral one-shot
/// channel. A panic from the CPU workflow is captured in the worker and resumes
/// unwinding on the polling thread; recoverable failures remain typed errors.
/// Operations transform owned in-memory catalogs without external side
/// effects, so discarding a completed result cannot leave partial external
/// state.
///
/// # Examples
///
/// ```
/// use conkit_signature::{SignatureContractKit, WorkOptions, WorkerPool};
/// use std::num::NonZeroUsize;
///
/// let kit = SignatureContractKit::builder()
///     .with_work_options(WorkOptions {
///         pool: WorkerPool::Dedicated {
///             worker_threads: NonZeroUsize::MIN,
///         },
///         max_in_flight_operations: NonZeroUsize::MIN,
///         max_pending_operations: 8,
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct WorkOptions {
    /// Selects a runtime-default, dedicated, or host-shared Rayon pool.
    pub pool: WorkerPool,
    /// Maximum root operations admitted to active execution.
    pub max_in_flight_operations: NonZeroUsize,
    /// Maximum additional admitted operations waiting for an active slot.
    pub max_pending_operations: usize,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            pool: WorkerPool::RuntimeDefault,
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        }
    }
}

/// Rayon worker-pool ownership for CPU-bound contract operations.
#[derive(Clone)]
pub enum WorkerPool {
    /// Build a kit-local pool using Rayon's platform-aware default thread count.
    RuntimeDefault,
    /// Build a kit-local pool with the exact non-zero worker-thread count.
    Dedicated {
        /// Number of Rayon workers in the dedicated pool.
        worker_threads: NonZeroUsize,
    },
    /// Reuse a host-owned Rayon pool, allowing both contract domains to share
    /// one worker set.
    Shared {
        /// Host-owned pool used for every complete root operation.
        pool: Arc<ThreadPool>,
    },
}

impl fmt::Debug for WorkerPool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeDefault => formatter.write_str("RuntimeDefault"),
            Self::Dedicated { worker_threads } => formatter
                .debug_struct("Dedicated")
                .field("worker_threads", worker_threads)
                .finish(),
            Self::Shared { .. } => formatter.debug_struct("Shared").finish_non_exhaustive(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AsyncWorkPool {
    pool: Arc<ThreadPool>,
    admitted: Arc<async_lock::Semaphore>,
    running: Arc<async_lock::Semaphore>,
}

impl AsyncWorkPool {
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SignatureContractKitError> {
        let admitted_capacity = options
            .max_in_flight_operations
            .get()
            .checked_add(options.max_pending_operations)
            .ok_or_else(|| {
                SignatureContractKitError::invalid_work_options(
                    "active and pending operation capacities overflow usize",
                )
            })?;
        let pool = match options.pool {
            WorkerPool::RuntimeDefault => {
                Arc::new(ThreadPoolBuilder::new().build().map_err(|source| {
                    SignatureContractKitError::worker_failed(source.to_string())
                })?)
            }
            WorkerPool::Dedicated { worker_threads } => Arc::new(
                ThreadPoolBuilder::new()
                    .num_threads(worker_threads.get())
                    .build()
                    .map_err(|source| {
                        SignatureContractKitError::worker_failed(source.to_string())
                    })?,
            ),
            WorkerPool::Shared { pool } => pool,
        };

        Ok(Self {
            pool,
            admitted: Arc::new(async_lock::Semaphore::new(admitted_capacity)),
            running: Arc::new(async_lock::Semaphore::new(
                options.max_in_flight_operations.get(),
            )),
        })
    }

    pub(crate) async fn execute<T, F>(&self, job: F) -> Result<T, AsyncWorkError>
    where
        T: Send + 'static,
        F: FnOnce(CancellationProbe) -> T + Send + 'static,
    {
        let admitted = self
            .admitted
            .try_acquire_arc()
            .ok_or(AsyncWorkError::QueueFull)?;
        let running = self.running.acquire_arc().await;
        let (sender, receiver) = oneshot::channel();
        let mut cancellation = WorkCancellation::new();
        let probe = cancellation.probe();
        self.pool.spawn(move || {
            if sender.is_canceled() || probe.is_canceled() {
                drop(running);
                drop(admitted);
                return;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(probe)));
            // Release both budgets before making completion observable.
            drop(running);
            drop(admitted);
            let _ = sender.send(outcome);
        });

        let outcome = receiver.await.map_err(|_| AsyncWorkError::WorkerDropped)?;
        cancellation.complete();
        Ok(outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    }
}

/// Cooperative cancellation view passed through one complete root operation.
#[derive(Clone, Debug)]
pub(crate) struct CancellationProbe {
    canceled: Arc<AtomicBool>,
}

impl CancellationProbe {
    pub(crate) fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn checkpoint(&self) -> Result<(), SignatureContractKitError> {
        if self.is_canceled() {
            Err(SignatureContractKitError::operation_canceled())
        } else {
            Ok(())
        }
    }

    pub(crate) fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }

    fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

struct WorkCancellation {
    probe: CancellationProbe,
    complete: bool,
}

impl WorkCancellation {
    fn new() -> Self {
        Self {
            probe: CancellationProbe::new(),
            complete: false,
        }
    }

    fn probe(&self) -> CancellationProbe {
        self.probe.clone()
    }

    fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for WorkCancellation {
    fn drop(&mut self) {
        if !self.complete {
            self.probe.cancel();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AsyncWorkError {
    #[error("background work queue is full")]
    QueueFull,
    #[error("background work did not complete")]
    WorkerDropped,
}

impl From<AsyncWorkError> for SignatureContractKitError {
    fn from(error: AsyncWorkError) -> Self {
        match error {
            AsyncWorkError::QueueFull => Self::queue_full(),
            AsyncWorkError::WorkerDropped => Self::worker_failed(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncWorkError, AsyncWorkPool, CancellationProbe, WorkOptions, WorkerPool};
    use futures_channel::oneshot;
    use rayon::ThreadPoolBuilder;
    use rayon::prelude::*;
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
        let options = WorkOptions::default();
        assert!(matches!(options.pool, WorkerPool::RuntimeDefault));
        assert_eq!(options.max_in_flight_operations, NonZeroUsize::MIN);
        assert_eq!(options.max_pending_operations, 0);
        AsyncWorkPool::new(options).expect("pool should build");
    }

    #[test]
    fn canceled_probe_rejects_the_next_cooperative_checkpoint() {
        let probe = CancellationProbe::new();
        probe.cancel();

        assert!(probe.is_canceled());
        assert!(probe.checkpoint().is_err());
    }

    #[test]
    fn dedicated_and_shared_worker_pools_build() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::new(2).expect("nonzero"),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        };
        AsyncWorkPool::new(options).expect("pool should build");

        let shared = Arc::new(
            ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("shared pool"),
        );
        let pool = AsyncWorkPool::new(WorkOptions {
            pool: WorkerPool::Shared {
                pool: Arc::clone(&shared),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        })
        .expect("pool should build");
        assert!(Arc::ptr_eq(&pool.pool, &shared));
    }

    #[test]
    fn execute_returns_value() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let actual = FutureProbe::new(pool.execute(|_| 42))
            .finish_bounded()
            .expect("work result");

        assert_eq!(actual, 42);
    }

    #[test]
    fn execute_runs_on_a_worker_thread() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let polling_thread = thread::current().id();

        let worker_thread = FutureProbe::new(pool.execute(|_| thread::current().id()))
            .finish_bounded()
            .expect("work result");

        assert_ne!(worker_thread, polling_thread);
    }

    #[test]
    fn worker_count_active_roots_and_pending_roots_are_independent() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::new(2).expect("nonzero"),
            max_pending_operations: 3,
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        assert_eq!(pool.pool.current_num_threads(), 1);
        let running_one = pool.running.try_acquire_arc().expect("running permit one");
        let running_two = pool.running.try_acquire_arc().expect("running permit two");
        assert!(pool.running.try_acquire_arc().is_none());
        drop((running_one, running_two));

        let admitted = (0..5)
            .map(|_| pool.admitted.try_acquire_arc().expect("admitted permit"))
            .collect::<Vec<_>>();
        assert!(pool.admitted.try_acquire_arc().is_none());
        drop(admitted);
    }

    #[test]
    fn queue_saturation_returns_immediately_and_queued_drop_releases_admission() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::sync_channel(0);
        let first_pool = pool.clone();
        let first_driver = OperationThread::spawn(move || {
            FutureProbe::new(first_pool.execute(move |_| {
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
            pool.running.try_acquire_arc().is_none(),
            "running root work should hold the only active permit"
        );

        let second_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&second_started);
        let mut second = FutureProbe::new(pool.execute(move |_| {
            task_started.store(true, Ordering::SeqCst);
            2
        }));

        assert!(matches!(second.poll_once(), Poll::Pending));
        assert!(!second_started.load(Ordering::SeqCst));

        let saturated = FutureProbe::new(pool.execute(|_| 3)).finish_bounded();
        assert!(matches!(saturated, Err(AsyncWorkError::QueueFull)));

        drop(second);
        let replacement_started = Arc::new(AtomicBool::new(false));
        let task_started = Arc::clone(&replacement_started);
        let mut replacement = FutureProbe::new(pool.execute(move |_| {
            task_started.store(true, Ordering::SeqCst);
            4
        }));
        assert!(matches!(replacement.poll_once(), Poll::Pending));

        first_release_tx.send(()).expect("release first job");
        assert_eq!(first_driver.finish_bounded().expect("first work result"), 1);
        assert_eq!(
            replacement
                .finish_bounded()
                .expect("replacement work result"),
            4
        );
        assert!(replacement_started.load(Ordering::SeqCst));
        assert!(!second_started.load(Ordering::SeqCst));
        assert!(
            pool.admitted.try_acquire_arc().is_some(),
            "completed root work should release admission"
        );
    }

    #[test]
    fn dropping_an_admission_waiter_prevents_submission() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (first_release_tx, first_release_rx) = mpsc::sync_channel(0);
        let first_pool = pool.clone();
        let first_driver = OperationThread::spawn(move || {
            FutureProbe::new(first_pool.execute(move |_| {
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
        let mut canceled = FutureProbe::new(pool.execute(move |_| {
            task_started.store(true, Ordering::SeqCst);
        }));

        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);
        first_release_tx.send(()).expect("release first job");
        first_driver.finish_bounded().expect("first work result");

        assert_eq!(
            FutureProbe::new(pool.execute(|_| 7))
                .finish_bounded()
                .expect("later work result"),
            7
        );
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_queued_work_skips_its_body_and_releases_admission() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 1,
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
        let mut canceled = FutureProbe::new(pool.execute(move |_| {
            task_started.store(true, Ordering::SeqCst);
        }));

        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);
        let later_pool = pool.clone();
        let later_driver = OperationThread::spawn(move || {
            FutureProbe::new(later_pool.execute(|_| 11)).finish_bounded()
        });
        blocker_release_tx.send(()).expect("release raw blocker");

        assert_eq!(
            later_driver.finish_bounded().expect("later work result"),
            11
        );
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_running_work_sets_the_cooperative_cancellation_probe() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let (running_started_tx, running_started_rx) = mpsc::channel();
        let (running_release_tx, running_release_rx) = mpsc::sync_channel(0);
        let (cancellation_tx, cancellation_rx) = mpsc::channel();
        let mut running = FutureProbe::new(pool.execute(move |probe| {
            running_started_tx.send(()).expect("signal running start");
            running_release_rx.recv().expect("release running job");
            cancellation_tx
                .send(probe.checkpoint().expect_err("future drop must cancel"))
                .expect("signal observed cancellation");
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("running job should start");
        drop(running);
        running_release_tx.send(()).expect("release running job");
        let cancellation = cancellation_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("running job should observe cancellation");
        assert!(cancellation.to_string().contains("canceled"));
    }

    #[test]
    fn max_in_flight_limits_active_root_jobs() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::new(4).expect("nonzero"),
            },
            max_in_flight_operations: NonZeroUsize::new(2).expect("nonzero"),
            max_pending_operations: 2,
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
                FutureProbe::new(task_pool.execute(move |_| {
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
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        };
        let pool = AsyncWorkPool::new(options).expect("pool");
        let polling_thread = thread::current().id();
        let (worker_thread_tx, worker_thread_rx) = mpsc::channel();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            FutureProbe::new(pool.execute(move |_| {
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
            FutureProbe::new(pool.execute(|_| 17))
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

    #[test]
    fn completion_is_observable_only_after_both_permits_are_released() {
        let pool = AsyncWorkPool::new(WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        })
        .expect("pool");

        assert_eq!(
            FutureProbe::new(pool.execute(|_| 19))
                .finish_bounded()
                .expect("work result"),
            19
        );
        assert!(pool.running.try_acquire_arc().is_some());
        assert!(pool.admitted.try_acquire_arc().is_some());
    }

    #[test]
    fn one_worker_executes_nested_rayon_work_without_deadlock() {
        let shared = Arc::new(
            ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("shared pool"),
        );
        let pool = AsyncWorkPool::new(WorkOptions {
            pool: WorkerPool::Shared {
                pool: Arc::clone(&shared),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        })
        .expect("pool");

        let result = FutureProbe::new(
            pool.execute(move |_| shared.install(|| (0_u64..1_000).into_par_iter().sum::<u64>())),
        )
        .finish_bounded()
        .expect("nested Rayon work");

        assert_eq!(result, 499_500);
    }

    #[test]
    fn a_fresh_probe_is_not_canceled() {
        CancellationProbe::new().checkpoint().expect("fresh probe");
    }
}
