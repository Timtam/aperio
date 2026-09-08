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
use tracing::error;

use crate::error_map::{cal_error_to_response, sync_error_to_response, vc_error_to_response};
use crate::instance::PluginInstance;
use crate::response::{error_response, ok_empty_response, ok_response};

/// Run one plugin call, and let a panic in it cost the CALL rather than the app.
///
/// An `extern "C"` function that unwinds is a process abort — Rust inserts the
/// abort itself rather than letting the unwind cross the boundary. So a panic
/// anywhere in a vtable method takes the whole of Aperio down: every other
/// account, mid-sentence, with nothing on screen and nothing in the log saying
/// which adapter did it. An `unwrap` on a field a server stopped sending is
/// enough.
///
/// Caught here it becomes one call returning `PLUGIN_CALL_ERR_INTERNAL`, which
/// is a state the host already handles everywhere — the account reports an
/// error and the rest of the app carries on.
///
/// # What it costs, honestly
///
/// The adapter's own state may be inconsistent afterwards. That is the price of
/// not aborting, and it is the trade Rust's own FFI guidance makes: the
/// instance stays alive and the user can reconnect that one account. Aborting
/// would have taken the same inconsistent state down along with eleven adapters
/// that were fine.
///
/// # Why the SDK and not the plugin author
///
/// "Do not panic" is not a contract anyone can audit their way into keeping.
/// The boundary is the one place the rule can actually be enforced, and it is
/// the same rule the host already applies to its own callback in the other
/// direction (`plugin_core`'s `forward_host_event`).
///
/// # In a release build of a bundled plugin this does nothing
///
/// The workspace sets `panic = "abort"` in `[profile.release]`, so there is no
/// unwinding left to catch. It earns its keep exactly where that profile does
/// not reach: a debug build, and an adapter built in its OWN repository — where
/// a workspace profile does not travel and unwind is cargo's default.
fn catching_panics(call: impl FnOnce() -> PluginCallResult) -> PluginCallResult {
    // AssertUnwindSafe: the closure holds a borrow of the adapter and the
    // runtime, neither of which is read again on this path once the panic is
    // caught — the response is built from the payload alone and the call is
    // over. The state the assertion waives is the adapter's own, and the
    // paragraph above is the decision about it.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(response) => response,
        Err(payload) => {
            // `&*payload`, not `&payload`. A `Box<dyn Any + Send>` is itself a
            // concrete `Any`, so `&payload` unsize-coerces the BOX and every
            // downcast below misses — which reads as "no message" for every
            // panic there has ever been, losing the one clue to where it
            // happened. The test asserts the text for this reason.
            let what = panic_text(&*payload);
            // Into the host log, not just stderr: on the desktop the plugin's
            // tracing is forwarded to `aperio.log`, and a log line is how this
            // is diagnosed by someone who cannot see a console.
            error!(panic = %what, "adapter panicked during a plugin call");
            error_response(
                PLUGIN_CALL_ERR_INTERNAL,
                &format!("the adapter panicked: {what}"),
            )
        }
    }
}

/// What a caught panic said, for the message the user's log will carry.
///
/// `panic!("…")` with a literal boxes a `&str`; with arguments, a `String`.
/// Anything else is a payload no formatter can read, and saying so beats an
/// empty message.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "no message".to_string()
    }
}

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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
    catching_panics(|| match inst.runtime().block_on(call(p_static)) {
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
