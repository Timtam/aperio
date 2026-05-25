//! `LogFile` — the on-disk shape of a per-device, per-session
//! batch of [`crate::SyncEvent`]s.
//!
//! Wire format: one JSON-Lines file per device per "session"
//! (where session is loosely "until the writer rotates" — see the
//! event-log writer in Phase Sb). The file name encodes the
//! timestamp and the originator's device id so a directory listing
//! sorts chronologically and two devices never write to the same
//! path:
//!
//! ```text
//! sync/log/
//! ├── 2025-05-12T09-14-22Z_device-a.jsonl
//! ├── 2025-05-12T11-03-41Z_device-b.jsonl
//! └── ...
//! ```
//!
//! Each line is one [`crate::EventEnvelope`] serialised as compact
//! JSON. We use Unix LF (`\n`) regardless of platform so a file
//! round-tripped through different OSes stays parseable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::error::{SyncError, SyncResult};
use crate::event::EventEnvelope;

/// Parsed form of a log file's filename.
///
/// The wire form is `<rfc3339-timestamp-with-dashes>_<device_id>.jsonl`
/// — we replace colons with hyphens in the timestamp so the path is
/// valid on Windows (which rejects colons in file names) and on
/// every other filesystem we care about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFileName {
    /// When the file was first written. Used as the "since"
    /// cursor for incremental syncs.
    pub timestamp: DateTime<Utc>,
    /// Originator's device id.
    pub device_id: DeviceId,
}

impl LogFileName {
    /// Construct from a timestamp + device id pair.
    pub fn new(timestamp: DateTime<Utc>, device_id: DeviceId) -> Self {
        Self {
            timestamp,
            device_id,
        }
    }

    /// Encode as the on-disk filename form, including the
    /// `.jsonl` suffix.
    pub fn to_filename(&self) -> String {
        // chrono RFC3339 looks like `2025-05-12T09:14:22.341Z`.
        // Colons aren't legal on NTFS — strip them. Dots in the
        // sub-second portion are fine on every filesystem.
        let ts = self
            .timestamp
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            .replace(':', "-");
        format!("{ts}_{}.jsonl", self.device_id.as_str())
    }

    /// Parse a `<timestamp>_<device_id>.jsonl` name back into the
    /// structured form. Returns a Protocol error if the shape
    /// doesn't match.
    pub fn from_filename(name: &str) -> SyncResult<Self> {
        let stem = name.strip_suffix(".jsonl").ok_or_else(|| {
            SyncError::protocol(format!("log filename '{name}' missing .jsonl suffix",))
        })?;
        // The timestamp portion never contains underscores (only
        // hyphens, digits, T, Z, and an optional dot for
        // sub-seconds — none of which we ever emit since we strip
        // to second precision in `to_filename`). The device id
        // CAN contain underscores (e.g. test fixtures). So we
        // split on the FIRST underscore — that's unambiguously
        // the timestamp/device-id boundary.
        let split = stem.find('_').ok_or_else(|| {
            SyncError::protocol(format!(
                "log filename '{name}' missing '_' device separator",
            ))
        })?;
        let (ts_part, dev_part) = stem.split_at(split);
        let device_part = &dev_part[1..]; // drop the underscore
        if device_part.is_empty() {
            return Err(SyncError::protocol(format!(
                "log filename '{name}' has empty device id",
            )));
        }
        // Restore colons before parsing as RFC 3339. The hyphen-
        // for-colon swap only touched positions in the time
        // section, NOT the date — so we have to be precise: only
        // the two hyphens that appear AFTER the `T` separator
        // were originally colons.
        let ts_string = restore_colons_in_time(ts_part);
        let timestamp = DateTime::parse_from_rfc3339(&ts_string)
            .map_err(|err| {
                SyncError::protocol(format!(
                    "log filename '{name}' has invalid timestamp '{ts_part}': {err}",
                ))
            })?
            .with_timezone(&Utc);
        Ok(Self {
            timestamp,
            device_id: DeviceId::from_string(device_part.to_string()),
        })
    }
}

/// Inverse of the `replace(':', "-")` we apply when encoding. The
/// only hyphens that came from colons are the two in the
/// `HH-MM-SS` block after the `T`. Date hyphens come before `T`
/// and we leave them alone.
fn restore_colons_in_time(stamp: &str) -> String {
    // Find the `T` separator. If missing, return as-is so the
    // RFC 3339 parser can raise the proper error.
    let Some(t_idx) = stamp.find('T') else {
        return stamp.to_string();
    };
    let (date, time) = stamp.split_at(t_idx);
    // The time portion always starts with the literal `T`. We
    // swap the first two hyphens back to colons; later hyphens
    // (e.g. timezone offsets like `-05:00`) are intentionally
    // not present because we encode in UTC `Z` form.
    let mut restored = String::with_capacity(time.len());
    let mut swapped = 0;
    for c in time.chars() {
        if c == '-' && swapped < 2 {
            restored.push(':');
            swapped += 1;
        } else {
            restored.push(c);
        }
    }
    format!("{date}{restored}")
}

/// A whole log file — name + bytes.
///
/// Adapters hand `LogFile`s back from `fetch_new_logs` and accept
/// them in `push_log`. The bytes are the raw JSONL payload
/// (possibly encrypted if `meta.e2e_enabled` is set; encryption
/// is a layer the caller wraps around adapters, not something
/// adapters themselves see).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogFile {
    /// Structured file name. The adapter materialises it to the
    /// platform-specific path when writing.
    pub name: LogFileName,
    /// Raw bytes. Owns the buffer so the adapter and the applier
    /// don't have to coordinate lifetimes.
    pub bytes: Vec<u8>,
}

impl LogFile {
    /// Build a log file from a slice of envelopes. Serialises each
    /// envelope onto its own line. Use this on the writer side
    /// just before handing the file to `SyncAdapter::push_log`.
    pub fn from_envelopes(
        device_id: DeviceId,
        timestamp: DateTime<Utc>,
        envelopes: &[EventEnvelope],
    ) -> SyncResult<Self> {
        let mut bytes: Vec<u8> = Vec::with_capacity(envelopes.len() * 128);
        for env in envelopes {
            let line = serde_json::to_vec(env)?;
            bytes.extend_from_slice(&line);
            bytes.push(b'\n');
        }
        Ok(Self {
            name: LogFileName::new(timestamp, device_id),
            bytes,
        })
    }

    /// Parse the file bytes back into a list of envelopes. Empty
    /// lines are skipped; malformed lines surface a Protocol
    /// error with the originating line number so debugging
    /// against a malformed remote dataset is tractable.
    pub fn into_envelopes(&self) -> SyncResult<Vec<EventEnvelope>> {
        let text = std::str::from_utf8(&self.bytes).map_err(|err| {
            SyncError::protocol(format!(
                "log file '{}' is not utf-8: {err}",
                self.name.to_filename(),
            ))
        })?;
        let mut out: Vec<EventEnvelope> = Vec::new();
        for (idx, raw) in text.lines().enumerate() {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let env: EventEnvelope = serde_json::from_str(trimmed).map_err(|err| {
                SyncError::protocol(format!(
                    "log file '{}' line {}: {err}",
                    self.name.to_filename(),
                    idx + 1,
                ))
            })?;
            out.push(env);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventPayload, SyncEvent};
    use chrono::TimeZone;

    fn fixture_id() -> DeviceId {
        DeviceId::from_string("dev-a".into())
    }

    #[test]
    fn filename_round_trips_via_string() {
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        let name = LogFileName::new(stamp, fixture_id());
        let encoded = name.to_filename();
        // Sanity: colons replaced, .jsonl suffix appended.
        assert!(!encoded.contains(':'));
        assert!(encoded.ends_with(".jsonl"));
        assert!(encoded.contains("dev-a"));
        let decoded = LogFileName::from_filename(&encoded).unwrap();
        assert_eq!(decoded.timestamp, stamp);
        assert_eq!(decoded.device_id, fixture_id());
    }

    #[test]
    fn filename_parser_keeps_device_id_with_underscores() {
        // Some test fixtures use device ids that themselves
        // contain underscores. The parser splits on the LAST '_'
        // so the timestamp ↔ id boundary is unambiguous.
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        let dev = DeviceId::from_string("dev_with_unders".into());
        let name = LogFileName::new(stamp, dev.clone());
        let encoded = name.to_filename();
        let decoded = LogFileName::from_filename(&encoded).unwrap();
        assert_eq!(decoded.device_id, dev);
        assert_eq!(decoded.timestamp, stamp);
    }

    #[test]
    fn filename_parser_rejects_missing_suffix() {
        let err = LogFileName::from_filename("no-suffix").unwrap_err();
        assert!(matches!(err, SyncError::Protocol(_)));
    }

    #[test]
    fn filename_parser_rejects_missing_underscore() {
        let err = LogFileName::from_filename("2025-05-12T09-14-22Z.jsonl").unwrap_err();
        assert!(matches!(err, SyncError::Protocol(_)));
    }

    #[test]
    fn log_file_round_trips_envelopes() {
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        let envelopes = vec![
            EventEnvelope {
                id: "evt_a".into(),
                device_id: fixture_id(),
                timestamp: stamp,
                event: SyncEvent::EventCreated(EventPayload {
                    id: "row-1".into(),
                    fields: serde_json::json!({ "title": "x" }),
                }),
            },
            EventEnvelope {
                id: "evt_b".into(),
                device_id: fixture_id(),
                timestamp: stamp,
                event: SyncEvent::EventDeleted(crate::event::IdPayload { id: "row-1".into() }),
            },
        ];
        let file = LogFile::from_envelopes(fixture_id(), stamp, &envelopes).unwrap();
        // One newline per envelope.
        assert_eq!(
            file.bytes.iter().filter(|&&b| b == b'\n').count(),
            envelopes.len()
        );
        let reparsed = file.into_envelopes().unwrap();
        assert_eq!(reparsed, envelopes);
    }

    #[test]
    fn log_file_skips_blank_lines_when_parsing() {
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        // A file with a trailing newline (the normal case) has an
        // empty final line. The parser must NOT error on that.
        let file = LogFile {
            name: LogFileName::new(stamp, fixture_id()),
            bytes: b"\n\n".to_vec(),
        };
        let parsed = file.into_envelopes().unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn log_file_surfaces_line_number_on_malformed_input() {
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        let file = LogFile {
            name: LogFileName::new(stamp, fixture_id()),
            bytes: b"{\"id\":\"ok\",\"device_id\":\"d\",\"timestamp\":\"2025-05-12T09:14:22Z\",\"type\":\"event.deleted\",\"payload\":{\"id\":\"x\"}}\n{not json".to_vec(),
        };
        let err = file.into_envelopes().unwrap_err();
        match err {
            SyncError::Protocol(msg) => assert!(msg.contains("line 2")),
            other => panic!("expected Protocol with line ref, got {other:?}"),
        }
    }
}
