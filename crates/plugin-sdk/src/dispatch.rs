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

use crate::error_map::{
    cal_error_to_response, sync_error_to_response, vc_error_to_response,
};
use crate::instance::PluginInstance;
use crate::response::{error_response, ok_empty_response, ok_response};

/// Borrow the per-instance handle, returning a typed
/// [`PluginInstance`] reference or an internal error response
/// when the handle is NULL.
pub fn instance<'a, A>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<A>, PluginCallResult> {
    unsafe { PluginInstance::<A>::from_handle(handle) }
        .ok_or_else(|| {
            error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle")
        })
}

/// Drive a calendar/tasks/contacts trait method through the
/// plugin's runtime and marshal `cal_core::Result<T>` into a
/// `PluginCallResult`.
///
/// The closure receives a `&'static A` so it can build async
/// futures that don't carry a borrow back into the macro
/// expansion — the static lifetime is sound because the borrow
/// exits before `block_on` returns.
pub fn cal_dispatch<A, T, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
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
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

/// Unit-returning sibling of [`cal_dispatch`].
pub fn cal_dispatch_unit<A, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = cal_core::error::Result<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => cal_error_to_response(e),
    }
}

/// Sync-adapter counterpart to [`cal_dispatch`] — drives a
/// `SyncResult<T>`-returning trait method through the plugin's
/// runtime.
pub fn sync_dispatch<A, T, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
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
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => sync_error_to_response(e),
    }
}

/// Unit-returning sibling of [`sync_dispatch`].
pub fn sync_dispatch_unit<A, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = sync_core::SyncResult<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => sync_error_to_response(e),
    }
}

/// VC-adapter counterpart to [`cal_dispatch`] — drives a
/// `VcResult<T>`-returning trait method through the plugin's
/// runtime.
pub fn vc_dispatch<A, T, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
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
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => vc_error_to_response(e),
    }
}

/// Unit-returning sibling of [`vc_dispatch`].
pub fn vc_dispatch_unit<A, F, Fut>(
    handle: *mut c_void,
    call: F,
) -> PluginCallResult
where
    A: 'static,
    F: FnOnce(&'static A) -> Fut,
    Fut: Future<Output = vc_core::VcResult<()>>,
{
    let inst = match instance::<A>(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static A =
        unsafe { std::mem::transmute::<&A, &'static A>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => vc_error_to_response(e),
    }
}
