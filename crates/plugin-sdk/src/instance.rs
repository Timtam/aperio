//! Per-account plugin instance — boxed handle the FFI boundary
//! sees as `*mut c_void`.
//!
//! Replaces the v1-era `PluginSingleton<T>` (one instance per
//! loaded library). ABI v2 (DESIGN.md §6.4) lets a single loaded
//! library back N independent adapter instances — each carrying
//! its own per-account state — by handing the host an opaque
//! handle from `open_instance` and threading that handle through
//! every vtable method.
//!
//! The canonical layout:
//!
//!   - `Instance<T>` bundles the plugin's adapter `T` with a
//!     dedicated `PluginRuntime`. One per opened account.
//!   - `Instance::<T>::into_raw_handle()` boxes the instance +
//!     leaks the `Box` so the resulting `*mut c_void` outlives
//!     every vtable call. The host stores it and passes it back
//!     into every method.
//!   - `Instance::<T>::from_handle(h)` borrows the instance
//!     back inside an FFI fn so the dispatcher can reach the
//!     adapter + runtime.
//!   - `Instance::<T>::drop_handle(h)` is the canonical
//!     `close_instance` body — reclaims the box + drops the
//!     adapter, which in turn releases any HTTP clients,
//!     caches, OAuth tokens, …

use std::os::raw::c_void;

use crate::runtime::PluginRuntime;

/// Owned per-account instance. Constructed inside the plugin's
/// `open_instance` hook and immediately handed to
/// [`Self::into_raw_handle`]; the host stores the returned
/// pointer and passes it back into every vtable call.
pub struct PluginInstance<T> {
    plugin: T,
    runtime: PluginRuntime,
}

impl<T> PluginInstance<T> {
    /// Build a fresh instance + runtime around the user's
    /// adapter value. Returns [`InitError::Runtime`] if the
    /// runtime construction fails (rare — usually OOM).
    pub fn new(plugin: T) -> Result<Self, InitError> {
        let runtime = PluginRuntime::new().map_err(InitError::Runtime)?;
        Ok(Self { plugin, runtime })
    }

    /// Borrow the adapter value. Used by the plugin's vtable fn
    /// dispatcher after it borrows the instance back from the
    /// FFI handle.
    pub fn plugin(&self) -> &T {
        &self.plugin
    }

    /// Borrow the runtime. The dispatcher uses this to `block_on`
    /// the adapter's async trait method inside the synchronous
    /// FFI fn body.
    pub fn runtime(&self) -> &PluginRuntime {
        &self.runtime
    }

    /// Convenience — same shape as v1's `PluginSingleton::parts`.
    /// The dispatcher reads both at once so the "instance not
    /// initialised" branch only has to be tested once per call.
    pub fn parts(&self) -> (&T, &PluginRuntime) {
        (&self.plugin, &self.runtime)
    }

    /// Box the instance and leak it across the FFI boundary as
    /// `*mut c_void`. The host stores the pointer and passes it
    /// back to every vtable method on this instance; on
    /// shutdown the host calls the descriptor's `close_instance`
    /// hook with the same pointer to free it.
    pub fn into_raw_handle(self) -> *mut c_void {
        Box::into_raw(Box::new(self)) as *mut c_void
    }

    /// Reclaim a handle previously returned by
    /// [`Self::into_raw_handle`] and drop the instance — which
    /// in turn drops the adapter (releasing HTTP clients, OAuth
    /// tokens, caches, …) and the runtime.
    ///
    /// NULL is treated as a no-op so the descriptor's
    /// `close_instance` hook can defensively call this without
    /// branching.
    ///
    /// # Safety
    ///
    /// `handle` must have been produced by [`Self::into_raw_handle`]
    /// on the same `T` and must not be used after this call.
    /// The host's [`plugin_core::manager::LoadedInstance::drop`]
    /// guarantees the pointer is exactly what `open_instance`
    /// returned and is only freed once.
    pub unsafe fn drop_handle(handle: *mut c_void) {
        if handle.is_null() {
            return;
        }
        let _ = Box::from_raw(handle as *mut Self);
    }

    /// Borrow the instance back from an FFI handle. Returns
    /// `None` when `handle` is NULL — the dispatcher then
    /// surfaces `PLUGIN_CALL_ERR_INTERNAL` to the host.
    ///
    /// # Safety
    ///
    /// `handle` must be a live pointer produced by
    /// [`Self::into_raw_handle`] for the same `T`. The returned
    /// reference is valid as long as the host hasn't yet called
    /// `close_instance` on the same handle — which the host
    /// guarantees by holding the `LoadedInstance` Arc that owns
    /// the pointer for the duration of every method call.
    pub unsafe fn from_handle<'a>(handle: *mut c_void) -> Option<&'a Self> {
        if handle.is_null() {
            return None;
        }
        Some(&*(handle as *const Self))
    }
}

/// Reasons [`PluginInstance::new`] can fail. The plugin author
/// wraps this in the matching `APERIO_PLUGIN_ERR_*` code via
/// [`crate::open_instance::open_instance_with`].
#[derive(Debug)]
pub enum InitError {
    /// Couldn't build the tokio runtime (rare — usually OOM).
    Runtime(std::io::Error),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "build runtime: {err}"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(err) => Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_handle_preserves_plugin_value() {
        let inst = PluginInstance::new(42u32).expect("new");
        let handle = inst.into_raw_handle();
        // SAFETY: handle came from into_raw_handle::<u32> on the
        // line above; not yet freed.
        let borrowed = unsafe { PluginInstance::<u32>::from_handle(handle) }
            .expect("non-null");
        assert_eq!(*borrowed.plugin(), 42);
        // SAFETY: same — drop reclaims the box exactly once.
        unsafe { PluginInstance::<u32>::drop_handle(handle) };
    }

    #[test]
    fn null_handle_borrow_returns_none() {
        let r: Option<&PluginInstance<u32>> =
            unsafe { PluginInstance::<u32>::from_handle(std::ptr::null_mut()) };
        assert!(r.is_none());
    }

    #[test]
    fn drop_null_handle_is_a_noop() {
        unsafe { PluginInstance::<u32>::drop_handle(std::ptr::null_mut()) };
    }

    #[test]
    fn parts_returns_both_slots() {
        let inst = PluginInstance::new("hello".to_string()).expect("new");
        let (plug, _rt) = inst.parts();
        assert_eq!(plug, "hello");
    }
}
