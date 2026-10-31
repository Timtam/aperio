//! What a build before 0.2.0 pushed of a file it left in `pending/`
//! (DESIGN-sync-snapshot-coverage.md, review round 6 of #130): those builds
//! never removed a pushed file, and their compactors deleted by name, so a
//! leftover's remote copy may be gone. See
//! `SyncOrchestrator::settle_leftovers`.

use chrono::{DateTime, Utc};
use sync_core::EventEnvelope;

/// The bytes of the complete lines at the start of `bytes` whose events are
/// all at or before `cutoff`.
pub fn head_until(bytes: &[u8], cutoff: DateTime<Utc>) -> usize {
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
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sync_core::{DeviceId, IdPayload, SyncEvent};

    fn line(id: &str, secs: i64) -> String {
        serde_json::to_string(&EventEnvelope {
            id: id.into(),
            device_id: DeviceId::from_string("dev-me".into()),
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            event: SyncEvent::EventDeleted(IdPayload { id: "row".into() }),
        })
        .unwrap()
    }

    #[test]
    fn the_head_ends_at_the_first_line_after_the_cutoff() {
        let early = line("ev-1", 1_000);
        let late = line("ev-2", 5_000);
        let bytes = format!("{early}\n{late}\n").into_bytes();
        let cutoff = Utc.timestamp_opt(2_000, 0).unwrap();
        assert_eq!(head_until(&bytes, cutoff), early.len() + 1);
        assert_eq!(head_until(format!("{late}\n").as_bytes(), cutoff), 0);
        // A torn last line is never part of it.
        assert_eq!(head_until(format!("{early}").as_bytes(), cutoff), 0);
    }
}
