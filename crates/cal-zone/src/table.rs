//! The process-wide table of zones: built once from jiff-tzdb's names, each
//! zone's data parsed the first time it is asked for.

use std::sync::{LazyLock, OnceLock};

/// Bundled names that are no zone a person, a server or a file means.
/// `Factory` is tzdata's placeholder for a machine whose zone was never set.
pub(crate) const NOT_ZONES: &[&str] = &["Factory"];

pub(crate) struct Slot {
    pub(crate) name: &'static str,
    data: &'static [u8],
    tz: OnceLock<Option<jiff::tz::TimeZone>>,
}

impl Slot {
    fn new(name: &'static str) -> Slot {
        // jiff_tzdb::get ignores ASCII case; the name it answers with must
        // be this one, or the slot gets no data and never parses.
        let data = jiff_tzdb::get(name)
            .filter(|(canonical, _)| *canonical == name)
            .map_or(&[][..], |(_, data)| data);
        Slot {
            name,
            data,
            tz: OnceLock::new(),
        }
    }

    /// The zone's data, parsed on first use; None if it does not parse.
    pub(crate) fn zone(&self) -> Option<&jiff::tz::TimeZone> {
        self.tz
            .get_or_init(|| jiff::tz::TimeZone::tzif(self.name, self.data).ok())
            .as_ref()
    }
}

/// Slot 0 is "UTC"; the rest are the other bundled names in byte order, so
/// an exact-bytes binary search finds them. jiff_tzdb::available() lists
/// "UTC" too and promises no order (it is ASCII-case-insensitive today).
static TABLE: LazyLock<Box<[Slot]>> = LazyLock::new(|| {
    let mut names: Vec<&'static str> = jiff_tzdb::available()
        .filter(|name| *name != "UTC" && !NOT_ZONES.contains(name))
        .collect();
    names.sort_unstable();
    names.dedup();
    std::iter::once("UTC").chain(names).map(Slot::new).collect()
});

pub(crate) fn slot(index: u16) -> &'static Slot {
    &TABLE[usize::from(index)]
}

/// The index of a name other than "UTC", in exact bytes.
pub(crate) fn position(name: &str) -> Option<u16> {
    let at = TABLE[1..]
        .binary_search_by(|slot| slot.name.cmp(name))
        .ok()?;
    u16::try_from(at + 1).ok()
}

pub(crate) fn indices() -> impl Iterator<Item = u16> {
    (0..TABLE.len()).filter_map(|index| u16::try_from(index).ok())
}
