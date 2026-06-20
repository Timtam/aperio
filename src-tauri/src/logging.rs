//! Desktop logging glue (§ Diagnostics).
//!
//! The file-sink builders + the read/export/redaction helpers now live in
//! [`host_core::logging`] (shared with the mobile cal-ffi Host). This is the
//! desktop assembly: it builds the global `tracing` subscriber (a console sink +
//! the rolling file sink, both behind one reloadable `EnvFilter`), owns the
//! Tauri-managed [`LogState`] (which keeps the writer's `WorkerGuard` alive),
//! and installs the panic hook. One source of truth for the pure logic; the
//! `.init()` global one-shot stays here because it's desktop glue.

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*};

// Re-export the shared surface so existing `crate::logging::*` references
// (commands/logs.rs) keep resolving.
pub use host_core::logging::{clear, collect, recent_lines, ReloadHandle, DEFAULT_LEVEL};

/// Owns the pieces the rest of the app must hold onto: the non-blocking
/// writer's `WorkerGuard` (dropping it stops the background flush thread) and
/// the reload handle for live verbosity changes. Stored in Tauri state.
pub struct LogState {
    /// `<data_dir>/logs` — where the rolling files live.
    pub logs_dir: PathBuf,
    handle: ReloadHandle,
    /// Kept alive for the process lifetime; never read directly.
    _guard: WorkerGuard,
}

impl LogState {
    /// Swap the active filter directive (e.g. `"info"`, `"debug"`). An invalid
    /// directive falls back to [`DEFAULT_LEVEL`] rather than silencing logs.
    pub fn set_filter(&self, directive: &str) {
        host_core::logging::reload_filter(&self.handle, directive);
    }
}

/// Initialise tracing with a console sink + a daily rolling file sink under
/// `<data_dir>/logs/`, both behind one reloadable `EnvFilter` (built by
/// host-core). The initial level is `RUST_LOG` or [`DEFAULT_LEVEL`]; the caller
/// applies the persisted user choice once the DB is open via
/// [`LogState::set_filter`]. Returns the state the app must `manage()` (and keep
/// alive — the writer guard lives inside it).
pub fn init(data_dir: &Path) -> LogState {
    let logs_dir = host_core::logging::prepare_logs_dir(data_dir);
    let (filter_layer, handle) =
        host_core::logging::build_reloadable_filter(&host_core::logging::initial_directive());
    let (file_writer, guard) = host_core::logging::build_file_appender(&logs_dir);

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt::layer())
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .init();

    LogState {
        logs_dir,
        handle,
        _guard: guard,
    }
}

/// Route Rust panics into the logs so a user hitting a hard crash has something
/// to send. Thin wrapper over [`host_core::logging::install_panic_hook`] —
/// passes the desktop app's version (its own `CARGO_PKG_VERSION`) so the crash
/// report is tagged with the right build.
pub fn install_panic_hook(logs_dir: PathBuf) {
    host_core::logging::install_panic_hook(logs_dir, env!("CARGO_PKG_VERSION"));
}
