//! Bounded Cargo process lifecycle, pipe readers, reaping, and cleanup evidence.

use std::ffi::OsString;
use std::fmt;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use command_group::{CommandGroup as _, GroupChild};

use super::SUPPORTED_COMPILER_TOOLCHAIN;
use super::error::CompilerError;
use super::limits::{CompilerTemporaryTree, CompilerUsage};
use super::probe::RustdocProbe;

pub(super) const PROCESS_CLEANUP_EVIDENCE_BYTES: usize = 512;

pub(super) struct CargoProgram {
    pub(super) executable: OsString,
    pub(super) prefix: Vec<OsString>,
}

impl CargoProgram {
    pub(super) fn supported() -> Self {
        Self {
            executable: OsString::from("cargo"),
            prefix: vec![OsString::from(format!("+{SUPPORTED_COMPILER_TOOLCHAIN}"))],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompilerOperation {
    Metadata,
    PackageId,
    CompilerVersion,
    Configuration,
    RustdocConfigurationProbe,
    Rustdoc,
}

impl fmt::Display for CompilerOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Metadata => "Cargo metadata",
            Self::PackageId => "Cargo package resolution",
            Self::CompilerVersion => "Cargo compiler-version query",
            Self::Configuration => "Cargo cfg query",
            Self::RustdocConfigurationProbe => "Cargo rustdoc cfg-context probe",
            Self::Rustdoc => "Cargo rustdoc JSON extraction",
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompilerSemanticResource {
    MetadataPackages,
    MetadataTargets,
    RustdocItems,
    SourceMappings,
}

impl fmt::Display for CompilerSemanticResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MetadataPackages => "Cargo metadata package count",
            Self::MetadataTargets => "Cargo metadata target count",
            Self::RustdocItems => "rustdoc item count",
            Self::SourceMappings => "rustdoc source-mapping count",
        })
    }
}

pub(super) struct CargoProcess<'operation> {
    operation: CompilerOperation,
    child: GroupChild,
    readers: BoundedPipes,
    temporary_tree: Option<CompilerTemporaryTree<'operation>>,
    usage: &'operation Arc<CompilerUsage>,
}

impl<'operation> CargoProcess<'operation> {
    pub(super) fn spawn(
        cargo: &CargoProgram,
        operation: CompilerOperation,
        current_directory: &Path,
        arguments: Vec<OsString>,
        temporary_tree: Option<&'operation Path>,
        usage: &'operation Arc<CompilerUsage>,
        environment: CargoEnvironment<'_>,
    ) -> Result<Self, CompilerError> {
        usage.checkpoint(operation)?;
        let mut command = Command::new(&cargo.executable);
        command
            .args(&cargo.prefix)
            .args(arguments)
            .current_dir(current_directory)
            .env("CARGO_TERM_COLOR", "never")
            .env("RUST_BACKTRACE", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        environment.apply(&mut command);
        usage.checkpoint(operation)?;
        let mut child = command
            .group_spawn()
            .map_err(|source| CompilerError::CargoSpawn {
                executable: cargo.executable.clone(),
                source,
            })?;
        let temporary_tree = temporary_tree.map(|root| CompilerTemporaryTree {
            root,
            operation,
            usage: usage.as_ref(),
            next_scan: Instant::now(),
        });
        let readers = match BoundedPipes::start(child.inner(), operation, Arc::clone(usage)) {
            Ok(readers) => readers,
            Err(failure) => {
                let (primary, readers) = failure.into_parts();
                return Err(Self {
                    operation,
                    child,
                    readers,
                    temporary_tree,
                    usage,
                }
                .fail(primary));
            }
        };
        Ok(Self {
            operation,
            child,
            readers,
            temporary_tree,
            usage,
        })
    }

    pub(super) fn execute(self) -> Result<CargoOutput, CompilerError> {
        let operation = self.operation;
        let completion = self.complete()?;
        if !completion.status.success() {
            return Err(completion.output.failure(operation, completion.status));
        }
        Ok(completion.output)
    }

    pub(super) fn complete(mut self) -> Result<CargoCompletion, CompilerError> {
        let status = match self.monitor() {
            Ok(status) => status,
            Err(primary) => return Err(self.fail(primary)),
        };
        while !self.readers.finished() {
            if let Err(error) = self.usage.checkpoint(self.operation) {
                return Err(self.fail(error));
            }
            std::thread::sleep(self.usage.limits().poll_interval);
        }
        if let Err(error) = self.usage.checkpoint(self.operation) {
            return Err(self.fail(error));
        }
        let Self {
            operation,
            child: _,
            readers,
            temporary_tree,
            usage,
        } = self;
        let output = readers.finish_preserving_failure(operation, status, usage)?;
        if let Some(temporary_tree) = temporary_tree.as_ref() {
            temporary_tree.inspect_after_completion()?;
        }
        Ok(CargoCompletion { status, output })
    }

    fn monitor(&mut self) -> Result<ExitStatus, CompilerError> {
        loop {
            self.usage.checkpoint(self.operation)?;
            if self.readers.exceeded(CompilerStream::Stdout) {
                return Err(CompilerError::ProcessOutputLimit {
                    operation: self.operation,
                    stream: CompilerStream::Stdout,
                    limit: self.usage.limits().stdout_bytes,
                    observed_at_least: self.usage.output_observed(CompilerStream::Stdout),
                });
            }
            if self.readers.exceeded(CompilerStream::Stderr) {
                return Err(CompilerError::ProcessOutputLimit {
                    operation: self.operation,
                    stream: CompilerStream::Stderr,
                    limit: self.usage.limits().stderr_bytes,
                    observed_at_least: self.usage.output_observed(CompilerStream::Stderr),
                });
            }
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {}
                Err(source) => return Err(CompilerError::ProcessWait(source)),
            }
            if let Some(temporary_tree) = self.temporary_tree.as_mut() {
                temporary_tree.inspect_if_due(Instant::now())?;
            }
            std::thread::sleep(self.usage.limits().poll_interval);
        }
    }

    fn fail(self, primary: CompilerError) -> CompilerError {
        self.cleanup().attach(primary)
    }

    fn cleanup(mut self) -> ProcessCleanupEvidence {
        let mut evidence = ProcessCleanupEvidence::new(self.usage);
        let reaped = Self::reap(ReapTarget::Group(&mut self.child), &mut evidence);
        evidence.drain_readers(self.readers, reaped);
        evidence
    }

    fn reap(mut target: ReapTarget<'_>, evidence: &mut ProcessCleanupEvidence) -> bool {
        // Group termination targets the complete Unix process group or Windows
        // job. A failed group kill falls back to the process leader without
        // performing another group operation through `GroupChild`.
        let terminated = match &mut target {
            ReapTarget::Group(child) => match child.kill() {
                Ok(()) => true,
                Err(source) if source.kind() == ErrorKind::InvalidInput => true,
                Err(source) => {
                    evidence.record("kill process group", &source);
                    return Self::reap(
                        ReapTarget::Leader {
                            child: child.inner(),
                            was_terminated: false,
                        },
                        evidence,
                    );
                }
            },
            ReapTarget::Leader {
                child,
                was_terminated,
            } => {
                *was_terminated = match child.kill() {
                    Ok(()) => true,
                    Err(source) if source.kind() == ErrorKind::InvalidInput => true,
                    Err(source) => {
                        evidence.record("kill process leader", &source);
                        false
                    }
                };
                *was_terminated
            }
        };
        let is_group = matches!(&target, ReapTarget::Group(_));
        let started = Instant::now();
        loop {
            let cancelled = evidence.usage.cancellation_requested();
            match target.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None)
                    if !cancelled
                        && started.elapsed() <= evidence.usage.limits().cleanup_timeout =>
                {
                    std::thread::sleep(evidence.usage.limits().poll_interval);
                }
                Ok(None) if terminated => {
                    if is_group && !cancelled {
                        evidence.record_message(
                            "reap process group",
                            "cleanup polling deadline elapsed; waiting for the terminated group",
                        );
                    }
                    return match target.wait() {
                        Ok(_) => true,
                        Err(source) => {
                            evidence.record(
                                if is_group {
                                    "reap process group"
                                } else {
                                    "reap process leader"
                                },
                                &source,
                            );
                            false
                        }
                    };
                }
                Ok(None) => {
                    evidence.record_message(
                        "reap process leader",
                        "group and direct termination failed; bounded cleanup refused an unbounded wait",
                    );
                    return false;
                }
                Err(source) => {
                    evidence.record(
                        if is_group {
                            "reap process group"
                        } else {
                            "poll process leader"
                        },
                        &source,
                    );
                    if !terminated {
                        return false;
                    }
                    return match target.wait() {
                        Ok(_) => true,
                        Err(source) => {
                            evidence.record(
                                if is_group {
                                    "wait for process group"
                                } else {
                                    "reap process leader"
                                },
                                &source,
                            );
                            false
                        }
                    };
                }
            }
        }
    }
}

pub(super) enum ReapTarget<'child> {
    Group(&'child mut GroupChild),
    Leader {
        child: &'child mut std::process::Child,
        was_terminated: bool,
    },
}

impl ReapTarget<'_> {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self {
            Self::Group(child) => child.try_wait(),
            Self::Leader { child, .. } => child.try_wait(),
        }
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        match self {
            Self::Group(child) => child.wait(),
            Self::Leader { child, .. } => child.wait(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum CargoEnvironment<'a> {
    Pinned,
    RustdocProbe(&'a RustdocProbe),
}

impl CargoEnvironment<'_> {
    pub(in crate::compiler) fn apply(self, command: &mut Command) {
        command
            .env("RUSTC", "rustc")
            .env("RUSTDOC", "rustdoc")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER");
        match self {
            Self::Pinned => {}
            Self::RustdocProbe(probe) => probe.apply(command),
        }
    }
}

pub(super) struct CargoCompletion {
    pub(super) status: ExitStatus,
    pub(super) output: CargoOutput,
}

#[derive(Debug)]
pub(super) struct ProcessCleanupEvidence {
    usage: Arc<CompilerUsage>,
    rendered: String,
}

impl ProcessCleanupEvidence {
    fn new(usage: &Arc<CompilerUsage>) -> Self {
        Self {
            usage: Arc::clone(usage),
            rendered: String::new(),
        }
    }

    fn record(&mut self, step: &'static str, error: &impl fmt::Display) {
        self.record_message(step, &error.to_string());
    }

    fn record_message(&mut self, step: &'static str, message: &str) {
        let mut detail = message.to_owned();
        if detail.len() > PROCESS_CLEANUP_EVIDENCE_BYTES {
            let mut boundary = PROCESS_CLEANUP_EVIDENCE_BYTES.saturating_sub(3);
            while boundary > 0 && !detail.is_char_boundary(boundary) {
                boundary -= 1;
            }
            detail.truncate(boundary);
            detail.push_str("...");
        }
        let entry = format!("; {step}: {detail}");
        let granted = usize::try_from(
            self.usage
                .reserve_cleanup_evidence(u64::try_from(entry.len()).unwrap_or(u64::MAX)),
        )
        .unwrap_or(usize::MAX)
        .min(entry.len());
        let mut boundary = granted;
        while boundary > 0 && !entry.is_char_boundary(boundary) {
            boundary -= 1;
        }
        self.rendered.push_str(&entry[..boundary]);
    }

    fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }

    fn attach(self, primary: CompilerError) -> CompilerError {
        if self.is_empty() {
            primary
        } else {
            CompilerError::ProcessFailureWithEvidence {
                primary: Box::new(primary),
                evidence: self.to_string(),
            }
        }
    }

    fn drain_readers(&mut self, readers: BoundedPipes, reaped: bool) {
        let drain_started = Instant::now();
        while reaped
            && !readers.finished()
            && drain_started.elapsed() <= self.usage.limits().cleanup_timeout
        {
            std::thread::sleep(self.usage.limits().poll_interval);
        }
        if reaped && readers.finished() {
            readers.drain_into(self);
        } else {
            self.record_message(
                "drain process output",
                "readers were not joined because process reaping or pipe closure was incomplete",
            );
        }
    }
}

impl fmt::Display for ProcessCleanupEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.rendered.strip_prefix("; ").unwrap_or(&self.rendered))
    }
}

pub(super) struct BoundedPipes {
    stdout: Option<BoundedPipe>,
    stderr: Option<BoundedPipe>,
}

/// Preserves readers already started before a later reader thread fails.
///
/// The owning Cargo invocation must terminate and reap the child before
/// draining these readers. This makes pipe startup transactional even when the
/// operating system refuses a second reader thread.
pub(super) struct ProcessReaderStartupFailure {
    primary: Box<CompilerError>,
    readers: BoundedPipes,
}

impl BoundedPipes {
    fn start(
        child: &mut std::process::Child,
        operation: CompilerOperation,
        usage: Arc<CompilerUsage>,
    ) -> Result<Self, ProcessReaderStartupFailure> {
        let mut readers = Self {
            stdout: None,
            stderr: None,
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                return Err(ProcessReaderStartupFailure {
                    primary: Box::new(CompilerError::MissingProcessPipe {
                        stream: CompilerStream::Stdout,
                    }),
                    readers,
                });
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                return Err(ProcessReaderStartupFailure {
                    primary: Box::new(CompilerError::MissingProcessPipe {
                        stream: CompilerStream::Stderr,
                    }),
                    readers,
                });
            }
        };
        let stdout = match BoundedPipe::start(
            stdout,
            operation,
            CompilerStream::Stdout,
            Arc::clone(&usage),
        ) {
            Ok(stdout) => stdout,
            Err(primary) => {
                return Err(ProcessReaderStartupFailure {
                    primary: Box::new(primary),
                    readers,
                });
            }
        };
        readers.stdout = Some(stdout);
        let stderr = match BoundedPipe::start(stderr, operation, CompilerStream::Stderr, usage) {
            Ok(stderr) => stderr,
            Err(primary) => {
                return Err(ProcessReaderStartupFailure {
                    primary: Box::new(primary),
                    readers,
                });
            }
        };
        readers.stderr = Some(stderr);
        Ok(readers)
    }

    fn exceeded(&self, stream: CompilerStream) -> bool {
        match stream {
            CompilerStream::Stdout => self.stdout.as_ref().is_some_and(BoundedPipe::exceeded),
            CompilerStream::Stderr => self.stderr.as_ref().is_some_and(BoundedPipe::exceeded),
        }
    }

    fn finished(&self) -> bool {
        self.stdout.as_ref().is_none_or(BoundedPipe::finished)
            && self.stderr.as_ref().is_none_or(BoundedPipe::finished)
    }

    fn finish_preserving_failure(
        self,
        operation: CompilerOperation,
        status: ExitStatus,
        usage: &Arc<CompilerUsage>,
    ) -> Result<CargoOutput, CompilerError> {
        let stdout = self
            .stdout
            .ok_or(CompilerError::MissingProcessPipe {
                stream: CompilerStream::Stdout,
            })
            .and_then(BoundedPipe::finish);
        let stderr = self
            .stderr
            .ok_or(CompilerError::MissingProcessPipe {
                stream: CompilerStream::Stderr,
            })
            .and_then(BoundedPipe::finish);
        match (stdout, stderr) {
            (Ok(stdout), Ok(stderr)) => Ok(CargoOutput { stdout, stderr }),
            (stdout, stderr) if !status.success() => {
                let mut evidence = ProcessCleanupEvidence::new(usage);
                if let Err(error) = &stdout {
                    evidence.record("finish stdout", error);
                }
                if let Err(error) = &stderr {
                    evidence.record("finish stderr", error);
                }
                let output = CargoOutput {
                    stdout: stdout.unwrap_or_default(),
                    stderr: stderr.unwrap_or_default(),
                };
                Err(evidence.attach(output.failure(operation, status)))
            }
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }

    fn drain_into(self, evidence: &mut ProcessCleanupEvidence) {
        if let Some(stdout) = self.stdout
            && let Err(error) = stdout.drain()
        {
            evidence.record("drain stdout", &error);
        }
        if let Some(stderr) = self.stderr
            && let Err(error) = stderr.drain()
        {
            evidence.record("drain stderr", &error);
        }
    }
}

impl ProcessReaderStartupFailure {
    fn into_parts(self) -> (CompilerError, BoundedPipes) {
        (*self.primary, self.readers)
    }
}

pub(super) struct BoundedPipe {
    exceeded: Arc<AtomicBool>,
    reader: JoinHandle<Result<Vec<u8>, CompilerError>>,
}

impl BoundedPipe {
    fn start(
        mut pipe: impl Read + Send + 'static,
        operation: CompilerOperation,
        stream: CompilerStream,
        usage: Arc<CompilerUsage>,
    ) -> Result<Self, CompilerError> {
        let exceeded = Arc::new(AtomicBool::new(false));
        let reader_exceeded = Arc::clone(&exceeded);
        let reader = std::thread::Builder::new()
            .name(format!("conkit-cargo-{stream}"))
            .spawn(move || {
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 8 * 1024];
                loop {
                    usage.checkpoint(operation)?;
                    let read = pipe.read(&mut buffer).map_err(CompilerError::ProcessRead)?;
                    if read == 0 {
                        break;
                    }
                    if let Err(error) = usage.account_output(
                        operation,
                        stream,
                        u64::try_from(read).unwrap_or(u64::MAX),
                    ) {
                        if matches!(error, CompilerError::ProcessOutputLimit { .. }) {
                            reader_exceeded.store(true, Ordering::Release);
                        }
                        return Err(error);
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                Ok(bytes)
            })
            .map_err(|source| CompilerError::ProcessReaderSpawn { stream, source })?;
        Ok(Self { exceeded, reader })
    }

    fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Acquire)
    }

    fn finished(&self) -> bool {
        self.reader.is_finished()
    }

    fn finish(self) -> Result<Vec<u8>, CompilerError> {
        let bytes = self
            .reader
            .join()
            .map_err(|_| CompilerError::ProcessReaderPanicked)??;
        Ok(bytes)
    }

    fn drain(self) -> Result<(), CompilerError> {
        self.reader
            .join()
            .map_err(|_| CompilerError::ProcessReaderPanicked)??;
        Ok(())
    }
}

pub(super) struct CargoOutput {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl CargoOutput {
    pub(super) fn utf8_stdout(self, operation: CompilerOperation) -> Result<String, CompilerError> {
        String::from_utf8(self.stdout).map_err(|source| CompilerError::NonUtf8ProcessOutput {
            operation,
            message: source.to_string(),
        })
    }

    pub(super) fn failure(self, operation: CompilerOperation, status: ExitStatus) -> CompilerError {
        self.failure_with_status_code(operation, status.code())
    }

    pub(super) fn failure_with_status_code(
        self,
        operation: CompilerOperation,
        status: Option<i32>,
    ) -> CompilerError {
        let stderr = String::from_utf8_lossy(&self.stderr).trim().to_owned();
        let lowercase = stderr.to_ascii_lowercase();
        if lowercase.contains(SUPPORTED_COMPILER_TOOLCHAIN)
            && (lowercase.contains("not installed")
                || lowercase.contains("no release found")
                || lowercase.contains("toolchain is not installed")
                || lowercase.contains("toolchain is not installable"))
        {
            CompilerError::SupportedToolchainUnavailable {
                toolchain: SUPPORTED_COMPILER_TOOLCHAIN,
                detail: stderr,
            }
        } else {
            CompilerError::CargoFailed {
                operation,
                status,
                stderr,
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompilerStream {
    Stdout,
    Stderr,
}

impl fmt::Display for CompilerStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    #[ignore = "spawned explicitly by CargoProcess unit tests"]
    fn cargo_process_helper() {
        ProcessBehavior::detect(
            &std::env::current_dir().expect("process helper current directory"),
        )
        .run();
    }

    #[test]
    fn only_the_exact_missing_supported_toolchain_gets_actionable_classification() {
        let missing = CargoOutput {
            stdout: Vec::new(),
            stderr: format!("error: toolchain '{SUPPORTED_COMPILER_TOOLCHAIN}' is not installed")
                .into_bytes(),
        }
        .failure_with_status_code(CompilerOperation::Metadata, Some(1));
        let other = CargoOutput {
            stdout: Vec::new(),
            stderr: b"error: toolchain 'nightly' is not installed".to_vec(),
        }
        .failure_with_status_code(CompilerOperation::Metadata, Some(1));

        match missing {
            CompilerError::SupportedToolchainUnavailable { toolchain, .. } => {
                assert_eq!(toolchain, SUPPORTED_COMPILER_TOOLCHAIN);
            }
            other => panic!("expected supported-toolchain error, got {other:?}"),
        }
        assert!(matches!(other, CompilerError::CargoFailed { .. }));
    }

    #[test]
    fn pinned_cargo_environment_excludes_ambient_compiler_wrappers() {
        let mut command = Command::new("cargo");
        CargoEnvironment::Pinned.apply(&mut command);
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment.get(OsStr::new("RUSTC")),
            Some(&Some(OsString::from("rustc")))
        );
        assert_eq!(
            environment.get(OsStr::new("RUSTDOC")),
            Some(&Some(OsString::from("rustdoc")))
        );
        assert_eq!(
            environment.get(OsStr::new("RUSTUP_AUTO_INSTALL")),
            Some(&Some(OsString::from("0")))
        );
        assert_eq!(environment.get(OsStr::new("RUSTC_WRAPPER")), Some(&None));
        assert_eq!(
            environment.get(OsStr::new("RUSTC_WORKSPACE_WRAPPER")),
            Some(&None)
        );
        assert!(!environment.contains_key(OsStr::new("RUSTDOCFLAGS")));
    }

    #[test]
    fn cleanup_evidence_wraps_without_replacing_the_primary_failure() {
        let primary = CompilerError::ProcessTimeout {
            operation: CompilerOperation::Rustdoc,
            timeout: Duration::from_secs(1),
        };
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let mut evidence = ProcessCleanupEvidence::new(&usage);
        evidence.record_message("reap process group", "cleanup timeout elapsed");

        let error = evidence.attach(primary);

        match error {
            CompilerError::ProcessFailureWithEvidence { primary, evidence } => {
                assert!(matches!(*primary, CompilerError::ProcessTimeout { .. }));
                assert!(evidence.contains("reap process group"));
            }
            other => panic!("expected preserved primary failure, got {other:?}"),
        }
    }

    #[test]
    fn cleanup_evidence_is_unicode_safe_and_bounded() {
        let step = "reap process group";
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let mut evidence = ProcessCleanupEvidence::new(&usage);
        evidence.record_message(step, &"é".repeat(PROCESS_CLEANUP_EVIDENCE_BYTES));
        let rendered = evidence.to_string();

        assert!(rendered.ends_with("..."));
        assert!(rendered.len() <= step.len() + 2 + PROCESS_CLEANUP_EVIDENCE_BYTES);
    }

    #[test]
    fn partial_reader_startup_failure_retains_started_reader_for_cleanup() {
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let stdout = BoundedPipe::start(
            Cursor::new(b"partial output".to_vec()),
            CompilerOperation::Metadata,
            CompilerStream::Stdout,
            Arc::clone(&usage),
        )
        .expect("first reader must start for the partial-start fixture");
        let failure = ProcessReaderStartupFailure {
            primary: Box::new(CompilerError::ProcessReaderSpawn {
                stream: CompilerStream::Stderr,
                source: std::io::Error::other("injected second-reader failure"),
            }),
            readers: BoundedPipes {
                stdout: Some(stdout),
                stderr: None,
            },
        };

        let (primary, readers) = failure.into_parts();
        assert!(matches!(
            primary,
            CompilerError::ProcessReaderSpawn {
                stream: CompilerStream::Stderr,
                ..
            }
        ));
        let mut evidence = ProcessCleanupEvidence::new(&usage);
        readers.drain_into(&mut evidence);
        assert!(evidence.is_empty());
    }

    #[test]
    fn cleanup_evidence_uses_one_cumulative_unicode_safe_budget() {
        let limits = CompilerLimits {
            cleanup_evidence_bytes: 48,
            ..CompilerLimits::default()
        };
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
        let mut first = ProcessCleanupEvidence::new(&usage);
        let mut second = ProcessCleanupEvidence::new(&usage);

        first.record_message("kill process group", &"é".repeat(64));
        second.record_message("reap process group", &"é".repeat(64));

        assert!(first.to_string().len() + second.to_string().len() <= 48);
        assert_eq!(usage.cleanup_evidence_observed(), 48);
    }

    #[test]
    fn missing_cargo_executable_returns_a_typed_spawn_error() {
        let cargo = CargoProgram {
            executable: OsString::from("conkit-deliberately-missing-cargo-executable"),
            prefix: Vec::new(),
        };
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let error = CargoProcess::spawn(
            &cargo,
            CompilerOperation::Metadata,
            Path::new("."),
            Vec::new(),
            None,
            &usage,
            CargoEnvironment::Pinned,
        )
        .err()
        .expect("a missing executable remains a typed spawn failure");

        assert!(matches!(error, CompilerError::CargoSpawn { .. }));
    }

    #[test]
    fn cancellation_is_checked_before_process_creation() {
        let cargo = CargoProgram {
            executable: OsString::from("conkit-deliberately-missing-cargo-executable"),
            prefix: Vec::new(),
        };
        let usage = CompilerUsage::new(
            CompilerLimits::default(),
            CompilerCancellation::from_flag(Arc::new(AtomicBool::new(true))),
        );
        let error = CargoProcess::spawn(
            &cargo,
            CompilerOperation::Metadata,
            Path::new("."),
            Vec::new(),
            None,
            &usage,
            CargoEnvironment::Pinned,
        )
        .err()
        .expect("pre-cancelled extraction must not attempt a spawn");

        assert!(matches!(error, CompilerError::CompilerExtractionCancelled));
    }

    #[test]
    fn cargo_process_preserves_success_and_nonzero_completion() {
        let success = ProcessFixture::new(
            ProcessBehavior::Success,
            CompilerFixture::process_limits(Duration::from_secs(2), 64 * 1024),
            CompilerFixture::cancellation(),
        );
        let output = success
            .spawn(CompilerOperation::Metadata)
            .execute()
            .expect("successful helper output");
        assert!(String::from_utf8_lossy(&output.stdout).contains("bounded success output"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("bounded success diagnostic"));

        let nonzero = ProcessFixture::new(
            ProcessBehavior::Nonzero,
            CompilerFixture::process_limits(Duration::from_secs(2), 64 * 1024),
            CompilerFixture::cancellation(),
        );
        let completion = nonzero
            .spawn(CompilerOperation::RustdocConfigurationProbe)
            .complete()
            .expect("probe-style completion retains nonzero status");
        assert_eq!(completion.status.code(), Some(17));

        let error = nonzero
            .spawn(CompilerOperation::Metadata)
            .execute()
            .err()
            .expect("ordinary execution converts nonzero status");
        assert!(matches!(
            error,
            CompilerError::CargoFailed {
                status: Some(17),
                ..
            }
        ));
    }

    #[test]
    fn runaway_process_output_is_bounded_and_reaped() {
        let fixture = ProcessFixture::new(
            ProcessBehavior::RunawayOutput,
            CompilerFixture::process_limits(Duration::from_secs(2), 32),
            CompilerFixture::cancellation(),
        );
        let error = fixture
            .spawn(CompilerOperation::Metadata)
            .execute()
            .err()
            .expect("unbounded output must terminate the child");

        assert!(matches!(
            CompilerFixture::primary_error(&error),
            CompilerError::ProcessOutputLimit { .. }
        ));
    }

    #[test]
    fn timed_out_process_is_killed_and_reaped() {
        let fixture = ProcessFixture::new(
            ProcessBehavior::Busy,
            CompilerFixture::process_limits(Duration::from_millis(20), 4 * 1024),
            CompilerFixture::cancellation(),
        );
        let error = fixture
            .spawn(CompilerOperation::Rustdoc)
            .execute()
            .err()
            .expect("the timeout must terminate the child");

        assert!(matches!(
            CompilerFixture::primary_error(&error),
            CompilerError::ProcessTimeout { .. }
        ));
    }

    #[test]
    fn cancelled_process_group_is_terminated_and_reaped() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let fixture = ProcessFixture::new(
            ProcessBehavior::Busy,
            CompilerFixture::process_limits(Duration::from_secs(2), 4 * 1024),
            CompilerCancellation::from_flag(Arc::clone(&cancelled)),
        );
        let process = fixture.spawn(CompilerOperation::Rustdoc);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            cancelled.store(true, Ordering::Release);
        });
        let error = process
            .execute()
            .err()
            .expect("cancellation must terminate and reap the process group");
        canceller.join().expect("cancellation helper");

        assert!(matches!(
            CompilerFixture::primary_error(&error),
            CompilerError::CompilerExtractionCancelled
        ));
    }

    #[cfg(unix)]
    #[test]
    fn exhausted_group_kill_uses_leader_fallback() {
        use std::process::Stdio;

        use command_group::CommandGroup as _;

        use super::ReapTarget;

        let root = assert_fs::TempDir::new().expect("leader fallback root");
        let cargo = ProcessBehavior::Success.program(root.path());
        let mut command = Command::new(&cargo.executable);
        command
            .args(&cargo.prefix)
            .current_dir(root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.group_spawn().expect("grouped fallback helper");
        child
            .inner()
            .wait()
            .expect("reap leader before exhausted-group cleanup");
        let usage = CompilerUsage::new(
            CompilerFixture::process_limits(Duration::from_secs(2), 4 * 1024),
            CompilerFixture::cancellation(),
        );
        let mut evidence = ProcessCleanupEvidence::new(&usage);

        assert!(CargoProcess::reap(
            ReapTarget::Group(&mut child),
            &mut evidence
        ));
        assert!(evidence.to_string().contains("kill process group"));
    }

    #[test]
    fn cleanup_reaps_before_joining_or_refusing_readers() {
        let fixture = ProcessFixture::new(
            ProcessBehavior::Busy,
            CompilerFixture::process_limits(Duration::from_secs(2), 4 * 1024),
            CompilerFixture::cancellation(),
        );
        let evidence = fixture.spawn(CompilerOperation::Rustdoc).cleanup();
        assert!(evidence.is_empty());

        let reader = BoundedPipe::start(
            Cursor::new(b"detached output".to_vec()),
            CompilerOperation::Metadata,
            CompilerStream::Stdout,
            Arc::clone(&fixture.usage),
        )
        .expect("detached reader fixture");
        let mut incomplete = ProcessCleanupEvidence::new(&fixture.usage);
        incomplete.drain_readers(
            BoundedPipes {
                stdout: Some(reader),
                stderr: None,
            },
            false,
        );
        assert!(incomplete.to_string().contains("readers were not joined"));
    }

    #[test]
    fn cancellation_visible_before_final_poll_beats_completed_status() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let fixture = ProcessFixture::new(
            ProcessBehavior::Success,
            CompilerFixture::process_limits(Duration::from_secs(2), 64 * 1024),
            CompilerCancellation::from_flag(Arc::clone(&cancelled)),
        );
        let mut process = fixture.spawn(CompilerOperation::Metadata);
        process
            .child
            .inner()
            .wait()
            .expect("helper completes before final poll");
        cancelled.store(true, Ordering::Release);

        let error = process
            .execute()
            .err()
            .expect("visible cancellation preserves monitor precedence");
        assert!(matches!(
            CompilerFixture::primary_error(&error),
            CompilerError::CompilerExtractionCancelled
        ));
    }

    #[test]
    fn quiescent_tree_failure_precedes_nonzero_status_conversion() {
        let root = assert_fs::TempDir::new().expect("quiescent process root");
        let tree = root.path().join("tree");
        std::fs::create_dir(&tree).expect("temporary tree");
        std::fs::write(tree.join("artifact"), b"x").expect("temporary artifact");
        let cargo = ProcessBehavior::Nonzero.program(root.path());
        let limits = CompilerLimits {
            temporary_tree_entries: 1,
            temporary_tree_scan_interval: Duration::from_secs(60),
            ..CompilerFixture::process_limits(Duration::from_secs(2), 64 * 1024)
        };
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
        let mut process = CargoProcess::spawn(
            &cargo,
            CompilerOperation::Rustdoc,
            root.path(),
            Vec::new(),
            Some(&tree),
            &usage,
            CargoEnvironment::Pinned,
        )
        .expect("spawn nonzero helper");
        process
            .temporary_tree
            .as_mut()
            .expect("temporary tree meter")
            .next_scan = Instant::now() + Duration::from_secs(60);
        let error = process
            .execute()
            .err()
            .expect("quiescent tree validation remains authoritative");

        assert!(matches!(
            error,
            CompilerError::TemporaryArtifactEntryLimit { .. }
        ));
    }
}
