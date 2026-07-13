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
/// Rayon pool. Each operation asynchronously waits for one root-admission
/// permit, then submits its complete workflow. The permit moves into the Rayon
/// closure and remains held while that job is queued or running. Completion is
/// delivered through a runtime-neutral oneshot channel. A worker panic is
/// forwarded by resuming the panic on the thread polling the future; ordinary
/// failures remain typed operation errors.
///
/// [`WorkParallelism::Fixed`] uses its value as both the worker count and the
/// maximum number of admitted root operations for that kit.
/// [`WorkOptions::default`] selects [`WorkParallelism::RuntimeDefault`], for
/// which both values use the worker count Rayon selected when the pool was
/// built. Nested Rayon work within an admitted operation does not acquire
/// another root permit.
///
/// Dropping a future before admission prevents its job from being submitted.
/// Dropping it after submission but before execution allows the worker to skip
/// the canceled job on a best-effort basis. Once finite CPU work has started,
/// it runs to completion and its result may be discarded. A caller deadline
/// therefore bounds how long the caller waits; it does not preempt CPU work.
/// The admission bound also does not limit how many pending futures and owned
/// catalogs callers may create, so high-concurrency hosts must impose their own
/// request or task limit. Admission and worker scheduling make no FIFO or
/// starvation guarantee. Operations transform owned in-memory catalogs and
/// have no external side effects, so discarding a result cannot leave partial
/// filesystem or network state.
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
    /// Selects the worker count and per-kit root-operation admission capacity.
    pub parallelism: WorkParallelism,
}

impl Default for WorkOptions {
    fn default() -> Self {
        Self {
            parallelism: WorkParallelism::RuntimeDefault,
        }
    }
}

/// Worker-count and root-operation admission policy for the crate-owned CPU pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkParallelism {
    /// Let Rayon choose the worker count and use it as the admission capacity.
    ///
    /// This variant does not select an async runtime.
    RuntimeDefault,
    /// Use one explicit non-zero value for workers and admitted root operations.
    Fixed(NonZeroUsize),
}

#[derive(Clone)]
pub(crate) struct AsyncWorkPool {
    pool: Arc<ThreadPool>,
    admission: Arc<async_lock::Semaphore>,
}

impl AsyncWorkPool {
    pub(crate) fn new(options: WorkOptions) -> Result<Self, SketchContractKitError> {
        let mut builder = ThreadPoolBuilder::new();

        if let WorkParallelism::Fixed(count) = options.parallelism {
            builder = builder.num_threads(count.get());
        }

        let pool = Arc::new(
            builder
                .build()
                .map_err(|source| SketchContractKitError::worker_failed(source.to_string()))?,
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

        Ok(outcome.unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
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
    use super::{AsyncWorkError, AsyncWorkPool, WorkOptions, WorkParallelism};
    use futures_channel::oneshot;
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
                parallelism: WorkParallelism::Fixed(
                    NonZeroUsize::new(worker_count).expect("nonzero worker count"),
                ),
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
    fn fixed_parallelism_builds_pool() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::new(2).expect("nonzero")),
        };

        AsyncWorkPool::new(options).expect("pool should build");
    }

    #[test]
    fn executed_job_returns_value() {
        let pool = AsyncWorkPool::new(WorkOptions::default()).expect("pool");
        let actual = futures_executor::block_on(pool.execute(|| 42)).expect("work result");

        assert_eq!(actual, 42);
    }

    #[test]
    fn job_runs_on_worker_thread() {
        let pool = WorkTest::fixed(1);
        let polling_thread = thread::current().id();

        let worker_thread =
            futures_executor::block_on(pool.execute(|| thread::current().id())).expect("work");

        assert_ne!(worker_thread, polling_thread);
    }

    #[test]
    fn fixed_one_bounds_admission() {
        let pool = WorkTest::fixed(1);
        let (first_gate, first_job) = ExecutionGate::new();
        let mut first = FutureProbe::new(pool.execute(move || {
            first_job.hold();
            1
        }));

        assert!(matches!(first.poll_once(), Poll::Pending));
        first_gate.wait_until_started();
        assert!(
            pool.admission.try_acquire_arc().is_none(),
            "running root work should hold the only admission permit"
        );

        let second_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&second_started);
        let mut second = FutureProbe::new(pool.execute(move || {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));

        assert!(matches!(second.poll_once(), Poll::Pending));
        assert!(!second_started.load(Ordering::SeqCst));

        first_gate.release();
        assert_eq!(first.complete().expect("first work"), 1);
        assert_eq!(second.complete().expect("second work"), 2);
        assert!(second_started.load(Ordering::SeqCst));
        assert!(
            pool.admission.try_acquire_arc().is_some(),
            "completed root work should release the admission permit"
        );
    }

    #[test]
    fn dropping_admission_waiter_prevents_submission() {
        let pool = WorkTest::fixed(1);
        let (running_gate, running_job) = ExecutionGate::new();
        let mut running = FutureProbe::new(pool.execute(move || {
            running_job.hold();
            1
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_gate.wait_until_started();

        let canceled_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&canceled_started);
        let mut canceled = FutureProbe::new(pool.execute(move || {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));
        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);

        running_gate.release();
        assert_eq!(running.complete().expect("running work"), 1);

        let task_pool = pool.clone();
        let reusable = OperationThread::start(async move { task_pool.execute(|| 3).await });
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
        let mut canceled = FutureProbe::new(pool.execute(move || {
            worker_started.store(true, Ordering::SeqCst);
            2
        }));
        assert!(matches!(canceled.poll_once(), Poll::Pending));
        drop(canceled);

        let task_pool = pool.clone();
        let later = OperationThread::start(async move { task_pool.execute(|| 3).await });
        raw_gate.release();

        assert_eq!(later.finish().expect("later operation"), 3);
        assert!(!canceled_started.load(Ordering::SeqCst));
    }

    #[test]
    fn dropping_started_job_retains_permit_until_completion() {
        let pool = WorkTest::fixed(1);
        let (running_gate, running_job) = ExecutionGate::new();
        let (finished_sender, finished) = mpsc::sync_channel(1);
        let mut running = FutureProbe::new(pool.execute(move || {
            running_job.hold();
            finished_sender.send(()).expect("test should await finish");
            1
        }));

        assert!(matches!(running.poll_once(), Poll::Pending));
        running_gate.wait_until_started();
        drop(running);

        let next_started = Arc::new(AtomicBool::new(false));
        let worker_started = Arc::clone(&next_started);
        let mut next = FutureProbe::new(pool.execute(move || {
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
                    .execute(move || {
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
            futures_executor::block_on(pool.execute(|| -> () {
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
            futures_executor::block_on(pool.execute(|| 42)).expect("pool should remain usable"),
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
