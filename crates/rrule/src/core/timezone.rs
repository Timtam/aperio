// Modified for Aperio (see SOURCE).
use chrono::Local;

/// A wrapper around `cal_zone::Zone` that is able to represent `Local` timezone also.
///
/// # Usage
///
/// ```
/// use rrule::Tz;
///
/// let utc = Tz::UTC;
/// let local = Tz::LOCAL;
/// let berlin = Tz::named("Europe/Berlin").unwrap();
/// ```
#[derive(Clone, Copy)]
pub enum Tz {
    /// Local timezone
    Local(Local),
    /// Timezone represented by `cal_zone::Zone`
    Tz(cal_zone::Zone),
}

impl Tz {
    /// Name of timezone
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Local(_) => "Local",
            Self::Tz(tz) => tz.name(),
        }
    }

    /// Check if timezone is the Local timezone
    #[must_use]
    pub fn is_local(&self) -> bool {
        match self {
            Self::Local(_) => true,
            Self::Tz(_) => false,
        }
    }

    /// Local timezone
    #[allow(non_upper_case_globals)]
    pub const LOCAL: Self = Self::Local(Local);

    /// UTC, the zone named "UTC" (`Etc/UTC` is another zone).
    pub const UTC: Self = Self::Tz(cal_zone::Zone::UTC);

    /// The zone tzdata names `name`, in exact case and nothing trimmed;
    /// "UTC" gives [`Tz::UTC`].
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        cal_zone::Zone::named(name).map(Self::Tz)
    }
}
