//! What this device pushed of each of its pending files
//! (DESIGN-sync-snapshot-coverage.md, review round 5 of #130).
//!
//! A file pushed once does not go up again. Once a compactor has deleted its
//! remote copy, the peers holding its events, and a device that joined from
//! a later snapshot, would take them as unknown and apply months-old changes.
//! The record survives app starts, so a finished session pushed whole is
//! only removed, and of a file whose remote copy may be gone only what never
//! went up is pushed, in a file of its own.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use sync_core::{DeviceRecord, EventEnvelope, LogFile};

/// Each pending file's bytes this device pushed (JSON, filename to length).
/// Device-local. A join and a start over write it empty: nothing there was
/// pushed yet. Absent before this build: the first push fills it once from
/// this device's own record in `meta.json` ([`legacy`]) and keeps it.
pub const SYNC_PUSHED_LENGTHS_PREF_KEY: &str = "sync.push.pushedLengths";

/// The record as `raw` holds it; `None` when there is none, or it does not
/// read.
pub fn parse(raw: Option<&str>) -> Option<HashMap<String, u64>> {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
}

/// The record as it is stored.
pub fn encode(record: &HashMap<String, u64>) -> String {
    serde_json::to_string(record).unwrap_or_else(|_| "{}".into())
}

/// What a build before 0.2.0 pushed of `files`, as far as this device's own
/// record in `meta.json` tells it: the complete lines up to the first event
/// newer than its last round, less ten minutes — a round pushes before it
/// reports in, and an event written during the round may have missed its
/// push. A record 0.2.0 wrote says nothing here: what that build pushed, its
/// own record of it says, and a device without one pushes everything.
pub fn legacy(files: &[LogFile], own: Option<&DeviceRecord>) -> HashMap<String, u64> {
    let Some(seen) = own
        .filter(|record| sync_core::version_less(&record.app_version, "0.2.0"))
        .and_then(|record| record.last_seen)
    else {
        return HashMap::new();
    };
    let cutoff = seen - Duration::minutes(10);
    files
        .iter()
        .map(|file| (file.name.to_filename(), head_until(&file.bytes, cutoff)))
        .filter(|(_, len)| *len > 0)
        .collect()
}

/// The bytes of the complete lines at the start of `bytes` whose events are
/// all at or before `cutoff`.
fn head_until(bytes: &[u8], cutoff: DateTime<Utc>) -> u64 {
    let mut end = 0usize;
    while let Some(newline) = bytes[end..].iter().position(|byte| *byte == b'\n') {
        let line = bytes[end..end + newline].trim_ascii();
        if !line.is_empty() {
            match serde_json::from_slice::<EventEnvelope>(line) {
                Ok(envelope) if envelope.timestamp <= cutoff => {}
                _ => break,
            }
        }
        end += newline + 1;
    }
    end as u64
}

/// `pushed` cut back to the end of the last complete line in `bytes`.
pub fn complete(bytes: &[u8], pushed: u64) -> usize {
    let pushed = usize::try_from(pushed)
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    bytes[..pushed]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1)
}

/// The events of `file` this device pushed, as `record` tells it.
pub fn pushed_ids(file: &LogFile, record: &HashMap<String, u64>) -> HashSet<String> {
    let Some(pushed) = record.get(&file.name.to_filename()) else {
        return HashSet::new();
    };
    // Pushed as it stands: all of it. Pushed shorter: its complete lines.
    let end = if usize::try_from(*pushed).is_ok_and(|pushed| pushed >= file.bytes.len()) {
        file.bytes.len()
    } else {
        complete(&file.bytes, *pushed)
    };
    LogFile {
        name: file.name.clone(),
        bytes: file.bytes[..end].to_vec(),
    }
    .into_envelopes()
    .map(|envelopes| envelopes.into_iter().map(|envelope| envelope.id).collect())
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sync_core::{DeviceId, IdPayload, LogFileName, SyncEvent};

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn line(id: &str, secs: i64) -> String {
        serde_json::to_string(&EventEnvelope {
            id: id.into(),
            device_id: DeviceId::from_string("dev-me".into()),
            timestamp: at(secs),
            event: SyncEvent::EventDeleted(IdPayload { id: "row".into() }),
        })
        .unwrap()
    }

    fn file(lines: &[String]) -> LogFile {
        LogFile {
            name: LogFileName::new(at(1_000), DeviceId::from_string("dev-me".into())),
            bytes: lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>()
                .into_bytes(),
        }
    }

    fn record(version: &str, seen: i64) -> DeviceRecord {
        DeviceRecord {
            name: None,
            last_seen_log: at(0),
            last_seen: Some(at(seen)),
            app_version: version.into(),
            stale: false,
            exact: false,
            unreadable: Vec::new(),
        }
    }

    #[test]
    fn a_build_before_0_2_pushed_what_came_before_its_last_round() {
        let early = line("ev-1", 1_000);
        let late = line("ev-2", 5_000);
        let file = file(&[early.clone(), late]);

        let pushed = legacy(std::slice::from_ref(&file), Some(&record("0.1.0", 2_000)));

        assert_eq!(
            pushed.get(&file.name.to_filename()),
            Some(&(early.len() as u64 + 1)),
        );
        assert_eq!(
            pushed_ids(&file, &pushed),
            HashSet::from(["ev-1".to_string()]),
        );
    }

    #[test]
    fn a_record_0_2_wrote_or_none_says_nothing_was_pushed() {
        let file = file(&[line("ev-1", 1_000)]);
        assert!(legacy(std::slice::from_ref(&file), Some(&record("0.2.0", 9_000))).is_empty());
        assert!(legacy(std::slice::from_ref(&file), None).is_empty());
    }

    #[test]
    fn a_push_counts_only_up_to_its_last_complete_line() {
        let bytes = format!("{}\n{{\"id\":", line("ev-1", 1_000)).into_bytes();
        assert_eq!(
            complete(&bytes, bytes.len() as u64),
            line("ev-1", 1_000).len() + 1
        );
        assert_eq!(complete(&bytes, 3), 0);
    }
}
