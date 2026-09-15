//! `windows-zones` — the Windows time-zone ids Exchange speaks, generated from
//! the Unicode CLDR `windowsZones.xml` the EWS adapter carries.
//!
//! # Why generated, and from what
//!
//! Exchange names a series' zone by a Windows id. The mapping to tzdata is
//! CLDR's; a hand-kept copy of it had drifted to three wrong rows and wrote 174
//! of the 312 listed zones without a zone. The file is pinned in
//! `crates/adapter-ews/cldr/` by release tag and sha256, with its licence, and
//! every name in it resolves through `cal_core` — the rule the adapter asks at
//! runtime — so the table and the adapter cannot disagree about a spelling.
//!
//! # What it refuses, each by name and line
//!
//! A file that does not match its pin; an element the format does not have, in
//! a place the format does not put it, or with an attribute it does not carry;
//! text between the elements; a Windows id without exactly one default ("001")
//! zone; a name tzdata does not know; a name under two ids; a zone whose other
//! spellings sit under different ids; a default zone that is a UTC name for an
//! id other than `UTC`.
//!
//! # The clock guard
//!
//! CLDR can lag tzdata: in 2025b America/Scoresbysund runs −02:00/−01:00 while
//! CLDR files it under the Azores' id (−01:00/+00:00). A zone whose id's
//! default zone reads another offset at any time in the five years from the
//! release's publication is not written, because Exchange would store another
//! clock (decision 22a). The default zone stands in for Windows' own rules: on
//! one Windows 11 machine their offsets matched for all 139 ids in January and
//! July 2026 (DESIGN-series-time-zone.md, stage 4); Exchange's own tables are
//! not measured. Offsets are compared at the start of every UTC day, and every
//! hour of a day on which either clock changes, so a difference that starts
//! and ends between two whole hours goes unseen.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Months, NaiveDate, Offset, TimeZone, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;
use sha2::{Digest, Sha256};

use super::{metadata, USAGE};

/// Where the pinned CLDR file lives, relative to the workspace root.
const CLDR_DIR: &str = "crates/adapter-ews/cldr";

/// Where the task writes, relative to the workspace root.
const WINDOWS_ZONES: &str = "crates/adapter-ews/src/windows_tz/windows_zones.rs";

/// How long the clock guard looks ahead of the pinned release's publication.
const CLOCK_MONTHS: u32 = 60;

/// Every element the windowsZones format has: its name, the element it sits
/// in (empty for the document's root), and the attributes it may carry.
const FORMAT: &[(&str, &str, &[&str])] = &[
    ("supplementalData", "", &[]),
    ("version", "supplementalData", &["number"]),
    ("windowsZones", "supplementalData", &[]),
    (
        "mapTimezones",
        "windowsZones",
        &["otherVersion", "typeVersion"],
    ),
    ("mapZone", "mapTimezones", &["other", "territory", "type"]),
];

/// Reading rows every generation must hold, by name. A read that went wrong
/// would otherwise write a table that compiles and translates nothing.
const READ_MUST_HAVE: &[(&str, &str)] = &[
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("India Standard Time", "Asia/Kolkata"),
    ("FLE Standard Time", "Europe/Kyiv"),
    ("Greenwich Standard Time", "Africa/Abidjan"),
    ("UTC", "Etc/UTC"),
];

/// Writing rows every generation must hold, by name.
const WRITE_MUST_HAVE: &[(&str, &str)] = &[
    ("Europe/Berlin", "W. Europe Standard Time"),
    ("Europe/Vienna", "W. Europe Standard Time"),
    ("Asia/Kolkata", "India Standard Time"),
    ("America/Tijuana", "Pacific Standard Time (Mexico)"),
    ("America/Chihuahua", "Central Standard Time (Mexico)"),
    ("Asia/Beirut", "Middle East Standard Time"),
    ("Pacific/Kanton", "UTC+13"),
    ("Etc/GMT-1", "W. Central Africa Standard Time"),
];

/// `windows-zones` — generate `windows_zones.rs` for the EWS adapter.
pub(crate) fn windows_zones(args: &[String]) -> Result<String, String> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    if cal_core::TZDATA_VERSION != chrono_tz::IANA_TZDB_VERSION {
        return Err(format!(
            "cal-core resolves names from tzdata {}, but chrono-tz is built from {}: \
             run `cargo xtask tz-list` first",
            cal_core::TZDATA_VERSION,
            chrono_tz::IANA_TZDB_VERSION,
        ));
    }

    let meta = metadata()?;
    let root = PathBuf::from(
        meta["workspace_root"]
            .as_str()
            .ok_or("`cargo metadata` printed no workspace_root")?,
    );
    let dir = root.join(CLDR_DIR);
    let read = |name: &str| {
        fs::read_to_string(dir.join(name)).map_err(|e| format!("reading {CLDR_DIR}/{name}: {e}"))
    };
    let source = read_source(&read("SOURCE")?)?;
    let xml = read("windowsZones.xml")?;
    if read("LICENSE")?.trim().is_empty() {
        return Err(format!(
            "{CLDR_DIR}/LICENSE is empty: the CLDR licence travels with its data"
        ));
    }
    check_pin(&xml, &source)?;
    let rows = read_map_zones(&xml.replace("\r\n", "\n"))?;
    let window = clock_window(source.published)?;
    let tables = build_tables(&rows, &canonical_zones(), window)?;
    holds_must_have(&tables)?;
    let fresh = render_windows_zones(&source, &tables, window);
    let summary = format!(
        "{} Windows ids, {} zones written, {} on another clock, {} unmapped; CLDR {}, tzdata {}",
        tables.read.len(),
        tables.write.len(),
        tables.other_clock.len(),
        tables.unmapped.len(),
        source.release,
        cal_core::TZDATA_VERSION,
    );

    let target = root.join(WINDOWS_ZONES);
    if check {
        let have = match fs::read_to_string(&target) {
            Ok(text) => text.replace("\r\n", "\n"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("reading {}: {e}", target.display())),
        };
        return match describe_windows_drift(&have, &fresh) {
            None => Ok(format!("{WINDOWS_ZONES} is current — {summary}.")),
            Some(drift) => Err(format!(
                "{WINDOWS_ZONES} does not match {CLDR_DIR}:\n{drift}\n\n\
                 Run `cargo xtask windows-zones` and commit the result."
            )),
        };
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    // Written beside the table and moved over it, as `tz-list` does: a write
    // that fails halfway must not leave a table the adapter cannot build with.
    let staging = target.with_extension("rs.tmp");
    fs::write(&staging, &fresh).map_err(|e| format!("writing {}: {e}", staging.display()))?;
    fs::rename(&staging, &target).map_err(|e| {
        format!(
            "moving {} over {}: {e}",
            staging.display(),
            target.display()
        )
    })?;
    Ok(format!("{WINDOWS_ZONES}: {summary}."))
}

/// What `cldr/SOURCE` pins.
#[derive(Debug)]
struct Source {
    release: String,
    published: NaiveDate,
    sha256: String,
}

fn read_source(text: &str) -> Result<Source, String> {
    let mut values: BTreeMap<&str, &str> = BTreeMap::new();
    for (at, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{CLDR_DIR}/SOURCE:{}: not `key = value`: {line}", at + 1))?;
        let key = key.trim();
        if !["release", "published", "url", "licence", "sha256"].contains(&key) {
            return Err(format!("{CLDR_DIR}/SOURCE:{}: unknown key `{key}`", at + 1));
        }
        if values.insert(key, value.trim()).is_some() {
            return Err(format!("{CLDR_DIR}/SOURCE:{}: `{key}` twice", at + 1));
        }
    }
    let take = |key: &str| {
        values
            .get(key)
            .map(|value| value.to_string())
            .ok_or_else(|| format!("{CLDR_DIR}/SOURCE has no `{key}`"))
    };
    // Where the files came from is for the reader, but it must be written down.
    take("url")?;
    take("licence")?;
    let published = take("published")?;
    Ok(Source {
        release: take("release")?,
        published: NaiveDate::parse_from_str(&published, "%Y-%m-%d").map_err(|e| {
            format!("{CLDR_DIR}/SOURCE: published `{published}` is not a date: {e}")
        })?,
        sha256: take("sha256")?.to_ascii_lowercase(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The XML is the file `SOURCE` pins. Its line endings are read as LF, so a
/// checkout that turned them into CRLF still matches.
fn check_pin(xml: &str, source: &Source) -> Result<(), String> {
    let have = sha256_hex(xml.replace("\r\n", "\n").as_bytes());
    if have == source.sha256 {
        Ok(())
    } else {
        Err(format!(
            "{CLDR_DIR}/windowsZones.xml does not match {CLDR_DIR}/SOURCE: its sha256 is {have}, \
             SOURCE pins {}. Take windowsZones.xml and LICENSE from one CLDR release tag, and \
             write that tag, its publication date, both URLs and the XML's sha256 into SOURCE; \
             never edit the XML.",
            source.sha256
        ))
    }
}

/// One `<mapZone>` row.
#[derive(Debug)]
struct MapZone {
    windows: String,
    territory: String,
    zones: Vec<String>,
    line: usize,
}

/// Every `<mapZone>` row, read strictly: only the elements of the windowsZones
/// format, each in its place and with its attributes, and no text between
/// them. `type` is a list separated by whitespace, and since CLDR 43 one row
/// ends in a space.
fn read_map_zones(xml: &str) -> Result<Vec<MapZone>, String> {
    let line_at = |pos: u64| {
        let end = usize::try_from(pos).map_or(xml.len(), |pos| pos.min(xml.len()));
        xml[..end].matches('\n').count() + 1
    };
    let place = |element: &str| {
        if element.is_empty() {
            "the document".to_string()
        } else {
            format!("<{element}>")
        }
    };
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::new();
    let mut open: Vec<String> = Vec::new();
    let mut map_timezones = 0;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(e) => {
                return Err(format!(
                    "windowsZones.xml:{}: {e}",
                    line_at(reader.error_position())
                ))
            }
        };
        let line = line_at(reader.buffer_position());
        let (element, empty) = match event {
            XmlEvent::Start(element) => (element, false),
            XmlEvent::Empty(element) => (element, true),
            XmlEvent::End(_) => {
                open.pop();
                continue;
            }
            XmlEvent::Decl(_) | XmlEvent::DocType(_) | XmlEvent::Comment(_) => continue,
            XmlEvent::Eof => break,
            XmlEvent::Text(text) => {
                return Err(format!(
                    "windowsZones.xml:{line}: text this reader does not know: {:?}",
                    String::from_utf8_lossy(&text)
                ))
            }
            other => {
                return Err(format!(
                    "windowsZones.xml:{line}: something this reader does not know: {other:?}"
                ))
            }
        };
        let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
        let parent = open.last().map(String::as_str).unwrap_or("");
        let Some((_, within, allowed)) = FORMAT.iter().find(|(known, _, _)| *known == name) else {
            return Err(format!(
                "windowsZones.xml:{line}: an element this reader does not know: <{name}>"
            ));
        };
        if *within != parent {
            return Err(format!(
                "windowsZones.xml:{line}: <{name}> belongs in {}, not in {}",
                place(within),
                place(parent)
            ));
        }
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for attribute in element.attributes() {
            let attribute =
                attribute.map_err(|e| format!("windowsZones.xml:{line}: <{name}>: {e}"))?;
            let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
            if !allowed.contains(&key.as_str()) {
                return Err(format!(
                    "windowsZones.xml:{line}: <{name}> has an attribute this reader does not know: {key}"
                ));
            }
            let value = attribute
                .unescape_value()
                .map_err(|e| format!("windowsZones.xml:{line}: <{name}>: {e}"))?
                .into_owned();
            values.insert(key, value);
        }
        if name == "mapTimezones" {
            map_timezones += 1;
        }
        if name == "mapZone" {
            if !empty {
                return Err(format!("windowsZones.xml:{line}: <mapZone> must be empty"));
            }
            let missing = |what: &str| format!("windowsZones.xml:{line}: <mapZone> has no {what}");
            rows.push(MapZone {
                windows: values.remove("other").ok_or_else(|| missing("other"))?,
                territory: values
                    .remove("territory")
                    .ok_or_else(|| missing("territory"))?,
                zones: values
                    .remove("type")
                    .ok_or_else(|| missing("type"))?
                    .split_ascii_whitespace()
                    .map(str::to_string)
                    .collect(),
                line,
            });
        }
        if !empty {
            open.push(name);
        }
    }
    if map_timezones != 1 {
        return Err(format!(
            "windowsZones.xml holds {map_timezones} <mapTimezones> elements, not one"
        ));
    }
    Ok(rows)
}

/// Every canonical tzdata zone the core knows that is not a UTC name.
fn canonical_zones() -> BTreeSet<&'static str> {
    chrono_tz::TZ_VARIANTS
        .iter()
        .filter_map(|tz| cal_core::canonical_zone(tz.name()))
        .filter(|zone| cal_core::series_clock_zone(Some(zone)).is_some())
        .collect()
}

/// The years the clock guard compares, from the pinned release's publication.
fn clock_window(published: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let until = published
        .checked_add_months(Months::new(CLOCK_MONTHS))
        .ok_or_else(|| format!("{published} plus {CLOCK_MONTHS} months is not a date"))?;
    let midnight =
        |day: NaiveDate| Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).expect("midnight"));
    Ok((midnight(published), midnight(until)))
}

/// A zone whose Windows id runs another clock.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OtherClock {
    windows: String,
    at: DateTime<Utc>,
    zone_seconds: i32,
    windows_seconds: i32,
}

/// The generated tables.
#[derive(Debug, Default)]
struct WindowsTables {
    /// Windows id → its default zone, canonical.
    read: BTreeMap<String, &'static str>,
    /// Canonical zone → the Windows id Exchange stores it under.
    write: BTreeMap<&'static str, String>,
    /// Canonical zones whose CLDR id runs another clock.
    other_clock: BTreeMap<&'static str, OtherClock>,
    /// Canonical zones CLDR has no id for.
    unmapped: BTreeSet<&'static str>,
}

fn build_tables(
    rows: &[MapZone],
    zones: &BTreeSet<&'static str>,
    window: (DateTime<Utc>, DateTime<Utc>),
) -> Result<WindowsTables, String> {
    let mut problems = Vec::new();
    let mut tables = WindowsTables::default();
    let written = |text: &str| {
        !text.is_empty() && text.is_ascii() && !text.contains(['"', '\\']) && text.trim() == text
    };

    // Every spelling, the id it sits under, and where.
    let mut spellings: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
    for row in rows {
        if !written(&row.windows) {
            problems.push(format!(
                "line {}: the Windows id {:?} cannot be written into the table",
                row.line, row.windows
            ));
        }
        for zone in &row.zones {
            if cal_core::canonical_zone(zone).is_none() {
                problems.push(format!(
                    "line {}: {zone} (under {}) is not a tzdata {} name",
                    row.line,
                    row.windows,
                    cal_core::TZDATA_VERSION
                ));
            }
            match spellings.get(zone.as_str()) {
                Some((windows, line)) if *windows != row.windows => problems.push(format!(
                    "{zone} is under both {windows} (line {line}) and {} (line {})",
                    row.windows, row.line
                )),
                Some(_) => {}
                None => {
                    spellings.insert(zone, (&row.windows, row.line));
                }
            }
        }
    }

    // Reading: the one default zone of every id.
    let mut ids: BTreeMap<&str, Vec<&MapZone>> = BTreeMap::new();
    for row in rows {
        ids.entry(&row.windows).or_default().push(row);
    }
    let mut folded_ids: BTreeMap<String, &str> = BTreeMap::new();
    for (windows, rows) in &ids {
        if let Some(other) = folded_ids.insert(windows.to_ascii_lowercase(), windows) {
            problems.push(format!(
                "the Windows ids {other} and {windows} differ only in ASCII case"
            ));
        }
        let defaults: Vec<&&MapZone> = rows.iter().filter(|row| row.territory == "001").collect();
        let [default] = defaults.as_slice() else {
            problems.push(format!(
                "{windows} has {} default (001) rows, not one",
                defaults.len()
            ));
            continue;
        };
        let [zone] = default.zones.as_slice() else {
            problems.push(format!(
                "line {}: the default row of {windows} names {} zones, not one",
                default.line,
                default.zones.len()
            ));
            continue;
        };
        let Some(canonical) = cal_core::canonical_zone(zone) else {
            continue; // refused above
        };
        if cal_core::series_clock_zone(Some(canonical)).is_none() && *windows != "UTC" {
            problems.push(format!(
                "line {}: the default zone of {windows} is the UTC name {zone}; only the id UTC may \
                 read as no zone",
                default.line
            ));
        }
        tables.read.insert(windows.to_string(), canonical);
    }

    // Writing: the id of the row that spells the zone, else the one id its
    // other spellings agree on.
    for &zone in zones {
        let windows = match spellings.get(zone) {
            Some((windows, _)) => Some(windows.to_string()),
            None => {
                let mut under: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
                for (spelling, (windows, _)) in &spellings {
                    if cal_core::canonical_zone(spelling) == Some(zone) {
                        under.entry(*windows).or_default().push(*spelling);
                    }
                }
                match under.len() {
                    0 => None,
                    1 => under.keys().next().map(|windows| windows.to_string()),
                    _ => {
                        let spelled: Vec<String> = under
                            .iter()
                            .map(|(windows, spellings)| {
                                format!("{} under {windows}", spellings.join(", "))
                            })
                            .collect();
                        problems.push(format!(
                            "{zone} has no row of its own, and its other spellings disagree: {}",
                            spelled.join("; ")
                        ));
                        continue;
                    }
                }
            }
        };
        let Some(windows) = windows else {
            tables.unmapped.insert(zone);
            continue;
        };
        let Some(&default) = tables.read.get(&windows) else {
            continue; // the id's own problem is reported above
        };
        match first_difference(zone, default, window) {
            None => {
                tables.write.insert(zone, windows);
            }
            Some((at, zone_seconds, windows_seconds)) => {
                tables.other_clock.insert(
                    zone,
                    OtherClock {
                        windows,
                        at,
                        zone_seconds,
                        windows_seconds,
                    },
                );
            }
        }
    }

    if problems.is_empty() {
        Ok(tables)
    } else {
        problems.sort();
        problems.dedup();
        Err(format!(
            "{CLDR_DIR}/windowsZones.xml cannot be read into a table:\n  {}",
            problems.join("\n  ")
        ))
    }
}

/// The first instant in `window` at which `zone` and `windows_default` read
/// different offsets, with both offsets in seconds.
fn first_difference(
    zone: &str,
    windows_default: &str,
    (from, until): (DateTime<Utc>, DateTime<Utc>),
) -> Option<(DateTime<Utc>, i32, i32)> {
    if zone == windows_default {
        return None;
    }
    let parse = |name: &str| {
        name.parse::<chrono_tz::Tz>()
            .unwrap_or_else(|_| panic!("{name} is a zone cal-core resolved, so chrono-tz knows it"))
    };
    let (zone, windows) = (parse(zone), parse(windows_default));
    let offset = |tz: chrono_tz::Tz, at: DateTime<Utc>| {
        tz.offset_from_utc_datetime(&at.naive_utc())
            .fix()
            .local_minus_utc()
    };
    let mut day = from;
    while day < until {
        let next = (day + Duration::days(1)).min(until);
        let (z, w) = (offset(zone, day), offset(windows, day));
        if z != w {
            return Some((day, z, w));
        }
        if offset(zone, next) != z || offset(windows, next) != w {
            let mut hour = day + Duration::hours(1);
            while hour < next {
                let (z, w) = (offset(zone, hour), offset(windows, hour));
                if z != w {
                    return Some((hour, z, w));
                }
                hour += Duration::hours(1);
            }
        }
        day = next;
    }
    None
}

fn holds_must_have(tables: &WindowsTables) -> Result<(), String> {
    let mut missing = Vec::new();
    for (windows, zone) in READ_MUST_HAVE {
        if tables.read.get(*windows) != Some(zone) {
            missing.push(format!(
                "read {windows} -> {zone} (the table has {:?})",
                tables.read.get(*windows)
            ));
        }
    }
    for (zone, windows) in WRITE_MUST_HAVE {
        if tables.write.get(zone).map(String::as_str) != Some(*windows) {
            missing.push(format!(
                "write {zone} -> {windows} (the table has {:?})",
                tables.write.get(zone)
            ));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the generated table lost rows it must hold:\n  {}",
            missing.join("\n  ")
        ))
    }
}

/// `+01:00`, `-02:30`, from seconds.
fn written_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Sorted with ASCII letters lowercased, which is how the adapter searches.
fn folded_order<T>(rows: impl IntoIterator<Item = (String, T)>) -> Vec<(String, T)> {
    let mut rows: Vec<(String, T)> = rows.into_iter().collect();
    rows.sort_by_key(|(key, _)| key.to_ascii_lowercase());
    rows
}

/// The first 16 hex digits of the sha256 of the table's rows — the pairs and
/// the names, not the comments or the window they were compared in — so the
/// id changes exactly when a translation does.
fn table_id(tables: &WindowsTables) -> String {
    let mut rows = String::new();
    for (windows, zone) in folded_order(tables.read.iter().map(|(w, z)| (w.clone(), *z))) {
        rows.push_str(&format!("read\t{windows}\t{zone}\n"));
    }
    for (zone, windows) in folded_order(tables.write.iter().map(|(z, w)| (z.to_string(), w))) {
        rows.push_str(&format!("write\t{zone}\t{windows}\n"));
    }
    for (zone, other) in folded_order(tables.other_clock.iter().map(|(z, o)| (z.to_string(), o))) {
        rows.push_str(&format!("other clock\t{zone}\t{}\n", other.windows));
    }
    for (zone, _) in folded_order(tables.unmapped.iter().map(|z| (z.to_string(), ()))) {
        rows.push_str(&format!("unmapped\t{zone}\n"));
    }
    sha256_hex(rows.as_bytes())[..16].to_string()
}

/// The generated Rust file. One row per line, so an update reads as a diff of
/// the rows it touched.
fn render_windows_zones(
    source: &Source,
    tables: &WindowsTables,
    (from, until): (DateTime<Utc>, DateTime<Utc>),
) -> String {
    let mut out = format!(
        "// @generated by `cargo xtask windows-zones` from Unicode CLDR {release}\n\
         // common/supplemental/windowsZones.xml (sha256 {sha}),\n\
         // names resolved with cal_core (IANA tzdata {tzdata}), clocks compared {from} to {until}.\n\
         // CLDR data © Unicode, Inc., Unicode License V3: see ../../cldr/LICENSE.\n\
         // Do not edit: run the task and commit what it writes. See ../windows_tz.rs.\n\
         \n\
         /// The CLDR release the table was generated from.\n\
         pub const CLDR_RELEASE: &str = \"{release}\";\n\
         \n\
         /// The IANA tzdata release the names were resolved with.\n\
         pub const TZDATA_VERSION: &str = \"{tzdata}\";\n\
         \n\
         /// The first 16 hex digits of the sha256 of the rows below (not their\n\
         /// comments): which table the cached events were translated with.\n\
         pub const TABLE_ID: &str = \"{table_id}\";\n\
         \n\
         /// Windows id → the zone of its default (001) row, resolved to tzdata's zone.\n\
         /// Sorted by the id with ASCII letters lowercased, which is how it is searched.\n\
         pub const WINDOWS_ZONES: &[(&str, &str)] = &[\n",
        release = source.release,
        sha = source.sha256,
        tzdata = cal_core::TZDATA_VERSION,
        from = from.format("%Y-%m-%d"),
        until = until.format("%Y-%m-%d"),
        table_id = table_id(tables),
    );
    for (windows, zone) in folded_order(tables.read.iter().map(|(w, z)| (w.clone(), *z))) {
        out.push_str(&format!("    (\"{windows}\", \"{zone}\"),\n"));
    }
    out.push_str(
        "];\n\n\
         /// Canonical tzdata zone → the Windows id Exchange stores it under.\n\
         /// Sorted by the zone with ASCII letters lowercased.\n\
         pub const ZONE_WINDOWS: &[(&str, &str)] = &[\n",
    );
    for (zone, windows) in folded_order(tables.write.iter().map(|(z, w)| (z.to_string(), w))) {
        out.push_str(&format!("    (\"{zone}\", \"{windows}\"),\n"));
    }
    out.push_str(
        "];\n\n\
         /// Canonical zones whose CLDR Windows id runs another clock, with that id:\n\
         /// Exchange cannot store them (decision 22a). Sorted like ZONE_WINDOWS.\n\
         pub const OTHER_CLOCK: &[(&str, &str)] = &[\n",
    );
    for (zone, other) in folded_order(tables.other_clock.iter().map(|(z, o)| (z.to_string(), o))) {
        out.push_str(&format!(
            "    (\"{zone}\", \"{}\"), // from {}: {} here, {} on the Windows zone\n",
            other.windows,
            other.at.format("%Y-%m-%dT%H:%MZ"),
            written_offset(other.zone_seconds),
            written_offset(other.windows_seconds),
        ));
    }
    out.push_str(
        "];\n\n\
         /// Canonical zones CLDR has no Windows id for: Exchange cannot store them\n\
         /// (decision 22a). Sorted like ZONE_WINDOWS.\n\
         pub const UNMAPPED: &[&str] = &[\n",
    );
    for (zone, _) in folded_order(tables.unmapped.iter().map(|z| (z.to_string(), ()))) {
        out.push_str(&format!("    \"{zone}\",\n"));
    }
    out.push_str("];\n");
    out
}

/// A generated file read back.
#[derive(Debug, Default)]
struct GeneratedWindows {
    release: Option<String>,
    tzdata: Option<String>,
    read: BTreeMap<String, String>,
    write: BTreeMap<String, String>,
    /// Zone → its Windows id and the comment on its row.
    other_clock: BTreeMap<String, (String, String)>,
    unmapped: BTreeSet<String>,
}

fn read_generated_windows(text: &str) -> GeneratedWindows {
    let mut out = GeneratedWindows::default();
    let constant = |line: &str, name: &str| {
        line.strip_prefix(&format!("pub const {name}: &str = "))
            .map(|value| value.trim_end_matches(';').trim_matches('"').to_string())
    };
    let pair = |row: &str| {
        row.trim_end()
            .strip_prefix("(\"")
            .and_then(|row| row.strip_suffix("\"),"))
            .and_then(|row| row.split_once("\", \""))
            .map(|(a, b)| (a.to_string(), b.to_string()))
    };
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if let Some(value) = constant(line, "CLDR_RELEASE") {
            out.release = Some(value);
        } else if let Some(value) = constant(line, "TZDATA_VERSION") {
            out.tzdata = Some(value);
        } else if line.starts_with("pub const WINDOWS_ZONES") {
            section = "read";
        } else if line.starts_with("pub const ZONE_WINDOWS") {
            section = "write";
        } else if line.starts_with("pub const OTHER_CLOCK") {
            section = "other";
        } else if line.starts_with("pub const UNMAPPED") {
            section = "unmapped";
        } else if line == "];" {
            section = "";
        } else if section == "unmapped" {
            if let Some(zone) = line.strip_prefix('"').and_then(|z| z.strip_suffix("\",")) {
                out.unmapped.insert(zone.to_string());
            }
        } else {
            let (row, note) = match line.split_once(" // ") {
                Some((row, note)) => (row, note.to_string()),
                None => (line, String::new()),
            };
            let Some((key, value)) = pair(row) else {
                continue;
            };
            match section {
                "read" => {
                    out.read.insert(key, value);
                }
                "write" => {
                    out.write.insert(key, value);
                }
                "other" => {
                    out.other_clock.insert(key, (value, note));
                }
                _ => {}
            }
        }
    }
    out
}

/// What moved between the committed table and a fresh one, by NAME.
fn describe_windows_drift(have: &str, fresh: &str) -> Option<String> {
    if have == fresh {
        return None;
    }
    if have.is_empty() {
        return Some(format!("  missing:     {WINDOWS_ZONES} does not exist"));
    }
    let old = read_generated_windows(have);
    let new = read_generated_windows(fresh);
    let none = || "none".to_string();
    let mut lines = Vec::new();
    if old.release != new.release {
        lines.push(format!(
            "  source:      {} -> {}",
            old.release.clone().unwrap_or_else(none),
            new.release.clone().unwrap_or_else(none)
        ));
    }
    if old.tzdata != new.tzdata {
        lines.push(format!(
            "  tzdata:      {} -> {}",
            old.tzdata.clone().unwrap_or_else(none),
            new.tzdata.clone().unwrap_or_else(none)
        ));
    }
    for (windows, zone) in &new.read {
        match old.read.get(windows) {
            None => lines.push(format!("  added id:    {windows} -> {zone}")),
            Some(was) if was != zone => {
                lines.push(format!("  read:        {windows} -> {zone} (was {was})"))
            }
            Some(_) => {}
        }
    }
    for windows in old.read.keys().filter(|w| !new.read.contains_key(*w)) {
        lines.push(format!("  removed id:  {windows}"));
    }
    for (zone, windows) in &new.write {
        match old.write.get(zone) {
            None => lines.push(format!(
                "  write:       {zone} -> {windows} (was not written)"
            )),
            Some(was) if was != windows => {
                lines.push(format!("  write:       {zone} -> {windows} (was {was})"))
            }
            Some(_) => {}
        }
    }
    for (zone, was) in old
        .write
        .iter()
        .filter(|(z, _)| !new.write.contains_key(*z))
    {
        lines.push(format!("  not written: {zone} (was {was})"));
    }
    for (zone, (windows, note)) in &new.other_clock {
        match old.other_clock.get(zone) {
            None => lines.push(format!(
                "  other clock: +{zone} ({windows}) — no longer written to Exchange"
            )),
            Some((was, _)) if was != windows => {
                lines.push(format!("  other clock: {zone} -> {windows} (was {was})"))
            }
            Some((_, was_note)) if was_note != note => {
                lines.push(format!("  other clock: {zone} {note} (was {was_note})"))
            }
            Some(_) => {}
        }
    }
    for (zone, (windows, _)) in old
        .other_clock
        .iter()
        .filter(|(z, _)| !new.other_clock.contains_key(*z))
    {
        lines.push(format!("  other clock: -{zone} ({windows})"));
    }
    for zone in new.unmapped.difference(&old.unmapped) {
        lines.push(format!("  unmapped:    +{zone}"));
    }
    for zone in old.unmapped.difference(&new.unmapped) {
        lines.push(format!("  unmapped:    -{zone}"));
    }
    if lines.is_empty() {
        lines.push(
            "  the rows agree, but the file's text does not (its header, layout or order)"
                .to_string(),
        );
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(rows: &[(&str, &str, &str)]) -> String {
        let rows: String = rows
            .iter()
            .map(|(windows, territory, zones)| {
                format!(
                    "\t\t\t<mapZone other=\"{windows}\" territory=\"{territory}\" type=\"{zones}\"/>\n"
                )
            })
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n\
             <!DOCTYPE supplementalData SYSTEM \"../../common/dtd/ldmlSupplemental.dtd\">\n\
             <!-- a comment -->\n\
             <supplementalData>\n\
             \t<version number=\"$Revision$\"/>\n\
             \t<windowsZones>\n\
             \t\t<mapTimezones otherVersion=\"7e11800\" typeVersion=\"2021a\">\n\
             {rows}\
             \t\t</mapTimezones>\n\
             \t</windowsZones>\n\
             </supplementalData>\n"
        )
    }

    fn window() -> (DateTime<Utc>, DateTime<Utc>) {
        clock_window(NaiveDate::from_ymd_opt(2026, 1, 1).expect("a date")).expect("a window")
    }

    fn tables(rows: &[(&str, &str, &str)]) -> Result<WindowsTables, String> {
        let read = read_map_zones(&xml(rows)).expect("the test file reads");
        build_tables(&read, &canonical_zones(), window())
    }

    fn source(release: &str, published: (i32, u32, u32)) -> Source {
        Source {
            release: release.into(),
            published: NaiveDate::from_ymd_opt(published.0, published.1, published.2)
                .expect("a date"),
            sha256: "abc".into(),
        }
    }

    #[test]
    fn the_reader_splits_types_on_whitespace_and_a_trailing_space() {
        let rows = read_map_zones(&xml(&[(
            "Central Standard Time (Mexico)",
            "MX",
            "America/Mexico_City America/Bahia_Banderas America/Merida America/Monterrey America/Chihuahua ",
        )]))
        .expect("reads");
        assert_eq!(rows[0].zones.len(), 5);
        assert_eq!(rows[0].zones[4], "America/Chihuahua");
    }

    #[test]
    fn the_reader_names_an_element_it_does_not_know_and_its_line() {
        let text = xml(&[("UTC", "001", "Etc/UTC")]).replace(
            "<mapZone other=\"UTC\"",
            "<note/>\n\t\t\t<mapZone other=\"UTC\"",
        );
        let err = read_map_zones(&text).expect_err("an unknown element");
        // The declaration, doctype, comment and three opening elements come
        // first: the first row is on line 8.
        assert!(
            err.contains("<note>") && err.contains("windowsZones.xml:8"),
            "{err}"
        );
    }

    #[test]
    fn the_reader_refuses_an_attribute_it_does_not_know_on_any_element() {
        let base = xml(&[("UTC", "001", "Etc/UTC")]);
        let err = read_map_zones(&base.replace("territory=", "alt=\"x\" territory="))
            .expect_err("an unknown attribute on a row");
        assert!(
            err.contains("<mapZone> has an attribute") && err.contains("alt"),
            "{err}"
        );
        let err =
            read_map_zones(&base.replace("typeVersion=", "draft=\"provisional\" typeVersion="))
                .expect_err("an unknown attribute on mapTimezones");
        assert!(
            err.contains("<mapTimezones> has an attribute") && err.contains("draft"),
            "{err}"
        );
        let err =
            read_map_zones(&base.replace("<supplementalData>", "<supplementalData bogus=\"1\">"))
                .expect_err("an unknown attribute on the root");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn the_reader_refuses_text_and_rows_out_of_place() {
        let base = xml(&[("UTC", "001", "Etc/UTC")]);
        let err = read_map_zones(
            &base.replace("\t\t</mapTimezones>", "stray words\n\t\t</mapTimezones>"),
        )
        .expect_err("text between the rows");
        assert!(err.contains("stray words"), "{err}");
        let err = read_map_zones(&base.replace(
            "\t</windowsZones>",
            "\t\t<mapZone other=\"UTC\" territory=\"ZZ\" type=\"Etc/UTC\"/>\n\t</windowsZones>",
        ))
        .expect_err("a row outside mapTimezones");
        assert!(
            err.contains("<mapZone> belongs in <mapTimezones>, not in <windowsZones>"),
            "{err}"
        );
        let err =
            read_map_zones(&base.replace("type=\"Etc/UTC\"/>", "type=\"Etc/UTC\"></mapZone>"))
                .expect_err("a row with content");
        assert!(err.contains("<mapZone> must be empty"), "{err}");
    }

    #[test]
    fn an_xml_error_names_its_own_line() {
        let text =
            xml(&[("UTC", "001", "Etc/UTC")]).replace("\t\t</mapTimezones>", "\t\t</mapTimezone>");
        let err = read_map_zones(&text).expect_err("a mistyped end tag");
        assert!(err.contains("windowsZones.xml:9"), "{err}");
    }

    #[test]
    fn a_name_tzdata_does_not_know_is_refused_by_name() {
        let err = tables(&[("W. Europe Standard Time", "001", "Europe/Atlantis")])
            .expect_err("an unknown name");
        assert!(
            err.contains("Europe/Atlantis") && err.contains("line 8"),
            "{err}"
        );
    }

    #[test]
    fn a_spelling_under_two_ids_is_refused_but_a_repeat_under_one_is_fine() {
        let fine = tables(&[
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            (
                "W. Europe Standard Time",
                "DE",
                "Europe/Berlin Europe/Busingen",
            ),
        ]);
        assert!(fine.is_ok(), "{fine:?}");
        let err = tables(&[
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("Romance Standard Time", "001", "Europe/Paris"),
            ("Romance Standard Time", "DE", "Europe/Berlin"),
        ])
        .expect_err("Berlin under two ids");
        assert!(
            err.contains("Europe/Berlin is under both W. Europe Standard Time"),
            "{err}"
        );
    }

    #[test]
    fn an_id_needs_exactly_one_default_zone() {
        let err = tables(&[("W. Europe Standard Time", "DE", "Europe/Berlin")])
            .expect_err("no default row");
        assert!(
            err.contains("W. Europe Standard Time has 0 default (001) rows"),
            "{err}"
        );
        let err = tables(&[(
            "W. Europe Standard Time",
            "001",
            "Europe/Berlin Europe/Vienna",
        )])
        .expect_err("two default zones");
        assert!(err.contains("names 2 zones"), "{err}");
    }

    #[test]
    fn the_row_that_spells_the_zone_wins() {
        // tzdata merged Copenhagen into Berlin and Amsterdam into Brussels
        // (25b), but CLDR files them the other way round.
        let t = tables(&[
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("W. Europe Standard Time", "NL", "Europe/Amsterdam"),
            ("Romance Standard Time", "001", "Europe/Paris"),
            ("Romance Standard Time", "BE", "Europe/Brussels"),
            ("Romance Standard Time", "DK", "Europe/Copenhagen"),
        ])
        .expect("reads");
        assert_eq!(
            t.write.get("Europe/Berlin").map(String::as_str),
            Some("W. Europe Standard Time")
        );
        assert_eq!(
            t.write.get("Europe/Brussels").map(String::as_str),
            Some("Romance Standard Time")
        );
    }

    #[test]
    fn a_zone_without_its_own_row_takes_the_one_id_its_spellings_agree_on() {
        let t = tables(&[("India Standard Time", "001", "Asia/Calcutta")]).expect("reads");
        assert_eq!(
            t.write.get("Asia/Kolkata").map(String::as_str),
            Some("India Standard Time")
        );
        assert_eq!(t.read.get("India Standard Time"), Some(&"Asia/Kolkata"));
        // Berlin has no row here; Oslo and Copenhagen, both merged into it,
        // sit under different ids.
        let err = tables(&[
            ("W. Europe Standard Time", "001", "Europe/Oslo"),
            ("Romance Standard Time", "001", "Europe/Copenhagen"),
        ])
        .expect_err("the spellings disagree");
        assert!(
            err.contains("Europe/Berlin has no row of its own, and its other spellings disagree"),
            "{err}"
        );
    }

    #[test]
    fn a_zone_whose_windows_zone_runs_another_clock_is_not_written() {
        let t = tables(&[
            ("Azores Standard Time", "001", "Atlantic/Azores"),
            ("Azores Standard Time", "GL", "America/Scoresbysund"),
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("W. Europe Standard Time", "AT", "Europe/Vienna"),
        ])
        .expect("reads");
        assert!(!t.write.contains_key("America/Scoresbysund"));
        let other = &t.other_clock["America/Scoresbysund"];
        assert_eq!(other.windows, "Azores Standard Time");
        assert_eq!((other.zone_seconds, other.windows_seconds), (-7200, -3600));
        assert_eq!(
            t.write.get("Europe/Vienna").map(String::as_str),
            Some("W. Europe Standard Time"),
            "Vienna reads back as Berlin, on the same clock"
        );
    }

    #[test]
    fn the_clock_guard_finds_an_hour_that_differs_inside_a_day() {
        // Havana and New York both start summer time on 8 March 2026, Havana at
        // midnight local time (05:00Z) and New York at 02:00 (07:00Z): the two
        // agree at the start of the day and the day after, and differ between.
        let (at, zone, windows) = first_difference("America/Havana", "America/New_York", window())
            .expect("the transition hours differ");
        assert_eq!(
            at.format("%Y-%m-%dT%H:%MZ").to_string(),
            "2026-03-08T05:00Z"
        );
        assert_eq!((zone, windows), (-14400, -18000));
    }

    #[test]
    fn the_default_zone_of_an_id_must_not_be_a_utc_name_unless_the_id_is_utc() {
        let err = tables(&[("Greenwich Standard Time", "001", "Etc/GMT")])
            .expect_err("a UTC name as a default");
        assert!(err.contains("the UTC name Etc/GMT"), "{err}");
        assert!(tables(&[("UTC", "001", "Etc/UTC")]).is_ok());
    }

    #[test]
    fn the_source_pin_is_read_and_checked() {
        let source = read_source(
            "# comment\nrelease = release-48-2\npublished = 2026-03-17\nurl = u\nlicence = l\nsha256 = ABC\n",
        )
        .expect("reads");
        assert_eq!(source.release, "release-48-2");
        assert_eq!(source.sha256, "abc");
        let err = check_pin("<x/>", &source).expect_err("another file");
        assert!(
            err.contains("does not match") && err.contains("never edit the XML"),
            "{err}"
        );
        let pinned = Source {
            sha256: sha256_hex(b"<x/>\n"),
            ..source
        };
        // The pin is checked on LF line endings, whatever the checkout wrote.
        assert!(check_pin("<x/>\r\n", &pinned).is_ok());
        assert!(check_pin("<x/>\n", &pinned).is_ok());
        let err = read_source("release = r\n").expect_err("keys missing");
        assert!(err.contains("has no `url`"), "{err}");
        let err = read_source("colour = red\n").expect_err("an unknown key");
        assert!(err.contains("unknown key `colour`"), "{err}");
    }

    #[test]
    fn a_table_without_a_must_have_row_is_refused() {
        let err = holds_must_have(&WindowsTables::default()).expect_err("an empty table");
        assert!(
            err.contains("read W. Europe Standard Time -> Europe/Berlin"),
            "{err}"
        );
        assert!(
            err.contains("write Asia/Beirut -> Middle East Standard Time"),
            "{err}"
        );
    }

    #[test]
    fn the_table_id_follows_the_rows_not_the_text() {
        let rows = [
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("Azores Standard Time", "001", "Atlantic/Azores"),
            ("Azores Standard Time", "GL", "America/Scoresbysund"),
        ];
        let t = tables(&rows).expect("reads");
        let id = |text: &str| {
            text.lines()
                .find_map(|line| constant_value(line, "TABLE_ID"))
                .expect("a TABLE_ID")
        };
        // The same rows under another release and window: other comments, same id.
        let first = render_windows_zones(&source("release-48-2", (2026, 1, 1)), &t, window());
        let later_window =
            clock_window(NaiveDate::from_ymd_opt(2026, 10, 1).expect("a date")).expect("a window");
        let second = render_windows_zones(&source("release-49", (2026, 10, 1)), &t, later_window);
        assert_ne!(first, second);
        assert_eq!(id(&first), id(&second));
        // A moved row is another translation.
        let moved = tables(&[
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("W. Europe Standard Time", "AT", "Europe/Vienna"),
            ("Azores Standard Time", "001", "Atlantic/Azores"),
            ("Azores Standard Time", "GL", "America/Scoresbysund"),
        ])
        .expect("reads");
        let third = render_windows_zones(&source("release-48-2", (2026, 1, 1)), &moved, window());
        assert_ne!(id(&first), id(&third));
    }

    fn constant_value(line: &str, name: &str) -> Option<String> {
        line.strip_prefix(&format!("pub const {name}: &str = "))
            .map(|value| value.trim_end_matches(';').trim_matches('"').to_string())
    }

    #[test]
    fn the_generated_table_reads_back_and_its_drift_is_named() {
        let rows = [
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("W. Europe Standard Time", "AT", "Europe/Vienna"),
            ("Azores Standard Time", "001", "Atlantic/Azores"),
            ("Azores Standard Time", "GL", "America/Scoresbysund"),
        ];
        let old = render_windows_zones(
            &source("release-48-2", (2026, 1, 1)),
            &tables(&rows).expect("reads"),
            window(),
        );
        let back = read_generated_windows(&old);
        assert_eq!(back.release.as_deref(), Some("release-48-2"));
        assert_eq!(back.tzdata.as_deref(), Some(cal_core::TZDATA_VERSION));
        assert_eq!(back.read["W. Europe Standard Time"], "Europe/Berlin");
        assert_eq!(back.write["Europe/Vienna"], "W. Europe Standard Time");
        assert_eq!(
            back.other_clock["America/Scoresbysund"].0,
            "Azores Standard Time"
        );
        assert!(back.unmapped.contains("Antarctica/Troll"));
        assert_eq!(describe_windows_drift(&old, &old), None);

        let moved = [
            ("W. Europe Standard Time", "001", "Europe/Berlin"),
            ("Romance Standard Time", "001", "Europe/Paris"),
            ("Romance Standard Time", "AT", "Europe/Vienna"),
            ("Azores Standard Time", "001", "Atlantic/Azores"),
        ];
        let fresh = render_windows_zones(
            &source("release-49", (2026, 1, 1)),
            &tables(&moved).expect("reads"),
            window(),
        );
        let drift = describe_windows_drift(&old, &fresh).expect("rows moved");
        assert!(
            drift.contains("source:      release-48-2 -> release-49"),
            "{drift}"
        );
        assert!(
            drift.contains("added id:    Romance Standard Time -> Europe/Paris"),
            "{drift}"
        );
        assert!(
            drift.contains(
                "write:       Europe/Vienna -> Romance Standard Time (was W. Europe Standard Time)"
            ),
            "{drift}"
        );
        assert!(
            drift.contains("other clock: -America/Scoresbysund"),
            "{drift}"
        );
        assert!(
            drift.contains("unmapped:    +America/Scoresbysund"),
            "{drift}"
        );

        // A zone that stays on another clock, but under another id or with
        // another difference, is named too.
        let other_id = old.replace(
            "(\"America/Scoresbysund\", \"Azores Standard Time\")",
            "(\"America/Scoresbysund\", \"Cape Verde Standard Time\")",
        );
        let drift = describe_windows_drift(&old, &other_id).expect("the id moved");
        assert!(
            drift.contains(
                "other clock: America/Scoresbysund -> Cape Verde Standard Time (was Azores Standard Time)"
            ),
            "{drift}"
        );
        let other_note = old.replace("from 2026-01-01T00:00Z", "from 2027-03-28T01:00Z");
        let drift = describe_windows_drift(&old, &other_note).expect("the note moved");
        assert!(
            drift.contains("other clock: America/Scoresbysund from 2027-03-28T01:00Z"),
            "{drift}"
        );

        assert!(describe_windows_drift(
            &old,
            &old.replace("// Do not edit", "// Please do not edit")
        )
        .expect("the text differs")
        .contains("the rows agree"),);
    }
}
