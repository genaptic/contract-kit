//! Runtime dependencies shared by command handlers.
//!
//! `CommandContext` is created after clap parsing and before command execution.
//! It owns the signature service, the concrete sketch adapter, the bounded
//! Cargo/rustdoc extractor, CLI filesystem limits, process cancellation, and
//! terminal output. Initialization builds one application-owned Rayon pool for
//! both domains while preserving their independent active and pending root-
//! operation admission.

use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use anyhow::Result;

use crate::catalog::CatalogReadLimits;
use crate::compiler::{CompilerCancellation, CompilerExtractor};
use crate::contracts::SketchAdapter;
use crate::error::CliError;
use crate::output::Output;

/// Process-wide cancellation shared by signal handling and command execution.
///
/// The operating-system handler owns only a clone of this small synchronization
/// object. It performs no I/O or domain work and releases its short-lived
/// registration mutex before invoking the executor waker. Dropping a pending
/// domain future remains the mechanism that activates each domain's existing
/// cooperative cancellation.
#[derive(Clone, Debug)]
pub(crate) struct ApplicationCancellation {
    requested: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<Waker>>>,
}

/// Removes the executor waker when a raced command completes or is dropped.
struct CancellationRegistration<'cancellation> {
    cancellation: &'cancellation ApplicationCancellation,
}

impl ApplicationCancellation {
    /// Creates an unregistered cancellation source for direct application tests.
    pub(crate) fn new() -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(false)),
            waker: Arc::new(Mutex::new(None)),
        }
    }

    /// Installs the one process-level Ctrl-C/termination handler.
    ///
    /// # Errors
    ///
    /// Returns an error if the host process handler cannot be registered.
    fn install() -> Result<Self, CliError> {
        let cancellation = Self::new();
        let handler = cancellation.clone();
        ctrlc::try_set_handler(move || handler.request())
            .map_err(|source| CliError::SignalHandler { source })?;
        Ok(cancellation)
    }

    /// Returns a source connected to the host process termination signals.
    ///
    /// # Errors
    ///
    /// Returns an error if the one process-level signal handler cannot be
    /// installed.
    pub(crate) fn process() -> Result<Self, CliError> {
        Self::install()
    }

    /// Returns the shared flag observed by synchronous compiler extraction.
    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.requested)
    }

    /// Requests cancellation and wakes a command blocked in the async executor.
    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::Release);
        // Extract the waker before invoking it. In Rust 2024 an `if let`
        // scrutinee temporary may live through the body, and a re-entrant
        // waker must be able to acquire this mutex while polling the race.
        let waker = self.lock_waker().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Stops a synchronous command boundary before it publishes output.
    ///
    /// # Errors
    ///
    /// Returns an operation-canceled error after process cancellation has been
    /// requested.
    pub(crate) fn checkpoint(&self) -> Result<(), CliError> {
        if self.requested.load(Ordering::Acquire) {
            Err(CliError::OperationCanceled)
        } else {
            Ok(())
        }
    }

    /// Races a command future against process cancellation.
    ///
    /// # Errors
    ///
    /// Returns an operation-canceled error when cancellation wins before the
    /// supplied future completes. A completed future result wins its final poll.
    pub(crate) async fn race<F>(&self, future: F) -> Result<F::Output, CliError>
    where
        F: Future,
    {
        let _registration = CancellationRegistration { cancellation: self };
        let mut future = std::pin::pin!(future);

        std::future::poll_fn(|context| {
            if self.poll_requested(context).is_ready() {
                return Poll::Ready(Err(CliError::OperationCanceled));
            }

            match future.as_mut().poll(context) {
                Poll::Ready(outcome) => Poll::Ready(Ok(outcome)),
                Poll::Pending if self.requested.load(Ordering::Acquire) => {
                    Poll::Ready(Err(CliError::OperationCanceled))
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    fn poll_requested(&self, context: &mut Context<'_>) -> Poll<()> {
        if self.requested.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        let mut waker = self.lock_waker();
        if waker
            .as_ref()
            .is_none_or(|registered| !registered.will_wake(context.waker()))
        {
            *waker = Some(context.waker().clone());
        }

        if self.requested.load(Ordering::Acquire) {
            waker.take();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn lock_waker(&self) -> std::sync::MutexGuard<'_, Option<Waker>> {
        self.waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn clear_waker(&self) {
        self.lock_waker().take();
    }
}

impl Drop for CancellationRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.clear_waker();
    }
}

/// Initialized services available while a command executes.
pub(crate) struct CommandContext {
    signature: conkit_signature::SignatureContractKit,
    sketch: SketchAdapter,
    compiler: CompilerExtractor,
    catalog_read_limits: CatalogReadLimits,
    cancellation: ApplicationCancellation,
    output: Output,
}

impl CommandContext {
    /// Initializes every CLI-owned runtime dependency.
    ///
    /// # Errors
    ///
    /// Returns an error if the signature or sketch contract adapter cannot be
    /// initialized or the shared CPU pool cannot be constructed.
    pub(crate) fn initialize(cancellation: ApplicationCancellation) -> Result<Self> {
        let worker_threads = std::thread::available_parallelism()?;
        let shared_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(worker_threads.get())
                .build()?,
        );
        let max_in_flight_operations = NonZeroUsize::MIN;
        let max_pending_operations = 0;
        let signature_work = conkit_signature::WorkOptions {
            pool: conkit_signature::WorkerPool::Shared {
                pool: Arc::clone(&shared_pool),
            },
            max_in_flight_operations,
            max_pending_operations,
        };
        let sketch_work = conkit_sketch::WorkOptions {
            pool: conkit_sketch::WorkerPool::Shared { pool: shared_pool },
            max_in_flight_operations,
            max_pending_operations,
        };
        let catalog_read_limits = CatalogReadLimits::default();
        let compiler = CompilerExtractor::new(CompilerCancellation::from_flag(cancellation.flag()));

        Ok(Self {
            signature: conkit_signature::SignatureContractKit::builder()
                .with_work_options(signature_work)
                .with_limits(conkit_signature::SignatureLimits::default())
                .build()?,
            sketch: SketchAdapter::initialize(
                sketch_work,
                conkit_sketch::SketchLimits::default(),
                &cancellation,
            )?,
            compiler,
            catalog_read_limits,
            cancellation,
            output: Output,
        })
    }

    /// Returns the signature contract adapter.
    pub(crate) fn signature(&self) -> &conkit_signature::SignatureContractKit {
        &self.signature
    }

    /// Returns the sketch contract adapter.
    pub(crate) fn sketch(&self) -> &SketchAdapter {
        &self.sketch
    }

    /// Returns the CLI-owned Cargo/rustdoc extractor.
    pub(crate) fn compiler(&self) -> &CompilerExtractor {
        &self.compiler
    }

    /// Returns the CLI filesystem budgets applied before domain validation.
    pub(crate) fn catalog_read_limits(&self) -> CatalogReadLimits {
        self.catalog_read_limits
    }

    /// Returns the process cancellation source for command execution.
    pub(crate) fn cancellation(&self) -> &ApplicationCancellation {
        &self.cancellation
    }

    /// Returns the output sink for user-facing summaries.
    pub(crate) fn output(&self) -> &Output {
        &self.output
    }
}

#[cfg(test)]
mod tests {
    use std::future::{Future, pending};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use super::ApplicationCancellation;

    struct WakeCount {
        count: AtomicUsize,
    }

    struct CancelWhilePolled {
        cancellation: ApplicationCancellation,
    }

    struct ReentrantLockObservation {
        cancellation: ApplicationCancellation,
        lock_was_available: AtomicBool,
    }

    impl Future for CancelWhilePolled {
        type Output = usize;

        fn poll(self: std::pin::Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            self.cancellation.request();
            Poll::Ready(42)
        }
    }

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl ReentrantLockObservation {
        fn record(&self) {
            let lock_was_available = self.cancellation.waker.try_lock().is_ok();
            self.lock_was_available
                .store(lock_was_available, Ordering::Relaxed);
        }
    }

    impl Wake for ReentrantLockObservation {
        fn wake(self: Arc<Self>) {
            self.record();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.record();
        }
    }

    #[test]
    fn cancellation_wakes_pending_command_and_wins_the_race() {
        let cancellation = ApplicationCancellation::new();
        let wake_count = Arc::new(WakeCount {
            count: AtomicUsize::new(0),
        });
        let waker = Waker::from(Arc::clone(&wake_count));
        let mut context = Context::from_waker(&waker);
        let mut raced = std::pin::pin!(cancellation.race(pending::<()>()));

        assert!(matches!(raced.as_mut().poll(&mut context), Poll::Pending));
        cancellation.request();
        assert_eq!(wake_count.count.load(Ordering::Relaxed), 1);
        assert!(matches!(
            raced.as_mut().poll(&mut context),
            Poll::Ready(Err(super::CliError::OperationCanceled))
        ));
    }

    #[test]
    fn cancellation_releases_the_registration_mutex_before_waking() {
        let cancellation = ApplicationCancellation::new();
        let observation = Arc::new(ReentrantLockObservation {
            cancellation: cancellation.clone(),
            lock_was_available: AtomicBool::new(false),
        });
        let waker = Waker::from(Arc::clone(&observation));
        let mut context = Context::from_waker(&waker);
        let mut raced = std::pin::pin!(cancellation.race(pending::<()>()));

        assert!(matches!(raced.as_mut().poll(&mut context), Poll::Pending));
        cancellation.request();

        assert!(observation.lock_was_available.load(Ordering::Relaxed));
        assert!(matches!(
            cancellation.checkpoint(),
            Err(super::CliError::OperationCanceled)
        ));
    }

    #[test]
    fn dropping_pending_race_releases_registered_waker() {
        let cancellation = ApplicationCancellation::new();
        let wake_count = Arc::new(WakeCount {
            count: AtomicUsize::new(0),
        });
        let waker = Waker::from(wake_count);
        let mut context = Context::from_waker(&waker);
        let mut raced = Box::pin(cancellation.race(pending::<()>()));

        assert!(matches!(raced.as_mut().poll(&mut context), Poll::Pending));
        assert!(cancellation.lock_waker().is_some());
        drop(raced);
        assert!(cancellation.lock_waker().is_none());
    }

    #[test]
    fn completed_command_value_is_preserved() {
        let cancellation = ApplicationCancellation::new();

        let value = futures_executor::block_on(cancellation.race(async { 42 }))
            .expect("uncancelled command must complete");

        assert_eq!(value, 42);
    }

    #[test]
    fn completed_command_wins_if_cancellation_arrives_during_its_final_poll() {
        let cancellation = ApplicationCancellation::new();
        let command = CancelWhilePolled {
            cancellation: cancellation.clone(),
        };

        let value = futures_executor::block_on(cancellation.race(command))
            .expect("completed command result must not be reported as canceled");

        assert_eq!(value, 42);
    }
}
