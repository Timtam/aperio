//! The list's groups (DESIGN-series-time-zone.md, decision 118a and the
//! stage 11 decisions): zones with the same rules from today on stand as one
//! entry. Two listed zones share a group when their total offset is the same
//! at every instant from `today` to `today` plus [`GROUP_HORIZON_DAYS`], change
//! instants included; a series cannot tell them apart in that time. The 312
//! listed zones form about sixty groups (62 on 2026-11-22 with tzdata 2026f).
//!
//! A group names its cities by rank ([`super::prominence`]) and says its clock
//! (fixed, summer time, or a coming change). Picked from the list it stores the
//! series' own zone if that is in the group (120a), else the device's zone if
//! that is (118a), else its best-ranked member, always one the calendar stores
//! and the device can use. The search still finds every city and stores
//! exactly it (118a); only a query of offsets alone finds groups (121a).
//!
//! The core reads no clock: `today` is the surface's. The types are in every
//! build, because a search question carries a groups answer; the computation
//! needs tzdata's rules and the `zones` feature.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::{ZoneChoice, ZoneOffset, ZoneRef};
use crate::SeriesTimeZones;

/// How far "from today on" reaches when zones are grouped: two years, the end
/// included. One year let Havana, a month before its last summer-time change,
/// fall into the fixed −04:00 group that says "ohne Sommerzeit".
pub const GROUP_HORIZON_DAYS: i64 = 730;
/// The year a standard offset is read from (decision 29a), the end included.
pub const STANDARD_YEAR_DAYS: i64 = 365;
/// How many cities a group's entry names before "und N weitere".
pub const NAMED_CITIES: usize = 3;

/// A group's clock from today on, as its entry says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum GroupClock {
    /// No change in the window: "UTC+05:30, ohne Sommerzeit".
    Fixed { offset: ZoneOffset },
    /// Two offsets taking turns, with a change inside the year:
    /// "UTC+01:00, Sommerzeit UTC+02:00".
    Seasonal {
        standard: ZoneOffset,
        summer: ZoneOffset,
    },
    /// Anything else, such as a change that comes once: the offset now and
    /// the first change, "UTC−04:00, ab 7. November 2027 UTC−05:00".
    /// `next_on` is the date on the clock before the change.
    Changing {
        now: ZoneOffset,
        next: ZoneOffset,
        next_at: DateTime<Utc>,
        next_on: NaiveDate,
    },
}

impl GroupClock {
    /// Every offset the entry says, which an offset search matches.
    pub(super) fn offsets(&self) -> Vec<&ZoneOffset> {
        match self {
            GroupClock::Fixed { offset } => vec![offset],
            GroupClock::Seasonal { standard, summer } => vec![standard, summer],
            GroupClock::Changing { now, next, .. } => vec![now, next],
        }
    }
}

/// Why a zone or a group cannot be chosen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ZoneRefusal {
    /// The calendar cannot store it (22a).
    Calendar,
    /// This device's clock library refuses it.
    Device,
}

/// Where a group's pick comes from, so the form writes it as it should.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum PickSource {
    /// The series' own zone (120a): choosing the group rewrites nothing.
    Stored,
    /// The device's zone, written in the device's spelling where the device
    /// cannot use tzdata's (117a).
    Device,
    /// A member by rank, written canonically.
    Rank,
}

/// The zone a group stores when it is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct GroupPick {
    /// The listed position.
    pub position: usize,
    pub source: PickSource,
}

/// One group: zones with the same rules from today on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneGroup {
    /// Every member by listed position, best-ranked first. The first is the
    /// group's main city, whatever the device or the calendar.
    pub members: Vec<usize>,
    pub clock: GroupClock,
    /// The standard offset as of today (29a), which orders the list.
    pub standard: ZoneOffset,
    /// What choosing the group stores; `null` when no member can be chosen.
    pub pick: Option<GroupPick>,
    /// The cities the entry names, at most [`NAMED_CITIES`]: the pick first,
    /// then the other members that can be chosen, by rank. With no pick, the
    /// first members by rank.
    pub named: Vec<usize>,
    /// The members that can be chosen and are not named; with no pick, every
    /// member not named.
    pub more: usize,
    /// Why the group cannot be chosen: `calendar` when the calendar stores no
    /// member, `device` otherwise. Set exactly when `pick` is `null`.
    pub refusal: Option<ZoneRefusal>,
}

/// Which row above the list a head entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum HeadKind {
    /// "Aktuell": the zone the series stores.
    Current,
    /// "Dieses Gerät".
    Device,
    /// The UTC entry.
    Utc,
}

/// A row above the list while the search is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct HeadEntry {
    pub kind: HeadKind,
    pub choice: ZoneChoice,
    /// Why the row cannot be chosen: `calendar` where the calendar cannot
    /// store its zone, else `device` where this device refuses it ("Aktuell"
    /// by its stored spelling). The device's own row is never refused by the
    /// device. A stored name tzdata does not know is never refused: it says
    /// its own reason, and keeping it changes nothing.
    pub refusal: Option<ZoneRefusal>,
    /// Whether the row says the zone the series stores: "Aktuell" always, the
    /// device's row where "Aktuell" is left out for it or where it keeps the
    /// series' zone, the UTC row for a series on UTC. Also where the row is
    /// refused: it still says which zone the series is on.
    pub current: bool,
    /// Whether choosing the row keeps the series' zone as it is spelled
    /// (120a), so it rewrites nothing.
    pub keeps: bool,
}

/// The question [`zone_groups`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneGroupsQuestion {
    /// Today, from the surface's clock, as given.
    pub today: DateTime<Utc>,
    /// The zone the series stores, as it stores it.
    #[serde(default)]
    pub stored: Option<String>,
    /// The zone name the device reports, as it reports it.
    #[serde(default)]
    pub device: Option<String>,
    /// The zones the series' calendar stores, in the manifest's form (97a).
    #[serde(default)]
    #[cfg_attr(feature = "ts-export", ts(type = "\"all\" | \"none\" | Array<string>"))]
    pub calendar_zones: SeriesTimeZones,
    /// Names this device's clock library refuses: every listed name it
    /// refuses, and the series' stored name in its own spelling when it
    /// refuses that. A stored zone is kept as it is spelled (120a), so its
    /// spelling decides, not its listed name; compared in any ASCII case, as
    /// clock libraries match zone ids.
    #[serde(default)]
    pub unusable: Vec<String>,
}

/// The list as groups, the rows above it, and the marks of every listed zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneGroups {
    pub today: DateTime<Utc>,
    /// Where the grouping window ends: `today` plus [`GROUP_HORIZON_DAYS`].
    pub until: DateTime<Utc>,
    /// Every group, in the order of the list.
    pub groups: Vec<ZoneGroup>,
    /// The UTC entry and every group, in the order of the list: standard
    /// offset, then main city (29a, 30a). The UTC entry opens the groups whose
    /// standard offset is zero or more.
    pub order: Vec<ZoneRef>,
    /// The rows above the list: "Aktuell" (unless it is the device's zone or
    /// UTC), "Dieses Gerät" (for a zone tzdata knows) and UTC.
    pub head: Vec<HeadEntry>,
    /// By listed position: why each zone cannot be chosen, for city hits.
    pub refused: Vec<Option<ZoneRefusal>>,
    /// The listed position whose city and group keep the series' zone as it
    /// is spelled (120a): the stored zone's, where the calendar stores it and
    /// this device can use its stored spelling. That city hit is then not
    /// refused for the device, though its listed name may be. `null` where no
    /// listed entry keeps it.
    pub kept: Option<usize>,
}

#[cfg(feature = "zones")]
pub use compute::{year_standard_seconds, zone_groups, zone_groups_json};

#[cfg(feature = "zones")]
mod compute {
    use std::cmp::Reverse;
    use std::collections::{BTreeMap, BTreeSet};

    use chrono::{DateTime, TimeDelta, Utc};

    use super::super::prominence::PROMINENCE;
    use super::super::{choice_of, fold, listed_zone_labels, ListedZone, ZoneChoice};
    use super::*;
    use crate::series_clock::{self, NameKind};

    /// A listed zone's rules from `today`: its offset then, and every change
    /// of its total offset after it within `days`, as (instant, new offset).
    /// A change of the abbreviation or the summer-time flag alone is none.
    struct ZoneRules {
        now: i32,
        changes: Vec<(i64, i32)>,
    }

    fn zone_rules(name: &str, today: DateTime<Utc>, days: i64) -> ZoneRules {
        let zone = cal_zone::Zone::named(name)
            .unwrap_or_else(|| panic!("the listed zone {name} is a bundled tzdata zone"));
        let end = today + TimeDelta::days(days);
        ZoneRules {
            now: zone.offset_at(today).local_minus_utc(),
            changes: zone
                .offset_changes(today)
                .take_while(|change| change.at <= end)
                .map(|change| (change.at.timestamp(), change.to.local_minus_utc()))
                .collect(),
        }
    }

    /// The standard offset as of `today` (29a): the smallest total offset in
    /// the year from it, its end included.
    fn year_standard(rules: &ZoneRules, today: DateTime<Utc>) -> i32 {
        let year_end = (today + TimeDelta::days(STANDARD_YEAR_DAYS)).timestamp();
        rules
            .changes
            .iter()
            .take_while(|(at, _)| *at <= year_end)
            .map(|(_, offset)| *offset)
            .chain(std::iter::once(rules.now))
            .min()
            .unwrap_or(rules.now)
    }

    /// A listed zone's standard offset as of `today`, for the list of cities.
    pub fn year_standard_seconds(name: &str, today: DateTime<Utc>) -> i32 {
        year_standard(&zone_rules(name, today, STANDARD_YEAR_DAYS), today)
    }

    fn clock_of(rules: &ZoneRules, today: DateTime<Utc>) -> GroupClock {
        let Some(&(first_at, first_to)) = rules.changes.first() else {
            return GroupClock::Fixed {
                offset: ZoneOffset::from_seconds(rules.now),
            };
        };
        let mut values: Vec<i32> = std::iter::once(rules.now)
            .chain(rules.changes.iter().map(|(_, offset)| *offset))
            .collect();
        values.sort_unstable();
        values.dedup();
        let year_end = (today + TimeDelta::days(STANDARD_YEAR_DAYS)).timestamp();
        if values.len() == 2 && rules.changes.len() >= 2 && first_at <= year_end {
            return GroupClock::Seasonal {
                standard: ZoneOffset::from_seconds(values[0]),
                summer: ZoneOffset::from_seconds(values[1]),
            };
        }
        let next_at = DateTime::<Utc>::from_timestamp(first_at, 0)
            .unwrap_or_else(|| panic!("a tzdata change at {first_at} is an instant"));
        GroupClock::Changing {
            now: ZoneOffset::from_seconds(rules.now),
            next: ZoneOffset::from_seconds(first_to),
            next_at,
            next_on: (next_at + TimeDelta::seconds(i64::from(rules.now))).date_naive(),
        }
    }

    /// How many other tzdata names resolve to each listed zone.
    fn link_counts(listed_count: usize) -> Vec<usize> {
        let mut counts = vec![0; listed_count];
        for &(_, target, kind) in series_clock::names() {
            if kind == NameKind::Zone {
                continue;
            }
            if let Some(position) = series_clock::listed_position(target) {
                counts[position] += 1;
            }
        }
        counts
    }

    type RankKey = (usize, Reverse<usize>, String, &'static str);

    fn rank_key(position: usize, labels: &[ListedZone], counts: &[usize]) -> RankKey {
        let zone = series_clock::listed_zones()[position];
        (
            PROMINENCE
                .iter()
                .position(|ranked| *ranked == zone)
                .unwrap_or(PROMINENCE.len()),
            Reverse(counts[position]),
            fold(&labels[position].city),
            zone,
        )
    }

    /// The list as groups, the rows above it and every listed zone's marks.
    pub fn zone_groups(question: &ZoneGroupsQuestion) -> ZoneGroups {
        let listed = series_clock::listed_zones();
        let labels = listed_zone_labels();
        let today = question.today;
        let rules: Vec<ZoneRules> = listed
            .iter()
            .map(|name| zone_rules(name, today, GROUP_HORIZON_DAYS))
            .collect();

        let listed_at = |choice: &ZoneChoice| match choice {
            ZoneChoice::Listed { position } => Some(*position),
            _ => None,
        };
        let device = question
            .device
            .as_deref()
            .map(|name| choice_of(Some(name), true));
        let device_position = device.as_ref().and_then(listed_at);
        let stored_position = listed_at(&choice_of(question.stored.as_deref(), true));
        // The series' own zone is kept in its stored spelling (120a): the
        // calendar must store its zone, and the device use that spelling.
        let stored_unusable = question.stored.as_deref().is_some_and(|stored| {
            question
                .unusable
                .iter()
                .any(|name| name.eq_ignore_ascii_case(stored))
        });
        let kept = stored_position.filter(|position| {
            question.calendar_zones.stores(listed[*position]) && !stored_unusable
        });

        let unusable: BTreeSet<&str> = question.unusable.iter().map(String::as_str).collect();
        let refused: Vec<Option<ZoneRefusal>> = listed
            .iter()
            .enumerate()
            .map(|(position, name)| {
                if !question.calendar_zones.stores(name) {
                    Some(ZoneRefusal::Calendar)
                } else if unusable.contains(name) && Some(position) != device_position {
                    Some(ZoneRefusal::Device)
                } else {
                    None
                }
            })
            .collect();
        let choosable = |position: usize| refused[position].is_none();

        let mut by_rules: BTreeMap<(i32, Vec<(i64, i32)>), Vec<usize>> = BTreeMap::new();
        for (position, zone) in rules.iter().enumerate() {
            by_rules
                .entry((zone.now, zone.changes.clone()))
                .or_default()
                .push(position);
        }

        let counts = link_counts(listed.len());
        let mut groups: Vec<ZoneGroup> = by_rules
            .into_values()
            .map(|mut members| {
                members.sort_by_cached_key(|position| rank_key(*position, &labels, &counts));
                let rules = &rules[members[0]];
                let in_group = |position: &usize| members.contains(position);
                let stored = kept
                    .filter(|position| in_group(position))
                    .map(|position| GroupPick {
                        position,
                        source: PickSource::Stored,
                    });
                let pick = stored.or_else(|| {
                    device_position
                        .filter(in_group)
                        .map(|position| (position, PickSource::Device))
                        .into_iter()
                        .chain(members.iter().map(|position| (*position, PickSource::Rank)))
                        .find(|(position, _)| choosable(*position))
                        .map(|(position, source)| GroupPick { position, source })
                });
                let (named, more, refusal) = match pick {
                    Some(pick) => {
                        let others: Vec<usize> = members
                            .iter()
                            .copied()
                            .filter(|position| *position != pick.position && choosable(*position))
                            .collect();
                        let shown = others.len().min(NAMED_CITIES - 1);
                        let named = std::iter::once(pick.position)
                            .chain(others[..shown].iter().copied())
                            .collect();
                        (named, others.len() - shown, None)
                    }
                    None => {
                        let shown = members.len().min(NAMED_CITIES);
                        let stores_one = members
                            .iter()
                            .any(|position| refused[*position] != Some(ZoneRefusal::Calendar));
                        (
                            members[..shown].to_vec(),
                            members.len() - shown,
                            Some(if stores_one {
                                ZoneRefusal::Device
                            } else {
                                ZoneRefusal::Calendar
                            }),
                        )
                    }
                };
                ZoneGroup {
                    clock: clock_of(rules, today),
                    standard: ZoneOffset::from_seconds(year_standard(rules, today)),
                    members,
                    pick,
                    named,
                    more,
                    refusal,
                }
            })
            .collect();
        groups.sort_by_cached_key(|group| {
            let main = group.members[0];
            (
                group.standard.seconds,
                fold(&labels[main].city),
                listed[main],
            )
        });

        let mut order = Vec::with_capacity(groups.len() + 1);
        for (index, group) in groups.iter().enumerate() {
            if group.standard.seconds >= 0 && !order.contains(&ZoneRef::Utc) {
                order.push(ZoneRef::Utc);
            }
            order.push(ZoneRef::Group { group: index });
        }
        if !order.contains(&ZoneRef::Utc) {
            order.push(ZoneRef::Utc);
        }

        // The rows above the list. "Aktuell" is left out where the device's
        // row would say it again: the same listed zone, or the same name. A
        // device name tzdata does not know has no row, so it hides nothing.
        let stored = choice_of(question.stored.as_deref(), false);
        let on_utc = matches!(stored, ZoneChoice::Utc);
        let same_as_device = match (&stored, &device) {
            (ZoneChoice::Listed { position }, Some(ZoneChoice::Listed { position: own })) => {
                position == own
            }
            (_, None | Some(ZoneChoice::Utc | ZoneChoice::Unreadable { .. })) => false,
            _ => question
                .stored
                .as_deref()
                .zip(question.device.as_deref())
                .is_some_and(|(stored, device)| stored.eq_ignore_ascii_case(device)),
        };
        // "Aktuell" is marked as the zone's other rows are: where the calendar
        // cannot store it, else where this device refuses its stored
        // spelling. A name tzdata does not know says its own reason instead.
        let current_refusal = match &stored {
            ZoneChoice::Listed { position } => Some(listed[*position]),
            ZoneChoice::NotListed { canonical, .. } => Some(canonical.as_str()),
            ZoneChoice::Utc | ZoneChoice::Unreadable { .. } => None,
        }
        .and_then(|canonical| {
            if !question.calendar_zones.stores(canonical) {
                Some(ZoneRefusal::Calendar)
            } else if stored_unusable {
                Some(ZoneRefusal::Device)
            } else {
                None
            }
        });
        let mut head = Vec::new();
        if !on_utc && !same_as_device {
            head.push(HeadEntry {
                kind: HeadKind::Current,
                choice: stored,
                refusal: current_refusal,
                current: true,
                keeps: current_refusal.is_none(),
            });
        }
        // The device's row keeps the series' zone where it is the kept listed
        // zone, or, outside the list, the same name, stored by the calendar.
        match device {
            Some(choice @ ZoneChoice::Listed { position }) => {
                let keeps = kept == Some(position);
                head.push(HeadEntry {
                    kind: HeadKind::Device,
                    choice,
                    refusal: refused[position],
                    current: same_as_device || keeps,
                    keeps,
                });
            }
            Some(choice @ ZoneChoice::NotListed { .. }) => {
                let refusal = match &choice {
                    ZoneChoice::NotListed { canonical, .. }
                        if !question.calendar_zones.stores(canonical) =>
                    {
                        Some(ZoneRefusal::Calendar)
                    }
                    _ => None,
                };
                head.push(HeadEntry {
                    kind: HeadKind::Device,
                    choice,
                    refusal,
                    current: same_as_device,
                    keeps: same_as_device && refusal.is_none() && !stored_unusable,
                });
            }
            _ => {}
        }
        head.push(HeadEntry {
            kind: HeadKind::Utc,
            choice: ZoneChoice::Utc,
            refusal: None,
            current: on_utc,
            keeps: on_utc,
        });

        ZoneGroups {
            today,
            until: today + TimeDelta::days(GROUP_HORIZON_DAYS),
            groups,
            order,
            head,
            refused,
            kept,
        }
    }

    /// The door: a [`ZoneGroupsQuestion`] as JSON in, [`ZoneGroups`] out.
    pub fn zone_groups_json(input_json: &str) -> Result<String, serde_json::Error> {
        let question: ZoneGroupsQuestion = serde_json::from_str(input_json)?;
        serde_json::to_string(&zone_groups(&question))
    }
}
