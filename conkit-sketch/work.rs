use crate::error::SketchContractKitError;
use futures_channel::oneshot;
use rayon::{ThreadPool, ThreadPoolBuilder};
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// CPU work-pool configuration for [`SketchContractKit`](crate::SketchContractKit).
///
/// Public operations return runtime-neutral futures: `conkit-sketch` does not select
/// or depend on the async executor that polls them. CPU-heavy parsing,
/// matching, semantic diffing, rendering, and generation run on the selected
/// Rayon pool. Worker threads, active root operations, and queued root
/// operations are independent budgets. A request which would exceed active
/// plus pending admission returns a queue-full error immediately.
///
/// An admitted operation waits asynchronously for an active-operation permit,
/// then submits its complete workflow. Both permits move into the Rayon closure
/// and are released before completion becomes observable through the
/// runtime-neutral oneshot channel. Dropping a queued future releases its
/// admission permit. Dropping a submitted future signals cooperative
/// cancellation; running work observes that signal at domain checkpoints.
/// Worker panics resume on the polling thread, while recoverable failures remain
/// typed operation errors. Admission and Rayon scheduling make no FIFO or
/// starvation guarantee.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{SketchContractKit, WorkOptions, WorkerPool};
/// use std::num::NonZeroUsize;
///
/// let kit = SketchContractKit::builder()
///     .with_work_options(WorkOptions {
///         pool: WorkerPool::Dedicated {
///             worker_threads: NonZeroUsize::MIN,
///         },
///         max_in_flight_operations: NonZeroUsize::MIN,
///         max_pending_operations: 0,
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct WorkOptions {
    /// Selects a default, dedicated, or application-shared Rayon pool.
    pub pool: WorkerPool,
    /// Maximum root operations actively executing on the pool.
    pub max_in_flight_operations: NonZeroUsize,
    /// Maximum admitted root operations waiting for active capacity.
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

/// Rayon worker-pool ownership policy.
#[derive(Clone)]
pub enum WorkerPool {
    /// Build a kit-owned pool using Rayon's platform default worker count.
    RuntimeDefault,
    /// Build a kit-owned pool with an explicit worker count.
    Dedicated {
        /// Number of Rayon workers in the dedicated pool.
        worker_threads: NonZeroUsize,
    },
    /// Reuse an application-owned pool, allowing both Contract Kit domains to
    /// share one worker set.
    Shared {
        /// Application-owned Rayon pool.
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
            Self::Shared { pool } => formatter
                .debug_struct("Shared")
                .field("worker_threads", &pool.current_num_threads())
                .finish_non_exhaustive(),
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
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SketchContractKitError> {
        let WorkOptions {
            pool,
            max_in_flight_operations,
            max_pending_operations,
        } = options;
        let pool = match pool {
            WorkerPool::RuntimeDefault => Arc::new(
                ThreadPoolBuilder::new()
                    .build()
                    .map_err(|source| SketchContractKitError::worker_failed(source.to_string()))?,
            ),
            WorkerPool::Dedicated { worker_threads } => Arc::new(
                ThreadPoolBuilder::new()
                    .num_threads(worker_threads.get())
                    .build()
                    .map_err(|source| SketchContractKitError::worker_failed(source.to_string()))?,
            ),
            WorkerPool::Shared { pool } => pool,
        };
        let admitted_capacity = max_in_flight_operations
            .get()
            .checked_add(max_pending_operations)
            .ok_or_else(SketchContractKitError::work_capacity_overflow)?;

        Ok(Self {
            pool,
            admitted: Arc::new(async_lock::Semaphore::new(admitted_capacity)),
            running: Arc::new(async_lock::Semaphore::new(max_in_flight_operations.get())),
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
        let cancellation = CancellationProbe::new();
        let mut cancellation_guard = CancellationGuard::new(cancellation.clone());
        let running = self.running.acquire_arc().await;
        let (sender, receiver) = oneshot::channel();
        let worker_cancellation = cancellation.clone();

        self.pool.spawn(move || {
            if sender.is_canceled() || worker_cancellation.is_cancelled() {
                return;
            }

            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(worker_cancellation)));
            drop(running);
            drop(admitted);
            let _ = sender.send(outcome);
        });

        let outcome = receiver.await.map_err(|_| AsyncWorkError::WorkerDropped)?;
        cancellation_guard.complete();

        Ok(outcome.unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
    }
}

#[derive(Clone)]
pub(crate) struct CancellationProbe {
    cancelled: Arc<AtomicBool>,
}

impl CancellationProbe {
    const CHECKPOINT_INTERVAL: usize = 1_024;

    pub(crate) fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn checkpoint(&self) -> Result<(), SketchContractKitError> {
        if self.is_cancelled() {
            Err(SketchContractKitError::operation_cancelled())
        } else {
            Ok(())
        }
    }

    pub(crate) fn checkpoint_at(&self, index: usize) -> Result<(), SketchContractKitError> {
        if index.is_multiple_of(Self::CHECKPOINT_INTERVAL) {
            self.checkpoint()?;
        }
        Ok(())
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct CancellationGuard {
    probe: CancellationProbe,
    armed: bool,
}

impl CancellationGuard {
    fn new(probe: CancellationProbe) -> Self {
        Self { probe, armed: true }
    }

    fn complete(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.probe.cancel();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AsyncWorkError {
    #[error("work queue is full")]
    QueueFull,
    #[error("background work did not complete")]
    WorkerDropped,
}

impl From<AsyncWorkError> for SketchContractKitError {
    fn from(error: AsyncWorkError) -> Self {
        match error {
            AsyncWorkError::QueueFull => SketchContractKitError::queue_full(),
            AsyncWorkError::WorkerDropped => {
                SketchContractKitError::worker_failed(error.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncWorkError, AsyncWorkPool, WorkOptions, WorkerPool};
    use futures_channel::oneshot;
    use rayon::prelude::*;
    use std::future::Future;
    use std::num::NonZeroUsize;
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;
    use std::time::Duration;

    struct WorkTest;

    impl WorkTest {
        const WAIT: Duration = Duration::from_secs(5);

        fn fixed(worker_count: usize) -> AsyncWorkPool {
            AsyncWorkPool::new(WorkOptions {
                pool: WorkerPool::Dedicated {
                    worker_threads: NonZeroUsize::new(worker_count).expect("nonzero worker count"),
                },
                max_in_flight_operations: NonZeroUsize::new(worker_count)
                    .expect("nonzero active count"),
                max_pending_operations: 8,
            })
            .expect("pool should build")
        }

        fn configured(worker_count: usize, active: usize, pending: usize) -> AsyncWorkPool {
            AsyncWorkPool::new(WorkOptions {
                pool: WorkerPool::Dedicated {
                    worker_threads: NonZeroUsize::new(worker_count).expect("nonzero worker count"),
                },
                max_in_flight_operations: NonZeroUsize::new(active).expect("nonzero active count"),
                max_pending_operations: pending,
            })
            .expect("pool should build")
        }

        fn receive<T>(receiver: &Receiver<T>, context: &str) -> T {
            receiver.recv_timeout(Self::WAIT).expect(context)
        }
    }

    struct FutureProbe<F> {
        future: Pin<Box<F>>,
    }

    struct PollWake {
        ready: SyncSender<()>,
    }

    impl PollWake {
        fn new(ready: SyncSender<()>) -> Self {
            Self { ready }
        }

        fn signal(&self) {
            let _ = self.ready.try_send(());
        }
    }

    impl Wake for PollWake {
        fn wake(self: Arc<Self>) {
            self.signal();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.signal();
        }
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

        fn complete(&mut self) -> F::Output {
            let (ready_sender, ready) = mpsc::sync_channel(1);
            let waker = Waker::from(Arc::new(PollWake::new(ready_sender)));
            let mut context = Context::from_waker(&waker);

            loop {
                match self.future.as_mut().poll(&mut context) {
                    Poll::Ready(output) => return output,
                    Poll::Pending => {
                        WorkTest::receive(&ready, "future should wake for another poll");
                    }
                }
            }
        }
    }

    struct ExecutionGate {
        started: Receiver<()>,
        release: SyncSender<()>,
    }

    impl ExecutionGate {
        fn new() -> (Self, BlockedJob) {
            let (started_sender, started) = mpsc::sync_channel(1);
            let (release, release_receiver) = mpsc::sync_channel(1);

            (
                Self { started, release },
                BlockedJob {
                    started: started_sender,
                    release: release_receiver,
                },
            )
        }

        fn wait_until_started(&self) {
            WorkTest::receive(&self.started, "job should start");
        }

        fn release(&self) {
            self.release.send(()).expect("job should still be waiting");
        }
    }

    struct BlockedJob {
        started: SyncSender<()>,
        release: Receiver<()>,
    }

    impl BlockedJob {
        fn hold(self) {
            self.started.send(()).expect("test should await job start");
            WorkTest::receive(&self.release, "job should be released");
        }
    }

    struct OperationThread<T> {
        result: Receiver<T>,
        handle: thread::JoinHandle<()>,
    }

    impl<T> OperationThread<T>
    where
        T: Send + 'static,
    {
        fn start<F>(future: F) -> Self
        where
            F: Future<Output = T> + Send + 'static,
        {
            let (sender, result) = mpsc::sync_channel(1);
            let handle = thread::spawn(move || {
                let result = futures_executor::block_on(future);
                sender.send(result).expect("test should await task result");
            });

            Self { result, handle }
        }

        fn finish(self) -> T {
            let result = WorkTest::receive(&self.result, "operation should complete");
            self.handle
                .join()
                .expect("operation thread should not panic");
            result
        }
    }

    struct ActiveRootJobs {
        active: AtomicUsize,
        maximum: AtomicUsize,
    }

    impl ActiveRootJobs {
        fn new() -> Self {
            Self {
                active: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
            }
        }

        fn enter(&self) {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
        }

        fn leave(&self) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }

        fn active(&self) -> usize {
            self.active.load(Ordering::SeqCst)
        }

        fn maximum(&self) -> usize {
            self.maximum.load(Ordering::SeqCst)
        }
    }

    struct WorkReceiver<T> {
        receiver: oneshot::Receiver<T>,
    }

    impl<T> WorkReceiver<T> {
        fn new(receiver: oneshot::Receiver<T>) -> Self {
            Self { receiver }
        }

        async fn receive(self) -> Result<T, AsyncWorkError> {
            self.receiver
                .await
                .map_err(|_| AsyncWorkError::WorkerDropped)
        }
    }

    #[test]
    fn default_options_build_pool() {
        AsyncWorkPool::new(WorkOptions::default()).expect("pool should build");
    }

    #[test]
    fn dedicated_pool_builds_independently_from_operation_budgets() {
        let options = WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::new(2).expect("nonzero"),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 3,
        };

        let pool = AsyncWorkPool::new(options).expect("pool should build");
        assert_eq!(pool.pool.current_num_threads(), 2);
        assert!(pool.running.try_acquire_arc().is_some());
    }

    #[test]
    fn executed_job_returns_value() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let actual = futures_executor::block_on(pool.execute(|_| 42)).expect("work result");

        assert_eq!(actual, 42);
    }

    #[test]
    fn job_runs_on_worker_thread() {
        let pool = WorkTest::fixed(1);
        let polling_thread = thread::current().id();

        let worker_thread =
            futures_executor::block_on(pool.execute(|_| thread::current().id())).expect("work");

        assert_ne!(worker_thread, polling_thread);
    }

    #[test]
    fn active_capacity_is_independent_from_pending_capacity() {
        let pool = WorkTest::configured(2, 1, 1);
        let (first_gate, first_job) = ExecutionGate::new();
        let mut first = FutureProbe::new(pool.execute(move |_| {
            first_job.hold();
            1
        }));

        assert!(matches!(first.poll_once(), Poll::Pending));
        first_gate.wait_until_started();
        assert!(
            pool.running.try_acquire_arc().is_none(),
            "running root work should hold the only active permit"
        );

        let second_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&second_started);
        let mut second = FutureProbe::new(pool.execute(move |_| {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));

        assert!(matches!(second.poll_once(), Poll::Pending));
        assert!(!second_started.load(Ordering::SeqCst));
        assert!(
            pool.admitted.try_acquire_arc().is_none(),
            "one active and one pending operation should fill admission"
        );

        first_gate.release();
        assert_eq!(first.complete().expect("first work"), 1);
        assert_eq!(second.complete().expect("second work"), 2);
        assert!(second_started.load(Ordering::SeqCst));
        assert!(
            pool.admitted.try_acquire_arc().is_some(),
            "completed root work should release the admission permit"
        );
    }

    #[test]
    fn queue_full_is_returned_immediately_when_active_and_pending_are_full() {
        let pool = WorkTest::configured(1, 1, 0);
        let (gate, job) = ExecutionGate::new();
        let mut running = FutureProbe::new(pool.execute(move |_| {
            job.hold();
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        gate.wait_until_started();
        let error = futures_executor::block_on(pool.execute(|_| ()))
            .expect_err("a full queue must reject admission");
        assert!(matches!(error, AsyncWorkError::QueueFull));

        gate.release();
        running.complete().expect("running operation");
    }

    #[test]
    fn dropping_admission_waiter_prevents_submission() {
        let pool = WorkTest::fixed(1);
        let (running_gate, running_job) = ExecutionGate::new();
        let mut running = FutureProbe::new(pool.execute(move |_| {
            running_job.hold();
            1
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_gate.wait_until_started();

        let canceled_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&canceled_started);
        let mut canceled = FutureProbe::new(pool.execute(move |_| {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));
        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);

        running_gate.release();
        assert_eq!(running.complete().expect("running work"), 1);

        let task_pool = pool.clone();
        let reusable = OperationThread::start(async move { task_pool.execute(|_| 3).await });
        assert_eq!(reusable.finish().expect("reusable permit"), 3);
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_queued_job_skips_body_and_releases_permit() {
        let pool = WorkTest::fixed(1);
        let (raw_gate, raw_job) = ExecutionGate::new();
        pool.pool.spawn(move || raw_job.hold());
        raw_gate.wait_until_started();

        let canceled_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&canceled_started);
        let mut canceled = FutureProbe::new(pool.execute(move |_| {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));
        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);

        let task_pool = pool.clone();
        let later = OperationThread::start(async move { task_pool.execute(|_| 3).await });
        raw_gate.release();

        assert_eq!(later.finish().expect("later operation"), 3);
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_started_job_retains_permit_until_completion() {
        let pool = WorkTest::fixed(1);
        let (running_gate, running_job) = ExecutionGate::new();
        let (finished_sender, finished) = mpsc::sync_channel(1);
        let mut running = FutureProbe::new(pool.execute(move |_| {
            running_job.hold();
            finished_sender.send(()).expect("test should await finish");
            1
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_gate.wait_until_started();
        drop(running);

        let next_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&next_started);
        let mut next = FutureProbe::new(pool.execute(move |_| {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));
        assert!(matches!(next.poll_once(), Poll::Pending));
        assert!(!next_started.load(Ordering::SeqCst));

        running_gate.release();
        WorkTest::receive(&finished, "started job should finish");
        assert_eq!(next.complete().expect("next operation"), 2);
        assert!(next_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_started_future_signals_cooperative_cancellation() {
        let pool = WorkTest::fixed(1);
        let (started_sender, started) = mpsc::sync_channel(1);
        let (cancelled_sender, cancelled) = mpsc::sync_channel(1);
        let mut operation = FutureProbe::new(pool.execute(move |probe| {
            started_sender
                .send(())
                .expect("test should await operation start");
            while !probe.is_cancelled() {
                thread::yield_now();
            }
            probe
                .checkpoint()
                .expect_err("dropped operation must observe cancellation");
            cancelled_sender
                .send(())
                .expect("test should await cancellation checkpoint");
        }));

        assert!(matches!(operation.poll_once(), Poll::Pending));
        WorkTest::receive(&started, "operation should start");
        drop(operation);
        WorkTest::receive(&cancelled, "worker should observe cancellation");
    }

    #[test]
    fn shared_configuration_reuses_the_application_pool() {
        let shared = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .expect("shared pool"),
        );
        let options = WorkOptions {
            pool: WorkerPool::Shared {
                pool: Arc::clone(&shared),
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        };
        let first = AsyncWorkPool::new(options.clone()).expect("first kit pool");
        let second = AsyncWorkPool::new(options).expect("second kit pool");

        assert!(Arc::ptr_eq(&first.pool, &shared));
        assert!(Arc::ptr_eq(&second.pool, &shared));
        assert_eq!(
            futures_executor::block_on(first.execute(|_| 20)).expect("first work")
                + futures_executor::block_on(second.execute(|_| 22)).expect("second work"),
            42
        );
    }

    #[test]
    fn one_worker_allows_nested_rayon_work_without_deadlock() {
        let pool = WorkTest::configured(1, 1, 0);
        let sum =
            futures_executor::block_on(pool.execute(|_| (0_u64..100).into_par_iter().sum::<u64>()))
                .expect("nested Rayon work");

        assert_eq!(sum, 4_950);
    }

    #[test]
    fn fixed_two_caps_active_root_jobs() {
        let pool = WorkTest::fixed(2);
        let active = Arc::new(ActiveRootJobs::new());
        let (started_sender, started) = mpsc::channel();
        let mut releases = Vec::new();
        let mut workers = Vec::new();

        for operation_id in 0..4 {
            let (release, release_receiver) = mpsc::sync_channel(1);
            releases.push(release);

            let task_pool = pool.clone();
            let task_active = Arc::clone(&active);
            let task_started = started_sender.clone();
            workers.push(OperationThread::start(async move {
                task_pool
                    .execute(move |_| {
                        task_active.enter();
                        task_started
                            .send(operation_id)
                            .expect("test should await started job");
                        WorkTest::receive(&release_receiver, "active job should be released");
                        task_active.leave();
                        operation_id
                    })
                    .await
            }));
        }
        drop(started_sender);

        let first = WorkTest::receive(&started, "first operation should start");
        let second = WorkTest::receive(&started, "second operation should start");
        assert_ne!(first, second);
        assert_eq!(active.active(), 2);
        assert!(matches!(started.try_recv(), Err(TryRecvError::Empty)));

        releases[first].send(()).expect("release first operation");
        releases[second].send(()).expect("release second operation");

        let third = WorkTest::receive(&started, "third operation should start");
        let fourth = WorkTest::receive(&started, "fourth operation should start");
        assert_ne!(third, fourth);
        releases[third].send(()).expect("release third operation");
        releases[fourth].send(()).expect("release fourth operation");

        let mut completed = workers
            .into_iter()
            .map(|worker| worker.finish().expect("operation should succeed"))
            .collect::<Vec<_>>();
        completed.sort_unstable();

        assert_eq!(completed, vec![0, 1, 2, 3]);
        assert_eq!(active.active(), 0);
        assert!(active.maximum() <= 2);
    }

    #[test]
    fn worker_panic_resumes_on_polling_thread_and_pool_remains_usable() {
        let pool = WorkTest::fixed(1);
        let polling_thread = thread::current().id();

        let panic = catch_unwind(AssertUnwindSafe(|| {
            futures_executor::block_on(pool.execute(|_| -> () {
                panic_any(thread::current().id());
            }))
        }))
        .expect_err("worker panic should resume on polling thread");
        let worker_thread = *panic
            .downcast::<thread::ThreadId>()
            .expect("panic payload should contain worker thread id");

        assert_eq!(thread::current().id(), polling_thread);
        assert_ne!(worker_thread, polling_thread);
        assert_eq!(
            futures_executor::block_on(pool.execute(|_| 42)).expect("pool should remain usable"),
            42
        );
    }

    #[test]
    fn dropped_worker_sender_returns_worker_dropped_error() {
        let (sender, receiver) = oneshot::channel::<()>();
        drop(sender);

        let error = futures_executor::block_on(WorkReceiver::new(receiver).receive())
            .expect_err("dropped sender should fail");

        assert!(matches!(error, AsyncWorkError::WorkerDropped));
    }
}
