//! Time zones from the IANA tzdata release Aperio bundles.
//!
//! Every zone comes from jiff-tzdb's bundled data (IANA tzdata in its
//! rearguard form) and is read by jiff. No system time zone database is ever
//! asked: jiff is built without any database feature, and this crate hands it
//! the TZif bytes itself.
//!
//! A [`Zone`] is a two-byte `Copy` handle that names one tzdata zone or link
//! exactly as tzdata spells it, and it is a `chrono::TimeZone`. So
//! `DateTime<Zone>` stays `Copy`, and code written for chrono zones takes it
//! as it is.
//!
//! The rules this crate holds, once:
//! - A name matches in exact case, nothing trimmed, links kept as themselves
//!   ([`Zone::named`]). Resolving a stored name (case folded, links followed)
//!   is cal-core's rule, built on this one.
//! - Every lookup answers. Outside [`COVERED_YEARS`] it answers as at the
//!   nearer edge, so nothing here fails or panics on a chrono value.
//! - A wall time a clock change skips or repeats reads as [`Reading::Gap`] or
//!   [`Reading::Fold`], with half-open bounds. Turning a wall time into an
//!   instant names a [`WallPolicy`], or goes through the chrono mapping:
//!   a gap is `LocalResult::None`, a fold `Ambiguous(earlier, later)`.
//!
//! A handle is an index into the table of the copy of this crate it came
//! from, and every plugin library links its own copy. Store and send
//! [`Zone::name`], never the handle.

#![forbid(unsafe_code)]

mod chrono_impl;
mod info;
mod policy;
mod table;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, Timelike, Utc};

pub use chrono_impl::ZonedOffset;
pub use info::{OffsetChange, OffsetInfo};
pub use policy::WallPolicy;

/// The IANA tzdata release of the bundled data, such as "2026f".
///
/// Three checks hold it equal to the generated `cal_core::TZDATA_VERSION` and
/// to the pinned `crates/cal-core/tzdata/version`: a cal-core test,
/// `cargo xtask tz-list --check` and `cargo xtask windows-zones --check`.
pub fn tzdata_version() -> &'static str {
    jiff_tzdb::VERSION
}

/// The UTC years jiff covers in full.
///
/// jiff's instants run from -9999-01-02T01:59:59Z to
/// 9999-12-30T22:00:00.999999999Z, and its wall times over the years
/// -9999..=9999. chrono runs to ±262143. Outside this range every answer is
/// the one at the nearer edge.
pub const COVERED_YEARS: core::ops::RangeInclusive<i32> = -9998..=9998;

/// A tzdata zone or link, named the way tzdata writes it.
///
/// Equality and hashing go by name, as chrono-tz's did: `US/Pacific` is not
/// `America/Los_Angeles`, `Etc/UTC` is not `UTC`, `Asia/Calcutta` is not
/// `Asia/Kolkata`. A handle exists only for data that parsed, so no lookup on
/// a `Zone` can fail.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Zone(u16);

/// How a zone reads a wall time. All offsets are total offsets (base plus
/// daylight saving).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// The clock shows this wall time once.
    Single(FixedOffset),
    /// The clock skips it: `before` was in force before the change, `after`
    /// after it.
    Gap {
        /// The offset before the change.
        before: FixedOffset,
        /// The offset after the change.
        after: FixedOffset,
    },
    /// The clock shows it twice: `earlier` (the offset before the change)
    /// gives the earlier instant, `later` the later one.
    Fold {
        /// The offset before the change; the earlier instant.
        earlier: FixedOffset,
        /// The offset after the change; the later instant.
        later: FixedOffset,
    },
}

/// A text that is no tzdata name in exact case: what parsing a [`Zone`]
/// fails with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownZone;

impl core::fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("not a tzdata zone name")
    }
}

impl std::error::Error for UnknownZone {}

impl Zone {
    /// The zone named "UTC". Its lookups never touch the table or jiff: the
    /// offset is always 0 and every wall time reads once.
    pub const UTC: Zone = Zone(0);

    /// tzdata's own name in EXACT case, nothing trimmed, links kept as
    /// themselves.
    ///
    /// "UTC" gives [`Zone::UTC`]; "Etc/UTC", "GMT", "US/Eastern" and
    /// "EST5EDT" give their own handles. None for another case
    /// ("europe/berlin"), surrounding spaces, "", "Factory" (bundled, but no
    /// zone anyone means), "Etc/Unknown" (not bundled), Windows ids and
    /// offsets such as "+05:30".
    pub fn named(name: &str) -> Option<Zone> {
        if name == "UTC" {
            return Some(Zone::UTC);
        }
        let zone = Zone(table::position(name)?);
        zone.tz()?;
        Some(zone)
    }

    /// The name as tzdata spells it: "UTC" for [`Zone::UTC`], "US/Eastern"
    /// for that link.
    pub fn name(self) -> &'static str {
        table::slot(self.0).name
    }

    /// Every zone [`Zone::named`] accepts: [`Zone::UTC`] first, then in byte
    /// order of their names.
    pub fn all() -> impl Iterator<Item = Zone> {
        table::indices()
            .map(Zone)
            .filter(|zone| *zone == Zone::UTC || zone.tz().is_some())
    }

    /// The total UTC offset at an instant: base plus daylight saving.
    pub fn offset_at(self, at: DateTime<Utc>) -> FixedOffset {
        fixed(self.seconds_at(at.timestamp()))
    }

    /// The wall time at an instant.
    pub fn wall(self, at: DateTime<Utc>) -> NaiveDateTime {
        let offset = self.offset_at(at);
        at.naive_utc()
            .checked_add_offset(offset)
            .unwrap_or(if offset.local_minus_utc() > 0 {
                NaiveDateTime::MAX
            } else {
                NaiveDateTime::MIN
            })
    }

    /// How the zone reads a wall time. A gap is `[change + before,
    /// change + after)`, a fold `[change + after, change + before)`, as
    /// chrono-tz and jiff both bound them. A chrono leap second reads as the
    /// last nanosecond of its second.
    pub fn reading(self, wall: NaiveDateTime) -> Reading {
        use jiff::tz::AmbiguousOffset;
        if self == Zone::UTC {
            return Reading::Single(fixed(0));
        }
        match self.parsed().to_ambiguous_timestamp(civil(wall)).offset() {
            AmbiguousOffset::Unambiguous { offset } => Reading::Single(fixed(offset.seconds())),
            AmbiguousOffset::Gap { before, after } => Reading::Gap {
                before: fixed(before.seconds()),
                after: fixed(after.seconds()),
            },
            AmbiguousOffset::Fold { before, after } => Reading::Fold {
                earlier: fixed(before.seconds()),
                later: fixed(after.seconds()),
            },
        }
    }

    /// The instant a wall time names under `policy`.
    pub fn to_utc(self, wall: NaiveDateTime, policy: WallPolicy) -> Option<DateTime<Utc>> {
        policy.resolve(&self, wall)
    }

    /// The parsed data. Every handle but [`Zone::UTC`] comes from a slot
    /// whose data parsed (see [`Zone::named`] and [`Zone::all`]).
    fn tz(self) -> Option<&'static jiff::tz::TimeZone> {
        table::slot(self.0).zone()
    }

    fn parsed(self) -> &'static jiff::tz::TimeZone {
        self.tz().expect("a Zone is made only for data that parsed")
    }

    fn seconds_at(self, unix_seconds: i64) -> i32 {
        if self == Zone::UTC {
            return 0;
        }
        self.parsed().to_offset(timestamp(unix_seconds)).seconds()
    }
}

impl core::fmt::Display for Zone {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// The bare name, as chrono-tz printed a zone, so logs read as before.
impl core::fmt::Debug for Zone {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// [`Zone::named`]: exact case, nothing trimmed.
impl core::str::FromStr for Zone {
    type Err = UnknownZone;

    fn from_str(name: &str) -> Result<Zone, UnknownZone> {
        Zone::named(name).ok_or(UnknownZone)
    }
}

/// A total offset in seconds as chrono's offset. tzdata's largest offset is
/// Asia/Manila's local mean time, 57 368 s, well inside chrono's limit of a
/// day; the clamp only keeps this function total, and a test proves no zone
/// reaches it.
pub(crate) fn fixed(seconds: i32) -> FixedOffset {
    FixedOffset::east_opt(seconds.clamp(-86_399, 86_399)).expect("an offset under a day")
}

/// An instant in seconds as jiff's, clamped into jiff's range. Sub-second
/// parts play no part: every transition lies on a whole second.
pub(crate) fn timestamp(unix_seconds: i64) -> jiff::Timestamp {
    let seconds = unix_seconds.clamp(
        jiff::Timestamp::MIN.as_second(),
        jiff::Timestamp::MAX.as_second(),
    );
    jiff::Timestamp::from_second(seconds).expect("clamped into jiff's range")
}

/// jiff's instant as chrono's. chrono's range is the wider one.
pub(crate) fn utc(at: jiff::Timestamp) -> DateTime<Utc> {
    // jiff truncates toward zero and gives the fraction the timestamp's sign;
    // chrono counts whole seconds down and the fraction up.
    let (seconds, nanos) = match at.subsec_nanosecond() {
        nanos if nanos < 0 => (at.as_second() - 1, nanos + 1_000_000_000),
        nanos => (at.as_second(), nanos),
    };
    DateTime::from_timestamp(seconds, nanos.unsigned_abs()).expect("chrono's range holds jiff's")
}

/// A wall time as jiff's civil time. The year is clamped as a WHOLE value:
/// a wall time before jiff's first year reads as its first moment, one after
/// its last year as its last, because rebuilding the date with a clamped year
/// can name a day that does not exist (10000-02-29). A chrono leap second
/// (nanoseconds >= 1e9) reads as the last nanosecond of its second.
fn civil(wall: NaiveDateTime) -> jiff::civil::DateTime {
    let year = wall.year();
    if year < -9999 {
        return jiff::civil::DateTime::MIN;
    }
    if year > 9999 {
        return jiff::civil::DateTime::MAX;
    }
    jiff::civil::DateTime::new(
        year as i16,
        wall.month() as i8,
        wall.day() as i8,
        wall.hour() as i8,
        wall.minute() as i8,
        wall.second() as i8,
        wall.nanosecond().min(999_999_999) as i32,
    )
    .expect("a chrono wall time in jiff's years is a jiff wall time")
}
