//! Contract Kit command-line interface.
//!
//! This binary crate owns the `conkit` executable surface: it parses command-line
//! arguments, walks source and contract directories, adapts filesystem bytes
//! into in-memory catalogs, and prints user-facing summaries. Reusable contract
//! semantics stay in the `conkit-signature` and `conkit-sketch` crates.
//!
//! The CLI currently exposes four command families:
//!
//! ```text
//! conkit check <all|signatures|sketches> --source <DIR> --contracts <DIR> --output <FILE> [--default|--strict|--warning]
//! conkit generate <all|signatures|sketches> --source <DIR> --contracts <DIR> [--adopt-existing]
//! conkit archive --contracts <DIR> --archive <DIR> [--gzip]
//! conkit diff --contracts <DIR> --archive <FILE>
//! ```
//!
//! `signature` and `sketch` are aliases for the plural targets. Omitting a
//! check mode selects the same behavior as `--default`. Gzip is the only
//! archive format, so both omitted and explicit `--gzip` select it. Sketches
//! are opt-in records linked from signatures in the same combined document;
//! the CLI asks the signature domain to resolve exact Rust-item seeds before
//! the independent sketch domain refreshes them.
//!
//! # Boundaries
//!
//! `conkit` is a binary adapter, not a reusable library API. It owns clap grammar,
//! process exit mapping, stdout output, report and archive file writes, and
//! portable path validation. Domain crates should continue to accept
//! catalogs and typed requests instead of depending on command-line parsing,
//! terminal output, or host filesystem paths.
#![deny(rustdoc::broken_intra_doc_links)]

mod app;
mod archive;
mod args;
mod catalog;
mod command;
mod context;
mod contracts;
mod error;
mod output;
mod platform;
mod report;

use std::io::Write;
use std::process::ExitCode;

/// Runs the parsed command and maps its result to the process exit status.
///
/// Command failures are written to standard error on a best-effort basis.
fn main() -> ExitCode {
    match futures_executor::block_on(app::App::from_env_and_run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{error:?}");
            ExitCode::FAILURE
        }
    }
}
