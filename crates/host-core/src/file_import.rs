//! File import (DESIGN-file-import.md): which formats the loaded plugins read,
//! and a file handed to the plugin that reads its format.
//!
//! The host parses nothing. A plugin declares its formats in its manifest
//! (`file_imports`) and reads the file itself; this module only lists what the
//! enabled plugins offer, for the open dialog's filter, and routes a file by
//! its extension. Both hosts call these, so the desktop and the phone offer
//! the same formats and read a file the same way.

use std::path::Path;

use cal_core::{Error, FileImport};
use plugin_core::manager::ImportFileError;
use plugin_core::PluginManager;
use serde::Serialize;

/// The largest file read for an import. A calendar export of many years is a
/// few megabytes; anything far beyond that is not a calendar file.
pub const MAX_IMPORT_BYTES: u64 = 25 * 1024 * 1024;

/// The token a too large file's error starts with, so a surface says it in
/// its own words (`shared/fileImport.ts` reads it).
pub const TOO_LARGE: &str = "import-too-large";

/// One format a plugin reads, as the open dialog and the share sheet offer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ImportFormat {
    pub plugin_id: String,
    pub format_id: String,
    /// The format's name in the asked language.
    pub name: String,
    /// Lower case, without the dot.
    pub extensions: Vec<String>,
    pub mime_types: Vec<String>,
}

/// The formats the enabled plugins read, in load order.
pub fn import_formats(manager: &PluginManager, lang: &str) -> Vec<ImportFormat> {
    let mut formats = Vec::new();
    for plugin in manager.all() {
        if !manager.is_enabled(&plugin.manifest.id) {
            continue;
        }
        let strings = PluginManager::strings_for(&plugin, lang);
        for format in plugin.file_imports() {
            let name = plugin_core::resolve_label(
                Some(strings.as_ref()),
                format.name_key.as_deref(),
                &format.name,
                lang,
            );
            formats.push(ImportFormat {
                plugin_id: plugin.manifest.id.clone(),
                format_id: format.id.clone(),
                name: name.to_string(),
                extensions: format.extensions.clone(),
                mime_types: format.mime_types.clone(),
            });
        }
    }
    formats
}

/// The format a file is read as: the first, in load order, that names its
/// extension. Two plugins claiming the same extension is allowed; the order
/// is stable, so the same one answers on every launch.
pub fn format_for(manager: &PluginManager, file_name: &str) -> Option<ImportFormat> {
    let extension = Path::new(file_name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    import_formats(manager, "en")
        .into_iter()
        .find(|format| format.extensions.contains(&extension))
}

/// Read the file at `path` through the plugin whose format names its
/// extension.
///
/// Errors, for the surfaces to say:
/// - [`Error::Unsupported`] — no enabled plugin reads files of this kind.
/// - [`Error::InvalidInput`] — the file is too large, or its plugin could not
///   read it (the plugin's own words).
/// - [`Error::Internal`] — the file could not be opened.
pub async fn import_file(manager: &PluginManager, path: &Path) -> Result<FileImport, Error> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let format = format_for(manager, &file_name)
        .ok_or_else(|| Error::Unsupported(format!("no import format reads {file_name}")))?;
    let size = std::fs::metadata(path)
        .map_err(|err| Error::Internal(format!("could not open {file_name}: {err}")))?
        .len();
    if size > MAX_IMPORT_BYTES {
        return Err(Error::InvalidInput(format!(
            "{TOO_LARGE}: {file_name} is {size} bytes, more than an import reads"
        )));
    }
    let content = std::fs::read(path)
        .map_err(|err| Error::Internal(format!("could not read {file_name}: {err}")))?;
    read_through(manager, &format, &file_name, &content).await
}

/// Read a file the surface already holds — one dropped on the desktop window,
/// which comes without a path — through the plugin whose format names the
/// extension of `file_name`. Errors as [`import_file`] has them.
pub async fn import_content(
    manager: &PluginManager,
    file_name: &str,
    content: &[u8],
) -> Result<FileImport, Error> {
    let format = format_for(manager, file_name)
        .ok_or_else(|| Error::Unsupported(format!("no import format reads {file_name}")))?;
    if content.len() as u64 > MAX_IMPORT_BYTES {
        return Err(Error::InvalidInput(format!(
            "{TOO_LARGE}: {file_name} is {} bytes, more than an import reads",
            content.len()
        )));
    }
    read_through(manager, &format, file_name, content).await
}

/// Hand the bytes to the format's plugin and read its answer.
async fn read_through(
    manager: &PluginManager,
    format: &ImportFormat,
    file_name: &str,
    content: &[u8],
) -> Result<FileImport, Error> {
    let answer = manager
        .import_file(&format.plugin_id, &format.format_id, file_name, content)
        .await
        .map_err(|err| match err {
            ImportFileError::PluginMissing(_) | ImportFileError::Unsupported { .. } => {
                Error::Unsupported(err.to_string())
            }
            ImportFileError::Plugin(message) => Error::InvalidInput(message),
        })?;
    serde_json::from_slice(&answer).map_err(|err| {
        Error::Internal(format!(
            "{} answered an import that does not read: {err}",
            format.plugin_id
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An import hook that reads the file's text as one event's title; a file
    /// saying `broken` it cannot read.
    unsafe extern "C" fn stub_import(
        args_ptr: *const u8,
        args_len: usize,
    ) -> plugin_core::ffi::PluginCallResult {
        use base64::Engine as _;
        let bytes = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
        let args: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let content = base64::engine::general_purpose::STANDARD
            .decode(args["content"].as_str().unwrap())
            .unwrap();
        let text = String::from_utf8(content).unwrap();
        let (status, payload) = if text == "broken" {
            (
                plugin_core::ffi::PLUGIN_CALL_ERR_INVALID,
                b"not a calendar file".to_vec(),
            )
        } else {
            let import = serde_json::json!({"events": [{
                "title": text,
                "start": "2026-11-16T09:00:00Z",
                "end": "2026-11-16T10:00:00Z",
                "all_day": false,
            }]});
            (
                plugin_core::ffi::PLUGIN_CALL_OK,
                import.to_string().into_bytes(),
            )
        };
        // Leaked on purpose: the host copies the bytes and calls `free`,
        // a no-op for a test buffer.
        let boxed = payload.into_boxed_slice();
        let len = boxed.len();
        let data = Box::into_raw(boxed) as *mut u8;
        unsafe extern "C" fn noop_free(_: *mut u8, _: usize) {}
        plugin_core::ffi::PluginCallResult {
            status,
            payload: plugin_core::ffi::PluginBytes {
                data,
                len,
                free: Some(noop_free),
            },
        }
    }

    /// The shipped iCal plugin, declaring one format and answering it with
    /// the stub above.
    fn manager_reading_ics(name_key: Option<&str>) -> PluginManager {
        let manager = PluginManager::new("0.1.0");
        let mut manifest =
            plugin_core::manifest::PluginManifest::from_bytes(adapter_ical_plugin::MANIFEST)
                .expect("the shipped iCal manifest parses");
        manifest.file_imports = vec![plugin_core::manifest::FileImportFormat {
            id: "ics".to_string(),
            name: "iCalendar file".to_string(),
            name_key: name_key.map(str::to_string),
            extensions: vec!["ics".to_string(), "ical".to_string()],
            mime_types: vec!["text/calendar".to_string()],
            yields: vec![plugin_core::manifest::ImportYield::Events],
        }];
        let descriptor = unsafe { adapter_ical_plugin::build_descriptor() };
        manager
            .register_static_with_hooks(
                manifest,
                descriptor,
                adapter_ical_plugin::DESTROY_FN,
                plugin_core::manager::StaticHooks {
                    import_file_fn: Some(stub_import),
                    ..Default::default()
                },
            )
            .expect("register the static iCal plugin");
        manager
    }

    #[test]
    fn lists_the_formats_of_the_enabled_plugins_in_the_asked_language() {
        let manager = manager_reading_ics(Some("kind.ical.name"));
        let de = import_formats(&manager, "de");
        assert_eq!(de.len(), 1);
        assert_eq!(de[0].format_id, "ics");
        assert_eq!(de[0].plugin_id, "com.aperio.cal-adapter-ical");
        // Resolved through the plugin's own catalogue.
        assert_eq!(de[0].name, "iCal (URL / Datei)");
        assert_eq!(import_formats(&manager, "en")[0].name, "iCal (URL / file)");
        assert_eq!(de[0].extensions, ["ics", "ical"]);
        // Without a key, the verbatim name.
        let plain = manager_reading_ics(None);
        assert_eq!(import_formats(&plain, "de")[0].name, "iCalendar file");
        // A disabled plugin offers nothing.
        manager.set_enabled("com.aperio.cal-adapter-ical", false);
        assert!(import_formats(&manager, "de").is_empty());
    }

    #[test]
    fn a_file_is_routed_by_its_extension_whatever_its_case() {
        let manager = manager_reading_ics(None);
        assert_eq!(format_for(&manager, "Termin.ICS").unwrap().format_id, "ics");
        assert_eq!(
            format_for(&manager, "export.ical").unwrap().format_id,
            "ics"
        );
        assert!(format_for(&manager, "kontakte.vcf").is_none());
        assert!(format_for(&manager, "ohne-endung").is_none());
    }

    #[tokio::test]
    async fn imports_a_file_through_its_plugin_and_says_what_failed() {
        let manager = manager_reading_ics(None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Termin.ics");
        std::fs::write(&file, "Zahnarzt").unwrap();
        let import = import_file(&manager, &file).await.unwrap();
        assert_eq!(import.events.len(), 1);
        assert_eq!(import.events[0].title, "Zahnarzt");

        // The plugin's own words for a file it cannot read.
        std::fs::write(&file, "broken").unwrap();
        match import_file(&manager, &file).await {
            Err(Error::InvalidInput(message)) => assert_eq!(message, "not a calendar file"),
            other => panic!("expected the plugin's words, got {other:?}"),
        }

        // No plugin reads it.
        let other = dir.path().join("kontakte.vcf");
        std::fs::write(&other, "BEGIN:VCARD").unwrap();
        assert!(matches!(
            import_file(&manager, &other).await,
            Err(Error::Unsupported(_))
        ));

        // Too large to be a calendar file: refused before it is read.
        let large = dir.path().join("riesig.ics");
        let handle = std::fs::File::create(&large).unwrap();
        handle.set_len(MAX_IMPORT_BYTES + 1).unwrap();
        drop(handle);
        assert!(matches!(
            import_file(&manager, &large).await,
            Err(Error::InvalidInput(message)) if message.starts_with(TOO_LARGE)
        ));

        // A file that is gone.
        let gone = dir.path().join("weg.ics");
        assert!(matches!(
            import_file(&manager, &gone).await,
            Err(Error::Internal(_))
        ));
    }

    /// The shipped iCal plugin end to end, registered as the phone registers
    /// it: its own manifest declares the format, its own export reads it.
    #[tokio::test]
    async fn the_ical_plugin_reads_a_calendar_file_end_to_end() {
        let manager = PluginManager::new("0.1.0");
        let manifest =
            plugin_core::manifest::PluginManifest::from_bytes(adapter_ical_plugin::MANIFEST)
                .expect("the shipped iCal manifest parses");
        let descriptor = unsafe { adapter_ical_plugin::build_descriptor() };
        manager
            .register_static_with_hooks(
                manifest,
                descriptor,
                adapter_ical_plugin::DESTROY_FN,
                plugin_core::manager::StaticHooks {
                    import_file_fn: Some(adapter_ical_plugin::__aperio_import_file_impl),
                    ..Default::default()
                },
            )
            .expect("register the static iCal plugin");
        let formats = import_formats(&manager, "de");
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].name, "iCalendar-Datei (.ics)");
        assert!(formats[0].mime_types.contains(&"text/calendar".to_string()));

        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../adapter-ical/tests/fixtures/teams-series.ics");
        let import = import_file(&manager, &file).await.unwrap();
        assert_eq!(import.events.len(), 2);
        assert_eq!(import.events[0].title, "Jour fixe");
        assert_eq!(import.events[1].title, "Jour fixe (verschoben)");

        // The same bytes handed over without a path, as a drop on the window
        // brings them.
        let content = std::fs::read(&file).unwrap();
        let dropped = import_content(&manager, "Serie.ICS", &content)
            .await
            .unwrap();
        assert_eq!(dropped, import);
        assert!(matches!(
            import_content(&manager, "Serie.txt", &content).await,
            Err(Error::Unsupported(_))
        ));
        let large = vec![b' '; MAX_IMPORT_BYTES as usize + 1];
        assert!(matches!(
            import_content(&manager, "riesig.ics", &large).await,
            Err(Error::InvalidInput(message)) if message.starts_with(TOO_LARGE)
        ));
    }
}
