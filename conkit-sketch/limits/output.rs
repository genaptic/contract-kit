use super::{CatalogLimits, LimitExceeded, LimitResource};
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::fmt;

/// Diagnostic collection and bounded-evidence budgets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticLimits {
    /// Maximum correctness diagnostics returned by one check.
    pub count: u64,
    /// Maximum serialized bytes for the complete correctness-diagnostic array.
    pub serialized_bytes: u64,
    /// Maximum source bytes escaped into one expected or actual excerpt.
    pub excerpt_bytes: u64,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            count: 10_000,
            serialized_bytes: 16 * 1024 * 1024,
            excerpt_bytes: 256,
        }
    }
}

impl DiagnosticLimits {
    pub(crate) fn excerpt_maximum(&self) -> usize {
        CatalogLimits::parser_limit(self.excerpt_bytes)
    }
}

/// Returned-output and live generated-text budgets.
///
/// Returned output and simultaneously retained scratch each default to
/// 512 MiB and are enforced independently.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutputLimits {
    /// Maximum aggregate bytes returned in one generated catalog.
    pub generated_bytes: u64,
    /// Maximum simultaneously retained generated or edit text bytes.
    #[serde(default = "OutputLimits::default_scratch_bytes")]
    pub scratch_bytes: u64,
}

impl OutputLimits {
    const DEFAULT_BYTES: u64 = 512 * 1024 * 1024;

    const fn default_scratch_bytes() -> u64 {
        Self::DEFAULT_BYTES
    }
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            generated_bytes: Self::DEFAULT_BYTES,
            scratch_bytes: Self::default_scratch_bytes(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DiagnosticBytes {
    count: u64,
    total: u64,
}

#[derive(Debug)]
pub(crate) struct DiagnosticReservation {
    previous_count: u64,
    previous_bytes: u64,
    next_count: u64,
    next_bytes: u64,
}

impl DiagnosticBytes {
    pub(crate) fn new(limits: &DiagnosticLimits) -> Result<Self, SketchContractKitError> {
        let total = 2;
        if total > limits.serialized_bytes {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                total,
                None,
            )
            .into());
        }
        Ok(Self { count: 0, total })
    }

    pub(crate) fn reserve<T>(
        &self,
        diagnostic: &T,
        additional_bytes: u64,
        limits: &DiagnosticLimits,
        file: Option<&CatalogPath>,
    ) -> Result<DiagnosticReservation, SketchContractKitError>
    where
        T: Serialize + ?Sized,
    {
        let next_count = self.preflight_count(limits, file)?;

        let mut counter = DiagnosticByteCounter::new(limits.serialized_bytes, self.total);
        if self.count > 0 && std::io::Write::write_all(&mut counter, b",").is_err() {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                counter.observed,
                file.cloned(),
            )
            .into());
        }
        let result = serde_json::to_writer(&mut counter, diagnostic);
        if counter.exceeded {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                counter.observed,
                file.cloned(),
            )
            .into());
        }
        result.map_err(|source| {
            SketchContractKitError::write_failed(
                "sketch diagnostic resource accounting",
                source.to_string(),
            )
        })?;
        let next_bytes = counter.observed.saturating_add(additional_bytes);
        if next_bytes > limits.serialized_bytes {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                limits.serialized_bytes.saturating_add(1),
                file.cloned(),
            )
            .into());
        }

        Ok(DiagnosticReservation {
            previous_count: self.count,
            previous_bytes: self.total,
            next_count,
            next_bytes,
        })
    }

    pub(crate) fn preflight_count(
        &self,
        limits: &DiagnosticLimits,
        file: Option<&CatalogPath>,
    ) -> Result<u64, SketchContractKitError> {
        let next_count = self.count.saturating_add(1);
        if next_count > limits.count {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticCount,
                limits.count,
                next_count,
                file.cloned(),
            )
            .into());
        }
        Ok(next_count)
    }

    pub(crate) fn add_bytes(
        &self,
        mut reservation: DiagnosticReservation,
        additional_bytes: u64,
        limits: &DiagnosticLimits,
        file: Option<&CatalogPath>,
    ) -> Result<DiagnosticReservation, SketchContractKitError> {
        debug_assert_eq!(self.count, reservation.previous_count);
        debug_assert_eq!(self.total, reservation.previous_bytes);
        let next_bytes = reservation.next_bytes.saturating_add(additional_bytes);
        if next_bytes > limits.serialized_bytes {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                limits.serialized_bytes.saturating_add(1),
                file.cloned(),
            )
            .into());
        }
        reservation.next_bytes = next_bytes;
        Ok(reservation)
    }

    pub(crate) fn commit(&mut self, reservation: DiagnosticReservation) {
        debug_assert_eq!(self.count, reservation.previous_count);
        debug_assert_eq!(self.total, reservation.previous_bytes);
        self.count = reservation.next_count;
        self.total = reservation.next_bytes;
    }
}

struct DiagnosticByteCounter {
    limit: u64,
    observed: u64,
    exceeded: bool,
}

impl DiagnosticByteCounter {
    fn new(limit: u64, observed: u64) -> Self {
        Self {
            limit,
            observed,
            exceeded: false,
        }
    }
}

impl std::io::Write for DiagnosticByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .observed
            .saturating_add(CatalogLimits::observed(bytes.len()));
        if next > self.limit {
            self.observed = self.limit.saturating_add(1);
            self.exceeded = true;
            return Err(std::io::Error::other("diagnostic byte budget exceeded"));
        }
        self.observed = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) struct GeneratedBytes<'limits> {
    limits: &'limits OutputLimits,
    total: u64,
    retained_scratch_bytes: Cell<u64>,
    cancellation: CancellationProbe,
}

impl<'limits> GeneratedBytes<'limits> {
    pub(crate) fn new(limits: &'limits OutputLimits, cancellation: &CancellationProbe) -> Self {
        Self {
            limits,
            total: 0,
            retained_scratch_bytes: Cell::new(0),
            cancellation: cancellation.clone(),
        }
    }

    pub(crate) fn record(
        &mut self,
        path: &CatalogPath,
        bytes: usize,
    ) -> Result<(), SketchContractKitError> {
        self.cancellation.checkpoint()?;
        self.total = self.total.saturating_add(CatalogLimits::observed(bytes));
        if self.total > self.limits.generated_bytes {
            return Err(LimitExceeded::new(
                LimitResource::GeneratedOutputBytes,
                self.limits.generated_bytes,
                self.total,
                Some(path.clone()),
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn returned_buffer(&self, path: &CatalogPath) -> ReturnedOutput {
        ReturnedOutput::new(
            self.limits.generated_bytes,
            self.total,
            path.clone(),
            self.cancellation.clone(),
        )
    }

    pub(crate) fn scratch_writer(
        &self,
        path: &CatalogPath,
    ) -> Result<ScratchWriter<'_, 'limits>, SketchContractKitError> {
        self.cancellation.checkpoint()?;
        Ok(ScratchWriter::new(self, path.clone()))
    }

    fn release_scratch(&self, bytes: u64) {
        let retained = self.retained_scratch_bytes.get();
        debug_assert!(retained >= bytes);
        self.retained_scratch_bytes
            .set(retained.saturating_sub(bytes));
    }
}

enum OutputFailure {
    Cancelled,
    Limit { observed_at_least: u64 },
    Allocation { message: String },
}

impl OutputFailure {
    fn store(self, destination: &mut Option<Self>, message: &'static str) -> std::io::Error {
        if destination.is_none() {
            *destination = Some(self);
        }
        std::io::Error::other(message)
    }

    fn into_error(
        self,
        resource: LimitResource,
        limit: u64,
        file: &CatalogPath,
    ) -> SketchContractKitError {
        match self {
            Self::Cancelled => SketchContractKitError::operation_cancelled(),
            Self::Limit { observed_at_least } => {
                LimitExceeded::new(resource, limit, observed_at_least, Some(file.clone())).into()
            }
            Self::Allocation { message } => SketchContractKitError::write_failed(file, message),
        }
    }
}

pub(crate) struct ScratchWriter<'meter, 'limits> {
    meter: &'meter GeneratedBytes<'limits>,
    file: CatalogPath,
    bytes: Vec<u8>,
    reserved: u64,
    failure: Option<OutputFailure>,
}

impl<'meter, 'limits> ScratchWriter<'meter, 'limits> {
    fn new(meter: &'meter GeneratedBytes<'limits>, file: CatalogPath) -> Self {
        Self {
            meter,
            file,
            bytes: Vec::new(),
            reserved: 0,
            failure: None,
        }
    }

    pub(crate) fn finish_text<E>(
        mut self,
        result: Result<(), E>,
    ) -> Result<ScratchText<'meter, 'limits>, SketchContractKitError>
    where
        E: fmt::Display,
    {
        if self.meter.cancellation.is_cancelled() {
            return Err(SketchContractKitError::operation_cancelled());
        }
        if let Some(failure) = self.failure.take() {
            return Err(failure.into_error(
                LimitResource::OutputScratchBytes,
                self.meter.limits.scratch_bytes,
                &self.file,
            ));
        }
        result.map_err(|source| {
            SketchContractKitError::write_failed(&self.file, source.to_string())
        })?;

        let reserved = std::mem::take(&mut self.reserved);
        let bytes = std::mem::take(&mut self.bytes);
        match String::from_utf8(bytes) {
            Ok(text) => Ok(ScratchText {
                meter: self.meter,
                text,
                reserved,
            }),
            Err(source) => {
                self.reserved = reserved;
                Err(SketchContractKitError::write_failed(
                    &self.file,
                    source.to_string(),
                ))
            }
        }
    }

    fn fail(&mut self, failure: OutputFailure, message: &'static str) -> std::io::Error {
        failure.store(&mut self.failure, message)
    }
}

impl Drop for ScratchWriter<'_, '_> {
    fn drop(&mut self) {
        self.meter.release_scratch(self.reserved);
    }
}

impl std::fmt::Write for ScratchWriter<'_, '_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        std::io::Write::write_all(self, value.as_bytes()).map_err(|_| std::fmt::Error)
    }
}

impl std::io::Write for ScratchWriter<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("scratch writer has failed"));
        }
        if self.meter.cancellation.checkpoint().is_err() {
            return Err(self.fail(OutputFailure::Cancelled, "scratch generation cancelled"));
        }
        if bytes.is_empty() {
            return Ok(0);
        }

        const WRITE_CHUNK_BYTES: usize = 64 * 1024;
        let length = bytes.len().min(WRITE_CHUNK_BYTES);
        let additional = CatalogLimits::observed(length);
        let next = self
            .meter
            .retained_scratch_bytes
            .get()
            .saturating_add(additional);
        if next > self.meter.limits.scratch_bytes {
            return Err(self.fail(
                OutputFailure::Limit {
                    observed_at_least: self.meter.limits.scratch_bytes.saturating_add(1),
                },
                "output scratch byte budget exceeded",
            ));
        }
        if let Err(source) = self.bytes.try_reserve(length) {
            return Err(self.fail(
                OutputFailure::Allocation {
                    message: source.to_string(),
                },
                "output scratch allocation failed",
            ));
        }

        self.bytes.extend_from_slice(&bytes[..length]);
        self.meter.retained_scratch_bytes.set(next);
        self.reserved = self.reserved.saturating_add(additional);
        Ok(length)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("scratch writer has failed"));
        }
        if self.meter.cancellation.checkpoint().is_err() {
            return Err(self.fail(OutputFailure::Cancelled, "scratch generation cancelled"));
        }
        Ok(())
    }
}

pub(crate) struct ScratchText<'meter, 'limits> {
    meter: &'meter GeneratedBytes<'limits>,
    text: String,
    reserved: u64,
}

impl ScratchText<'_, '_> {
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn truncate_tail(&mut self, bytes: usize) -> bool {
        let Some(new_length) = self.text.len().checked_sub(bytes) else {
            return false;
        };
        if !self.text.is_char_boundary(new_length) {
            return false;
        }

        self.text.truncate(new_length);
        let released = CatalogLimits::observed(bytes);
        debug_assert!(self.reserved >= released);
        self.reserved = self.reserved.saturating_sub(released);
        self.meter.release_scratch(released);
        true
    }
}

impl Drop for ScratchText<'_, '_> {
    fn drop(&mut self) {
        self.meter.release_scratch(self.reserved);
    }
}

pub(crate) struct ReturnedOutput {
    limit: u64,
    observed: u64,
    path: CatalogPath,
    bytes: Vec<u8>,
    failure: Option<OutputFailure>,
    cancellation: CancellationProbe,
}

impl ReturnedOutput {
    fn new(limit: u64, observed: u64, path: CatalogPath, cancellation: CancellationProbe) -> Self {
        Self {
            limit,
            observed,
            path,
            bytes: Vec::new(),
            failure: None,
            cancellation,
        }
    }

    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("returned output writer has failed"));
        }
        let next = self
            .observed
            .saturating_add(CatalogLimits::observed(bytes.len()));
        if next > self.limit {
            return Err(OutputFailure::Limit {
                observed_at_least: self.limit.saturating_add(1),
            }
            .store(&mut self.failure, "generated output byte budget exceeded"));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        const WRITE_CHUNK_BYTES: usize = 64 * 1024;
        for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
            if self.cancellation.checkpoint().is_err() {
                return Err(
                    OutputFailure::Cancelled.store(&mut self.failure, "generated output cancelled")
                );
            }
            if let Err(source) = self.bytes.try_reserve(chunk.len()) {
                return Err(OutputFailure::Allocation {
                    message: source.to_string(),
                }
                .store(&mut self.failure, "generated output allocation failed"));
            }
            self.bytes.extend_from_slice(chunk);
        }
        self.observed = next;
        Ok(())
    }

    pub(crate) fn finish<E>(self, result: Result<(), E>) -> Result<Vec<u8>, SketchContractKitError>
    where
        E: fmt::Display,
    {
        if self.cancellation.is_cancelled() {
            return Err(SketchContractKitError::operation_cancelled());
        }
        if let Some(failure) = self.failure {
            return Err(failure.into_error(
                LimitResource::GeneratedOutputBytes,
                self.limit,
                &self.path,
            ));
        }
        result.map_err(|source| {
            SketchContractKitError::write_failed(&self.path, source.to_string())
        })?;
        Ok(self.bytes)
    }
}

impl std::fmt::Write for ReturnedOutput {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        std::io::Write::write_all(self, value.as_bytes()).map_err(|_| std::fmt::Error)
    }
}

impl std::io::Write for ReturnedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.append(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("returned output writer has failed"));
        }
        if self.cancellation.checkpoint().is_err() {
            return Err(
                OutputFailure::Cancelled.store(&mut self.failure, "generated output cancelled")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticBytes, DiagnosticLimits, GeneratedBytes, OutputFailure, OutputLimits};
    use crate::files::CatalogPath;
    use crate::limits::LimitResource;
    use crate::work::CancellationProbe;

    #[test]
    fn diagnostic_and_generated_byte_trackers_stop_at_first_exceedance() {
        let path = CatalogPath::new("report.json").expect("path");
        let diagnostic_limits = DiagnosticLimits {
            count: 1,
            serialized_bytes: 16,
            excerpt_bytes: 1,
        };
        let mut diagnostics =
            DiagnosticBytes::new(&diagnostic_limits).expect("diagnostic array framing");
        let first = diagnostics
            .reserve("a", 0, &diagnostic_limits, Some(&path))
            .expect("first diagnostic");
        diagnostics.commit(first);
        let count = diagnostics
            .reserve("b", 0, &diagnostic_limits, Some(&path))
            .expect_err("diagnostic count");
        assert_eq!(
            count.limit_exceeded().expect("typed count limit").resource,
            LimitResource::DiagnosticCount
        );

        let byte_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: 5,
            excerpt_bytes: 1,
        };
        let mut diagnostic_bytes =
            DiagnosticBytes::new(&byte_limits).expect("empty diagnostic array");
        let first = diagnostic_bytes
            .reserve("a", 0, &byte_limits, Some(&path))
            .expect("one string diagnostic exactly fills five-byte array budget");
        diagnostic_bytes.commit(first);
        let bytes = diagnostic_bytes
            .reserve("b", 0, &byte_limits, Some(&path))
            .expect_err("the comma for a second diagnostic must exceed the byte budget");
        assert_eq!(
            bytes.limit_exceeded().expect("typed byte limit").resource,
            LimitResource::DiagnosticBytes
        );
        let recovered_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: 9,
            excerpt_bytes: 1,
        };
        let recovered = diagnostic_bytes
            .reserve("b", 0, &recovered_limits, Some(&path))
            .expect("failed byte accounting must not commit its count or total");
        diagnostic_bytes.commit(recovered);

        let mut preflight =
            DiagnosticBytes::new(&recovered_limits).expect("empty diagnostic array");
        let first = preflight
            .reserve("a", 0, &recovered_limits, Some(&path))
            .expect("first diagnostic");
        preflight.commit(first);
        let crossing_preflight = DiagnosticLimits {
            count: 2,
            serialized_bytes: 8,
            excerpt_bytes: 1,
        };
        let reservation = preflight
            .reserve("", 0, &crossing_preflight, Some(&path))
            .expect("empty deferred evidence skeleton fits exactly");
        let preflight_error = preflight
            .add_bytes(reservation, 1, &crossing_preflight, Some(&path))
            .expect_err("deferred one-byte evidence must cross the exact aggregate budget");
        let preflight_limit = preflight_error
            .limit_exceeded()
            .expect("typed preflight limit");
        assert_eq!(preflight_limit.resource, LimitResource::DiagnosticBytes);
        assert_eq!(preflight_limit.observed_at_least, 9);
        let oversized = preflight
            .reserve("", 0, &crossing_preflight, Some(&path))
            .expect("empty deferred evidence skeleton fits exactly");
        let oversized_preflight_error = preflight
            .add_bytes(oversized, 1_024, &crossing_preflight, Some(&path))
            .expect_err("deferred evidence must report the first aggregate byte over budget");
        let oversized_preflight_limit = oversized_preflight_error
            .limit_exceeded()
            .expect("typed oversized preflight limit");
        assert_eq!(
            oversized_preflight_limit.observed_at_least,
            crossing_preflight.serialized_bytes + 1
        );
        let recovered = preflight
            .reserve("b", 0, &recovered_limits, Some(&path))
            .expect("failed preflight must not commit its count or total");
        preflight.commit(recovered);

        let empty_limit = DiagnosticLimits {
            count: 0,
            serialized_bytes: 1,
            excerpt_bytes: 1,
        };
        let empty_error = DiagnosticBytes::new(&empty_limit)
            .expect_err("even an empty JSON diagnostic array requires two bytes");
        let empty_breach = empty_error
            .limit_exceeded()
            .expect("typed empty-array limit");
        assert_eq!(empty_breach.resource, LimitResource::DiagnosticBytes);
        assert_eq!(empty_breach.observed_at_least, 2);
        DiagnosticBytes::new(&DiagnosticLimits {
            count: 0,
            serialized_bytes: 2,
            excerpt_bytes: 1,
        })
        .expect("an empty JSON diagnostic array exactly fits a two-byte budget");

        let comma_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: 9,
            excerpt_bytes: 1,
        };
        let mut comma_bytes = DiagnosticBytes::new(&comma_limits).expect("empty diagnostic array");
        let first = comma_bytes
            .reserve("a", 0, &comma_limits, None)
            .expect("first diagnostic");
        comma_bytes.commit(first);
        let second = comma_bytes
            .reserve("b", 0, &comma_limits, None)
            .expect("two string diagnostics plus framing and comma exactly fit nine bytes");
        comma_bytes.commit(second);

        let output = OutputLimits {
            generated_bytes: 3,
            ..OutputLimits::default()
        };
        let cancellation = CancellationProbe::new();
        let mut generated = GeneratedBytes::new(&output, &cancellation);
        generated.record(&path, 2).expect("first output");
        let error = generated.record(&path, 2).expect_err("generated bytes");
        let error = error.limit_exceeded().expect("typed output limit");
        assert_eq!(error.resource, LimitResource::GeneratedOutputBytes);
        assert_eq!(error.observed_at_least, 4);

        let exact_output = OutputLimits {
            generated_bytes: 3,
            ..OutputLimits::default()
        };
        let generated = GeneratedBytes::new(&exact_output, &cancellation);
        let mut exact = generated.returned_buffer(&path);
        let exact_result = std::io::Write::write_all(&mut exact, b"123");
        assert_eq!(
            exact.finish(exact_result).expect("exact output budget"),
            b"123"
        );

        let mut exceeded = generated.returned_buffer(&path);
        let result = std::io::Write::write_all(&mut exceeded, b"1234");
        let error = exceeded
            .finish(result)
            .expect_err("writer must stop before exceeding its buffer budget");
        let error = error
            .limit_exceeded()
            .expect("typed streaming output limit");
        assert_eq!(error.resource, LimitResource::GeneratedOutputBytes);
        assert_eq!(error.observed_at_least, 4);
    }

    #[test]
    fn generated_output_writer_preserves_typed_cancellation() {
        let limits = OutputLimits::default();
        let cancellation = CancellationProbe::new();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let path = CatalogPath::new("report.yml").expect("path");
        let mut output = generated.returned_buffer(&path);
        std::io::Write::write_all(&mut output, b"already-rendered").expect("active output prefix");
        cancellation.cancel();

        let continuation = vec![b'x'; 128 * 1024];
        let rendering = std::io::Write::write_all(&mut output, &continuation);
        let error = output
            .finish(rendering)
            .expect_err("cancelled output continuation must stop");

        assert!(error.is_operation_cancelled());
        assert!(error.limit_exceeded().is_none());
    }

    #[test]
    fn returned_output_preserves_first_failure_and_finish_precedence() {
        use std::io::Write as _;

        let path = CatalogPath::new("report.yml").expect("path");
        let limits = OutputLimits {
            generated_bytes: 0,
            ..OutputLimits::default()
        };
        let cancellation = CancellationProbe::new();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let mut limited = generated.returned_buffer(&path);
        limited
            .write_all(b"x")
            .expect_err("the first byte must establish the output limit failure");
        limited
            .flush()
            .expect_err("a failed writer must preserve its first failure");
        assert!(matches!(
            limited.failure,
            Some(OutputFailure::Limit {
                observed_at_least: 1
            })
        ));
        let error = limited
            .finish(Err::<(), _>("serializer failed"))
            .expect_err("stored limit failure must precede the serializer error");
        assert_eq!(
            error.limit_exceeded().expect("typed output limit").resource,
            LimitResource::GeneratedOutputBytes
        );

        let cancellation = CancellationProbe::new();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let mut cancelled = generated.returned_buffer(&path);
        cancelled
            .write_all(b"x")
            .expect_err("the first byte must establish the output limit failure");
        cancellation.cancel();
        let error = cancelled
            .finish(Err::<(), _>("serializer failed"))
            .expect_err("live cancellation must precede stored and serializer failures");
        assert!(error.is_operation_cancelled());
        assert!(error.limit_exceeded().is_none());
    }

    #[test]
    fn live_scratch_reservations_enforce_the_combined_boundary_and_release_on_drop() {
        let limits = OutputLimits {
            generated_bytes: 1,
            scratch_bytes: 4,
        };
        let cancellation = CancellationProbe::new();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let path = CatalogPath::new("contract.yml").expect("path");

        let mut first = generated
            .scratch_writer(&path)
            .expect("active first scratch writer");
        std::io::Write::write_all(&mut first, b"ab").expect("first scratch text");
        let first = first
            .finish_text(Ok::<(), std::fmt::Error>(()))
            .expect("first text");

        let mut second = generated
            .scratch_writer(&path)
            .expect("active second scratch writer");
        std::io::Write::write_all(&mut second, b"cd").expect("exact live boundary");
        let second = second
            .finish_text(Ok::<(), std::fmt::Error>(()))
            .expect("second text");
        assert_eq!(generated.retained_scratch_bytes.get(), 4);

        let mut exceeded = generated
            .scratch_writer(&path)
            .expect("active crossing writer");
        let write = std::io::Write::write_all(&mut exceeded, b"e");
        let Err(error) = exceeded.finish_text(write) else {
            panic!("one additional live byte must fail");
        };
        let limit = error.limit_exceeded().expect("typed scratch limit");
        assert_eq!(limit.resource, LimitResource::OutputScratchBytes);
        assert_eq!(limit.limit, 4);
        assert_eq!(limit.observed_at_least, 5);
        assert_eq!(limit.file.as_ref(), Some(&path));
        assert_eq!(generated.retained_scratch_bytes.get(), 4);

        drop(first);
        assert_eq!(generated.retained_scratch_bytes.get(), 2);
        drop(second);
        assert_eq!(generated.retained_scratch_bytes.get(), 0);
    }

    #[test]
    fn scratch_failure_cancellation_truncation_and_returned_output_keep_exact_accounting() {
        let limits = OutputLimits {
            generated_bytes: 3,
            scratch_bytes: 8,
        };
        let cancellation = CancellationProbe::new();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let path = CatalogPath::new("contract.yml").expect("path");

        let mut failed = generated
            .scratch_writer(&path)
            .expect("active fallible scratch writer");
        std::io::Write::write_all(&mut failed, b"xy").expect("partial serializer output");
        let failure = failed.finish_text(Err::<(), _>("serializer failed"));
        assert!(matches!(failure, Err(error) if error.to_string().contains("serializer failed")));
        assert_eq!(generated.retained_scratch_bytes.get(), 0);

        let mut text = generated
            .scratch_writer(&path)
            .expect("active scratch writer");
        std::io::Write::write_all(&mut text, b"abcd").expect("scratch prefix");
        let mut text = text
            .finish_text(Ok::<(), std::fmt::Error>(()))
            .expect("retained scratch text");
        assert!(text.truncate_tail(2));
        assert_eq!(text.as_str(), "ab");
        assert_eq!(generated.retained_scratch_bytes.get(), 2);

        let mut multibyte = generated
            .scratch_writer(&path)
            .expect("active multibyte scratch writer");
        std::io::Write::write_all(&mut multibyte, "é".as_bytes()).expect("multibyte scratch text");
        let mut multibyte = multibyte
            .finish_text(Ok::<(), std::fmt::Error>(()))
            .expect("retained multibyte scratch text");
        assert!(!multibyte.truncate_tail(1));
        assert_eq!(multibyte.as_str(), "é");
        assert_eq!(generated.retained_scratch_bytes.get(), 4);
        assert!(multibyte.truncate_tail(2));
        assert_eq!(multibyte.as_str(), "");
        assert_eq!(generated.retained_scratch_bytes.get(), 2);

        let mut returned = generated.returned_buffer(&path);
        let returned_write = std::io::Write::write_all(&mut returned, b"123");
        assert_eq!(
            returned.finish(returned_write).expect("returned bytes"),
            b"123"
        );
        assert_eq!(generated.retained_scratch_bytes.get(), 2);

        let mut cancelled = generated
            .scratch_writer(&path)
            .expect("active cancellable writer");
        std::io::Write::write_all(&mut cancelled, b"cd").expect("scratch before cancellation");
        cancellation.cancel();
        let continuation = std::io::Write::write_all(&mut cancelled, b"ef");
        let Err(error) = cancelled.finish_text(continuation) else {
            panic!("cancelled scratch continuation");
        };
        assert!(error.is_operation_cancelled());
        assert_eq!(generated.retained_scratch_bytes.get(), 2);

        drop(text);
        assert_eq!(generated.retained_scratch_bytes.get(), 0);

        let pre_cancelled = CancellationProbe::new();
        pre_cancelled.cancel();
        let pre_cancelled_generated = GeneratedBytes::new(&limits, &pre_cancelled);
        let construction = pre_cancelled_generated.scratch_writer(&path);
        assert!(matches!(construction, Err(error) if error.is_operation_cancelled()));
    }

    #[test]
    fn scratch_writer_checks_empty_writes_and_preserves_its_first_failure() {
        use std::io::Write as _;

        let path = CatalogPath::new("contract.yml").expect("path");
        let cancellation = CancellationProbe::new();
        let limits = OutputLimits::default();
        let generated = GeneratedBytes::new(&limits, &cancellation);
        let mut empty = generated
            .scratch_writer(&path)
            .expect("active scratch writer");
        cancellation.cancel();
        empty
            .write(&[])
            .expect_err("even an empty write must observe cancellation");
        assert!(matches!(empty.failure, Some(OutputFailure::Cancelled)));

        let cancellation = CancellationProbe::new();
        let zero_limits = OutputLimits {
            scratch_bytes: 0,
            ..OutputLimits::default()
        };
        let generated = GeneratedBytes::new(&zero_limits, &cancellation);
        let mut limited = generated
            .scratch_writer(&path)
            .expect("active zero-budget writer");
        limited
            .write_all(b"x")
            .expect_err("the first byte establishes the scratch-limit failure");
        cancellation.cancel();
        limited
            .flush()
            .expect_err("a failed writer remains failed after cancellation");
        assert!(matches!(
            limited.failure,
            Some(OutputFailure::Limit {
                observed_at_least: 1
            })
        ));
    }
}
