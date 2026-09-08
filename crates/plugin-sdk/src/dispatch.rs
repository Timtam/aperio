//! Shared per-instance handle dispatch helpers.
//!
//! Every plugin used to define a near-identical local triplet —
//! `instance`, `dispatch`, `dispatch_unit` — wrapping the
//! per-instance handle, blocking on the plugin's tokio runtime,
//! and marshalling the typed result into a `PluginCallResult`.
//! The only difference between any two plugins' copies was the
//! adapter type they specialised on. This module hoists those
//! helpers into plugin-sdk as generics so the per-plugin
//! boilerplate stops being copy-pasted.
//!
//! ## Two error-domain flavours
//!
//! Calendar / tasks / contacts adapters return
//! [`cal_core::error::Result`]; sync adapters return
//! [`sync_core::SyncResult`]. Each gets its own pair
//! (`cal_dispatch` / `cal_dispatch_unit` vs. `sync_dispatch`
//! / `sync_dispatch_unit`) so the per-call type inference
//! stays sharp + the error mapper is wired correctly.
//!
//! ## Lifetime extension
//!
//! All dispatch fns transmute `&A` to `&'static A` before
//! handing it to the user's async closure. This is sound
//! because the borrow exits before `block_on` returns —
//! `PluginInstance::plugin()` owns the adapter, and we don't
//! touch the borrow after the future completes.

use std::future::Future;
use std::os::raw::c_void;

use plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};

use crate::error_map::{cal_error_to_response, sync_error_to_response, vc_error_to_response};
use crate::instance::PluginInstance;
use crate::panic_guard::guarded;
use crate::response::{error_response, ok_empty_response, ok_response};

/// Borrow the per-instance handle, returning a typed
/// [`PluginInstance`] reference or an internal error response
/// when the handle is NULL.
///
/// The fn dereferences a raw pointer but stays safe by contract:
/// every call site is itself an `unsafe extern "C"` FFI shim that
/// just got the pointer from the host's `open_instance` round-
/// trip — the host promises the pointer is either a valid
/// `PluginInstance<A>` or NULL. `from_handle` returns `None` on
/// NULL; non-NULL is dereferenced inside an `unsafe` block.
/// Marking this fn itself `unsafe` would force every call site
/// to wrap it in another `unsafe` block for no diagnostic gain.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn instance<'a, A>(handle: *mut c_void) -> Result<&'a PluginInstance<A>, PluginCallResult> {
    unsafe { PluginInstance::<A>::from_handle(handle) }
        .ok_or_else(|| error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle"))
}

/// Drive a calendar/tasks/contacts trait method through the
/// plugin's runtime and marshal `cal_core::Result<T>` into a
/// `PluginCallResult`.
///
/// The closure receives a `&'static A` so it can build async
/// futures that don't carry a borrow back into the macro
/// expansion — the static lifetime is sound because the borrow
/// exits before `block_on` returns.
pub fn cal_dispatch<A, T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = cal_core::error::Result<T>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    })
}

/// Unit-returning sibling of [`cal_dispatch`].
pub fn cal_dispatch_unit<A, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = cal_core::error::Result<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => cal_error_to_response(e),
    })
}

/// Sync-adapter counterpart to [`cal_dispatch`] — drives a
/// `SyncResult<T>`-returning trait method through the plugin's
/// runtime.
pub fn sync_dispatch<A, T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = sync_core::SyncResult<T>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => sync_error_to_response(e),
    })
}

/// Unit-returning sibling of [`sync_dispatch`].
pub fn sync_dispatch_unit<A, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = sync_core::SyncResult<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => sync_error_to_response(e),
    })
}

/// VC-adapter counterpart to [`cal_dispatch`] — drives a
/// `VcResult<T>`-returning trait method through the plugin's
/// runtime.
pub fn vc_dispatch<A, T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    A: 'static,
    T: serde::Serialize,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = vc_core::VcResult<T>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => vc_error_to_response(e),
    })
}

/// Unit-returning sibling of [`vc_dispatch`].
pub fn vc_dispatch_unit<A, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = vc_core::VcResult<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A = unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    guarded(|| match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => vc_error_to_response(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_core::ffi::PLUGIN_CALL_OK;

    struct Adapter;

    /// The payload bytes of a result, so the message can be read back. Frees
    /// the buffer the way the host would.
    fn payload_of(result: &PluginCallResult) -> String {
        if result.payload.data.is_null() {
            return String::new();
        }
        // SAFETY: `bytes_to_response` allocated this as a boxed slice of
        // exactly `len` bytes, and nothing else has taken it.
        let text = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.payload.data,
                result.payload.len,
            ))
            .into_owned()
        };
        if let Some(free) = result.payload.free {
            // SAFETY: same allocation, freed once, with the plugin's own
            // deallocator — exactly what the host does with it.
            unsafe { free(result.payload.data, result.payload.len) };
        }
        text
    }

    /// Without the catch this test does not fail — it ABORTS the test binary,
    /// which is the whole point: that is what a panicking adapter does to
    /// Aperio.
    #[test]
    fn a_panicking_adapter_costs_its_call_and_not_the_process() {
        let handle = PluginInstance::new(Adapter)
            .expect("runtime")
            .into_raw_handle();

        let result = cal_dispatch::<Adapter, (), _, _>(handle, |_| async {
            panic!("the server sent a field I unwrapped");
        });

        assert_eq!(result.status, PLUGIN_CALL_ERR_INTERNAL);
        let message = payload_of(&result);
        assert!(
            message.contains("panicked"),
            "the caller has to be able to tell a panic from an ordinary \
             failure, got: {message}",
        );
        assert!(
            message.contains("the server sent a field I unwrapped"),
            "the panic's own message is the only clue to WHERE it happened, \
             got: {message}",
        );

        // SAFETY: the handle came from `into_raw_handle` just above and has
        // not been reclaimed.
        unsafe { PluginInstance::<Adapter>::drop_handle(handle) };
    }

    /// The instance survives a panic, because the host will keep using it —
    /// the account reports an error and the next call is expected to work.
    #[test]
    fn the_instance_still_answers_after_a_panic() {
        let handle = PluginInstance::new(Adapter)
            .expect("runtime")
            .into_raw_handle();

        let _ = cal_dispatch::<Adapter, (), _, _>(handle, |_| async { panic!("first call") });
        let second = cal_dispatch::<Adapter, u32, _, _>(handle, |_| async { Ok(7) });

        assert_eq!(second.status, PLUGIN_CALL_OK);
        assert_eq!(payload_of(&second), "7");

        // SAFETY: as above.
        unsafe { PluginInstance::<Adapter>::drop_handle(handle) };
    }

    #[test]
    fn a_panic_with_no_readable_payload_still_produces_a_message() {
        let handle = PluginInstance::new(Adapter)
            .expect("runtime")
            .into_raw_handle();

        let result = cal_dispatch::<Adapter, (), _, _>(handle, |_| async {
            std::panic::panic_any(42u8);
        });

        assert_eq!(result.status, PLUGIN_CALL_ERR_INTERNAL);
        assert!(payload_of(&result).contains("no message"));

        // SAFETY: as above.
        unsafe { PluginInstance::<Adapter>::drop_handle(handle) };
    }
}
