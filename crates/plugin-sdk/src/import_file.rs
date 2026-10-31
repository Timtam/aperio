//! Helper for the plugin's optional `import_file` FFI entry point
//! (DESIGN-file-import.md).
//!
//! The C-ABI signature is:
//!
//! ```ignore
//! unsafe extern "C" fn aperio_plugin_import_file(
//!     args_ptr: *const u8,
//!     args_len: usize,
//! ) -> PluginCallResult
//! ```
//!
//! Args are `{"format": "<id>", "file_name": "…", "content": "<base64>"}`; the
//! payload is a JSON [`cal_core::FileImport`].
//!
//! A plugin that reads files for an import declares the formats in its
//! manifest (`file_imports`) and answers here. No account and no network:
//! the file is all there is to read, so the handler is synchronous, as the
//! `strings` one is.

use base64::Engine as _;
use cal_core::FileImport;
use plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INVALID, PLUGIN_CALL_OK};

use crate::panic_guard::guarded;
use crate::response::{bytes_to_response, error_response};

/// The file the host hands over, decoded.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// The id of one of the manifest's `file_imports`.
    pub format: String,
    /// The file's name, for messages and as a fallback title. Never a path.
    pub file_name: String,
    /// The file's bytes, exactly as read.
    pub content: Vec<u8>,
}

/// Read one file and marshal what was read back across the boundary.
///
/// `handler` gets the decoded file. An `Err` is the plugin's own sentence for
/// a file it cannot read ("not a calendar file"), passed on verbatim as
/// [`PLUGIN_CALL_ERR_INVALID`].
///
/// # Safety
///
/// `args_ptr` + `args_len` must describe a buffer of JSON bytes the host owns
/// for the duration of the call.
pub unsafe fn import_file_with<F>(
    args_ptr: *const u8,
    args_len: usize,
    handler: F,
) -> PluginCallResult
where
    F: FnOnce(ImportRequest) -> Result<FileImport, String>,
{
    // The WHOLE body runs inside the catch: a panic while decoding aborts
    // just as hard as one inside the handler.
    guarded(|| {
        // SAFETY: forwarded from the caller's own contract, which the host
        // upholds for every named-export invocation.
        let args: ImportArgs = match unsafe { crate::args::decode_args(args_ptr, args_len) } {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
        let content = match base64::engine::general_purpose::STANDARD.decode(&args.content) {
            Ok(bytes) => bytes,
            Err(err) => {
                return error_response(
                    PLUGIN_CALL_ERR_INVALID,
                    &format!("the file's content is not base64: {err}"),
                )
            }
        };
        let request = ImportRequest {
            format: args.format,
            file_name: args.file_name,
            content,
        };
        match handler(request) {
            Ok(import) => match serde_json::to_vec(&import) {
                Ok(bytes) => bytes_to_response(PLUGIN_CALL_OK, bytes),
                Err(err) => error_response(
                    PLUGIN_CALL_ERR_INVALID,
                    &format!("could not serialise the import: {err}"),
                ),
            },
            Err(message) => error_response(PLUGIN_CALL_ERR_INVALID, &message),
        }
    })
}

#[derive(serde::Deserialize)]
struct ImportArgs {
    format: String,
    #[serde(default)]
    file_name: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{SkippedKind, SkippedPart};

    fn call(
        args: &str,
        handler: impl FnOnce(ImportRequest) -> Result<FileImport, String>,
    ) -> (i32, String) {
        let bytes = args.as_bytes();
        // SAFETY: `bytes` outlives the call.
        let result = unsafe { import_file_with(bytes.as_ptr(), bytes.len(), handler) };
        // SAFETY: the payload was produced by `bytes_to_response`/`error_response`.
        let text = String::from_utf8(unsafe { result.payload.as_slice().to_vec() }).unwrap();
        let mut payload = result.payload;
        unsafe { payload.free_in_place() };
        (result.status, text)
    }

    #[test]
    fn hands_the_decoded_file_over_and_returns_what_was_read() {
        let (status, text) = call(
            r#"{"format":"ics","file_name":"a.ics","content":"QkVHSU46VkNBTEVOREFS"}"#,
            |request| {
                assert_eq!(request.format, "ics");
                assert_eq!(request.file_name, "a.ics");
                assert_eq!(request.content, b"BEGIN:VCALENDAR");
                Ok(FileImport {
                    events: Vec::new(),
                    skipped: vec![SkippedPart {
                        kind: SkippedKind::Task,
                        title: None,
                    }],
                })
            },
        );
        assert_eq!(status, PLUGIN_CALL_OK);
        let import: FileImport = serde_json::from_str(&text).unwrap();
        assert_eq!(import.skipped[0].kind, SkippedKind::Task);
    }

    #[test]
    fn a_file_the_plugin_cannot_read_says_its_own_words() {
        let (status, text) = call(r#"{"format":"ics","content":""}"#, |_| {
            Err("not a calendar file".to_string())
        });
        assert_eq!(status, PLUGIN_CALL_ERR_INVALID);
        assert_eq!(text, "not a calendar file");
    }

    #[test]
    fn content_that_is_not_base64_never_reaches_the_handler() {
        let (status, text) = call(r#"{"format":"ics","content":"%%%"}"#, |_| {
            panic!("the handler must not run")
        });
        assert_eq!(status, PLUGIN_CALL_ERR_INVALID);
        assert!(text.contains("not base64"), "{text}");
    }
}
