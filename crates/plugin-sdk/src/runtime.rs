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
//! Constructed once in [`super::PluginInstance::new`] and shared
//! by every FFI fn the SDK generates for that instance.
//!
//! ## Drop discipline
//!
//! Dropping a `tokio::runtime::Runtime` synchronously blocks the
//! current thread waiting for in-flight tasks to wind down — and
//! panics outright when the current thread is already serving
//! another tokio runtime (the "Cannot drop a runtime in a context
//! where blocking is not allowed" error). [`PluginRuntime`] sits
//! inside [`super::PluginInstance`], which the host drops from
//! its own async context (a Tauri command handler, a sync round
//! background task, …), so we MUST tear the runtime down via
//! [`tokio::runtime::Runtime::shutdown_background`] instead. The
//! shutdown happens off the current thread + doesn't block; any
//! in-flight task gets cancelled, which is the right shape since
//! by the time the instance is dropping the host has stopped
//! routing new calls to it.

use std::future::Future;

use tokio::runtime::{Builder, Runtime};

/// Owned tokio runtime configured for plugin use.
pub struct PluginRuntime {
    /// Wrapped in `Option` so [`Drop`] can take ownership +
    /// hand the runtime off to `shutdown_background`. Always
    /// `Some` for a live [`PluginRuntime`]; the `take()` only
    /// runs from [`Drop::drop`].
    inner: Option<Runtime>,
}

impl PluginRuntime {
    /// Build a fresh runtime. Plugin authors don't usually call
    /// this directly — [`super::PluginInstance::new`] does it
    /// once at instance-open time.
    pub fn new() -> std::io::Result<Self> {
        let inner = Builder::new_current_thread().enable_all().build()?;
        Ok(Self { inner: Some(inner) })
    }

    /// Drive `fut` to completion on this runtime. The shared
    /// host contract is that vtable methods MAY block for the
    /// duration of one network round-trip; the host always
    /// invokes the FFI fn from inside `spawn_blocking` so the
    /// host's reactor stays free.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.inner
            .as_ref()
            .expect("PluginRuntime accessed after drop")
            .block_on(fut)
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        if let Some(rt) = self.inner.take() {
            // `shutdown_background` consumes the runtime + spins
            // teardown off onto its own thread. The current
            // thread keeps moving immediately — critical because
            // the LoadedInstance Arc dropping us can be sitting
            // inside the host's async context where blocking on
            // a runtime drop would panic.
            rt.shutdown_background();
        }
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

    /// Dropping a PluginRuntime from inside a `#[tokio::test]`
    /// (i.e. an async context) MUST NOT panic. v1's PluginRuntime
    /// would have panicked here with "Cannot drop a runtime in a
    /// context where blocking is not allowed" — `shutdown_background`
    /// is the fix.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_from_within_async_context_does_not_panic() {
        // Construct + drop inside the multi-thread test runtime.
        // Calling block_on here would panic (you can't enter a
        // runtime from inside another one), so the test only
        // exercises the constructor + Drop — which is exactly
        // the symptom v1 displayed: dropping the runtime from
        // inside the host's async context aborted the whole
        // test binary.
        let rt = PluginRuntime::new().expect("new");
        drop(rt);
    }
}
