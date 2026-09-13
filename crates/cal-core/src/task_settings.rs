//! The task settings: how the stored task and calendar preferences read, what
//! a list's effective coupling, auto-date and carry-over are, and what is
//! stored when a setting changes.
//!
//! Was written twice — the desktop `TaskCascadeProvider` and the mobile
//! `taskBehaviour.ts` — each with its own copy of every rule, reading the same
//! synced preferences each device writes for the other. The core reads them
//! once. The surfaces hand over the stored strings, and a key whose read
//! failed is simply not stored: only that setting falls back to its default
//! (decided 2026-09-13; mobile used to fall back for EVERY setting). Pinned by
//! `tests/fixtures/taskSettings.json`, measured on the real code of both
//! surfaces before the port; the `contract` module below reads it, and so does
//! the TypeScript contract test, which still runs both surfaces.
//!
//! # Read as JavaScript read it
//!
//! Rule by rule, the strings read the way the TypeScript read them. A
//! default-on switch is off only on the exact string `false`, the opt-in on
//! only on `true`. Numbers go through `parseInt` — leading JavaScript
//! whitespace, a sign, then ASCII digits, so `12abc` is 12, `1e2` is 1 and
//! `0x10` is 0 — and snap with `Math.round`, which rounds a half towards
//! positive infinity. Enumerations accept only their spelled members. The
//! override map keeps the key order a JavaScript object gives it: array-index
//! keys first in ascending order, then the rest as written, a repeated key
//! keeping its first place and its last value.
//!
//! One JavaScript artifact is gone on purpose: a list whose id is `__proto__`
//! is an ordinary list. The TypeScript lost it as a field and applied it
//! through the prototype anyway.
//!
//! # One day-start trigger
//!
//! [`day_start_trigger`] reads `tasks.dayStartTrigger` as the surfaces offer
//! it: `app-start`, `00:00`, `06:00`, `08:00` or `12:00`, anything else
//! `00:00`. `host_core::reminders` anchors all-day reminders with the same
//! reading (decided 2026-09-13); it used to accept any `HH:MM`.

use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::day_start::CarryOverDefault;
use crate::signatures::is_js_whitespace;

/// The countdown window when none is stored, and its bounds.
pub const DEFAULT_COUNTDOWN_DAYS: u32 = 3;
const MIN_COUNTDOWN_DAYS: f64 = 1.0;
const MAX_COUNTDOWN_DAYS: f64 = 30.0;
/// The visible day window's end when none is stored: the whole day.
const MINUTES_PER_DAY: u32 = 24 * 60;
/// The day-start triggers the surfaces offer.
pub const DAY_START_TRIGGERS: [&str; 5] = ["app-start", "00:00", "06:00", "08:00", "12:00"];

// ─────────────────────────────── The wire ───────────────────────────────────

/// How the check-off gesture advances a task's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum CheckoffMode {
    /// Open and completed, back and forth.
    #[default]
    Toggle,
    /// Open, in progress, completed.
    Cycle,
}

/// How the calendar's day and week views lay out events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum DayViewMode {
    #[default]
    Grid,
    List,
}

/// The stored strings, one per preference; `None` when not stored — or when
/// reading it failed.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskSettingsStored {
    /// `tasks.cascadeStatusCoupling`
    #[serde(default)]
    pub cascade: Option<String>,
    /// `tasks.autoDateOnStart`
    #[serde(default)]
    pub auto_date: Option<String>,
    /// `tasks.autoSelfAssign`
    #[serde(default)]
    pub auto_self_assign: Option<String>,
    /// `tasks.visualEffortSizing`
    #[serde(default)]
    pub visual_effort_sizing: Option<String>,
    /// `tasks.twoLevelPriority`
    #[serde(default)]
    pub two_level_priority: Option<String>,
    /// `tasks.remindUntimedToday`
    #[serde(default)]
    pub remind_untimed_today: Option<String>,
    /// `tasks.remindDeadlineArrived`
    #[serde(default)]
    pub remind_deadline_arrived: Option<String>,
    /// `tasks.remindDeadlineCountdown`
    #[serde(default)]
    pub remind_deadline_countdown: Option<String>,
    /// `tasks.deadlineCountdownDays`
    #[serde(default)]
    pub deadline_countdown_days: Option<String>,
    /// `calendar.dayViewMode`
    #[serde(default)]
    pub day_view_mode: Option<String>,
    /// `calendar.dayStartMin`
    #[serde(default)]
    pub day_start_min: Option<String>,
    /// `calendar.dayEndMin`
    #[serde(default)]
    pub day_end_min: Option<String>,
    /// `tasks.checkoffMode`
    #[serde(default)]
    pub checkoff_mode: Option<String>,
    /// `tasks.carryOverDefault`
    #[serde(default)]
    pub carry_over_default: Option<String>,
    /// `tasks.dayStartTrigger`
    #[serde(default)]
    pub day_start_trigger: Option<String>,
    /// `tasks.listOverrides`, a JSON object keyed by list id.
    #[serde(default)]
    pub list_overrides: Option<String>,
}

/// One list's override: any subset of the three per-list settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskListOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub cascade: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub auto_date: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub carry_over_default: Option<CarryOverDefault>,
}

impl TaskListOverride {
    fn is_empty(&self) -> bool {
        self.cascade.is_none() && self.auto_date.is_none() && self.carry_over_default.is_none()
    }
}

/// A list's override, with the list it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskListOverrideEntry {
    pub list_id: String,
    pub settings: TaskListOverride,
}

/// The settings, read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskSettingsRead {
    pub cascade_enabled: bool,
    pub auto_date: bool,
    pub auto_self_assign: bool,
    pub visual_effort_sizing: bool,
    pub two_level_priority: bool,
    pub remind_untimed_today: bool,
    pub remind_deadline_arrived: bool,
    pub remind_deadline_countdown: bool,
    /// 1..=30.
    pub deadline_countdown_days: u32,
    pub day_view_mode: DayViewMode,
    /// Minutes from midnight, on the half hour.
    pub day_start_min: u32,
    /// Minutes from midnight, on the half hour, after the start.
    pub day_end_min: u32,
    pub checkoff_mode: CheckoffMode,
    pub carry_over_default: CarryOverDefault,
    /// One of [`DAY_START_TRIGGERS`].
    pub day_start_trigger: String,
    /// In the key order a JavaScript object gives them.
    pub list_overrides: Vec<TaskListOverrideEntry>,
}

/// The three global settings a list override can override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskListGlobals {
    pub cascade_enabled: bool,
    pub auto_date: bool,
    pub carry_over_default: CarryOverDefault,
}

/// A list's settings in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TaskListEffective {
    pub cascade: bool,
    pub auto_date: bool,
    pub carry_over_default: CarryOverDefault,
}

/// The visible day window as it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct DayWindowMinutes {
    pub start_min: u32,
    pub end_min: u32,
}

/// One question to the task-settings rules.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum TaskSettingsQuestion {
    /// → [`TaskSettingsRead`], see [`read`]. Boxed: sixteen strings dwarf the
    /// other questions.
    Read { stored: Box<TaskSettingsStored> },
    /// → [`TaskListEffective`], see [`effective`].
    Effective {
        globals: TaskListGlobals,
        /// The list's own override, if it has one.
        #[serde(default)]
        list_override: Option<TaskListOverride>,
    },
    /// → days, see [`countdown_days_to_store`]. A number that is not finite
    /// travels as null.
    CountdownDaysToStore {
        #[serde(default)]
        value: Option<f64>,
    },
    /// → [`DayWindowMinutes`], see [`day_window_to_store`].
    DayWindowToStore {
        #[serde(default)]
        start_min: Option<f64>,
        #[serde(default)]
        end_min: Option<f64>,
    },
    /// → the override map, see [`with_list_override`].
    WithListOverride {
        #[serde(default)]
        list_overrides: Vec<TaskListOverrideEntry>,
        list_id: String,
        list_override: TaskListOverride,
    },
}

// ─────────────────────────────── Reading ────────────────────────────────────

/// `parseInt(text, 10)`: leading JavaScript whitespace, an optional sign, then
/// ASCII digits; `None` where JavaScript answers `NaN`.
fn js_parse_int(text: &str) -> Option<f64> {
    let text = text.trim_start_matches(is_js_whitespace);
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') => ("-", &text[1..]),
        Some(b'+') => ("", &text[1..]),
        _ => ("", text),
    };
    let digits = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    format!("{sign}{digits}").parse().ok()
}

/// `Math.round`: the nearest integer, a half towards positive infinity.
fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

fn clamp_countdown(days: f64) -> u32 {
    days.clamp(MIN_COUNTDOWN_DAYS, MAX_COUNTDOWN_DAYS) as u32
}

/// A minute snapped to the half hour and clamped to the day; `fallback` for a
/// value that is not a finite number.
fn snap_minute(value: Option<f64>, fallback: u32) -> u32 {
    match value.filter(|v| v.is_finite()) {
        Some(v) => (js_round(v / 30.0) * 30.0).clamp(0.0, f64::from(MINUTES_PER_DAY)) as u32,
        None => fallback,
    }
}

/// The member a stored carry-over spells, if it spells one.
fn carry_over_member(raw: &str) -> Option<CarryOverDefault> {
    match raw {
        "ask" => Some(CarryOverDefault::Ask),
        "today" => Some(CarryOverDefault::Today),
        "backlog" => Some(CarryOverDefault::Backlog),
        _ => None,
    }
}

/// The stored day-start trigger as the surfaces read it: one of the offered
/// values, anything else `00:00`.
pub fn day_start_trigger(raw: Option<&str>) -> &'static str {
    DAY_START_TRIGGERS
        .iter()
        .copied()
        .find(|offered| Some(*offered) == raw)
        .unwrap_or("00:00")
}

/// A JSON document's top level: an object's entries in document order, a
/// repeated key keeping its first place and its last value, or anything else.
enum TopLevel {
    Object(Vec<(String, Value)>),
    Other,
}

impl<'de> Deserialize<'de> for TopLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TopLevelVisitor)
    }
}

struct TopLevelVisitor;

impl<'de> Visitor<'de> for TopLevelVisitor {
    type Value = TopLevel;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TopLevel, A::Error> {
        let mut entries: Vec<(String, Value)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key, value)),
            }
        }
        Ok(TopLevel::Object(entries))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<TopLevel, A::Error> {
        while seq.next_element::<Value>()?.is_some() {}
        Ok(TopLevel::Other)
    }

    fn visit_bool<E>(self, _: bool) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }

    fn visit_i64<E>(self, _: i64) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }

    fn visit_u64<E>(self, _: u64) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }

    fn visit_f64<E>(self, _: f64) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }

    fn visit_str<E>(self, _: &str) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }

    fn visit_unit<E>(self) -> Result<TopLevel, E> {
        Ok(TopLevel::Other)
    }
}

/// A JavaScript array index: `0`, or a canonical decimal below 2³² − 1.
fn array_index(key: &str) -> Option<u32> {
    if key == "0" {
        return Some(0);
    }
    let bytes = key.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 10
        || bytes[0] == b'0'
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|&n| n < 4_294_967_295)
        .and_then(|n| u32::try_from(n).ok())
}

/// The key order of a JavaScript object: array indices ascending, then the
/// rest in insertion order.
fn js_key_order(mut entries: Vec<TaskListOverrideEntry>) -> Vec<TaskListOverrideEntry> {
    entries.sort_by_key(|e| array_index(&e.list_id).map_or((1, 0), |n| (0, n)));
    entries
}

/// The stored override map, checked per list and per field: an entry that is
/// not an object, or keeps no valid field, is dropped.
fn read_list_overrides(raw: Option<&str>) -> Vec<TaskListOverrideEntry> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Vec::new();
    };
    let Ok(TopLevel::Object(entries)) = serde_json::from_str::<TopLevel>(raw) else {
        return Vec::new();
    };
    let read: Vec<TaskListOverrideEntry> = entries
        .into_iter()
        .filter_map(|(list_id, value)| {
            let Value::Object(fields) = value else {
                return None;
            };
            let settings = TaskListOverride {
                cascade: fields.get("cascade").and_then(Value::as_bool),
                auto_date: fields.get("autoDate").and_then(Value::as_bool),
                carry_over_default: fields
                    .get("carryOverDefault")
                    .and_then(Value::as_str)
                    .and_then(carry_over_member),
            };
            (!settings.is_empty()).then_some(TaskListOverrideEntry { list_id, settings })
        })
        .collect();
    js_key_order(read)
}

// ─────────────────────────────── The rules ──────────────────────────────────

/// The settings from their stored strings. Nothing stored — or nothing
/// readable — is the default.
pub fn read(stored: &TaskSettingsStored) -> TaskSettingsRead {
    let on = |raw: &Option<String>| raw.as_deref() != Some("false");
    let window = day_window_to_store(
        stored
            .day_start_min
            .as_deref()
            .map_or(Some(0.0), js_parse_int),
        stored
            .day_end_min
            .as_deref()
            .map_or(Some(f64::from(MINUTES_PER_DAY)), js_parse_int),
    );
    TaskSettingsRead {
        cascade_enabled: on(&stored.cascade),
        auto_date: on(&stored.auto_date),
        auto_self_assign: on(&stored.auto_self_assign),
        visual_effort_sizing: on(&stored.visual_effort_sizing),
        two_level_priority: stored.two_level_priority.as_deref() == Some("true"),
        remind_untimed_today: on(&stored.remind_untimed_today),
        remind_deadline_arrived: on(&stored.remind_deadline_arrived),
        remind_deadline_countdown: on(&stored.remind_deadline_countdown),
        deadline_countdown_days: stored
            .deadline_countdown_days
            .as_deref()
            .and_then(js_parse_int)
            .map_or(DEFAULT_COUNTDOWN_DAYS, clamp_countdown),
        day_view_mode: if stored.day_view_mode.as_deref() == Some("list") {
            DayViewMode::List
        } else {
            DayViewMode::Grid
        },
        day_start_min: window.start_min,
        day_end_min: window.end_min,
        checkoff_mode: if stored.checkoff_mode.as_deref() == Some("cycle") {
            CheckoffMode::Cycle
        } else {
            CheckoffMode::Toggle
        },
        carry_over_default: stored
            .carry_over_default
            .as_deref()
            .and_then(carry_over_member)
            .unwrap_or_default(),
        day_start_trigger: day_start_trigger(stored.day_start_trigger.as_deref()).to_string(),
        list_overrides: read_list_overrides(stored.list_overrides.as_deref()),
    }
}

/// A list's settings in effect: its override per field, else the global.
pub fn effective(
    globals: &TaskListGlobals,
    list_override: Option<&TaskListOverride>,
) -> TaskListEffective {
    let own = list_override.copied().unwrap_or_default();
    TaskListEffective {
        cascade: own.cascade.unwrap_or(globals.cascade_enabled),
        auto_date: own.auto_date.unwrap_or(globals.auto_date),
        carry_over_default: own.carry_over_default.unwrap_or(globals.carry_over_default),
    }
}

/// The countdown window as it is stored: rounded, clamped to 1..=30, and the
/// default for a number that is not finite.
pub fn countdown_days_to_store(value: Option<f64>) -> u32 {
    value
        .filter(|v| v.is_finite())
        .map_or(DEFAULT_COUNTDOWN_DAYS, |v| clamp_countdown(js_round(v)))
}

/// The visible day window as it is stored: each edge snapped to the half hour
/// and clamped to the day, and the whole day when the start is not before the
/// end.
pub fn day_window_to_store(start_min: Option<f64>, end_min: Option<f64>) -> DayWindowMinutes {
    let start = snap_minute(start_min, 0);
    let end = snap_minute(end_min, MINUTES_PER_DAY);
    if start >= end {
        DayWindowMinutes {
            start_min: 0,
            end_min: MINUTES_PER_DAY,
        }
    } else {
        DayWindowMinutes {
            start_min: start,
            end_min: end,
        }
    }
}

/// The override map with `list_id` set to `settings`: an existing list keeps
/// its place, a new one joins in JavaScript key order, and an empty override
/// clears the list.
pub fn with_list_override(
    mut entries: Vec<TaskListOverrideEntry>,
    list_id: &str,
    settings: TaskListOverride,
) -> Vec<TaskListOverrideEntry> {
    let at = entries.iter().position(|e| e.list_id == list_id);
    match (settings.is_empty(), at) {
        (true, Some(i)) => {
            entries.remove(i);
        }
        (true, None) => {}
        (false, Some(i)) => entries[i].settings = settings,
        (false, None) => entries.push(TaskListOverrideEntry {
            list_id: list_id.to_string(),
            settings,
        }),
    }
    js_key_order(entries)
}

/// The task-settings rules over the wire: one [`TaskSettingsQuestion`] in, its
/// rule's answer out.
pub fn task_settings_json(input_json: &str) -> Result<String, serde_json::Error> {
    match serde_json::from_str::<TaskSettingsQuestion>(input_json)? {
        TaskSettingsQuestion::Read { stored } => serde_json::to_string(&read(&stored)),
        TaskSettingsQuestion::Effective {
            globals,
            list_override,
        } => serde_json::to_string(&effective(&globals, list_override.as_ref())),
        TaskSettingsQuestion::CountdownDaysToStore { value } => {
            serde_json::to_string(&countdown_days_to_store(value))
        }
        TaskSettingsQuestion::DayWindowToStore { start_min, end_min } => {
            serde_json::to_string(&day_window_to_store(start_min, end_min))
        }
        TaskSettingsQuestion::WithListOverride {
            list_overrides,
            list_id,
            list_override,
        } => serde_json::to_string(&with_list_override(list_overrides, &list_id, list_override)),
    }
}

#[cfg(test)]
mod reading {
    use super::*;

    #[test]
    fn parse_int_reads_like_javascript() {
        assert_eq!(js_parse_int("12abc"), Some(12.0));
        assert_eq!(js_parse_int("1e2"), Some(1.0));
        assert_eq!(js_parse_int("0x10"), Some(0.0));
        assert_eq!(js_parse_int(" \u{00A0}\u{FEFF}7"), Some(7.0));
        assert_eq!(js_parse_int("+4"), Some(4.0));
        assert_eq!(js_parse_int("-5"), Some(-5.0));
        assert_eq!(js_parse_int("+-5"), None);
        assert_eq!(js_parse_int(""), None);
        assert_eq!(js_parse_int("\u{0663}"), None);
        // NEL is not JavaScript whitespace.
        assert_eq!(js_parse_int("\u{0085}7"), None);
    }

    #[test]
    fn round_takes_a_half_towards_positive_infinity() {
        assert_eq!(js_round(14.5), 15.0);
        assert_eq!(js_round(-14.5), -14.0);
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(js_round(2.4), 2.0);
    }

    #[test]
    fn array_index_keys_come_first_in_ascending_order() {
        let entry = |id: &str| TaskListOverrideEntry {
            list_id: id.into(),
            settings: TaskListOverride {
                cascade: Some(true),
                ..TaskListOverride::default()
            },
        };
        let ids: Vec<String> = js_key_order(vec![
            entry("b"),
            entry("12"),
            entry("007"),
            entry("7"),
            entry("4294967295"),
            entry("a"),
            entry("0"),
        ])
        .into_iter()
        .map(|e| e.list_id)
        .collect();
        assert_eq!(ids, ["0", "7", "12", "b", "007", "4294967295", "a"]);
    }

    #[test]
    fn a_repeated_key_keeps_its_first_place_and_its_last_value() {
        let read = read_list_overrides(Some(
            r#"{"a":{"cascade":true},"b":{"autoDate":false},"a":{"cascade":false}}"#,
        ));
        assert_eq!(read[0].list_id, "a");
        assert_eq!(read[0].settings.cascade, Some(false));
        assert_eq!(read[1].list_id, "b");
    }

    #[test]
    fn the_trigger_is_one_of_the_offered_values() {
        assert_eq!(day_start_trigger(Some("06:00")), "06:00");
        assert_eq!(day_start_trigger(Some("app-start")), "app-start");
        assert_eq!(day_start_trigger(Some("07:00")), "00:00");
        assert_eq!(day_start_trigger(Some("08:00:00")), "00:00");
        assert_eq!(day_start_trigger(None), "00:00");
    }
}

/// The contract with the TypeScript this replaced: `tests/fixtures/taskSettings.json`,
/// measured on both surfaces. Its other half is
/// `src/state/taskSettings.contract.test.tsx`, which still runs both
/// surfaces on every row. Rows the port changed on purpose say so.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskSettings.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn cases<'d>(doc: &'d Value, section: &str) -> &'d [Value] {
        doc[section].as_array().expect("a section is an array")
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    /// A case's stored strings; the failing key reads as not stored.
    fn stored(doc: &Value, input: &Value) -> TaskSettingsStored {
        let get = |name: &str| -> Option<String> {
            let key = doc["keys"][name].as_str().expect("a named key");
            if input["failing"].as_str() == Some(key) {
                return None;
            }
            input["prefs"]
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        TaskSettingsStored {
            cascade: get("cascade"),
            auto_date: get("autoDate"),
            auto_self_assign: get("autoSelfAssign"),
            visual_effort_sizing: get("visualEffortSizing"),
            two_level_priority: get("twoLevelPriority"),
            remind_untimed_today: get("remindUntimedToday"),
            remind_deadline_arrived: get("remindDeadlineArrived"),
            remind_deadline_countdown: get("remindDeadlineCountdown"),
            deadline_countdown_days: get("deadlineCountdownDays"),
            day_view_mode: get("dayViewMode"),
            day_start_min: get("dayStartMin"),
            day_end_min: get("dayEndMin"),
            checkoff_mode: get("checkoffMode"),
            carry_over_default: get("carryOverDefault"),
            day_start_trigger: get("dayStartTrigger"),
            list_overrides: get("listOverrides"),
        }
    }

    /// The override fields as the surfaces store them, in their fixed order.
    fn stored_fields(settings: &TaskListOverride) -> String {
        let mut fields = Vec::new();
        if let Some(cascade) = settings.cascade {
            fields.push(format!("\"cascade\":{cascade}"));
        }
        if let Some(auto_date) = settings.auto_date {
            fields.push(format!("\"autoDate\":{auto_date}"));
        }
        if let Some(carry) = settings.carry_over_default {
            fields.push(format!(
                "\"carryOverDefault\":{}",
                serde_json::to_string(&carry).expect("a carry-over serialises")
            ));
        }
        format!("{{{}}}", fields.join(","))
    }

    /// The override map as the JSON the surfaces store, in entry order.
    fn stored_map(entries: &[TaskListOverrideEntry]) -> String {
        let pairs: Vec<String> = entries
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    serde_json::to_string(&e.list_id).expect("an id serialises"),
                    stored_fields(&e.settings)
                )
            })
            .collect();
        format!("{{{}}}", pairs.join(","))
    }

    fn override_from(value: &Value) -> TaskListOverride {
        TaskListOverride {
            cascade: value["cascade"].as_bool(),
            auto_date: value["autoDate"].as_bool(),
            carry_over_default: value["carryOverDefault"]
                .as_str()
                .and_then(carry_over_member),
        }
    }

    /// A fixture number: "NaN", "Infinity" and "-Infinity" are not finite and
    /// travel as null.
    fn num(value: &Value) -> Option<f64> {
        value.as_f64()
    }

    fn settings_json(s: &TaskSettingsRead) -> Value {
        let overrides: serde_json::Map<String, Value> = s
            .list_overrides
            .iter()
            .map(|e| {
                (
                    e.list_id.clone(),
                    serde_json::from_str(&stored_fields(&e.settings)).expect("fields parse"),
                )
            })
            .collect();
        json!({
            "cascadeEnabled": s.cascade_enabled,
            "autoDate": s.auto_date,
            "autoSelfAssign": s.auto_self_assign,
            "visualEffortSizing": s.visual_effort_sizing,
            "twoLevelPriority": s.two_level_priority,
            "remindUntimedToday": s.remind_untimed_today,
            "remindDeadlineArrived": s.remind_deadline_arrived,
            "remindDeadlineCountdown": s.remind_deadline_countdown,
            "deadlineCountdownDays": s.deadline_countdown_days,
            "dayViewMode": s.day_view_mode,
            "dayStartMin": s.day_start_min,
            "dayEndMin": s.day_end_min,
            "checkoffMode": s.checkoff_mode,
            "carryOverDefault": s.carry_over_default,
            "dayStartTrigger": s.day_start_trigger,
            "listOverrides": overrides,
        })
    }

    #[test]
    fn the_surfaces_no_longer_differ() {
        let doc = doc();
        for section in [
            "read",
            "effective",
            "countdownWrite",
            "dayWindowWrite",
            "overrideUpdate",
        ] {
            for case in cases(&doc, section) {
                assert!(
                    case.get("desktop").is_none(),
                    "{section}: {} still carries a desktop answer",
                    case["name"]
                );
            }
        }
    }

    #[test]
    fn every_read_row_holds() {
        let doc = doc();
        let rows = cases(&doc, "read");
        for name in [
            "an-on-knob-turns-off-only-on-a-literal-false",
            "countdown-days-trailing-garbage-is-ignored",
            "an-out-of-order-window-is-the-whole-day",
            "a-trigger-off-the-list-is-midnight",
            "a-bad-field-leaves-the-good-ones",
            "one-failed-read",
        ] {
            has_row(rows, name);
        }
        for case in rows {
            let got = settings_json(&read(&stored(&doc, &case["input"])));
            assert_eq!(got, case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_effective_row_holds() {
        let doc = doc();
        let rows = cases(&doc, "effective");
        has_row(rows, "an-override-wins-per-field");
        for case in rows {
            let settings = read(&stored(&doc, &case["input"]));
            let list_id = case["input"]["listId"].as_str().expect("a list id");
            let globals = TaskListGlobals {
                cascade_enabled: settings.cascade_enabled,
                auto_date: settings.auto_date,
                carry_over_default: settings.carry_over_default,
            };
            let own = settings
                .list_overrides
                .iter()
                .find(|e| e.list_id == list_id)
                .map(|e| e.settings);
            let got = effective(&globals, own.as_ref());
            assert_eq!(
                json!({
                    "cascade": got.cascade,
                    "autoDate": got.auto_date,
                    "carryOverDefault": got.carry_over_default,
                }),
                case["expect"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn every_countdown_write_row_holds() {
        let doc = doc();
        let rows = cases(&doc, "countdownWrite");
        has_row(rows, "infinity-is-written-as-the-default");
        for case in rows {
            let got = countdown_days_to_store(num(&case["input"]["value"])).to_string();
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_day_window_write_row_holds() {
        let doc = doc();
        let rows = cases(&doc, "dayWindowWrite");
        has_row(rows, "an-edge-between-half-hours-snaps");
        for case in rows {
            let w = day_window_to_store(num(&case["input"]["start"]), num(&case["input"]["end"]));
            assert_eq!(
                json!({ "start": w.start_min.to_string(), "end": w.end_min.to_string() }),
                case["expect"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn every_override_update_row_holds() {
        let doc = doc();
        let rows = cases(&doc, "overrideUpdate");
        has_row(rows, "an-update-keeps-its-place");
        has_row(rows, "fields-are-written-in-a-fixed-order");
        for case in rows {
            let input = &case["input"];
            let entries = read_list_overrides(input["stored"].as_str());
            let updated = with_list_override(
                entries,
                input["listId"].as_str().expect("a list id"),
                override_from(&input["override"]),
            );
            assert_eq!(
                json!(stored_map(&updated)),
                case["expect"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn the_door_answers_the_rule_it_was_asked() {
        assert_eq!(
            task_settings_json(r#"{"rule":"countdown_days_to_store","value":null}"#)
                .expect("a question"),
            "3"
        );
        let read = task_settings_json(r#"{"rule":"read","stored":{"day_start_trigger":"07:00"}}"#)
            .expect("a question");
        assert!(read.contains(r#""day_start_trigger":"00:00""#), "{read}");
        assert!(task_settings_json(r#"{"rule":"tomorrow"}"#).is_err());
    }
}
