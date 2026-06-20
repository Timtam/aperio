//! Forward a dlopen'd plugin's `tracing` events to the host log.
//!
//! Each plugin shared library statically links its own copy of
//! `tracing` with an independent global dispatcher, so a plugin's
//! `tracing::warn!` would otherwise vanish: the host's subscriber
//! lives in the host binary's `tracing` global, which the plugin's
//! code can't reach. [`install_log_forwarding`] closes that gap by
//! setting the *plugin's* global default to a subscriber that
//! re-emits every event through a host-supplied C-ABI callback
//! ([`AperioLogFn`]); the host then re-emits it into its own
//! subscriber (the log file).
//!
//! The host calls the plugin's `aperio_plugin_set_log` export (wired
//! by [`crate::declare_cdylib_exports!`]) once, right after `create`.
//! On the static-link (mobile) path the plugin shares the host's
//! `tracing` global already, the cdylib shell isn't built, and so
//! `set_log` is never called and this module is inert.

use std::ffi::CString;
use std::fmt;
use std::sync::OnceLock;

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use crate::plugin_core::abi::{
    AperioLogFn, LOG_LEVEL_DEBUG, LOG_LEVEL_ERROR, LOG_LEVEL_INFO, LOG_LEVEL_TRACE, LOG_LEVEL_WARN,
};

/// The host's log sink, stored once when forwarding is installed.
static LOG_FN: OnceLock<AperioLogFn> = OnceLock::new();

/// Install a `tracing` subscriber on the plugin's global dispatcher
/// that forwards events to the host via `cb`.
///
/// Idempotent: only the first call wins. The host invokes it exactly
/// once per loaded library, but a double-call (or a plugin that has
/// already set its own global default) is harmless — we never
/// overwrite an existing dispatcher.
///
/// Storing the pointer is safe; the unsafety lives at the call site
/// in [`ForwardSubscriber::event`], which trusts `cb` to be a valid
/// `'static` host function (the install contract).
pub fn install_log_forwarding(cb: AperioLogFn) {
    if LOG_FN.set(cb).is_err() {
        return; // already installed
    }
    // set_global_default sets THIS dylib's tracing global, NOT the
    // host's. Ignore the error so we never clobber a default the
    // plugin may have set for its own reasons.
    let _ = tracing::subscriber::set_global_default(ForwardSubscriber);
}

/// Map a `tracing::Level` to the ABI's `LOG_LEVEL_*` wire byte.
fn level_byte(level: &Level) -> u8 {
    match *level {
        Level::ERROR => LOG_LEVEL_ERROR,
        Level::WARN => LOG_LEVEL_WARN,
        Level::INFO => LOG_LEVEL_INFO,
        Level::DEBUG => LOG_LEVEL_DEBUG,
        Level::TRACE => LOG_LEVEL_TRACE,
    }
}

/// Minimal `Subscriber` that forwards events and ignores spans. We
/// only carry log records across the boundary; tracking span
/// enter/exit would add cost for data the host doesn't re-emit.
struct ForwardSubscriber;

impl Subscriber for ForwardSubscriber {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        // Generate down to DEBUG; skip TRACE so a hot adapter loop
        // can't flood the boundary. The host's own level filter is
        // the real gate — it decides what actually lands in the log
        // when we re-emit there.
        *meta.level() <= Level::DEBUG
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        // Lets tracing statically short-circuit TRACE callsites
        // without consulting `enabled`.
        Some(LevelFilter::DEBUG)
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        // Spans aren't tracked; hand back a stable non-zero id.
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let Some(cb) = LOG_FN.get() else {
            return;
        };
        let meta = event.metadata();
        let mut buf = String::new();
        event.record(&mut MessageVisitor { out: &mut buf });
        let level = level_byte(meta.level());
        // CString::new fails only on an interior NUL — vanishingly
        // rare in a log line; fall back to empty rather than panic.
        let target = CString::new(meta.target()).unwrap_or_default();
        let message = CString::new(buf).unwrap_or_default();
        // SAFETY: `cb` is the host's valid `'static` fn pointer (the
        // install contract); the CStrings outlive this synchronous
        // call.
        unsafe {
            cb(level, target.as_ptr(), message.as_ptr());
        }
    }

    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Renders an event's fields into `out`: the `message` field bare,
/// every other field as ` name=value`. Implementing only
/// `record_debug` suffices — the typed `record_*` methods default to
/// delegating here.
struct MessageVisitor<'a> {
    out: &'a mut String,
}

impl Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            let _ = write!(self.out, "{value:?}");
        } else {
            let _ = write!(self.out, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Mutex;

    // Captured by the fake host sink so the test can assert what the
    // forwarding subscriber rendered + handed across the boundary.
    static LAST_LEVEL: AtomicU8 = AtomicU8::new(0);
    static LAST_MESSAGE: Mutex<String> = Mutex::new(String::new());
    static LAST_TARGET: Mutex<String> = Mutex::new(String::new());

    unsafe extern "C" fn capture(level: u8, target: *const c_char, message: *const c_char) {
        LAST_LEVEL.store(level, Ordering::SeqCst);
        let t = unsafe { std::ffi::CStr::from_ptr(target) }
            .to_string_lossy()
            .into_owned();
        let m = unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        *LAST_TARGET.lock().unwrap() = t;
        *LAST_MESSAGE.lock().unwrap() = m;
    }

    #[test]
    fn level_mapping_matches_abi() {
        assert_eq!(level_byte(&Level::ERROR), LOG_LEVEL_ERROR);
        assert_eq!(level_byte(&Level::WARN), LOG_LEVEL_WARN);
        assert_eq!(level_byte(&Level::INFO), LOG_LEVEL_INFO);
        assert_eq!(level_byte(&Level::DEBUG), LOG_LEVEL_DEBUG);
        assert_eq!(level_byte(&Level::TRACE), LOG_LEVEL_TRACE);
    }

    #[test]
    fn forwards_an_event_through_the_callback() {
        // Install the capture sink + the forwarding subscriber on a
        // *scoped* (thread-local) dispatcher so the assertion is
        // deterministic and doesn't fight other tests' subscribers.
        // This exercises the full path: real `tracing::warn!` →
        // ForwardSubscriber::event → MessageVisitor → the C callback.
        let _ = LOG_FN.set(capture);
        tracing::subscriber::with_default(ForwardSubscriber, || {
            tracing::warn!(target: "aperio::test", answer = 42, "boom");
        });
        assert_eq!(LAST_LEVEL.load(Ordering::SeqCst), LOG_LEVEL_WARN);
        assert_eq!(*LAST_TARGET.lock().unwrap(), "aperio::test");
        // message field first (bare), then keyed fields.
        assert_eq!(*LAST_MESSAGE.lock().unwrap(), "boom answer=42");
    }
}
