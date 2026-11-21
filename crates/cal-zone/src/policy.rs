//! What a wall time becomes where a clock change skips it or shows it twice.

use chrono::{DateTime, MappedLocalTime, NaiveDateTime, Offset, TimeDelta, TimeZone, Utc};

/// What a wall time becomes where the clock skips it (a gap) or shows it
/// twice (a fold). Every caller that turns a wall time into an instant names
/// one. It works over any chrono zone, so a `chrono::Local` device clock
/// follows the same rule as a [`crate::Zone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallPolicy {
    /// Only a wall time the clock shows once has an instant; a gap or a fold
    /// gives None.
    Strict,
    /// A fold gives the earlier instant. A gap gives the same rule one hour
    /// later, None if that hour is skipped too (Pacific/Apia, 2011-12-30), as
    /// Exchange pushes an occurrence past a gap. Not jiff's `compatible()`:
    /// Lord Howe's 30-minute gap reads 02:15 as 03:15 here and as 02:45 there.
    EarlierElseHourLater,
    /// RFC 5545 section 3.3.5, as the views place an occurrence (`wallToReal`
    /// in shared/recurrence.ts). A fold gives the earlier instant; a gap
    /// reads the wall time with the offset in force one day before it (the
    /// wall time read as UTC, less a day). Never None for a tzdata zone
    /// inside chrono's range.
    Rfc5545,
}

impl WallPolicy {
    /// The instant `wall` names in `zone` under this policy.
    pub fn resolve<Z: TimeZone>(self, zone: &Z, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
        let utc = |at: DateTime<Z>| at.with_timezone(&Utc);
        match self {
            WallPolicy::Strict => zone.from_local_datetime(&wall).single().map(utc),
            WallPolicy::EarlierElseHourLater => {
                let earliest = |wall: NaiveDateTime| zone.from_local_datetime(&wall).earliest();
                earliest(wall)
                    .or_else(|| earliest(wall.checked_add_signed(TimeDelta::hours(1))?))
                    .map(utc)
            }
            WallPolicy::Rfc5545 => match zone.from_local_datetime(&wall) {
                MappedLocalTime::Single(at) | MappedLocalTime::Ambiguous(at, _) => Some(utc(at)),
                MappedLocalTime::None => {
                    let day_before = wall.checked_sub_signed(TimeDelta::days(1))?;
                    let before = zone.offset_from_utc_datetime(&day_before).fix();
                    wall.checked_sub_offset(before).map(|at| at.and_utc())
                }
            },
        }
    }
}
