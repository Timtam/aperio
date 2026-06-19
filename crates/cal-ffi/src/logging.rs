//! Mobile logging glue (§ Diagnostics).
//!
//! The desktop owns a Tauri-managed `LogState`; the mobile cal-ffi `Host` can be
//! constructed multiple times (the test binary opens many), while `tracing`'s
//! global subscriber installs exactly once per process. So the state is a
//! process-global behind a `Once` — NOT a `Host` field — and the writer's
//! `WorkerGuard` lives for the process lifetime. The pure pieces (the rolling
//! file sink + reloadable filter builders, the read/export/redaction helpers)
//! come from [`host_core::logging`]; this just assembles + installs a file-only
//! subscriber (no console layer — Android stdout isn't logcat anyway) and caches
//! the reload handle for live level changes.

use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*};

use host_core::logging::{
    build_file_appender, build_reloadable_filter, initial_directive, prepare_logs_dir,
    reload_filter, ReloadHandle,
};

struct MobileLogState {
    logs_dir: PathBuf,
    handle: ReloadHandle,
    /// Kept alive for the process lifetime; dropping it stops the flush thread.
    _guard: WorkerGuard,
}

static LOG_STATE: OnceLock<MobileLogState> = OnceLock::new();
static LOG_INIT: Once = Once::new();

/// Install the rolling-file log sink under `<data_dir>/logs` at most once per
/// process. Multiple `Host::open` calls (the test path opens many) must NOT
/// double-install the global subscriber — `Once` guarantees a single install,
/// and `try_init` is belt-and-suspenders if some embedder already set one. The
/// file sink binds to the first caller's `data_dir` (on a device there's one
/// stable dir; tests use tempdirs and don't assert on the file).
pub fn init_mobile_logging(data_dir: &Path) {
    LOG_INIT.call_once(|| {
        let logs_dir = prepare_logs_dir(data_dir);
        let (filter_layer, handle) = build_reloadable_filter(&initial_directive());
        let (file_writer, guard) = build_file_appender(&logs_dir);
        let _ = tracing_subscriber::registry()
            .with(filter_layer)
            .with(fmt::layer().with_ansi(false).with_writer(file_writer))
            .try_init();
        let _ = LOG_STATE.set(MobileLogState {
            logs_dir,
            handle,
            _guard: guard,
        });
    });
}

/// The logs directory, once logging has been initialised.
pub fn logs_dir() -> Option<&'static Path> {
    LOG_STATE.get().map(|s| s.logs_dir.as_path())
}

/// Apply a level directive to the live filter (no-op if not yet initialised).
pub fn set_level(level: &str) {
    if let Some(s) = LOG_STATE.get() {
        reload_filter(&s.handle, level);
    }
}
