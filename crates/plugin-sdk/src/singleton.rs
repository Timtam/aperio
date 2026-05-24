//! Process-singleton holder for the plugin instance + runtime.
//!
//! The plugin ABI says `aperio_plugin_create` returns a process-
//! lifetime singleton (one instance per loaded library). The SDK's
//! generated FFI fn pointers can't capture state, so they look up
//! the plugin + runtime through a `static` [`PluginSingleton<T>`]
//! variable the macro defines in the plugin crate.
//!
//! `PluginSingleton::new` is `const`, so the plugin declares the
//! holder as a `static`. Init runs once from inside the
//! lifecycle `init()` callback; thereafter every FFI vtable fn
//! reads through [`PluginSingleton::get`] / [`PluginSingleton::runtime`].

use std::sync::OnceLock;

use crate::runtime::PluginRuntime;

/// Holds (plugin instance, runtime) as a single `OnceLock`-backed
/// pair so the two always have the same lifetime. Use a single
/// `OnceLock<(T, PluginRuntime)>` rather than two separate ones
/// to avoid a race where a vtable fn reads `get()` between the
/// runtime init and the plugin init.
///
/// `T` is the plugin author's adapter type; they implement the
/// feature traits (`CalendarFeature`, etc.) on it. The SDK
/// stores it by value here so the singleton holds the full
/// adapter state (HTTP clients, caches, …) for the life of the
/// process.
pub struct PluginSingleton<T> {
    inner: OnceLock<Slot<T>>,
}

struct Slot<T> {
    plugin: T,
    runtime: PluginRuntime,
}

impl<T> Default for PluginSingleton<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> PluginSingleton<T> {
    /// Empty holder. Const so it can be used as a `static`.
    pub const fn new() -> Self {
        Self {
            inner: OnceLock::new(),
        }
    }

    /// One-shot initialisation. Called from the lifecycle
    /// `init` hook (or directly from `aperio_plugin_create` if
    /// the plugin doesn't need deferred init).
    ///
    /// Returns `Err` if the singleton was already initialised
    /// (a buggy host that called `init` twice without going
    /// through `destroy` first) or if building the runtime
    /// failed.
    pub fn init(&self, plugin: T) -> Result<(), InitError> {
        let runtime = PluginRuntime::new().map_err(InitError::Runtime)?;
        let slot = Slot { plugin, runtime };
        self.inner
            .set(slot)
            .map_err(|_| InitError::AlreadyInitialised)?;
        Ok(())
    }

    /// Borrow the plugin instance. Returns `None` if `init`
    /// hasn't run — every SDK-generated FFI fn checks this and
    /// returns a `PLUGIN_CALL_ERR_INTERNAL` response on the
    /// rare "called before init" path.
    pub fn get(&self) -> Option<&T> {
        self.inner.get().map(|s| &s.plugin)
    }

    /// Borrow the plugin's runtime. Same `None` semantics as
    /// [`Self::get`].
    pub fn runtime(&self) -> Option<&PluginRuntime> {
        self.inner.get().map(|s| &s.runtime)
    }

    /// Convenience that returns both at once. Used by the
    /// generated vtable fn pointers so the "init not called"
    /// branch only has to be tested once per call.
    pub fn parts(&self) -> Option<(&T, &PluginRuntime)> {
        self.inner.get().map(|s| (&s.plugin, &s.runtime))
    }
}

/// Reasons [`PluginSingleton::init`] can fail. The lifecycle
/// `init` hook converts these into the matching
/// `APERIO_PLUGIN_ERR_*` return code.
#[derive(Debug)]
pub enum InitError {
    /// Couldn't build the tokio runtime (rare — usually OOM).
    Runtime(std::io::Error),
    /// Singleton was already initialised. The host should never
    /// trigger this on a well-behaved plugin lifecycle, but
    /// catching it here gives a clear error instead of a panic
    /// on the second `OnceLock::set`.
    AlreadyInitialised,
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "build runtime: {err}"),
            Self::AlreadyInitialised => write!(f, "plugin singleton already initialised"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(err) => Some(err),
            Self::AlreadyInitialised => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holder starts empty: get() and runtime() return None
    /// before init runs.
    #[test]
    fn empty_holder_returns_none() {
        let s: PluginSingleton<u32> = PluginSingleton::new();
        assert!(s.get().is_none());
        assert!(s.runtime().is_none());
        assert!(s.parts().is_none());
    }

    /// One init call lights up all three accessors.
    #[test]
    fn init_lights_up_accessors() {
        let s: PluginSingleton<u32> = PluginSingleton::new();
        s.init(42).expect("init");
        assert_eq!(s.get(), Some(&42));
        assert!(s.runtime().is_some());
        assert!(s.parts().is_some());
    }

    /// A second init call is rejected.
    #[test]
    fn double_init_errors() {
        let s: PluginSingleton<u32> = PluginSingleton::new();
        s.init(1).expect("first");
        let err = s.init(2).unwrap_err();
        assert!(matches!(err, InitError::AlreadyInitialised));
        // First value is still the active one.
        assert_eq!(s.get(), Some(&1));
    }
}
