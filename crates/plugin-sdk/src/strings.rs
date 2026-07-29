//! Helper for the plugin's optional `strings` FFI entry point.
//!
//! The C-ABI signature is:
//!
//! ```ignore
//! unsafe extern "C" fn aperio_plugin_strings(
//!     args_ptr: *const u8,
//!     args_len: usize,
//! ) -> PluginCallResult
//! ```
//!
//! Args are `{"lang": "de"}`; the payload is that one language's key → string
//! map as JSON.
//!
//! Most plugins should never reach for this. Strings belong in the manifest's
//! `strings` block: no code runs to render a label, the language list is
//! readable without loading the library, and a translator can send a pull
//! request against a JSON file. This exists for the plugin whose translations
//! genuinely do not fit that shape — Fluent, gettext, plural rules, a catalogue
//! fetched at runtime.
//!
//! Synchronous on purpose, unlike `discover` and `interactive_auth`. A string
//! lookup is not a network operation, and the host calls this at most once per
//! language before caching, so there is nothing to await and no runtime to spin
//! up. A handler that wants to do IO should have done it earlier.

use std::collections::BTreeMap;

use plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_OK};

use crate::response::{bytes_to_response, error_response};

/// Answer one language's strings and marshal the map back across the boundary.
///
/// `handler` takes the BCP-47 tag the host asked for and returns its key →
/// string map. An empty map is a perfectly good answer: the host falls back to
/// the manifest catalogue and then to the verbatim labels, so a language the
/// plugin has not translated still renders something a reader can act on.
///
/// # Safety
///
/// `args_ptr` + `args_len` must describe a buffer of JSON bytes the host owns
/// for the duration of the call.
pub unsafe fn strings_with<F>(args_ptr: *const u8, args_len: usize, handler: F) -> PluginCallResult
where
    F: FnOnce(&str) -> BTreeMap<String, String>,
{
    // SAFETY: forwarded from the caller's own contract, which the host upholds
    // for every named-export invocation.
    let args: StringsArgs = match unsafe { crate::args::decode_args(args_ptr, args_len) } {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let map = handler(&args.lang);
    match serde_json::to_vec(&map) {
        Ok(bytes) => bytes_to_response(PLUGIN_CALL_OK, bytes),
        Err(err) => error_response(
            PLUGIN_CALL_ERR_INVALID,
            &format!("could not serialise strings: {err}"),
        ),
    }
}

#[derive(serde::Deserialize)]
struct StringsArgs {
    #[serde(default)]
    lang: String,
}
