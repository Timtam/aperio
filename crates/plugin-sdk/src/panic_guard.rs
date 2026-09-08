//! One rule, applied at every boundary this SDK owns: a panic costs the CALL,
//! never the process.
//!
//! It lives in its own module because it is not a dispatch concern. Every
//! `extern "C"` entry point a Rust plugin exposes needs it — the vtable slots
//! through [`crate::dispatch`], but equally `open_instance`, `discover`,
//! `interactive_auth`, `probe_host_key` and `strings`, each of which runs the
//! plugin author's own closure. The first version of this guard wrapped only
//! the six dispatch helpers while the header and the plugin docs promised the
//! whole boundary; that gap is what this module exists to close.

use plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use tracing::error;

use crate::response::error_response;

/// Run one plugin call, and let a panic in it cost the CALL rather than the app.
///
/// Exported for the vtable slots a plugin writes by hand. Most go through
/// [`crate::dispatch`], which already wraps them; the synchronous ones
/// (`capabilities`, `calendar_color`, `fetch_sound_asset`) marshal their own
/// result and so have to say it themselves:
///
/// ```ignore
/// unsafe extern "C" fn ffi_capabilities(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
///     plugin_sdk::guarded(|| { /* … */ })
/// }
/// ```
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
pub fn guarded(call: impl FnOnce() -> PluginCallResult) -> PluginCallResult {
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

/// [`catching_panics`] for an entry point that does not return a
/// [`PluginCallResult`].
///
/// `open_instance` answers with an `OpenInstanceResult`, and the lifecycle
/// exports with nothing at all, so each needs its own way of saying "that
/// failed". The catching is identical; only the shape of the answer differs.
pub fn catching_panics_into<T>(
    what: &str,
    on_panic: impl FnOnce(String) -> T,
    call: impl FnOnce() -> T,
) -> T {
    // AssertUnwindSafe for the same reason and with the same caveat as above.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
        Ok(value) => value,
        Err(payload) => {
            let text = panic_text(&*payload);
            error!(panic = %text, entry_point = what, "adapter panicked");
            on_panic(text)
        }
    }
}

/// [`guarded`] for an entry point that returns nothing.
///
/// `close_instance` and the lifecycle exports have no channel to report a
/// failure through, so a caught panic is logged and swallowed. That is not a
/// loss: the alternative is aborting the process during teardown, which
/// destroys the very state the user was about to keep.
pub fn guarded_void(what: &str, call: impl FnOnce()) {
    catching_panics_into(what, |_| (), call)
}
