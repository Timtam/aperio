//! Plugin-side async runtime.
//!
//! Plugins implement their feature traits with `async fn` methods,
//! but the FFI boundary only carries synchronous function calls.
//! Each plugin therefore needs its own tokio runtime to `block_on`
//! its async work — the host's runtime can't be reused because
//! the host already invokes the plugin's FFI fn from inside its
//! own `spawn_blocking` (which runs on a dedicated worker thread,
//! detached from the runtime). Trying to enter the host's runtime
//! from there would panic.
//!
//! The runtime is a single-thread `current_thread` flavour because:
//!
//!   1. The plugin's FFI fn runs on exactly one thread (the
//!      host's blocking worker). Spinning up extra threads inside
//!      a single FFI call would waste resources.
//!   2. A current-thread runtime drives all spawned tasks on the
//!      same OS thread, so `block_on` won't deadlock even if the
//!      plugin's async work spawns sub-tasks (they're polled
//!      cooperatively).
//!   3. The plugin's own concurrency model (concurrent calls from
//!      different host threads) is already provided by the host
//!      calling the FFI fn from multiple `spawn_blocking` workers
//!      in parallel — each gets its own thread + its own
//!      `block_on` invocation against the shared runtime.
//!
//! Constructed once in [`super::PluginSingleton::init`] and
//! shared by every FFI fn the SDK generates.

use std::future::Future;

use tokio::runtime::{Builder, Runtime};

/// Owned tokio runtime configured for plugin use. Wraps the
/// inner [`Runtime`] in a tiny struct so callers don't have to
/// pick the right `block_on` flavour themselves + so swapping
/// the flavour later (current-thread → multi-thread) only
/// touches this file.
pub struct PluginRuntime {
    inner: Runtime,
}

impl PluginRuntime {
    /// Build a fresh runtime. Plugin authors don't usually call
    /// this directly — [`super::PluginSingleton::init`] does it
    /// once at plugin-create time.
    ///
    /// `enable_all` turns on the tokio IO + time drivers so
    /// plugins that need `tokio::time::sleep` or
    /// `reqwest`-style HTTP calls work out of the box. The
    /// runtime is lightweight enough that always-on doesn't
    /// cost meaningful resources even for plugins that don't
    /// use them.
    pub fn new() -> std::io::Result<Self> {
        let inner = Builder::new_current_thread().enable_all().build()?;
        Ok(Self { inner })
    }

    /// Drive `fut` to completion on this runtime. The shared
    /// host contract is that vtable methods MAY block for the
    /// duration of one network round-trip; the host always
    /// invokes the FFI fn from inside `spawn_blocking` so the
    /// host's reactor stays free.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.inner.block_on(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_simple_future() {
        let rt = PluginRuntime::new().expect("new");
        let answer = rt.block_on(async { 42 });
        assert_eq!(answer, 42);
    }

    #[test]
    fn block_on_supports_sleep() {
        let rt = PluginRuntime::new().expect("new");
        let answer = rt.block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            "ok"
        });
        assert_eq!(answer, "ok");
    }
}
