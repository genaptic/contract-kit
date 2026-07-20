//! Contract Kit command-line interface.
//!
//! This binary crate owns the `conkit` process boundary: clap grammar, operating-
//! system paths, bounded filesystem reads and writes, Cargo/rustdoc child
//! processes, cancellation, terminal output, reports, generated-file ownership,
//! and mixed-catalog archives. Reusable signature and sketch semantics stay in
//! the `conkit-signature` and `conkit-sketch` crates.
//!
//! The CLI currently exposes four command families:
//!
//! ```text
//! conkit check <all|signatures> --source <DIR> --contracts <DIR> --output <FILE> [SIGNATURE EXTRACTION] [--default|--strict|--warning]
//! conkit check sketches --source <DIR> --contracts <DIR> --output <FILE> [--default|--strict|--warning]
//! conkit generate <all|signatures> --source <DIR> --contracts <DIR> [--crate-root CRATE_ID=KIND:RELATIVE_PATH]... [SIGNATURE EXTRACTION] [--adopt-existing]
//! conkit generate sketches --source <DIR> --contracts <DIR> [--adopt-existing]
//! conkit archive --contracts <DIR> --archive <DIR> [--gzip]
//! conkit diff --contracts <DIR> --archive <FILE>
//! ```
//!
//! `signature` and `sketch` are aliases for the plural targets. Check modes are
//! mutually exclusive: omission and `--default` select signature `Default`
//! with sketch `Enforce`, `--strict` selects signature `Strict` with sketch
//! `Enforce`, and `--warning` selects `Warning` in both domains. Gzip is the
//! only archive format, so both omitted and explicit `--gzip` select it. A
//! successful diff exits successfully even when changes are present.
//!
//! Signature extraction defaults to portable `syntax`. Opt-in `compiler`
//! extraction accepts the following Cargo-native selection:
//!
//! ```text
//! --signature-extractor compiler --manifest-path FILE
//!   [--package SPEC] [--lib|--bin NAME]
//!   [--features FEATURES|--all-features] [--no-default-features]
//!   [--target TRIPLE]
//! ```
//!
//! Compiler extraction invokes the pinned dated nightly through locked Cargo,
//! selects exactly one package library or binary target, and runs selected
//! build scripts and procedural macros unsandboxed with the user's permissions.
//! A private Cargo-owned rustdoc-probe invocation short-circuits normal clap
//! startup to capture rustdoc arguments. The normal extraction path uses an
//! isolated target directory, bounded child output and artifact traversal,
//! process-group or job-object cleanup, and source-snapshot revalidation before
//! passing one versioned in-memory artifact to `conkit-signature`; it never
//! invokes the compiler executable directly.
//!
//! Root-level combined contract documents use mandatory contract format v2.
//! Their exact source allowlists and extraction headers are reconciled before
//! domain work. Sketches remain opt-in records linked from signatures; the CLI
//! asks the signature domain for exact Rust-item seeds before adapting them for
//! the independent sketch domain.
//!
//! # Boundaries
//!
//! `conkit` is a binary adapter, not a reusable library API. One application-
//! owned Rayon pool serves both domains while their active and pending
//! admission limits remain independent. Each command carries one cumulative
//! catalog-read ledger across all of its filesystem inputs and observes one
//! process cancellation source. Generation recovers interrupted ownership,
//! completes domain work before locking, and reconciles against the exact
//! baseline before sequential individually atomic writes. Reports use bounded
//! individually atomic replacement; archives use deterministic gzip encoding,
//! collision-safe publication, mandatory-v2 validation, and the same command-
//! wide read ledger. Domain crates accept catalogs and typed requests instead
//! of command-line state, terminal policy, or host filesystem paths.
#![deny(rustdoc::broken_intra_doc_links)]

mod app;
mod archive;
mod args;
mod bounded_output;
mod catalog;
mod command;
mod compiler;
mod context;
mod contracts;
mod error;
mod output;
mod platform;
mod report;

use std::io::Write;
use std::process::ExitCode;

/// Runs a private rustdoc probe or maps the normal application result to an exit status.
///
/// A valid Cargo-owned probe request short-circuits clap and returns its one-shot
/// probe status. Otherwise the normal parsed command runs through the single
/// executor boundary; command failures are written to standard error on a best-
/// effort basis and mapped to a failing status.
fn main() -> ExitCode {
    if let Some(exit_code) = compiler::RustdocProbe::run_if_requested() {
        return exit_code;
    }
    match futures_executor::block_on(app::App::from_env_and_run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{error:?}");
            ExitCode::FAILURE
        }
    }
}
