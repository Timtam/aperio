//! A repeat rule in words (decision 84a).
//!
//! The editors show a rule as controls — a frequency, an interval, weekday
//! buttons. An invitation someone else organizes is read-only (77a), so there
//! are no controls to read, and a rule the controls cannot rebuild is shown
//! today as something other than what is stored. Both need one sentence:
//! "every Monday, 5 times", „jeden Montag, 5 Mal".
//!
//! What the sentence says is a rule, so it lives here; the words live in the
//! surfaces' locale files. This module answers with keys and values —
//! `every`, an optional `on`, an optional `end` — and
//! `shared/recurrenceSummary.ts` puts them together in the reader's language,
//! with the weekday and month names the platform already knows.
//!
//! The rule is read by [`crate::rrule_parts`], the same reading that moves a
//! series, so a rule this module describes is a rule the app can also move,
//! and a rule neither can read is refused by both.
//!
//! **A shape this module does not word is named, not guessed.** Nothing here
//! falls back to "the rule's first weekday" or drops a part it does not
//! understand: [`RecurrenceSummary::Undescribed`] says so, and the surfaces
//! read "Repeats monthly by a rule Aperio cannot put into words." That is a
//! sentence a reader can act on; a wrong sentence is not.
//!
//! The core reads no clock and knows no zones, so the day an `UNTIL` rule
//! last falls on is worked out by the shell, which expands the series anyway,
//! and handed in as [`RecurrenceSummaryQuestion::last_day`] (decision 85a).

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::rrule_parts::{
    parse_freq, parse_interval, parse_parts, parse_weekday_token, part, Freq, Part,
    WEEKDAYS_FROM_MONDAY,
};
use crate::types::Weekday;

/// Everything the sentence needs.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct RecurrenceSummaryQuestion {
    /// The rule, with or without the `RRULE:` prefix. Empty: no repeat.
    pub rrule: String,
    /// The day the series starts, `YYYY-MM-DD` on the series' own clock. A
    /// rule leaves out what its start already says ("every month" repeats on
    /// the start's day of the month).
    pub start: String,
    /// The day the last occurrence of a bounded rule falls on, same clock,
    /// when the shell worked it out. `UNTIL` is written three ways by three
    /// providers, and it is a bound, not an occurrence: only the expander the
    /// views already use can say which day the series really ends on
    /// (decision 85a). Without it the sentence says "with an end date".
    #[serde(default)]
    pub last_day: Option<String>,
}

/// One part of the sentence: a key the surfaces look up, and the values it
/// takes. The renderer puts no rule of its own on top — it looks up names and
/// joins lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct Phrase {
    /// The full i18n key, for example `recurrenceSummary.every.week`.
    pub key: String,
    /// i18next's `count`, which also picks the plural form.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub count: Option<u32>,
    /// Days of the week, Monday first whatever order the rule listed them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<Weekday>,
    /// One day of the week.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub weekday: Option<Weekday>,
    /// A position in the month or year: 1 to 5, or -1 for the last.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub ordinal: Option<i8>,
    /// Days of the month, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub month_days: Vec<u8>,
    /// A month, 1 to 12.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub month: Option<u8>,
    /// A day, `YYYY-MM-DD`, for the end of a bounded rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub date: Option<String>,
    /// A set of days a position counts within.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub set: Option<DaySet>,
}

impl Phrase {
    fn new(key: &str) -> Self {
        Self {
            key: format!("{KEY_PREFIX}{key}"),
            count: None,
            weekdays: Vec::new(),
            weekday: None,
            ordinal: None,
            month_days: Vec::new(),
            month: None,
            date: None,
            set: None,
        }
    }
}

/// The set a position counts within, for rules written with `BYSETPOS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum DaySet {
    /// Monday to Friday.
    Weekday,
    /// Saturday and Sunday.
    WeekendDay,
}

/// The unit a rule repeats in, for a rule that cannot be put into words but
/// whose `FREQ` was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum RepeatUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl RepeatUnit {
    const fn key(self) -> &'static str {
        match self {
            Self::Second => "second",
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

/// Why a rule was not put into words. Each one names a shape, so the surfaces
/// can say what kind of rule it is and a later version can word more of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum UndescribedReason {
    /// The rule, its `FREQ`, a value in it or the start day cannot be read, or
    /// a part is given twice.
    Unreadable,
    /// A part this module does not word, such as `BYWEEKNO` or `BYYEARDAY`.
    UnknownPart,
    /// A `BYSETPOS` over a set of days that is not "the workdays", "the
    /// weekend" or "every day".
    SetPosition,
    /// A position this module does not word, such as the second-to-last
    /// Friday.
    Ordinal,
    /// Several positions in one rule ("the first and third Monday").
    SeveralPositions,
    /// The rule recurs only in some months.
    LimitedMonths,
    /// Days of the month counted from both ends, or several from the end.
    MixedMonthDays,
    /// `COUNT=0`: the rule names no occurrence at all.
    NoOccurrences,
    /// Every other week with a week that does not start on Monday: which days
    /// share a week then depends on `WKST`.
    WeekStart,
    /// `COUNT` and `UNTIL` together, which RFC 5545 forbids and readers
    /// disagree about.
    CountAndUntil,
}

/// The sentence, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum RecurrenceSummary {
    /// The event does not repeat.
    None,
    /// The sentence: `key` takes `every`, `on` and `end` as values.
    Described {
        key: String,
        every: Phrase,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        on: Option<Phrase>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        end: Option<Phrase>,
    },
    /// The rule says something this module does not word.
    Undescribed {
        reason: UndescribedReason,
        /// Filled whenever `FREQ` was read, so the surfaces can still say
        /// "repeats monthly".
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        unit: Option<RepeatUnit>,
    },
}

/// Every key this module can emit. The locale test reads it, so a key without
/// a sentence in either language is a failing test and not a silent gap.
pub const SUMMARY_KEYS: &[&str] = &[
    "recurrenceSummary.sentence.every",
    "recurrenceSummary.sentence.everyOn",
    "recurrenceSummary.sentence.everyEnd",
    "recurrenceSummary.sentence.everyOnEnd",
    "recurrenceSummary.every.second",
    "recurrenceSummary.every.minute",
    "recurrenceSummary.every.hour",
    "recurrenceSummary.every.day",
    "recurrenceSummary.every.month",
    "recurrenceSummary.every.year",
    "recurrenceSummary.interval.second",
    "recurrenceSummary.interval.minute",
    "recurrenceSummary.interval.hour",
    "recurrenceSummary.interval.day",
    "recurrenceSummary.interval.week",
    "recurrenceSummary.interval.month",
    "recurrenceSummary.interval.year",
    "recurrenceSummary.weekly.onDays",
    "recurrenceSummary.weekly.workdays",
    "recurrenceSummary.on.weekdays",
    "recurrenceSummary.on.monthDay",
    "recurrenceSummary.on.lastDay",
    "recurrenceSummary.on.nthWeekday",
    "recurrenceSummary.on.nthOfSet",
    "recurrenceSummary.on.dateInYear",
    "recurrenceSummary.on.nthWeekdayInMonth",
    "recurrenceSummary.end.count",
    "recurrenceSummary.end.lastOn",
    "recurrenceSummary.end.bounded",
];

const KEY_PREFIX: &str = "recurrenceSummary.";

/// The rule in words, or why it cannot be.
pub fn describe_recurrence(question: &RecurrenceSummaryQuestion) -> RecurrenceSummary {
    if question.rrule.trim().is_empty() {
        return RecurrenceSummary::None;
    }
    match describe(question) {
        Ok(summary) => summary,
        Err(Undescribed { reason, unit }) => RecurrenceSummary::Undescribed { reason, unit },
    }
}

/// The door: a [`RecurrenceSummaryQuestion`] as JSON in, a
/// [`RecurrenceSummary`] as JSON out.
pub fn recurrence_summary_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: RecurrenceSummaryQuestion = serde_json::from_str(input_json)?;
    serde_json::to_string(&describe_recurrence(&question))
}

/// A shape that has no sentence, with the unit when `FREQ` was read.
struct Undescribed {
    reason: UndescribedReason,
    unit: Option<RepeatUnit>,
}

fn undescribed(reason: UndescribedReason, unit: Option<RepeatUnit>) -> Undescribed {
    Undescribed { reason, unit }
}

type Described = Result<RecurrenceSummary, Undescribed>;

fn describe(question: &RecurrenceSummaryQuestion) -> Described {
    use UndescribedReason::*;

    let (_, parts) = parse_parts(&question.rrule).map_err(|_| undescribed(Unreadable, None))?;
    let freq = part(&parts, "FREQ")
        .and_then(parse_freq)
        .ok_or_else(|| undescribed(Unreadable, None))?;
    let unit = Some(match freq {
        Freq::Secondly => RepeatUnit::Second,
        Freq::Minutely => RepeatUnit::Minute,
        Freq::Hourly => RepeatUnit::Hour,
        Freq::Daily => RepeatUnit::Day,
        Freq::Weekly => RepeatUnit::Week,
        Freq::Monthly => RepeatUnit::Month,
        Freq::Yearly => RepeatUnit::Year,
    });
    let interval =
        parse_interval(part(&parts, "INTERVAL")).ok_or_else(|| undescribed(Unreadable, unit))?;

    for p in &parts {
        match p.key.as_str() {
            "FREQ" | "INTERVAL" | "COUNT" | "UNTIL" | "BYDAY" | "BYMONTHDAY" | "BYMONTH"
            | "BYSETPOS" | "WKST" => {}
            _ => return Err(undescribed(UnknownPart, unit)),
        }
    }

    let end = end_phrase(&parts, question.last_day.as_deref(), unit)?;
    let every = every_phrase(freq, interval, unit);
    let on = on_phrase(&parts, question, freq, interval, unit)?;

    // A weekly rule with an interval of one says its days itself ("every
    // Monday and Wednesday"), so it needs no second half.
    let (every, on) = match on {
        OnPart::Replaces(phrase) => (phrase, None),
        OnPart::Beside(phrase) => (every, Some(phrase)),
        OnPart::None => (every, None),
    };

    let key = match (on.is_some(), end.is_some()) {
        (false, false) => "sentence.every",
        (true, false) => "sentence.everyOn",
        (false, true) => "sentence.everyEnd",
        (true, true) => "sentence.everyOnEnd",
    };
    Ok(RecurrenceSummary::Described {
        key: format!("{KEY_PREFIX}{key}"),
        every,
        on,
        end,
    })
}

/// How often: "every week" or "every 2 weeks". Apple and Outlook say it that
/// way, and it needs no ordinal ("every 2nd week") in either language.
fn every_phrase(freq: Freq, interval: u32, unit: Option<RepeatUnit>) -> Phrase {
    let unit_key = unit.expect("the unit is known once FREQ parsed").key();
    let _ = freq;
    if interval == 1 {
        Phrase::new(&format!("every.{unit_key}"))
    } else {
        let mut phrase = Phrase::new(&format!("interval.{unit_key}"));
        phrase.count = Some(interval);
        phrase
    }
}

/// The end of a bounded rule.
fn end_phrase(
    parts: &[Part],
    last_day: Option<&str>,
    unit: Option<RepeatUnit>,
) -> Result<Option<Phrase>, Undescribed> {
    use UndescribedReason::*;
    let count = part(parts, "COUNT");
    let until = part(parts, "UNTIL");
    if count.is_some() && until.is_some() {
        // RFC 5545 §3.3.10 allows only one of them; readers disagree on which
        // wins, so there is no one sentence.
        return Err(undescribed(CountAndUntil, unit));
    }
    if let Some(count) = count {
        let times: u32 = count.parse().map_err(|_| undescribed(Unreadable, unit))?;
        if times == 0 {
            return Err(undescribed(NoOccurrences, unit));
        }
        let mut phrase = Phrase::new("end.count");
        phrase.count = Some(times);
        return Ok(Some(phrase));
    }
    if until.is_some() {
        return Ok(Some(
            match last_day.map(str::trim).filter(|d| !d.is_empty()) {
                Some(day) => {
                    let mut phrase = Phrase::new("end.lastOn");
                    phrase.date = Some(day.to_string());
                    phrase
                }
                None => Phrase::new("end.bounded"),
            },
        ));
    }
    Ok(None)
}

/// What the `on` half of the sentence is, if there is one.
enum OnPart {
    /// A weekly rule says its days in the "every" half.
    Replaces(Phrase),
    Beside(Phrase),
    None,
}

fn on_phrase(
    parts: &[Part],
    question: &RecurrenceSummaryQuestion,
    freq: Freq,
    interval: u32,
    unit: Option<RepeatUnit>,
) -> Result<OnPart, Undescribed> {
    use UndescribedReason::*;

    let by_day = part(parts, "BYDAY");
    let by_month_day = part(parts, "BYMONTHDAY");
    let by_month = part(parts, "BYMONTH");
    let by_set_pos = part(parts, "BYSETPOS");
    let sub_daily = matches!(freq, Freq::Secondly | Freq::Minutely | Freq::Hourly);

    if by_month.is_some() && !matches!(freq, Freq::Yearly) {
        // "Every day in March" and the like: the rule runs in some months only.
        return Err(undescribed(LimitedMonths, unit));
    }
    if sub_daily && (by_day.is_some() || by_month_day.is_some() || by_set_pos.is_some()) {
        return Err(undescribed(UnknownPart, unit));
    }

    match freq {
        Freq::Secondly | Freq::Minutely | Freq::Hourly => Ok(OnPart::None),
        Freq::Daily => {
            if by_month_day.is_some() || by_set_pos.is_some() {
                return Err(undescribed(UnknownPart, unit));
            }
            match by_day {
                // "Every day, on Mondays and Wednesdays" is the weekly rule
                // written the other way round — but only without an interval:
                // "every 2 days on Monday" counts days, not weeks.
                Some(value) if interval == 1 => {
                    let days = plain_weekdays(value, unit)?;
                    Ok(OnPart::Replaces(weekly_days_phrase(&days)))
                }
                Some(_) => Err(undescribed(UnknownPart, unit)),
                None => Ok(OnPart::None),
            }
        }
        Freq::Weekly => {
            if by_month_day.is_some() || by_set_pos.is_some() {
                return Err(undescribed(UnknownPart, unit));
            }
            let days = match by_day {
                Some(value) => plain_weekdays(value, unit)?,
                // A weekly rule without days repeats on the start's day.
                None => vec![start_weekday(question, unit)?],
            };
            if interval > 1 && days.len() > 1 {
                if let Some(week_start) = part(parts, "WKST") {
                    if !week_start.trim().eq_ignore_ascii_case("MO") {
                        // Which days share a week depends on where the week
                        // begins, so "every 2 weeks on Sunday and Monday"
                        // means different days for different readers.
                        return Err(undescribed(WeekStart, unit));
                    }
                }
            }
            if interval == 1 {
                Ok(OnPart::Replaces(weekly_days_phrase(&days)))
            } else {
                let mut phrase = Phrase::new("on.weekdays");
                phrase.weekdays = days;
                Ok(OnPart::Beside(phrase))
            }
        }
        Freq::Monthly => {
            if let Some(set_pos) = by_set_pos {
                let days = by_day.ok_or_else(|| undescribed(SetPosition, unit))?;
                return Ok(OnPart::Beside(set_position_phrase(days, set_pos, unit)?));
            }
            if let Some(value) = by_month_day {
                return Ok(OnPart::Beside(month_days_phrase(value, unit)?));
            }
            if let Some(value) = by_day {
                return Ok(OnPart::Beside(nth_weekday_phrase(value, unit)?));
            }
            // Nothing else: the start's day of the month.
            let start = start_date(question, unit)?;
            let mut phrase = Phrase::new("on.monthDay");
            phrase.month_days = vec![start.day() as u8];
            phrase.count = Some(1);
            Ok(OnPart::Beside(phrase))
        }
        Freq::Yearly => {
            let month = match by_month {
                Some(value) => single_month(value, unit)?,
                None => start_date(question, unit)?.month() as u8,
            };
            let ordinal_day = match (by_day, by_set_pos) {
                (Some(days), Some(set_pos)) => {
                    let mut phrase = set_position_phrase(days, set_pos, unit)?;
                    // A set position in a year without a month would count
                    // within the whole year, which is not what it says here.
                    if by_month.is_none() {
                        return Err(undescribed(UnknownPart, unit));
                    }
                    phrase.month = Some(month);
                    return Ok(OnPart::Beside(phrase));
                }
                (Some(days), None) => {
                    let phrase = nth_weekday_phrase(days, unit)?;
                    if by_month.is_none() {
                        // RFC 5545 §3.3.10: without a month, "2MO" is the
                        // second Monday of the YEAR, not of a month.
                        return Err(undescribed(UnknownPart, unit));
                    }
                    Some(phrase)
                }
                (None, Some(_)) => return Err(undescribed(SetPosition, unit)),
                (None, None) => None,
            };
            if let Some(mut phrase) = ordinal_day {
                if phrase.key.ends_with("on.lastDay") {
                    return Err(undescribed(UnknownPart, unit));
                }
                phrase.key = format!("{KEY_PREFIX}on.nthWeekdayInMonth");
                phrase.month = Some(month);
                return Ok(OnPart::Beside(phrase));
            }
            let day = match by_month_day {
                Some(value) => single_month_day(value, unit)?,
                None => start_date(question, unit)?.day() as u8,
            };
            let mut phrase = Phrase::new("on.dateInYear");
            phrase.month = Some(month);
            phrase.month_days = vec![day];
            Ok(OnPart::Beside(phrase))
        }
    }
}

/// "every Monday and Wednesday", or the workday phrase for Monday to Friday.
fn weekly_days_phrase(days: &[Weekday]) -> Phrase {
    const WORKDAYS: [Weekday; 5] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
    ];
    if days == WORKDAYS {
        return Phrase::new("weekly.workdays");
    }
    let mut phrase = Phrase::new("weekly.onDays");
    phrase.weekdays = days.to_vec();
    phrase
}

/// `BYDAY` as plain weekdays, Monday first, each once. An ordinal token
/// ("2MO") is not a plain weekday.
fn plain_weekdays(value: &str, unit: Option<RepeatUnit>) -> Result<Vec<Weekday>, Undescribed> {
    use UndescribedReason::*;
    let mut days: Vec<Weekday> = Vec::new();
    for token in value.split(',') {
        match parse_weekday_token(token) {
            Some((None, day)) => {
                if !days.contains(&day) {
                    days.push(day);
                }
            }
            Some((Some(_), _)) => return Err(undescribed(Ordinal, unit)),
            None => return Err(undescribed(Unreadable, unit)),
        }
    }
    if days.is_empty() {
        return Err(undescribed(Unreadable, unit));
    }
    days.sort_by_key(|day| WEEKDAYS_FROM_MONDAY.iter().position(|d| d == day));
    Ok(days)
}

/// `BYMONTHDAY`: days counted from the start of the month, or the last day.
fn month_days_phrase(value: &str, unit: Option<RepeatUnit>) -> Result<Phrase, Undescribed> {
    use UndescribedReason::*;
    let mut days: Vec<i8> = Vec::new();
    for token in value.split(',') {
        let day: i8 = token
            .trim()
            .parse()
            .map_err(|_| undescribed(Unreadable, unit))?;
        if day == 0 || day > 31 || day < -31 {
            return Err(undescribed(Unreadable, unit));
        }
        if !days.contains(&day) {
            days.push(day);
        }
    }
    if days == [-1] {
        return Ok(Phrase::new("on.lastDay"));
    }
    if days.iter().any(|d| *d < 0) {
        // "The 20th and the second-to-last day" counts from both ends.
        return Err(undescribed(MixedMonthDays, unit));
    }
    days.sort_unstable();
    let mut phrase = Phrase::new("on.monthDay");
    phrase.count = Some(days.len() as u32);
    phrase.month_days = days.into_iter().map(|d| d as u8).collect();
    Ok(phrase)
}

/// `BYDAY` with one ordinal: "the second Tuesday", "the last Friday".
fn nth_weekday_phrase(value: &str, unit: Option<RepeatUnit>) -> Result<Phrase, Undescribed> {
    use UndescribedReason::*;
    let tokens: Vec<&str> = value.split(',').collect();
    if tokens.len() > 1 {
        // "The first and the third Monday" is two positions in one rule.
        return Err(undescribed(SeveralPositions, unit));
    }
    match parse_weekday_token(tokens[0]) {
        Some((Some(ordinal), day)) => {
            if !(1..=5).contains(&ordinal) && ordinal != -1 {
                // "The second-to-last Friday" has no plain sentence.
                return Err(undescribed(Ordinal, unit));
            }
            let mut phrase = Phrase::new("on.nthWeekday");
            phrase.ordinal = Some(ordinal);
            phrase.weekday = Some(day);
            Ok(phrase)
        }
        // A monthly rule on plain weekdays ("every Monday of the month")
        // repeats on all of them, which the app does not write and this
        // module does not word.
        Some((None, _)) => Err(undescribed(UnknownPart, unit)),
        None => Err(undescribed(Unreadable, unit)),
    }
}

/// `BYSETPOS` over a set of days: "the last workday", "the first weekend day",
/// or, over all seven days, simply that day of the month.
fn set_position_phrase(
    days: &str,
    set_pos: &str,
    unit: Option<RepeatUnit>,
) -> Result<Phrase, Undescribed> {
    use UndescribedReason::*;
    let positions: Vec<&str> = set_pos.split(',').collect();
    if positions.len() > 1 {
        return Err(undescribed(SeveralPositions, unit));
    }
    let position: i8 = positions[0]
        .trim()
        .parse()
        .map_err(|_| undescribed(Unreadable, unit))?;
    if !(1..=5).contains(&position) && position != -1 {
        return Err(undescribed(Ordinal, unit));
    }
    let days = plain_weekdays(days, unit)?;
    const WORKDAYS: [Weekday; 5] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
    ];
    const WEEKEND: [Weekday; 2] = [Weekday::Saturday, Weekday::Sunday];
    if days == WEEKDAYS_FROM_MONDAY {
        // Any day of the month, counted: that is the day of the month itself.
        if position == -1 {
            return Ok(Phrase::new("on.lastDay"));
        }
        let mut phrase = Phrase::new("on.monthDay");
        phrase.count = Some(1);
        phrase.month_days = vec![position as u8];
        return Ok(phrase);
    }
    let set = if days == WORKDAYS {
        DaySet::Weekday
    } else if days == WEEKEND {
        DaySet::WeekendDay
    } else {
        return Err(undescribed(SetPosition, unit));
    };
    let mut phrase = Phrase::new("on.nthOfSet");
    phrase.ordinal = Some(position);
    phrase.set = Some(set);
    Ok(phrase)
}

/// `BYMONTH` naming exactly one month.
fn single_month(value: &str, unit: Option<RepeatUnit>) -> Result<u8, Undescribed> {
    use UndescribedReason::*;
    let months: Vec<&str> = value.split(',').collect();
    if months.len() > 1 {
        // "In March and September" repeats in some months only.
        return Err(undescribed(LimitedMonths, unit));
    }
    let month: u8 = months[0]
        .trim()
        .parse()
        .map_err(|_| undescribed(Unreadable, unit))?;
    if !(1..=12).contains(&month) {
        return Err(undescribed(Unreadable, unit));
    }
    Ok(month)
}

/// `BYMONTHDAY` naming exactly one day from the start of the month.
fn single_month_day(value: &str, unit: Option<RepeatUnit>) -> Result<u8, Undescribed> {
    use UndescribedReason::*;
    let days: Vec<&str> = value.split(',').collect();
    if days.len() > 1 {
        return Err(undescribed(MixedMonthDays, unit));
    }
    let day: i8 = days[0]
        .trim()
        .parse()
        .map_err(|_| undescribed(Unreadable, unit))?;
    if !(1..=31).contains(&day) {
        // A yearly rule on the last day of a month is rare and has no plain
        // sentence here.
        return Err(undescribed(MixedMonthDays, unit));
    }
    Ok(day as u8)
}

fn start_date(
    question: &RecurrenceSummaryQuestion,
    unit: Option<RepeatUnit>,
) -> Result<NaiveDate, Undescribed> {
    NaiveDate::parse_from_str(question.start.trim(), "%Y-%m-%d")
        .map_err(|_| undescribed(UndescribedReason::Unreadable, unit))
}

fn start_weekday(
    question: &RecurrenceSummaryQuestion,
    unit: Option<RepeatUnit>,
) -> Result<Weekday, Undescribed> {
    let start = start_date(question, unit)?;
    Ok(WEEKDAYS_FROM_MONDAY[start.weekday().num_days_from_monday() as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(rrule: &str) -> RecurrenceSummary {
        describe_recurrence(&RecurrenceSummaryQuestion {
            rrule: rrule.into(),
            start: "2026-06-15".into(), // a Monday
            last_day: None,
        })
    }

    fn ask_until(rrule: &str, last_day: Option<&str>) -> RecurrenceSummary {
        describe_recurrence(&RecurrenceSummaryQuestion {
            rrule: rrule.into(),
            start: "2026-06-15".into(),
            last_day: last_day.map(str::to_string),
        })
    }

    fn described(summary: &RecurrenceSummary) -> (&str, &Phrase, Option<&Phrase>, Option<&Phrase>) {
        match summary {
            RecurrenceSummary::Described {
                key,
                every,
                on,
                end,
            } => (key, every, on.as_ref(), end.as_ref()),
            other => panic!("expected a sentence, got {other:?}"),
        }
    }

    fn reason(summary: &RecurrenceSummary) -> (UndescribedReason, Option<RepeatUnit>) {
        match summary {
            RecurrenceSummary::Undescribed { reason, unit } => (*reason, *unit),
            other => panic!("expected no sentence, got {other:?}"),
        }
    }

    #[test]
    fn no_rule_is_no_sentence() {
        assert_eq!(ask(""), RecurrenceSummary::None);
        assert_eq!(ask("   "), RecurrenceSummary::None);
    }

    #[test]
    fn a_weekly_rule_says_its_days_itself() {
        let summary = ask("FREQ=WEEKLY;BYDAY=WE,MO");
        let (key, every, on, end) = described(&summary);
        assert_eq!(key, "recurrenceSummary.sentence.every");
        assert_eq!(every.key, "recurrenceSummary.weekly.onDays");
        // Monday first, whatever order the rule listed them.
        assert_eq!(every.weekdays, [Weekday::Monday, Weekday::Wednesday]);
        assert!(on.is_none() && end.is_none());

        // Without days it repeats on the day it starts.
        let summary = ask("FREQ=WEEKLY");
        let (_, every, _, _) = described(&summary);
        assert_eq!(every.weekdays, [Weekday::Monday]);

        // Monday to Friday has its own phrase.
        let summary = ask("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR");
        let (_, every, _, _) = described(&summary);
        assert_eq!(every.key, "recurrenceSummary.weekly.workdays");
        assert!(every.weekdays.is_empty());

        // A daily rule on given weekdays is the same rule.
        let summary = ask("FREQ=DAILY;BYDAY=MO,WE");
        let (_, every, _, _) = described(&summary);
        assert_eq!(every.key, "recurrenceSummary.weekly.onDays");
        assert_eq!(every.weekdays, [Weekday::Monday, Weekday::Wednesday]);
    }

    #[test]
    fn an_interval_counts_units_and_names_the_days_beside_it() {
        let summary = ask("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH");
        let (key, every, on, _) = described(&summary);
        assert_eq!(key, "recurrenceSummary.sentence.everyOn");
        assert_eq!(every.key, "recurrenceSummary.interval.week");
        assert_eq!(every.count, Some(2));
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.weekdays");
        assert_eq!(on.weekdays, [Weekday::Tuesday, Weekday::Thursday]);

        let (_, every, on, _) = {
            let s = ask("FREQ=DAILY;INTERVAL=3");
            let (k, e, o, n) = described(&s);
            assert_eq!(k, "recurrenceSummary.sentence.every");
            (k.to_string(), e.clone(), o.cloned(), n.cloned())
        };
        assert_eq!(every.key, "recurrenceSummary.interval.day");
        assert_eq!(every.count, Some(3));
        assert!(on.is_none());
    }

    #[test]
    fn a_monthly_rule_says_which_day() {
        // A day of the month.
        let summary = ask("FREQ=MONTHLY;BYMONTHDAY=15,1");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.monthDay");
        assert_eq!(on.month_days, [1, 15]);
        assert_eq!(on.count, Some(2));

        // The last day.
        let summary = ask("FREQ=MONTHLY;BYMONTHDAY=-1");
        let (_, _, on, _) = described(&summary);
        assert_eq!(on.unwrap().key, "recurrenceSummary.on.lastDay");

        // A position: the last Friday.
        let summary = ask("FREQ=MONTHLY;BYDAY=-1FR");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.nthWeekday");
        assert_eq!((on.ordinal, on.weekday), (Some(-1), Some(Weekday::Friday)));

        // Nothing else: the day the series starts on.
        let summary = ask("FREQ=MONTHLY");
        let (_, _, on, _) = described(&summary);
        assert_eq!(on.unwrap().month_days, [15]);
    }

    #[test]
    fn a_set_position_is_worded_only_for_sets_with_a_name() {
        // The last workday.
        let summary = ask("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.nthOfSet");
        assert_eq!((on.ordinal, on.set), (Some(-1), Some(DaySet::Weekday)));

        // The first weekend day.
        let summary = ask("FREQ=MONTHLY;BYDAY=SA,SU;BYSETPOS=1");
        let (_, _, on, _) = described(&summary);
        assert_eq!(on.unwrap().set, Some(DaySet::WeekendDay));

        // All seven days: that is simply the day of the month.
        let summary = ask("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYSETPOS=3");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.monthDay");
        assert_eq!(on.month_days, [3]);

        // Any other set has no sentence.
        assert_eq!(
            reason(&ask("FREQ=MONTHLY;BYDAY=MO,WE;BYSETPOS=2")).0,
            UndescribedReason::SetPosition
        );
    }

    #[test]
    fn a_yearly_rule_names_its_month() {
        let summary = ask("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=17");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.dateInYear");
        assert_eq!((on.month, on.month_days.as_slice()), (Some(3), &[17][..]));

        // Halves the rule leaves out come from the start.
        let summary = ask("FREQ=YEARLY");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!((on.month, on.month_days.as_slice()), (Some(6), &[15][..]));

        // A position within a named month.
        let summary = ask("FREQ=YEARLY;BYMONTH=3;BYDAY=2TU");
        let (_, _, on, _) = described(&summary);
        let on = on.unwrap();
        assert_eq!(on.key, "recurrenceSummary.on.nthWeekdayInMonth");
        assert_eq!(
            (on.ordinal, on.weekday, on.month),
            (Some(2), Some(Weekday::Tuesday), Some(3))
        );

        // Without a month, a position counts within the whole year: no
        // sentence rather than a wrong one.
        assert_eq!(
            reason(&ask("FREQ=YEARLY;BYDAY=2TU")).0,
            UndescribedReason::UnknownPart
        );
        // Several months is a rule that runs in some months only.
        assert_eq!(
            reason(&ask("FREQ=YEARLY;BYMONTH=3,9;BYMONTHDAY=17")).0,
            UndescribedReason::LimitedMonths
        );
    }

    #[test]
    fn the_end_is_a_count_or_the_day_the_shell_worked_out() {
        let summary = ask("FREQ=WEEKLY;BYDAY=MO;COUNT=5");
        let (key, _, _, end) = described(&summary);
        assert_eq!(key, "recurrenceSummary.sentence.everyEnd");
        let end = end.unwrap();
        assert_eq!(end.key, "recurrenceSummary.end.count");
        assert_eq!(end.count, Some(5));

        // With the last day: the day itself. Without: that it ends at all.
        let summary = ask_until(
            "FREQ=WEEKLY;BYDAY=MO;UNTIL=20270331T235959Z",
            Some("2027-03-29"),
        );
        let (_, _, _, end) = described(&summary);
        let end = end.unwrap();
        assert_eq!(end.key, "recurrenceSummary.end.lastOn");
        assert_eq!(end.date.as_deref(), Some("2027-03-29"));
        let summary = ask_until("FREQ=WEEKLY;BYDAY=MO;UNTIL=20270331T235959Z", None);
        let (_, _, _, end) = described(&summary);
        assert_eq!(end.unwrap().key, "recurrenceSummary.end.bounded");

        // Both at once is invalid, and no reader agrees which one wins.
        assert_eq!(
            reason(&ask("FREQ=WEEKLY;COUNT=2;UNTIL=20270331T235959Z")).0,
            UndescribedReason::CountAndUntil
        );
        assert_eq!(
            reason(&ask("FREQ=WEEKLY;COUNT=0")).0,
            UndescribedReason::NoOccurrences
        );
    }

    #[test]
    fn a_shape_without_a_sentence_says_which_kind_it_is() {
        let cases = [
            (
                "FREQ=WEEKLY;BYWEEKNO=3",
                UndescribedReason::UnknownPart,
                RepeatUnit::Week,
            ),
            (
                "FREQ=DAILY;BYYEARDAY=100",
                UndescribedReason::UnknownPart,
                RepeatUnit::Day,
            ),
            (
                "FREQ=MONTHLY;BYDAY=1MO,3MO",
                UndescribedReason::SeveralPositions,
                RepeatUnit::Month,
            ),
            (
                "FREQ=MONTHLY;BYDAY=-2FR",
                UndescribedReason::Ordinal,
                RepeatUnit::Month,
            ),
            (
                "FREQ=MONTHLY;BYMONTHDAY=20,-2",
                UndescribedReason::MixedMonthDays,
                RepeatUnit::Month,
            ),
            (
                "FREQ=DAILY;BYMONTH=3",
                UndescribedReason::LimitedMonths,
                RepeatUnit::Day,
            ),
            (
                "FREQ=MONTHLY;BYDAY=MO",
                UndescribedReason::UnknownPart,
                RepeatUnit::Month,
            ),
            (
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,MO;WKST=SU",
                UndescribedReason::WeekStart,
                RepeatUnit::Week,
            ),
            (
                "FREQ=HOURLY;BYDAY=MO",
                UndescribedReason::UnknownPart,
                RepeatUnit::Hour,
            ),
            (
                "FREQ=WEEKLY;INTERVAL=0",
                UndescribedReason::Unreadable,
                RepeatUnit::Week,
            ),
        ];
        for (rrule, expected, unit) in cases {
            let (got, got_unit) = reason(&ask(rrule));
            assert_eq!(got, expected, "{rrule}");
            assert_eq!(got_unit, Some(unit), "{rrule}");
        }
        // Before FREQ is read there is no unit either.
        assert_eq!(
            reason(&ask("FREQ=FORTNIGHTLY")),
            (UndescribedReason::Unreadable, None)
        );
        assert_eq!(
            reason(&ask("BYDAY=MO")),
            (UndescribedReason::Unreadable, None)
        );
        assert_eq!(
            reason(&ask("FREQ=WEEKLY;BYDAY=MO;byday=TU")),
            (UndescribedReason::Unreadable, None)
        );
    }

    #[test]
    fn an_hourly_rule_and_a_prefix_are_read_like_everywhere_else() {
        let summary = ask("rrule:freq=hourly;interval=6");
        let (_, every, _, _) = described(&summary);
        assert_eq!(every.key, "recurrenceSummary.interval.hour");
        assert_eq!(every.count, Some(6));
        let summary = ask("FREQ=SECONDLY");
        let (_, every, _, _) = described(&summary);
        assert_eq!(every.key, "recurrenceSummary.every.second");
    }

    #[test]
    fn every_key_the_module_emits_is_listed() {
        // Anti-silence: the locale test reads SUMMARY_KEYS, so a key that is
        // emitted but not listed would never be checked for a sentence.
        let rules = [
            "",
            "FREQ=DAILY",
            "FREQ=DAILY;INTERVAL=2",
            "FREQ=WEEKLY;BYDAY=MO,WE",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU",
            "FREQ=MONTHLY",
            "FREQ=MONTHLY;BYMONTHDAY=1,15",
            "FREQ=MONTHLY;BYMONTHDAY=-1",
            "FREQ=MONTHLY;BYDAY=2TU",
            "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
            "FREQ=YEARLY",
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2TU",
            "FREQ=WEEKLY;COUNT=3",
            "FREQ=WEEKLY;UNTIL=20270331T235959Z",
            "FREQ=SECONDLY",
            "FREQ=MINUTELY",
            "FREQ=HOURLY",
            "FREQ=MONTHLY;INTERVAL=2",
            "FREQ=YEARLY;INTERVAL=2",
        ];
        for rule in rules {
            let summary = ask_until(rule, Some("2027-03-29"));
            if let RecurrenceSummary::Described {
                key,
                every,
                on,
                end,
            } = &summary
            {
                for emitted in [Some(key.as_str())]
                    .into_iter()
                    .chain([Some(every.key.as_str())])
                    .chain([on.as_ref().map(|p| p.key.as_str())])
                    .chain([end.as_ref().map(|p| p.key.as_str())])
                    .flatten()
                {
                    assert!(
                        SUMMARY_KEYS.contains(&emitted),
                        "{rule} emits {emitted}, which SUMMARY_KEYS does not list"
                    );
                }
            }
        }
    }

    #[test]
    fn the_door_answers_in_json() {
        let json =
            recurrence_summary_json(r#"{"rrule":"FREQ=WEEKLY;BYDAY=MO","start":"2026-06-15"}"#)
                .unwrap();
        assert!(json.contains("\"outcome\":\"described\""), "{json}");
        assert!(json.contains("recurrenceSummary.weekly.onDays"), "{json}");
        assert!(json.contains("\"monday\""), "{json}");
        let json = recurrence_summary_json(r#"{"rrule":"","start":"2026-06-15"}"#).unwrap();
        assert_eq!(json, r#"{"outcome":"none"}"#);
        assert!(recurrence_summary_json("not json").is_err());
    }
}

/// The rules of `tests/fixtures/recurrenceSummary.json`, row by row: the same
/// file the desktop reads through the WASM door and renders in both
/// languages, so core and surfaces cannot drift apart.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/recurrenceSummary.json"
    ));
    const EN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../locales/en/translation.json"
    ));
    const DE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../locales/de/translation.json"
    ));

    fn rows() -> Vec<Value> {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        doc["rows"].as_array().expect("rows").clone()
    }

    #[test]
    fn every_row_reads_as_the_contract_says() {
        let rows = rows();
        assert!(rows.len() >= 40, "the contract covers the shapes");
        for row in rows {
            let name = row["name"].as_str().expect("a name");
            let question = RecurrenceSummaryQuestion {
                rrule: row["rrule"].as_str().expect("a rule").to_string(),
                start: row["start"].as_str().expect("a start").to_string(),
                last_day: row["last_day"].as_str().map(str::to_string),
            };
            let got = serde_json::to_value(describe_recurrence(&question)).expect("serialises");
            assert_eq!(got, row["expected"], "{name}");
            // The sentence is the surfaces' half of the contract; a row
            // without one would let a shape through unread.
            for language in ["en", "de"] {
                let sentence = row[language].as_str().unwrap_or_default();
                assert_eq!(
                    sentence.is_empty(),
                    matches!(describe_recurrence(&question), RecurrenceSummary::None),
                    "{name} ({language}): only \"no repeat\" has no sentence"
                );
            }
        }
    }

    #[test]
    fn every_key_the_core_emits_has_a_sentence_in_both_languages() {
        for (language, file) in [("en", EN), ("de", DE)] {
            let doc: Value = serde_json::from_str(file).expect("the locale file parses");
            for key in SUMMARY_KEYS {
                let mut node = &doc;
                for part in key.split('.') {
                    node = &node[part];
                }
                // A key with a plural has its forms instead of a bare value.
                let present = node.is_string()
                    || ["_one", "_other"].iter().all(|suffix| {
                        let mut node = &doc;
                        let parts: Vec<&str> = key.split('.').collect();
                        for part in &parts[..parts.len() - 1] {
                            node = &node[part];
                        }
                        node[format!("{}{suffix}", parts[parts.len() - 1])].is_string()
                    });
                assert!(present, "{language} has no sentence for {key}");
            }
        }
    }

    #[test]
    fn every_key_the_core_emits_is_used_by_a_row() {
        let rows = rows();
        let mut seen: Vec<String> = Vec::new();
        for row in &rows {
            let mut collect = |value: &Value| {
                if let Some(key) = value["key"].as_str() {
                    seen.push(key.to_string());
                }
            };
            let expected = &row["expected"];
            collect(expected);
            for half in ["every", "on", "end"] {
                collect(&expected[half]);
            }
        }
        for key in SUMMARY_KEYS {
            assert!(
                seen.iter().any(|s| s == key),
                "no row in the contract produces {key}"
            );
        }
    }
}
