//! Cancellation-aware byte ceilings for concrete CLI output writers.
//!
//! The wrapper records the first cancellation or size breach and keeps that
//! failure sticky for later writes and flushes, allowing archive and report
//! owners to translate the same boundary evidence into their distinct errors.

use std::io::Write;

use crate::context::ApplicationCancellation;

const WRITE_CHUNK_BYTES: usize = 64 * 1024;

/// First boundary failure observed while writing one output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedOutputFailure {
    /// Root-operation cancellation was observed before another boundary failure.
    Cancelled,
    /// The requested write would cross the ceiling, with a lower-bound byte count.
    Limit { observed_at_least: u64 },
}

/// One cancellation-aware, byte-limited wrapper around a concrete writer.
pub(crate) struct BoundedOutput<'cancellation, W> {
    inner: W,
    cancellation: &'cancellation ApplicationCancellation,
    ceiling: u64,
    written: u64,
    failure: Option<BoundedOutputFailure>,
}

impl<'cancellation, W> BoundedOutput<'cancellation, W> {
    /// Starts one bounded output without taking ownership of cancellation.
    pub(crate) fn new(
        inner: W,
        cancellation: &'cancellation ApplicationCancellation,
        ceiling: u64,
    ) -> Self {
        Self {
            inner,
            cancellation,
            ceiling,
            written: 0,
            failure: None,
        }
    }

    /// Returns the first cancellation or limit failure, if one occurred.
    pub(crate) fn failure(&self) -> Option<BoundedOutputFailure> {
        self.failure
    }

    /// Consumes the boundary and returns its concrete writer.
    pub(crate) fn into_inner(self) -> W {
        self.inner
    }

    fn checkpoint(&mut self) -> std::io::Result<()> {
        if self.cancellation.checkpoint().is_err() {
            self.record_failure(BoundedOutputFailure::Cancelled);
            return Err(Self::failure_error(BoundedOutputFailure::Cancelled));
        }
        Ok(())
    }

    fn record_failure(&mut self, failure: BoundedOutputFailure) {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
    }

    fn failure_error(failure: BoundedOutputFailure) -> std::io::Error {
        let message = match failure {
            BoundedOutputFailure::Cancelled => "output operation canceled",
            BoundedOutputFailure::Limit { .. } => "output byte limit exceeded",
        };
        std::io::Error::other(message)
    }
}

impl<W: Write> Write for BoundedOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(failure) = self.failure {
            return Err(Self::failure_error(failure));
        }
        self.checkpoint()?;

        let requested = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let observed_at_least = self.written.saturating_add(requested);
        if observed_at_least > self.ceiling {
            let failure = BoundedOutputFailure::Limit { observed_at_least };
            self.record_failure(failure);
            return Err(Self::failure_error(failure));
        }

        let count = bytes.len().min(WRITE_CHUNK_BYTES);
        let written = self.inner.write(&bytes[..count])?;
        self.written = self
            .written
            .saturating_add(u64::try_from(written).unwrap_or(u64::MAX));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(failure) = self.failure {
            return Err(Self::failure_error(failure));
        }
        self.checkpoint()?;
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{BoundedOutput, BoundedOutputFailure, WRITE_CHUNK_BYTES};
    use crate::context::ApplicationCancellation;

    struct RecordingWriter {
        bytes: Vec<u8>,
        requests: Vec<usize>,
        maximum_write: usize,
        fail_write: bool,
        fail_flush: bool,
        flushes: usize,
    }

    impl RecordingWriter {
        fn new(maximum_write: usize) -> Self {
            Self {
                bytes: Vec::new(),
                requests: Vec::new(),
                maximum_write,
                fail_write: false,
                fail_flush: false,
                flushes: 0,
            }
        }

        fn failing_write() -> Self {
            Self {
                fail_write: true,
                ..Self::new(usize::MAX)
            }
        }

        fn failing_flush() -> Self {
            Self {
                fail_flush: true,
                ..Self::new(usize::MAX)
            }
        }
    }

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.requests.push(bytes.len());
            if self.fail_write {
                return Err(std::io::Error::other("injected write failure"));
            }
            let written = bytes.len().min(self.maximum_write);
            self.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            if self.fail_flush {
                Err(std::io::Error::other("injected flush failure"))
            } else {
                Ok(())
            }
        }
    }

    struct CancelAfterWrite {
        cancellation: ApplicationCancellation,
        bytes: Vec<u8>,
    }

    impl Write for CancelAfterWrite {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            self.cancellation.request();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn empty_zero_ceiling_and_exact_writes_succeed() {
        let cancellation = ApplicationCancellation::new();
        let mut empty = BoundedOutput::new(Vec::new(), &cancellation, 0);
        empty.write_all(b"").expect("empty output");
        assert!(empty.into_inner().is_empty());

        let mut exact = BoundedOutput::new(Vec::new(), &cancellation, 4);
        exact.write_all(b"four").expect("exact output ceiling");
        assert_eq!(exact.failure(), None);
        assert_eq!(exact.into_inner(), b"four");
    }

    #[test]
    fn limit_plus_one_is_recorded_before_delegation_and_remains_first() {
        let cancellation = ApplicationCancellation::new();
        let mut output = BoundedOutput::new(RecordingWriter::new(usize::MAX), &cancellation, 3);

        output.write_all(b"four").expect_err("limit plus one");
        assert_eq!(
            output.failure(),
            Some(BoundedOutputFailure::Limit {
                observed_at_least: 4
            })
        );
        assert!(output.inner.bytes.is_empty());
        cancellation.request();
        output.flush().expect_err("first limit failure is retained");
        assert!(matches!(
            output.failure(),
            Some(BoundedOutputFailure::Limit { .. })
        ));
    }

    #[test]
    fn large_and_short_writes_account_only_delegated_actual_bytes() {
        let cancellation = ApplicationCancellation::new();
        let bytes = vec![b'x'; WRITE_CHUNK_BYTES + 7];
        let mut chunked = BoundedOutput::new(
            RecordingWriter::new(usize::MAX),
            &cancellation,
            u64::try_from(bytes.len()).expect("fixture length"),
        );
        chunked.write_all(&bytes).expect("chunked output");
        let chunked = chunked.into_inner();
        assert_eq!(chunked.requests, [WRITE_CHUNK_BYTES, 7]);
        assert_eq!(chunked.bytes, bytes);

        let mut short = BoundedOutput::new(RecordingWriter::new(2), &cancellation, 4);
        short.write_all(b"four").expect("short writer");
        let short = short.into_inner();
        assert_eq!(short.requests, [4, 2]);
        assert_eq!(short.bytes, b"four");
    }

    #[test]
    fn write_zero_and_underlying_io_failures_are_not_boundary_failures() {
        let cancellation = ApplicationCancellation::new();
        let mut zero = BoundedOutput::new(RecordingWriter::new(0), &cancellation, 4);
        assert_eq!(
            zero.write_all(b"data").expect_err("write zero").kind(),
            std::io::ErrorKind::WriteZero
        );
        assert_eq!(zero.failure(), None);

        let mut failing = BoundedOutput::new(RecordingWriter::failing_write(), &cancellation, 4);
        assert!(failing.write_all(b"data").is_err());
        assert_eq!(failing.failure(), None);
    }

    #[test]
    fn cancellation_is_checked_before_write_between_chunks_and_before_flush() {
        let canceled = ApplicationCancellation::new();
        canceled.request();
        let mut before = BoundedOutput::new(Vec::new(), &canceled, 4);
        before.write_all(b"data").expect_err("pre-canceled write");
        assert_eq!(before.failure(), Some(BoundedOutputFailure::Cancelled));
        assert!(before.into_inner().is_empty());

        let cancellation = ApplicationCancellation::new();
        let writer = CancelAfterWrite {
            cancellation: cancellation.clone(),
            bytes: Vec::new(),
        };
        let mut between = BoundedOutput::new(
            writer,
            &cancellation,
            u64::try_from(WRITE_CHUNK_BYTES + 1).expect("fixture length"),
        );
        between
            .write_all(&vec![b'x'; WRITE_CHUNK_BYTES + 1])
            .expect_err("cancellation between chunks");
        assert_eq!(between.failure(), Some(BoundedOutputFailure::Cancelled));
        assert_eq!(between.inner.bytes.len(), WRITE_CHUNK_BYTES);

        let flush_cancellation = ApplicationCancellation::new();
        let mut before_flush =
            BoundedOutput::new(RecordingWriter::new(usize::MAX), &flush_cancellation, 4);
        before_flush
            .write_all(b"data")
            .expect("write before cancel");
        flush_cancellation.request();
        before_flush.flush().expect_err("pre-flush cancellation");
        assert_eq!(
            before_flush.failure(),
            Some(BoundedOutputFailure::Cancelled)
        );
        assert_eq!(before_flush.inner.flushes, 0);
    }

    #[test]
    fn flush_delegates_and_preserves_underlying_failure() {
        let cancellation = ApplicationCancellation::new();
        let mut output = BoundedOutput::new(RecordingWriter::new(usize::MAX), &cancellation, 0);
        output.flush().expect("delegated flush");
        assert_eq!(output.inner.flushes, 1);

        let mut failing = BoundedOutput::new(RecordingWriter::failing_flush(), &cancellation, 0);
        failing.flush().expect_err("underlying flush failure");
        assert_eq!(failing.failure(), None);
        assert_eq!(failing.inner.flushes, 1);
    }
}
