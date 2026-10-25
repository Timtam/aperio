//! What a file import reads out of a file (DESIGN-file-import.md).
//!
//! An adapter registers the formats it can read in its manifest
//! (`file_imports`) and answers `aperio_plugin_import_file` with a
//! [`FileImport`]: no account, no network, only the file. The host never
//! parses a format itself; it hands the file to the plugin whose format
//! carries the file's extension and passes the answer on to the surface,
//! which fills the event editor from it or lists it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{EventRecurrence, Reminder};

/// Everything a file held, as far as the adapter could read it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct FileImport {
    /// The appointments, in the order the file holds them.
    #[serde(default)]
    pub events: Vec<ImportedEvent>,
    /// What the file held that is not imported, each named: the surfaces say
    /// what was left out instead of counting it.
    #[serde(default)]
    pub skipped: Vec<SkippedPart>,
}

/// One appointment read from a file: what the event editor is filled with.
///
/// Not a [`crate::Event`]: it belongs to no calendar yet, and has no id a
/// provider gave it. A changed occurrence of a series arrives as an event of
/// its own; its series carries the occurrence's slot among its exceptions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ImportedEvent {
    /// The file's own identity of the appointment (iCalendar `UID`), when it
    /// had one: what tells an appointment already in a calendar apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub uid: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub recurrence: Option<EventRecurrence>,
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    /// The invitees, never the organizer (as on [`crate::Event`]).
    #[serde(default)]
    pub attendees: Vec<String>,
    /// Who organizes it, when the file names someone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub organizer: Option<String>,
}

/// A part of a file that is not imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SkippedPart {
    pub kind: SkippedKind,
    /// Its title, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub title: Option<String>,
}

/// What kind of part was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SkippedKind {
    /// A task (iCalendar `VTODO`): tasks are not imported yet.
    Task,
    /// A note (`VJOURNAL`).
    Journal,
    /// Busy times (`VFREEBUSY`).
    FreeBusy,
    /// A part that could not be read.
    Unreadable,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape the surfaces read: the optional fields are left out
    /// when absent, and an older answer without `skipped` still reads.
    #[test]
    fn reads_and_writes_the_wire_shape() {
        let import: FileImport = serde_json::from_str(
            r#"{"events":[{"title":"Probe","start":"2026-11-16T09:00:00Z","end":"2026-11-16T10:00:00Z","all_day":false}]}"#,
        )
        .unwrap();
        assert_eq!(import.events.len(), 1);
        assert!(import.skipped.is_empty());
        assert!(import.events[0].attendees.is_empty());
        let written = serde_json::to_string(&import.events[0]).unwrap();
        assert!(!written.contains("uid"), "{written}");
        let skipped = SkippedPart {
            kind: SkippedKind::FreeBusy,
            title: None,
        };
        assert_eq!(
            serde_json::to_string(&skipped).unwrap(),
            r#"{"kind":"free_busy"}"#
        );
    }
}
