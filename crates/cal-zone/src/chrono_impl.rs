//! [`Zone`] as a chrono zone: the drop-in for chrono-tz's zone in the rrule
//! fork, in code generic over `chrono::TimeZone`, and in tests.

use chrono::{FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone};

use crate::{Reading, Zone};

/// A chrono offset that carries its zone, so `DateTime<Zone>::timezone()`
/// gives the zone back. `Copy`, which keeps `DateTime<Zone>` `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ZonedOffset {
    zone: Zone,
    fix: FixedOffset,
}

impl ZonedOffset {
    /// The zone this offset was read in.
    pub fn zone(self) -> Zone {
        self.zone
    }
}

impl Offset for ZonedOffset {
    /// The total offset, base plus daylight saving.
    fn fix(&self) -> FixedOffset {
        self.fix
    }
}

/// The fixed offset, "+02:00". chrono-tz printed the abbreviation ("CEST");
/// only logs and assert messages ever see either.
impl core::fmt::Debug for ZonedOffset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.fix, f)
    }
}

impl core::fmt::Display for ZonedOffset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.fix, f)
    }
}

impl TimeZone for Zone {
    type Offset = ZonedOffset;

    fn from_offset(offset: &ZonedOffset) -> Zone {
        offset.zone
    }

    /// A date reads as ONE offset wherever it can, as chrono-tz 0.10.4 read
    /// it (timezone_impl.rs, MIT OR Apache-2.0): midnight's, else that of
    /// 23:59:59, else the first reading of either.
    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<ZonedOffset> {
        let earliest = self.offset_from_local_datetime(&local.and_time(NaiveTime::MIN));
        let latest = local
            .and_hms_opt(23, 59, 59)
            .map_or(MappedLocalTime::None, |last| {
                self.offset_from_local_datetime(&last)
            });
        match (earliest, latest) {
            (result @ MappedLocalTime::Single(_), _) => result,
            (_, result @ MappedLocalTime::Single(_)) => result,
            (MappedLocalTime::Ambiguous(offset, _), _) => MappedLocalTime::Single(offset),
            (_, MappedLocalTime::Ambiguous(offset, _)) => MappedLocalTime::Single(offset),
            (MappedLocalTime::None, MappedLocalTime::None) => MappedLocalTime::None,
        }
    }

    /// A gap is `None`, a fold `Ambiguous(earlier, later)`: `earliest()` is
    /// the earlier instant and `latest()` the later one.
    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<ZonedOffset> {
        let at = |fix| ZonedOffset { zone: *self, fix };
        match self.reading(*local) {
            Reading::Single(fix) => MappedLocalTime::Single(at(fix)),
            Reading::Gap { .. } => MappedLocalTime::None,
            Reading::Fold { earlier, later } => MappedLocalTime::Ambiguous(at(earlier), at(later)),
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> ZonedOffset {
        self.offset_from_utc_datetime(&utc.and_time(NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> ZonedOffset {
        ZonedOffset {
            zone: *self,
            fix: self.offset_at(utc.and_utc()),
        }
    }
}
