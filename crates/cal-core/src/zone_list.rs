//! The world list a series' zone is chosen from: its entries, how they are
//! named, the search over them, and how a stored or device zone relates to
//! them (DESIGN-series-time-zone.md, "Die Zonenliste"; decisions 17b, 25b, 26a,
//! 29a-32a).
//!
//! The list is the 312 zones outside `Etc/` from [`crate::listed_zones`] plus
//! one UTC entry. A zone is named from its tzdata id: the city is the last part
//! with `_` read as a space, a three-part id adds its middle part as the area
//! ("Indianapolis", "Indiana"), and the region is the first part. The core
//! answers with those parts and a [`ZoneRegion`]; each surface puts them into
//! words in its own language.
//!
//! Everything here but [`zone_offsets`] works without tzdata's rules, so it is
//! in every build, WebAssembly included. The offsets need chrono-tz and sit
//! behind the `zones` feature: the native bindings already link chrono-tz, the
//! desktop's WebAssembly module would grow by about 879 KB.
//!
//! The core reads no clock and no device zone: the instant an offset is asked
//! for, "today", and the device's zone name are all parameters.
//!
//! Pinned in `tests/fixtures/timeZoneFilter.json`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::series_clock::{self, NameKind};

/// The part of the world a listed zone's id starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ZoneRegion {
    Africa,
    America,
    Antarctica,
    Asia,
    Atlantic,
    Australia,
    Europe,
    Indian,
    Pacific,
}

impl ZoneRegion {
    /// Every region, so a surface can name each one.
    pub const ALL: [ZoneRegion; 9] = [
        ZoneRegion::Africa,
        ZoneRegion::America,
        ZoneRegion::Antarctica,
        ZoneRegion::Asia,
        ZoneRegion::Atlantic,
        ZoneRegion::Australia,
        ZoneRegion::Europe,
        ZoneRegion::Indian,
        ZoneRegion::Pacific,
    ];

    fn of_area(area: &str) -> Option<Self> {
        Some(match area {
            "Africa" => ZoneRegion::Africa,
            "America" => ZoneRegion::America,
            "Antarctica" => ZoneRegion::Antarctica,
            "Asia" => ZoneRegion::Asia,
            "Atlantic" => ZoneRegion::Atlantic,
            "Australia" => ZoneRegion::Australia,
            "Europe" => ZoneRegion::Europe,
            "Indian" => ZoneRegion::Indian,
            "Pacific" => ZoneRegion::Pacific,
            _ => return None,
        })
    }
}

/// A listed zone, named in parts. Its position in [`listed_zone_labels`] is
/// its position in [`crate::listed_zones`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ListedZone {
    /// The tzdata id, `America/Indiana/Indianapolis`.
    pub zone: String,
    /// The last part with `_` read as a space, `Indianapolis`.
    pub city: String,
    /// The middle part of a three-part id, `Indiana`; `null` otherwise.
    pub area: Option<String>,
    pub region: ZoneRegion,
}

/// One entry of the list: the UTC entry, or a listed zone by its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ZoneRef {
    Utc,
    Zone { position: usize },
}

/// Whether an offset is east of UTC (or zero) or west of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum OffsetSign {
    Plus,
    Minus,
}

/// An offset from UTC, with the parts a surface writes it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneOffset {
    /// The whole offset. Old local mean times carry seconds.
    pub seconds: i32,
    /// `sign`, `hh` and `mm` write the offset in whole minutes, the seconds cut
    /// toward zero: −00:44:30 is "−00:44", and less than a minute west of UTC
    /// is "+00:00", never "−00:00". The search matches these minutes.
    pub sign: OffsetSign,
    /// Hours, two digits: `"05"`.
    pub hh: String,
    /// Minutes, two digits: `"30"`.
    pub mm: String,
}

impl ZoneOffset {
    pub fn from_seconds(seconds: i32) -> Self {
        let minutes = seconds / 60;
        let sign = if minutes < 0 {
            OffsetSign::Minus
        } else {
            OffsetSign::Plus
        };
        let whole = minutes.unsigned_abs();
        Self {
            seconds,
            sign,
            hh: format!("{:02}", whole / 60),
            mm: format!("{:02}", whole % 60),
        }
    }
}

/// A listed zone's offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ListedOffset {
    /// At the instant asked for: the offset the entry shows.
    pub offset: ZoneOffset,
    /// Its standard time as of today: the smallest offset in the year from
    /// today (decision 29a). The list is ordered by it.
    pub standard: ZoneOffset,
}

/// The question [`zone_offsets`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneOffsetsQuestion {
    /// The instant the entries show their offset for: the series' start.
    pub at: DateTime<Utc>,
    /// Today, from the surface's clock: the list's order is read from the
    /// year that follows it.
    pub today: DateTime<Utc>,
}

/// Every listed zone's offsets, and the order of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneOffsets {
    pub at: DateTime<Utc>,
    pub today: DateTime<Utc>,
    /// By position, as [`listed_zone_labels`].
    pub zones: Vec<ListedOffset>,
    /// Every entry, the UTC entry included, in the order the list shows them:
    /// standard offset, then city (decision 30a). The UTC entry opens the
    /// zones whose standard offset is zero.
    pub order: Vec<ZoneRef>,
}

/// A region in the words of the surface asking, so the search finds "Europa".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct RegionName {
    pub region: ZoneRegion,
    pub name: String,
}

/// A search over the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneSearchQuestion {
    /// What the person typed. Words separated by spaces must all match.
    pub query: String,
    /// Every region's name in the surface's language; all nine are required.
    pub region_names: Vec<RegionName>,
    /// The offsets the list shows, from [`zone_offsets`]. With them hits come in
    /// the list's order; without them no word matches an offset, and hits come
    /// with the UTC entry first and then by position.
    #[serde(default)]
    pub offsets: Option<ZoneOffsets>,
}

/// Which part of an entry a word matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum MatchField {
    City,
    Area,
    Region,
    ZoneId,
    Alias,
    Offset,
}

/// Another tzdata name for an entry, and how the list calls it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneAlias {
    /// The tzdata name, `Europe/Oslo`.
    pub name: String,
    /// How the list names it: `Oslo` for a merged or alternate place,
    /// the whole name otherwise (`US/Pacific`, `CST6CDT`).
    pub label: String,
    pub kind: NameKind,
}

/// An entry the search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneHit {
    pub entry: ZoneRef,
    /// For each word, in order, the first part of the entry it matched.
    pub via: Vec<MatchField>,
    /// The other name a word found the entry by, when one did: "Berlin (auch
    /// Oslo)".
    pub also: Option<ZoneAlias>,
}

/// What a search found, in the order of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneSearchAnswer {
    pub query: String,
    pub hits: Vec<ZoneHit>,
}

/// A search question the core cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneSearchError {
    /// A region the question has no name for.
    MissingRegion(ZoneRegion),
    /// Offsets for another list than this build's.
    OffsetsDoNotFit,
}

impl fmt::Display for ZoneSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZoneSearchError::MissingRegion(region) => {
                write!(f, "the zone search has no name for the region {region:?}")
            }
            ZoneSearchError::OffsetsDoNotFit => write!(
                f,
                "the offsets handed to the zone search are for another list than this one"
            ),
        }
    }
}

/// The question [`zone_choice`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneChoiceQuestion {
    /// The zone the series stores, as it stores it.
    #[serde(default)]
    pub stored: Option<String>,
    /// The zone name the device reports, as it reports it.
    #[serde(default)]
    pub device: Option<String>,
}

/// How a zone name relates to the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ZoneChoice {
    /// No zone, or a UTC name: the UTC entry.
    Utc,
    /// A listed zone.
    Listed { position: usize },
    /// A zone tzdata knows that the list does not show under this name: an old
    /// or merged name, or a fixed offset such as `Etc/GMT+8`.
    NotListed { name: String, canonical: String },
    /// A name tzdata does not know: a Windows name, `+05:30`.
    Unreadable { name: String },
}

/// Where the stored zone and the device's zone stand in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ZoneChoiceAnswer {
    /// A stored zone counts as listed only in a listed zone's own spelling (in
    /// any ASCII case): an untouched stored name is never rewritten, and the
    /// list says "Aktuell: Europe/Kiev (nicht in Aperios Liste)".
    pub stored: ZoneChoice,
    /// A device zone resolves through tzdata first: `Asia/Calcutta` is Kolkata.
    pub device: Option<ZoneChoice>,
}

/// Every listed zone, named in parts, in the order of [`crate::listed_zones`].
pub fn listed_zone_labels() -> Vec<ListedZone> {
    series_clock::listed_zones()
        .iter()
        .map(|zone| label_of(zone))
        .collect()
}

fn label_of(zone: &str) -> ListedZone {
    let parts: Vec<&str> = zone.split('/').collect();
    let region = ZoneRegion::of_area(parts[0])
        .unwrap_or_else(|| panic!("the listed zone {zone} starts with no known region"));
    let (area, city) = match parts.as_slice() {
        [_, middle, city] => (Some(spaced(middle)), spaced(city)),
        _ => (None, spaced(parts[parts.len() - 1])),
    };
    ListedZone {
        zone: zone.to_string(),
        city,
        area,
        region,
    }
}

fn spaced(part: &str) -> String {
    part.replace('_', " ")
}

/// The door: the list's names as JSON.
pub fn zone_labels_json() -> Result<String, serde_json::Error> {
    serde_json::to_string(&listed_zone_labels())
}

/// Folding for the search, the same on both sides of a comparison: marks
/// removed after decomposition, lower case, `ß` as `ss`, and `ae`, `oe`, `ue`
/// read as `a`, `o`, `u` — so "Zürich" and "Zuerich" both find Zurich.
pub(crate) fn fold(text: &str) -> String {
    collapse_umlaut_spellings(&bare(text))
}

/// The fold short of reading `ae`, `oe`, `ue` as one letter.
fn bare(text: &str) -> String {
    let decomposed: Cow<str> = if text.is_ascii() {
        Cow::Borrowed(text)
    } else {
        icu_normalizer::DecomposingNormalizerBorrowed::new_nfd().normalize(text)
    };
    decomposed
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .collect::<String>()
        .to_lowercase()
        .replace('ß', "ss")
}

fn collapse_umlaut_spellings(bare: &str) -> String {
    let mut out = String::with_capacity(bare.len());
    let mut chars = bare.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if matches!(c, 'a' | 'o' | 'u') && chars.peek() == Some(&'e') {
            chars.next();
        }
    }
    out
}

/// A text in both forms a word is matched in.
#[derive(Clone)]
struct Folded {
    folded: String,
    bare: String,
}

impl Folded {
    fn new(text: &str) -> Self {
        let bare = bare(text);
        Self {
            folded: collapse_umlaut_spellings(&bare),
            bare,
        }
    }

    /// A word is part of a text when its fold is part of the text's fold
    /// ("Zuerich" in "Zürich"), or its bare form part of the text's bare form:
    /// "enix" keeps the `e` that folding "Phoenix" to "phonix" takes away.
    fn contains(&self, word: &Folded) -> bool {
        self.folded.contains(word.folded.as_str()) || self.bare.contains(word.bare.as_str())
    }
}

/// How the list names another tzdata name of a zone whose city is
/// `target_city`: the place's own name for a merged or alternate place with a
/// region part, unless it folds to the city itself; the whole name otherwise,
/// because `US/Pacific` or `Australia/South` mean nothing by their last part.
fn alias_label(name: &str, kind: NameKind, target_city: &str) -> String {
    let place = matches!(
        kind,
        NameKind::MergedZoneTab | NameKind::MergedNonZoneTab | NameKind::Alternate
    ) && name.contains('/');
    if place {
        let last = spaced(name.rsplit('/').next().unwrap_or(name));
        if fold(&last) != fold(target_city) {
            return last;
        }
    }
    name.to_string()
}

/// The two ids the UTC entry is found by as itself; every other UTC name is
/// one of its aliases.
const UTC_IDS: [&str; 2] = ["UTC", "Etc/UTC"];

struct Alias {
    alias: ZoneAlias,
    /// The label, which every word is matched in.
    label: Folded,
    /// The whole tzdata name, which a word with a `/` is matched in too: so
    /// "Europe/Kiev" finds Kyiv, as "US/Pacific" finds Los Angeles.
    name: Folded,
}

/// The aliases of each listed zone by position, and those of the UTC entry.
fn aliases(labels: &[ListedZone]) -> (Vec<Vec<Alias>>, Vec<Alias>) {
    let mut by_position: Vec<Vec<Alias>> = labels.iter().map(|_| Vec::new()).collect();
    let mut utc = Vec::new();
    for &(name, target, kind) in series_clock::names() {
        if matches!(target, "Etc/UTC" | "Etc/GMT") {
            if !UTC_IDS.contains(&name) {
                utc.push(Alias {
                    alias: ZoneAlias {
                        name: name.to_string(),
                        label: name.to_string(),
                        kind,
                    },
                    label: Folded::new(name),
                    name: Folded::new(name),
                });
            }
            continue;
        }
        if kind == NameKind::Zone {
            continue;
        }
        if let Some(position) = series_clock::listed_position(target) {
            let label = alias_label(name, kind, &labels[position].city);
            by_position[position].push(Alias {
                label: Folded::new(&label),
                name: Folded::new(name),
                alias: ZoneAlias {
                    name: name.to_string(),
                    label,
                    kind,
                },
            });
        }
    }
    (by_position, utc)
}

/// A word of the query, read once.
enum Word {
    /// `+2`, `UTC+2`, `+02:00`, `−3:30`, `5:30`: an offset needs a sign, a colon
    /// or a `utc` in front, and without minutes means the whole hour (31a). An
    /// unsigned word matches both signs.
    Offset { sign: Option<i32>, seconds: i32 },
    /// Any other word, and whether it holds a `/`.
    Text { text: Folded, id: bool },
}

impl Word {
    fn read(word: &str) -> Word {
        offset_word(word).unwrap_or_else(|| Word::Text {
            text: Folded::new(word),
            id: word.contains('/'),
        })
    }
}

fn offset_word(word: &str) -> Option<Word> {
    let lower = word.to_ascii_lowercase();
    let (prefixed, rest) = match lower.strip_prefix("utc") {
        Some(rest) => (true, rest),
        None => (false, lower.as_str()),
    };
    let (sign, rest) = if let Some(rest) = rest.strip_prefix('+') {
        (Some(1), rest)
    } else if let Some(rest) = rest
        .strip_prefix('-')
        .or_else(|| rest.strip_prefix('\u{2212}'))
    {
        (Some(-1), rest)
    } else {
        (None, rest)
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((hours, minutes)) => (hours, Some(minutes)),
        None => (rest, None),
    };
    if !(prefixed || sign.is_some() || minutes.is_some()) {
        return None;
    }
    let hours = digits(hours, 1)?;
    let minutes = match minutes {
        Some(minutes) => digits(minutes, 2)?,
        None => 0,
    };
    if hours > 14 || minutes > 59 {
        return None;
    }
    Some(Word::Offset {
        sign,
        seconds: hours * 3600 + minutes * 60,
    })
}

/// One or two ASCII digits (exactly two when `at_least` is 2).
fn digits(text: &str, at_least: usize) -> Option<i32> {
    if !(at_least..=2).contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Search the list: every word of the query must match an entry. A query of
/// nothing but spaces has no word to fail, so every entry is a hit, in list
/// order — the list as it stands.
pub fn zone_search(question: &ZoneSearchQuestion) -> Result<ZoneSearchAnswer, ZoneSearchError> {
    let words: Vec<Word> = question.query.split_whitespace().map(Word::read).collect();
    let mut regions = BTreeMap::new();
    for named in &question.region_names {
        regions.insert(named.region, Folded::new(&named.name));
    }
    if let Some(missing) = ZoneRegion::ALL.iter().find(|r| !regions.contains_key(r)) {
        return Err(ZoneSearchError::MissingRegion(*missing));
    }
    let labels = listed_zone_labels();
    if let Some(offsets) = &question.offsets {
        if offsets.zones.len() != labels.len() || offsets.order.len() != labels.len() + 1 {
            return Err(ZoneSearchError::OffsetsDoNotFit);
        }
    }
    let (zone_aliases, utc_aliases) = aliases(&labels);
    let order: Vec<ZoneRef> = match &question.offsets {
        Some(offsets) => offsets.order.clone(),
        None => std::iter::once(ZoneRef::Utc)
            .chain((0..labels.len()).map(|position| ZoneRef::Zone { position }))
            .collect(),
    };

    let mut hits = Vec::new();
    for entry in order {
        let (texts, entry_aliases, seconds): (Vec<(MatchField, Folded)>, &[Alias], Option<i32>) =
            match entry {
                ZoneRef::Utc => (
                    UTC_IDS
                        .iter()
                        .map(|id| (MatchField::ZoneId, Folded::new(id)))
                        .collect(),
                    &utc_aliases,
                    question.offsets.as_ref().map(|_| 0),
                ),
                ZoneRef::Zone { position } => {
                    let label = labels
                        .get(position)
                        .ok_or(ZoneSearchError::OffsetsDoNotFit)?;
                    let mut texts = vec![(MatchField::City, Folded::new(&label.city))];
                    if let Some(area) = &label.area {
                        texts.push((MatchField::Area, Folded::new(area)));
                    }
                    texts.push((MatchField::Region, regions[&label.region].clone()));
                    texts.push((MatchField::ZoneId, Folded::new(&label.zone)));
                    (
                        texts,
                        &zone_aliases[position],
                        // The whole minutes the entry shows (`ZoneOffset`):
                        // +00:09:21 is found as "+00:09", as it is written.
                        question
                            .offsets
                            .as_ref()
                            .map(|o| o.zones[position].offset.seconds / 60 * 60),
                    )
                }
            };
        if let Some(hit) = match_entry(entry, &words, &texts, entry_aliases, seconds) {
            hits.push(hit);
        }
    }
    Ok(ZoneSearchAnswer {
        query: question.query.clone(),
        hits,
    })
}

fn match_entry(
    entry: ZoneRef,
    words: &[Word],
    texts: &[(MatchField, Folded)],
    entry_aliases: &[Alias],
    seconds: Option<i32>,
) -> Option<ZoneHit> {
    let mut via = Vec::with_capacity(words.len());
    let mut also: Option<ZoneAlias> = None;
    for word in words {
        match word {
            Word::Offset {
                sign,
                seconds: wanted,
            } => {
                let shown = seconds?;
                let fits = match sign {
                    Some(sign) => shown == sign * wanted,
                    None => shown.abs() == *wanted,
                };
                if !fits {
                    return None;
                }
                via.push(MatchField::Offset);
            }
            Word::Text { text: word, id } => {
                if let Some((field, _)) = texts.iter().find(|(_, text)| text.contains(word)) {
                    via.push(*field);
                    continue;
                }
                let best = entry_aliases
                    .iter()
                    .filter(|a| a.label.contains(word) || (*id && a.name.contains(word)))
                    .min_by(|a, b| {
                        a.alias
                            .label
                            .chars()
                            .count()
                            .cmp(&b.alias.label.chars().count())
                            .then_with(|| a.alias.name.cmp(&b.alias.name))
                    })?;
                via.push(MatchField::Alias);
                if also.is_none() {
                    also = Some(best.alias.clone());
                }
            }
        }
    }
    Some(ZoneHit { entry, via, also })
}

/// The door: a [`ZoneSearchQuestion`] as JSON in, a [`ZoneSearchAnswer`] out.
pub fn zone_search_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: ZoneSearchQuestion = serde_json::from_str(input_json)?;
    let answer = zone_search(&question).map_err(<serde_json::Error as serde::de::Error>::custom)?;
    serde_json::to_string(&answer)
}

/// Where a stored zone and the device's zone stand in the list, by the one
/// rule for zones (`crate::series_clock`).
pub fn zone_choice(question: &ZoneChoiceQuestion) -> ZoneChoiceAnswer {
    ZoneChoiceAnswer {
        stored: choice_of(question.stored.as_deref(), false),
        device: question
            .device
            .as_deref()
            .map(|device| choice_of(Some(device), true)),
    }
}

fn choice_of(name: Option<&str>, resolve_links: bool) -> ZoneChoice {
    let Some(zone) = series_clock::series_clock_zone(name) else {
        return match name {
            Some(name) if !name.is_empty() && series_clock::canonical_zone(name).is_none() => {
                ZoneChoice::Unreadable {
                    name: name.to_string(),
                }
            }
            _ => ZoneChoice::Utc,
        };
    };
    let canonical = series_clock::canonical_zone(zone)
        .unwrap_or_else(|| panic!("{zone} counts as a zone, so tzdata resolves it"));
    match series_clock::listed_position(canonical) {
        Some(position) if resolve_links || zone.eq_ignore_ascii_case(canonical) => {
            ZoneChoice::Listed { position }
        }
        _ => ZoneChoice::NotListed {
            name: zone.to_string(),
            canonical: canonical.to_string(),
        },
    }
}

/// The door: a [`ZoneChoiceQuestion`] as JSON in, a [`ZoneChoiceAnswer`] out.
pub fn zone_choice_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: ZoneChoiceQuestion = serde_json::from_str(input_json)?;
    serde_json::to_string(&zone_choice(&question))
}

/// Every listed zone's offset at `at` and its standard offset as of `today`,
/// and the order of the list.
///
/// The standard offset is the smallest offset in the 365 days from `today`,
/// sampled every 15 days and on the 365th (decision 29a). It is read from total offsets only —
/// never from chrono-tz's split into base and daylight saving, which tzdata
/// writes differently between its data forms (Dublin, Casablanca) — so Dublin
/// sits at +00:00 beside London, and every series sees the same order.
#[cfg(feature = "zones")]
pub fn zone_offsets(question: &ZoneOffsetsQuestion) -> ZoneOffsets {
    use chrono::{Offset, TimeDelta, TimeZone};

    let listed = series_clock::listed_zones();
    let zones: Vec<ListedOffset> = listed
        .iter()
        .map(|name| {
            let tz: chrono_tz::Tz = name
                .parse()
                .unwrap_or_else(|_| panic!("the listed zone {name} is a chrono-tz zone"));
            let offset_at = |instant: DateTime<Utc>| {
                tz.offset_from_utc_datetime(&instant.naive_utc())
                    .fix()
                    .local_minus_utc()
            };
            let standard = (0..=24)
                .map(|k| 15 * k)
                .chain(std::iter::once(365))
                .map(|day| offset_at(question.today + TimeDelta::days(day)))
                .min()
                .unwrap_or_else(|| offset_at(question.today));
            ListedOffset {
                offset: ZoneOffset::from_seconds(offset_at(question.at)),
                standard: ZoneOffset::from_seconds(standard),
            }
        })
        .collect();

    let labels = listed_zone_labels();
    let mut keyed: Vec<(i32, u8, String, &str, ZoneRef)> =
        vec![(0, 0, String::new(), "", ZoneRef::Utc)];
    for (position, label) in labels.iter().enumerate() {
        keyed.push((
            zones[position].standard.seconds,
            1,
            fold(&label.city),
            listed[position],
            ZoneRef::Zone { position },
        ));
    }
    keyed.sort_by(|a, b| (a.0, a.1, &a.2, a.3).cmp(&(b.0, b.1, &b.2, b.3)));

    ZoneOffsets {
        at: question.at,
        today: question.today,
        zones,
        order: keyed.into_iter().map(|(_, _, _, _, entry)| entry).collect(),
    }
}

/// The door: a [`ZoneOffsetsQuestion`] as JSON in, [`ZoneOffsets`] out.
#[cfg(feature = "zones")]
pub fn zone_offsets_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: ZoneOffsetsQuestion = serde_json::from_str(input_json)?;
    serde_json::to_string(&zone_offsets(&question))
}

/// The generated table holds what the list assumes of it.
#[cfg(test)]
mod table {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_listed_zone_has_a_region_and_a_label_of_its_own() {
        let mut seen = BTreeSet::new();
        for label in listed_zone_labels() {
            let written = match &label.area {
                Some(area) => format!("{} ({area})", label.city),
                None => label.city.clone(),
            };
            assert!(
                seen.insert(fold(&written)),
                "{} is named like another zone: {written}",
                label.zone
            );
        }
    }

    #[test]
    fn every_alias_names_a_listed_zone_or_the_utc_entry() {
        let labels = listed_zone_labels();
        let (by_position, utc) = aliases(&labels);
        let links = series_clock::names()
            .iter()
            .filter(|(_, _, kind)| *kind != NameKind::Zone)
            .count();
        let placed: usize = by_position.iter().map(Vec::len).sum::<usize>() + utc.len();
        // Every link lands somewhere: a link to a zone outside the list would be
        // lost without a sound. `UTC` is a link but the UTC entry's own id, and
        // Etc/GMT is a zone but a UTC alias, so the two balance out.
        assert_eq!(placed, links);
        assert!(utc.iter().any(|a| a.alias.name == "Etc/GMT"));
        assert!(!utc.iter().any(|a| UTC_IDS.contains(&a.alias.name.as_str())));
    }

    #[test]
    fn an_offset_is_written_in_whole_minutes() {
        let written = |seconds: i32| {
            let offset = ZoneOffset::from_seconds(seconds);
            (offset.sign, offset.hh, offset.mm)
        };
        let plus = OffsetSign::Plus;
        let minus = OffsetSign::Minus;
        assert_eq!(written(0), (plus, "00".into(), "00".into()));
        assert_eq!(
            written(-30),
            (plus, "00".into(), "00".into()),
            "never −00:00"
        );
        assert_eq!(written(-60), (minus, "00".into(), "01".into()));
        assert_eq!(written(-2670), (minus, "00".into(), "44".into()));
        assert_eq!(written(561), (plus, "00".into(), "09".into()));
        assert_eq!(written(-34200), (minus, "09".into(), "30".into()));
    }

    #[test]
    fn a_search_the_core_cannot_answer_is_refused() {
        let names = |skip: usize| -> Vec<RegionName> {
            ZoneRegion::ALL
                .iter()
                .skip(skip)
                .map(|&region| RegionName {
                    region,
                    name: format!("{region:?}"),
                })
                .collect()
        };
        let asked = |region_names, offsets| {
            zone_search(&ZoneSearchQuestion {
                query: "berlin".into(),
                region_names,
                offsets,
            })
        };
        assert_eq!(
            asked(names(1), None),
            Err(ZoneSearchError::MissingRegion(ZoneRegion::ALL[0]))
        );
        let at = DateTime::<Utc>::UNIX_EPOCH;
        let short = ZoneOffsets {
            at,
            today: at,
            zones: Vec::new(),
            order: Vec::new(),
        };
        assert_eq!(
            asked(names(0), Some(short)),
            Err(ZoneSearchError::OffsetsDoNotFit)
        );
    }

    #[cfg(feature = "zones")]
    #[test]
    fn the_table_is_chrono_tz_s_release_in_its_own_spelling() {
        assert_eq!(series_clock::TZDATA_VERSION, chrono_tz::IANA_TZDB_VERSION);
        for zone in series_clock::listed_zones() {
            assert!(
                zone.parse::<chrono_tz::Tz>().is_ok(),
                "{zone} is not a chrono-tz zone"
            );
        }
    }
}

/// The list against `tests/fixtures/timeZoneFilter.json`.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/timeZoneFilter.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn position(zone: &str) -> usize {
        series_clock::listed_zones()
            .iter()
            .position(|listed| *listed == zone)
            .unwrap_or_else(|| panic!("{zone} is not listed"))
    }

    fn entry_zone(entry: &ZoneRef) -> String {
        match entry {
            ZoneRef::Utc => "utc".to_string(),
            ZoneRef::Zone { position } => series_clock::listed_zones()[*position].to_string(),
        }
    }

    fn region_names(doc: &Value, set: &str) -> Vec<RegionName> {
        doc["regions"][set]
            .as_object()
            .unwrap_or_else(|| panic!("no region set {set}"))
            .iter()
            .map(|(region, name)| RegionName {
                region: serde_json::from_value(Value::String(region.clone())).expect("a region"),
                name: name.as_str().expect("a region name").to_string(),
            })
            .collect()
    }

    #[cfg(feature = "zones")]
    fn instant(value: &Value) -> DateTime<Utc> {
        value
            .as_str()
            .expect("an instant")
            .parse()
            .expect("an RFC 3339 instant")
    }

    fn hit_is(hit: &ZoneHit, want: &Value) -> bool {
        let via: Vec<Value> = hit
            .via
            .iter()
            .map(|field| serde_json::to_value(field).expect("a field"))
            .collect();
        entry_zone(&hit.entry) == want["zone"].as_str().unwrap_or_default()
            && Value::Array(via) == want["via"]
            && match (&hit.also, want.get("also")) {
                (None, None) => true,
                (Some(also), Some(w)) => {
                    w["label"] == also.label.as_str()
                        && w.get("name").is_none_or(|name| name == also.name.as_str())
                        && w.get("kind").is_none_or(|kind| {
                            *kind == serde_json::to_value(also.kind).expect("a kind")
                        })
                }
                _ => false,
            }
    }

    fn check_search(doc: &Value, row: &Value, offsets: Option<ZoneOffsets>) {
        let query = row["query"].as_str().expect("a query");
        let note = row["note"].as_str().unwrap_or("");
        let question = ZoneSearchQuestion {
            query: query.to_string(),
            region_names: region_names(doc, row["regions"].as_str().expect("a region set")),
            offsets,
        };
        let answer = zone_search(&question).unwrap_or_else(|e| panic!("{query:?}: {e}"));
        if let Some(hits) = row.get("hits") {
            let want = hits.as_array().expect("hits");
            assert_eq!(
                answer.hits.len(),
                want.len(),
                "{query:?}: {note}\n{:?}",
                answer.hits
            );
            for (hit, want) in answer.hits.iter().zip(want) {
                assert!(
                    hit_is(hit, want),
                    "{query:?}: {note}\ngot {hit:?}\nwant {want}"
                );
            }
        }
        if let Some(count) = row.get("count") {
            assert_eq!(
                answer.hits.len() as u64,
                count.as_u64().expect("a count"),
                "{query:?}: {note}"
            );
        }
        for want in row["includes"].as_array().into_iter().flatten() {
            assert!(
                answer.hits.iter().any(|hit| hit_is(hit, want)),
                "{query:?}: {note}\nmissing {want}\ngot {:?}",
                answer.hits
            );
        }
        for zone in row["excludes"].as_array().into_iter().flatten() {
            let zone = zone.as_str().expect("a zone");
            assert!(
                answer.hits.iter().all(|hit| entry_zone(&hit.entry) != zone),
                "{query:?}: {note}\n{zone} must not be a hit"
            );
        }
    }

    #[test]
    fn every_label_row_holds() {
        let doc = doc();
        let labels = listed_zone_labels();
        // Anti-silence: the rows the rules turn on, by name.
        for zone in ["Europe/Berlin", "America/Indiana/Indianapolis"] {
            assert!(
                doc["labels"]
                    .as_array()
                    .expect("labels")
                    .iter()
                    .any(|row| row["zone"] == zone),
                "the contract lost the {zone} label row"
            );
        }
        for name in ["EST5EDT", "Europe/Oslo"] {
            assert!(
                doc["notListed"]
                    .as_array()
                    .expect("notListed")
                    .iter()
                    .any(|listed| listed == name),
                "the contract lost {name} from notListed"
            );
        }
        for row in doc["labels"].as_array().expect("labels") {
            let zone = row["zone"].as_str().expect("a zone");
            let label = &labels[position(zone)];
            assert_eq!(
                serde_json::to_value(label).expect("a label"),
                *row,
                "{zone}"
            );
        }
        for name in doc["notListed"].as_array().expect("notListed") {
            let name = name.as_str().expect("a name");
            assert!(
                !series_clock::listed_zones().contains(&name),
                "{name} is listed"
            );
        }
    }

    #[test]
    fn every_fold_row_holds() {
        let rows = doc()["fold"].as_array().expect("fold rows").clone();
        // Anti-silence: the umlaut spelled out, the sharp s, the decomposed
        // umlaut and the pair folding takes an `e` from.
        for text in ["Zuerich", "Straße", "Zu\u{308}rich", "Phoenix"] {
            assert!(
                rows.iter().any(|row| row["text"] == text),
                "the contract lost the {text:?} fold row"
            );
        }
        for row in &rows {
            let text = row["text"].as_str().expect("a text");
            assert_eq!(
                fold(text),
                row["folded"].as_str().expect("a folding"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn every_search_row_without_offsets_holds() {
        let doc = doc();
        let rows = doc["search"].as_array().expect("search rows");
        // Anti-silence: the rows the rules turn on, by query.
        for query in [
            "kiev",
            "Oslo",
            "Montreal",
            "Zuerich",
            "Wien",
            "Europa",
            "GMT",
            "   ",
            "5:30",
            "−3:30",
            "+5",
            "berlin europa",
            "enix",
            "Europe/Kiev",
            "+0",
            "+00:09",
        ] {
            assert!(
                rows.iter().any(|row| row["query"] == query),
                "the contract lost the {query:?} row"
            );
        }
        for row in rows.iter().filter(|row| row.get("offsets").is_none()) {
            check_search(&doc, row, None);
        }
    }

    #[cfg(feature = "zones")]
    #[test]
    fn every_search_row_with_offsets_holds() {
        let doc = doc();
        for row in doc["search"].as_array().expect("search rows") {
            let Some(asked) = row.get("offsets") else {
                continue;
            };
            let offsets = zone_offsets(&ZoneOffsetsQuestion {
                at: instant(&asked["at"]),
                today: instant(&asked["today"]),
            });
            check_search(&doc, row, Some(offsets));
        }
    }

    #[cfg(feature = "zones")]
    #[test]
    fn every_offset_row_holds() {
        let doc = doc();
        let rows = doc["offsets"].as_array().expect("offset rows");
        for zone in [
            "Europe/Dublin",
            "Australia/Lord_Howe",
            "Europe/Paris",
            "Asia/Almaty",
        ] {
            assert!(
                rows.iter().any(|row| row["zone"] == zone),
                "the contract lost the {zone} rows"
            );
        }
        for row in rows {
            let zone = row["zone"].as_str().expect("a zone");
            let answer = zone_offsets(&ZoneOffsetsQuestion {
                at: instant(&row["at"]),
                today: instant(&row["today"]),
            });
            let listed = &answer.zones[position(zone)];
            let at = row["at"].as_str().unwrap_or_default();
            assert_eq!(
                i64::from(listed.offset.seconds),
                row["seconds"].as_i64().expect("seconds"),
                "{zone} at {at}"
            );
            if let Some(hh) = row.get("hh") {
                assert_eq!(
                    listed.offset.hh,
                    hh.as_str().unwrap_or_default(),
                    "{zone} at {at}"
                );
            }
            if let Some(mm) = row.get("mm") {
                assert_eq!(
                    listed.offset.mm,
                    mm.as_str().unwrap_or_default(),
                    "{zone} at {at}"
                );
            }
            if let Some(sign) = row.get("sign") {
                assert_eq!(
                    serde_json::to_value(listed.offset.sign).expect("a sign"),
                    *sign,
                    "{zone} at {at}"
                );
            }
            if let Some(standard) = row.get("standard") {
                assert_eq!(
                    i64::from(listed.standard.seconds),
                    standard.as_i64().expect("a standard offset"),
                    "{zone} as of {}",
                    row["today"]
                );
            }
        }
    }

    #[cfg(feature = "zones")]
    #[test]
    fn the_order_rows_hold() {
        let rows = doc()["order"].as_array().expect("order rows").clone();
        // Anti-silence: the one order row pins 30a.
        assert!(
            rows.iter().any(|row| row["utcBefore"] == "Africa/Abidjan"),
            "the contract lost the order row"
        );
        for row in &rows {
            let answer = zone_offsets(&ZoneOffsetsQuestion {
                at: instant(&row["at"]),
                today: instant(&row["today"]),
            });
            let names: Vec<String> = answer.order.iter().map(entry_zone).collect();
            assert_eq!(names.len(), series_clock::listed_zones().len() + 1);
            let list = |key: &str| -> Vec<String> {
                row[key]
                    .as_array()
                    .expect("zones")
                    .iter()
                    .map(|zone| zone.as_str().expect("a zone").to_string())
                    .collect()
            };
            let head = list("head");
            let tail = list("tail");
            assert_eq!(names[..head.len()], head[..], "the head of the list");
            assert_eq!(
                names[names.len() - tail.len()..],
                tail[..],
                "the tail of the list"
            );
            let utc = names
                .iter()
                .position(|name| name == "utc")
                .expect("the UTC entry");
            assert_eq!(
                names[utc + 1],
                row["utcBefore"].as_str().unwrap_or_default()
            );
        }
    }

    fn choice_is(got: &ZoneChoice, want: &Value) {
        let mut got = serde_json::to_value(got).expect("a choice");
        if let Some(position) = got.get("position").and_then(Value::as_u64) {
            let zone = series_clock::listed_zones()[position as usize];
            let object = got.as_object_mut().expect("a tagged choice");
            object.remove("position");
            object.insert("zone".to_string(), Value::String(zone.to_string()));
        }
        assert_eq!(got, *want);
    }

    #[test]
    fn every_choice_row_holds() {
        let doc = doc();
        let rows = doc["choice"].as_array().expect("choice rows");
        for (stored, device) in [("Europe/Kiev", None), ("", Some("Asia/Calcutta"))] {
            assert!(
                rows.iter()
                    .any(|row| row["stored"].as_str().unwrap_or("") == stored
                        && device.is_none_or(|device| row["device"] == device)),
                "the contract lost the {stored:?} / {device:?} row"
            );
        }
        for row in rows {
            let question = ZoneChoiceQuestion {
                stored: row["stored"].as_str().map(str::to_string),
                device: row["device"].as_str().map(str::to_string),
            };
            let answer = zone_choice(&question);
            choice_is(&answer.stored, &row["expect"]["stored"]);
            match (&answer.device, row["expect"].get("device")) {
                (None, None) => {}
                (Some(device), Some(want)) => choice_is(device, want),
                (got, want) => panic!("{question:?}: got {got:?}, want {want:?}"),
            }
        }
    }
}
