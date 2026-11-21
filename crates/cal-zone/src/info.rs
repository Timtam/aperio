//! What a zone calls its offsets, and when its clock changes: what a
//! VTIMEZONE writes.

use chrono::{DateTime, FixedOffset, Utc};

use crate::{fixed, timestamp, utc, Zone};

/// The offset in force at an instant, with its daylight-saving flag and
/// abbreviation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetInfo {
    /// The total offset, base plus daylight saving.
    pub offset: FixedOffset,
    /// Whether tzdata calls it daylight saving time. The bundled data is
    /// tzdata's REARGUARD form: Europe/Dublin's winter GMT is standard time
    /// and its summer IST daylight saving, and the same for Casablanca and
    /// El_Aaiun up to 2026-09-20 and Windhoek 1994-2017.
    pub dst: bool,
    /// The abbreviation as tzdata writes it: "CEST", or a numeric one such as
    /// "+04" or "-03". Never empty.
    pub abbreviation: String,
}

/// A change of a zone's total offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetChange {
    /// The instant of the change.
    pub at: DateTime<Utc>,
    /// The total offset before it.
    pub from: FixedOffset,
    /// The total offset from it on.
    pub to: FixedOffset,
    /// Whether tzdata calls the new offset daylight saving time.
    pub dst: bool,
    /// The new offset's abbreviation.
    pub abbreviation: String,
}

impl Zone {
    /// The offset, daylight-saving flag and abbreviation in force at an
    /// instant.
    pub fn offset_info(self, at: DateTime<Utc>) -> OffsetInfo {
        if self == Zone::UTC {
            return OffsetInfo {
                offset: fixed(0),
                dst: false,
                abbreviation: "UTC".to_owned(),
            };
        }
        let info = self.parsed().to_offset_info(timestamp(at.timestamp()));
        OffsetInfo {
            offset: fixed(info.offset().seconds()),
            dst: info.dst().is_dst(),
            abbreviation: info.abbreviation().to_owned(),
        }
    }

    /// Every change of the TOTAL offset strictly after `after`, in order, up
    /// to the end of jiff's range. A tzdata transition that changes only the
    /// abbreviation or the daylight-saving flag (America/Vancouver's PDT to
    /// MST on 2026-11-01) is no clock change and is skipped. Empty for UTC
    /// and the fixed Etc zones.
    pub fn offset_changes(
        self,
        after: DateTime<Utc>,
    ) -> impl Iterator<Item = OffsetChange> + 'static {
        let tz = if self == Zone::UTC { None } else { self.tz() };
        let start = timestamp(after.timestamp());
        let mut before = tz.map_or(0, |tz| tz.to_offset(start).seconds());
        tz.into_iter()
            .flat_map(move |tz| tz.following(start))
            .filter_map(move |transition| {
                let to = transition.offset().seconds();
                if to == before {
                    return None;
                }
                let change = OffsetChange {
                    at: utc(transition.timestamp()),
                    from: fixed(before),
                    to: fixed(to),
                    dst: transition.dst().is_dst(),
                    abbreviation: transition.abbreviation().to_owned(),
                };
                before = to;
                Some(change)
            })
    }
}
