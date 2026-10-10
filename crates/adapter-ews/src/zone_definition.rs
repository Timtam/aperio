//! A time zone as Exchange defines it inside `StartTimeZone` /
//! `EndTimeZone` (`TimeZoneDefinitionType`), and the clock it describes.
//!
//! Most items carry an id the CLDR table maps to a tzdata zone; those are
//! read through [`crate::windows_tz`]. An item made by another client may
//! carry a definition of its own instead: `Customized Time Zone` (an
//! Exchange ActiveSync client such as the iPhone's calendar), an iCalendar
//! invitation's VTIMEZONE, or a registry-only id. Decision 232 reads such a
//! definition to find the item's midnights rather than refusing or guessing.
//!
//! The wire shape (Exchange 2010 and later):
//!
//! - `Periods/Period` with `Bias` (an `xs:duration`; UTC = local + Bias, so
//!   W. Europe standard time is `-PT1H`) and an opaque `Id`.
//! - `TransitionsGroups/TransitionsGroup` (an `Id`), holding either one plain
//!   `Transition` to a period (no clock change in that era) or
//!   `RecurringDayTransition` / `RecurringDateTransition` elements to periods,
//!   each with `TimeOffset` (the local wall time of the change, in the period
//!   being left), `Month`, and `DayOfWeek` + `Occurrence` (1..4 from the
//!   start of the month, -1..-4 from its end) or `Day`.
//! - `Transitions`: a plain `Transition` to the first group (or period), then
//!   `AbsoluteDateTransition`s, each naming a group from its naive `DateTime`
//!   on.
//!
//! [`ZoneDefinition`] keeps the text as Exchange sent it, so the cache holds
//! what was read; [`ZoneRules`] is the checked form the clock is computed
//! from, and every way a definition can fail to be one is a named
//! [`DefinitionError`].

use std::fmt;

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc, Weekday};
use serde::{Deserialize, Serialize};

/// A zone definition as Exchange sent it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneDefinition {
    #[serde(default)]
    pub periods: Vec<PeriodDef>,
    #[serde(default)]
    pub groups: Vec<GroupDef>,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
}

impl ZoneDefinition {
    /// Whether Exchange sent anything beyond the id.
    pub fn is_empty(&self) -> bool {
        self.periods.is_empty() && self.groups.is_empty() && self.transitions.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodDef {
    pub id: String,
    pub bias: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDef {
    pub id: String,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
}

/// One transition element, its fields as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionDef {
    Plain {
        to_kind: String,
        to: String,
    },
    RecurringDay {
        to_kind: String,
        to: String,
        time_offset: String,
        month: String,
        day_of_week: String,
        occurrence: String,
    },
    RecurringDate {
        to_kind: String,
        to: String,
        time_offset: String,
        month: String,
        day: String,
    },
    Absolute {
        to_kind: String,
        to: String,
        date_time: String,
    },
}

/// Why a definition is not a clock Aperio can compute with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    NoPeriods,
    NoTransitions,
    /// The first top-level transition is not a plain `Transition`.
    FirstNotPlain,
    /// A `Transition` after the first, at the top level.
    PlainAfterFirst,
    /// A recurring transition at the top level.
    RecurringAtTop,
    /// An absolute transition inside a group.
    AbsoluteInGroup,
    /// An `AbsoluteDateTransition` whose `DateTime` is not a naive date-time.
    BadDateTime(String),
    /// Absolute transitions not in strictly increasing order.
    Unordered,
    /// A `To` that names no period or group.
    UnknownTarget(String),
    /// A `To` whose `Kind` is neither `Period` nor `Group`, or a group target
    /// inside a group.
    BadTargetKind(String),
    /// A group with no transitions, or mixing a plain transition with
    /// recurring ones.
    BadGroup(String),
    BadBias(String),
    BadTimeOffset(String),
    BadMonth(String),
    BadDayOfWeek(String),
    BadOccurrence(String),
    BadDay(String),
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Machine tokens for the log, never shown to the user.
        let (name, detail) = match self {
            Self::NoPeriods => ("no-periods", None),
            Self::NoTransitions => ("no-transitions", None),
            Self::FirstNotPlain => ("first-not-plain", None),
            Self::PlainAfterFirst => ("plain-after-first", None),
            Self::RecurringAtTop => ("recurring-at-top", None),
            Self::AbsoluteInGroup => ("absolute-in-group", None),
            Self::BadDateTime(v) => ("bad-date-time", Some(v)),
            Self::Unordered => ("unordered", None),
            Self::UnknownTarget(v) => ("unknown-target", Some(v)),
            Self::BadTargetKind(v) => ("bad-target-kind", Some(v)),
            Self::BadGroup(v) => ("bad-group", Some(v)),
            Self::BadBias(v) => ("bad-bias", Some(v)),
            Self::BadTimeOffset(v) => ("bad-time-offset", Some(v)),
            Self::BadMonth(v) => ("bad-month", Some(v)),
            Self::BadDayOfWeek(v) => ("bad-day-of-week", Some(v)),
            Self::BadOccurrence(v) => ("bad-occurrence", Some(v)),
            Self::BadDay(v) => ("bad-day", Some(v)),
        };
        match detail {
            Some(v) => write!(f, "{name}:{v}"),
            None => f.write_str(name),
        }
    }
}

/// Walks one `StartTimeZone` / `EndTimeZone` element's children. The item
/// parsers hand it every event between the element's start and its end, as
/// they hand a `Recurrence` to the recurrence walker.
#[derive(Debug, Default)]
pub struct ZoneDefinitionWalker {
    def: ZoneDefinition,
    /// The id of a nested `TimeZoneDefinition`, for a server that wraps the
    /// definition one level down.
    nested_id: Option<String>,
    group: Option<GroupDef>,
    in_top_transitions: bool,
    transition: Option<TransitionBuilder>,
    text: Option<&'static str>,
    depth: usize,
}

#[derive(Debug, Default)]
struct TransitionBuilder {
    kind: &'static str,
    to_kind: String,
    to: String,
    time_offset: String,
    month: String,
    day_of_week: String,
    occurrence: String,
    day: String,
    date_time: String,
}

impl TransitionBuilder {
    fn finish(self) -> Option<TransitionDef> {
        let Self {
            kind,
            to_kind,
            to,
            time_offset,
            month,
            day_of_week,
            occurrence,
            day,
            date_time,
        } = self;
        Some(match kind {
            "transition" => TransitionDef::Plain { to_kind, to },
            "recurringdaytransition" => TransitionDef::RecurringDay {
                to_kind,
                to,
                time_offset,
                month,
                day_of_week,
                occurrence,
            },
            "recurringdatetransition" => TransitionDef::RecurringDate {
                to_kind,
                to,
                time_offset,
                month,
                day,
            },
            "absolutedatetransition" => TransitionDef::Absolute {
                to_kind,
                to,
                date_time,
            },
            _ => return None,
        })
    }
}

fn attribute(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref().eq_ignore_ascii_case(name))
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

impl ZoneDefinitionWalker {
    /// An element opened (`empty` for a self-closing one), its local name in
    /// ASCII lower case.
    pub fn observe_start(
        &mut self,
        local: &[u8],
        e: &quick_xml::events::BytesStart<'_>,
        empty: bool,
    ) {
        if !empty {
            self.depth += 1;
        }
        match local {
            b"timezonedefinition" => self.nested_id = attribute(e, b"Id"),
            b"period" => self.def.periods.push(PeriodDef {
                id: attribute(e, b"Id").unwrap_or_default(),
                bias: attribute(e, b"Bias").unwrap_or_default(),
            }),
            b"transitionsgroup" if !empty => {
                self.group = Some(GroupDef {
                    id: attribute(e, b"Id").unwrap_or_default(),
                    transitions: Vec::new(),
                });
            }
            b"transitions" if !empty => self.in_top_transitions = true,
            b"transition"
            | b"recurringdaytransition"
            | b"recurringdatetransition"
            | b"absolutedatetransition"
                if !empty =>
            {
                let kind = match local {
                    b"transition" => "transition",
                    b"recurringdaytransition" => "recurringdaytransition",
                    b"recurringdatetransition" => "recurringdatetransition",
                    _ => "absolutedatetransition",
                };
                self.transition = Some(TransitionBuilder {
                    kind,
                    ..Default::default()
                });
            }
            b"to" if self.transition.is_some() => {
                if let Some(t) = self.transition.as_mut() {
                    t.to_kind = attribute(e, b"Kind").unwrap_or_default();
                }
                self.text = (!empty).then_some("to");
            }
            b"timeoffset" => self.text = Some("time_offset"),
            b"month" => self.text = Some("month"),
            b"dayofweek" => self.text = Some("day_of_week"),
            b"occurrence" => self.text = Some("occurrence"),
            b"day" => self.text = Some("day"),
            b"datetime" => self.text = Some("date_time"),
            _ => {}
        }
        if empty && local != b"to" {
            // A self-closing text element carries no text.
            if matches!(
                local,
                b"timeoffset" | b"month" | b"dayofweek" | b"occurrence" | b"day" | b"datetime"
            ) {
                self.text = None;
            }
        }
    }

    pub fn observe_text(&mut self, text: &str) {
        let (Some(target), Some(t)) = (self.text, self.transition.as_mut()) else {
            return;
        };
        let slot = match target {
            "to" => &mut t.to,
            "time_offset" => &mut t.time_offset,
            "month" => &mut t.month,
            "day_of_week" => &mut t.day_of_week,
            "occurrence" => &mut t.occurrence,
            "day" => &mut t.day,
            "date_time" => &mut t.date_time,
            _ => return,
        };
        slot.push_str(text);
    }

    /// An element closed. Returns `true` when it was the zone element itself,
    /// so the caller takes the walker's result.
    pub fn observe_end(&mut self, local: &[u8]) -> bool {
        self.text = None;
        if self.depth == 0 {
            return true;
        }
        self.depth -= 1;
        match local {
            b"transition"
            | b"recurringdaytransition"
            | b"recurringdatetransition"
            | b"absolutedatetransition" => {
                if let Some(t) = self.transition.take().and_then(TransitionBuilder::finish) {
                    match self.group.as_mut() {
                        Some(group) => group.transitions.push(t),
                        None if self.in_top_transitions => self.def.transitions.push(t),
                        None => {}
                    }
                }
            }
            b"transitionsgroup" => {
                if let Some(group) = self.group.take() {
                    self.def.groups.push(group);
                }
            }
            b"transitions" => self.in_top_transitions = false,
            _ => {}
        }
        false
    }

    /// The definition read, and the id a nested `TimeZoneDefinition` named.
    pub fn finish(self) -> (Option<ZoneDefinition>, Option<String>) {
        let def = (!self.def.is_empty()).then_some(self.def);
        (def, self.nested_id)
    }
}

/// When in the year a clock change happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    /// The `nth` `weekday` of `month`; negative counts from the month's end.
    NthWeekday {
        month: u32,
        weekday: Weekday,
        nth: i8,
    },
    Date {
        month: u32,
        day: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Change {
    on: On,
    /// The local wall time of the change, in the period being left.
    at: Duration,
    /// The bias after the change, UTC = local + bias.
    to: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Fixed(Duration),
    Yearly(Vec<Change>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Era {
    /// The naive local date-time this era starts at; `None` for the first.
    from: Option<NaiveDateTime>,
    rule: Rule,
}

/// A checked definition: the clock it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneRules {
    eras: Vec<Era>,
}

impl TryFrom<&ZoneDefinition> for ZoneRules {
    type Error = DefinitionError;

    fn try_from(def: &ZoneDefinition) -> Result<Self, Self::Error> {
        if def.periods.is_empty() {
            return Err(DefinitionError::NoPeriods);
        }
        if def.transitions.is_empty() {
            return Err(DefinitionError::NoTransitions);
        }
        let period_bias = |id: &str| -> Result<Duration, DefinitionError> {
            let period = def
                .periods
                .iter()
                .find(|p| p.id.trim() == id.trim())
                .ok_or_else(|| DefinitionError::UnknownTarget(id.trim().to_string()))?;
            parse_duration(&period.bias)
                .ok_or_else(|| DefinitionError::BadBias(period.bias.clone()))
        };
        let group_rule = |id: &str| -> Result<Rule, DefinitionError> {
            let group = def
                .groups
                .iter()
                .find(|g| g.id.trim() == id.trim())
                .ok_or_else(|| DefinitionError::UnknownTarget(id.trim().to_string()))?;
            match group.transitions.as_slice() {
                [] => Err(DefinitionError::BadGroup(group.id.clone())),
                [TransitionDef::Plain { to_kind, to }] => {
                    if !to_kind.trim().eq_ignore_ascii_case("Period") {
                        return Err(DefinitionError::BadTargetKind(to_kind.clone()));
                    }
                    Ok(Rule::Fixed(period_bias(to)?))
                }
                transitions => {
                    let mut changes = Vec::with_capacity(transitions.len());
                    for t in transitions {
                        changes.push(match t {
                            TransitionDef::Plain { .. } => {
                                return Err(DefinitionError::BadGroup(group.id.clone()))
                            }
                            TransitionDef::Absolute { .. } => {
                                return Err(DefinitionError::AbsoluteInGroup)
                            }
                            TransitionDef::RecurringDay {
                                to_kind,
                                to,
                                time_offset,
                                month,
                                day_of_week,
                                occurrence,
                            } => Change {
                                on: On::NthWeekday {
                                    month: parse_month(month)?,
                                    weekday: parse_weekday(day_of_week)?,
                                    nth: parse_occurrence(occurrence)?,
                                },
                                at: parse_time_offset(time_offset)?,
                                to: period_target(to_kind, to, &period_bias)?,
                            },
                            TransitionDef::RecurringDate {
                                to_kind,
                                to,
                                time_offset,
                                month,
                                day,
                            } => {
                                let month_n = parse_month(month)?;
                                let day_n = day
                                    .trim()
                                    .parse::<u32>()
                                    .ok()
                                    .filter(|d| (1..=31).contains(d))
                                    .ok_or_else(|| DefinitionError::BadDay(day.clone()))?;
                                Change {
                                    on: On::Date {
                                        month: month_n,
                                        day: day_n,
                                    },
                                    at: parse_time_offset(time_offset)?,
                                    to: period_target(to_kind, to, &period_bias)?,
                                }
                            }
                        });
                    }
                    Ok(Rule::Yearly(changes))
                }
            }
        };
        let target_rule = |to_kind: &str, to: &str| -> Result<Rule, DefinitionError> {
            match to_kind.trim() {
                k if k.eq_ignore_ascii_case("Group") => group_rule(to),
                k if k.eq_ignore_ascii_case("Period") => Ok(Rule::Fixed(period_bias(to)?)),
                other => Err(DefinitionError::BadTargetKind(other.to_string())),
            }
        };

        let mut eras = Vec::with_capacity(def.transitions.len());
        for (i, t) in def.transitions.iter().enumerate() {
            match (i, t) {
                (0, TransitionDef::Plain { to_kind, to }) => eras.push(Era {
                    from: None,
                    rule: target_rule(to_kind, to)?,
                }),
                (0, _) => return Err(DefinitionError::FirstNotPlain),
                (_, TransitionDef::Plain { .. }) => return Err(DefinitionError::PlainAfterFirst),
                (
                    _,
                    TransitionDef::Absolute {
                        to_kind,
                        to,
                        date_time,
                    },
                ) => {
                    let from = parse_naive(date_time)
                        .ok_or_else(|| DefinitionError::BadDateTime(date_time.clone()))?;
                    if eras
                        .last()
                        .and_then(|e: &Era| e.from)
                        .is_some_and(|prev| from <= prev)
                    {
                        return Err(DefinitionError::Unordered);
                    }
                    eras.push(Era {
                        from: Some(from),
                        rule: target_rule(to_kind, to)?,
                    });
                }
                (_, _) => return Err(DefinitionError::RecurringAtTop),
            }
        }
        Ok(Self { eras })
    }
}

fn period_target(
    to_kind: &str,
    to: &str,
    period_bias: &impl Fn(&str) -> Result<Duration, DefinitionError>,
) -> Result<Duration, DefinitionError> {
    if !to_kind.trim().eq_ignore_ascii_case("Period") {
        return Err(DefinitionError::BadTargetKind(to_kind.to_string()));
    }
    period_bias(to)
}

impl ZoneRules {
    /// The era in force at a naive local (or, for a UTC instant, UTC)
    /// date-time. Eras change at year starts in every published definition,
    /// so the half-day between the two readings never decides one.
    fn era_at(&self, at: NaiveDateTime) -> &Era {
        self.eras
            .iter()
            .rev()
            .find(|e| e.from.is_none_or(|from| from <= at))
            .unwrap_or(&self.eras[0])
    }

    /// One year's clock: the bias before the year's first change, then each
    /// change as `(the UTC instant it happens, the bias after it)`, in order.
    /// `None` when the changes do not follow each other.
    fn year(changes: &[Change], year: i32) -> Option<(Duration, Vec<(NaiveDateTime, Duration)>)> {
        let mut local: Vec<(NaiveDateTime, Duration)> = changes
            .iter()
            .map(|c| Some((change_day(c.on, year)?.and_hms_opt(0, 0, 0)? + c.at, c.to)))
            .collect::<Option<_>>()?;
        local.sort_by_key(|(when, _)| *when);
        // Windows keeps one rule a year: the bias a year opens with is the
        // one its last change leaves (Sydney is on daylight time on 1 Jan).
        let opening = local.last()?.1;
        let mut before = opening;
        let mut utc = Vec::with_capacity(local.len());
        for (when, to) in local {
            let instant = when + before;
            if utc.last().is_some_and(|(prev, _)| instant <= *prev) {
                return None;
            }
            utc.push((instant, to));
            before = to;
        }
        Some((opening, utc))
    }

    /// The bias in force at a UTC instant.
    fn bias_at_utc(&self, utc: NaiveDateTime) -> Option<Duration> {
        match &self.era_at(utc).rule {
            Rule::Fixed(bias) => Some(*bias),
            Rule::Yearly(changes) => {
                let (opening, steps) = Self::year(changes, utc.year())?;
                Some(
                    steps
                        .iter()
                        .rev()
                        .find(|(at, _)| *at <= utc)
                        .map_or(opening, |(_, to)| *to),
                )
            }
        }
    }

    /// The local wall time at a UTC instant.
    pub fn wall(&self, when: DateTime<Utc>) -> Option<NaiveDateTime> {
        let utc = when.naive_utc();
        Some(utc - self.bias_at_utc(utc)?)
    }

    /// The earliest UTC instant whose local wall time is `local`; `None` when
    /// a clock change skips it.
    pub fn instant_of(&self, local: NaiveDateTime) -> Option<DateTime<Utc>> {
        let candidates: Vec<NaiveDateTime> = match &self.era_at(local).rule {
            Rule::Fixed(bias) => vec![local + *bias],
            Rule::Yearly(changes) => {
                let (opening, steps) = Self::year(changes, local.year())?;
                // Segment k runs from its change (or the year's start) to the
                // next change; a reading `local + bias` counts when it falls
                // inside the segment of that bias.
                let mut out = Vec::new();
                let bounds: Vec<(Option<NaiveDateTime>, Duration)> =
                    std::iter::once((None, opening))
                        .chain(steps.iter().map(|(at, to)| (Some(*at), *to)))
                        .collect();
                for (k, (start, bias)) in bounds.iter().enumerate() {
                    let reading = local + *bias;
                    let after_start = start.is_none_or(|s| reading >= s);
                    let before_end = bounds
                        .get(k + 1)
                        .is_none_or(|(next, _)| next.is_none_or(|n| reading < n));
                    if after_start && before_end {
                        out.push(reading);
                    }
                }
                out
            }
        };
        candidates.into_iter().min().map(|utc| utc.and_utc())
    }

    /// The UTC instant of local midnight on `day`: the first one where the
    /// clock reaches it twice, and the first hour after it where a clock
    /// change skips it (as `all_day_anchor` places a skipped midnight).
    pub fn midnight(&self, day: NaiveDate) -> Option<DateTime<Utc>> {
        let midnight = day.and_hms_opt(0, 0, 0)?;
        self.instant_of(midnight)
            .or_else(|| self.instant_of(midnight + Duration::hours(1)))
    }
}

/// The day a change falls on in `year`.
fn change_day(on: On, year: i32) -> Option<NaiveDate> {
    match on {
        On::Date { month, day } => NaiveDate::from_ymd_opt(year, month, day),
        On::NthWeekday {
            month,
            weekday,
            nth,
        } if nth > 0 => NaiveDate::from_weekday_of_month_opt(year, month, weekday, nth as u8),
        On::NthWeekday {
            month,
            weekday,
            nth,
        } => {
            // Counted from the month's end: -1 is its last such weekday.
            let first_next = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)?
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)?
            };
            let last = first_next.pred_opt()?;
            let back = (7 + last.weekday().num_days_from_monday() as i64
                - weekday.num_days_from_monday() as i64)
                % 7;
            let last_such = last - Duration::days(back);
            Some(last_such - Duration::weeks((-nth - 1) as i64))
        }
    }
}

/// An `xs:duration` of days, hours, minutes and seconds (with a fraction):
/// `-PT1H`, `PT2H`, `-PT5H30M`, `PT23H59M59.999S`, `P1D`. Years and months
/// have no fixed length and are refused.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let rest = rest.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let mut millis: i64 = 0;
    let mut any = false;
    let mut take = |part: &str, units: &[(char, i64)], fraction_on: Option<char>| -> Option<()> {
        let mut number = String::new();
        let mut unit_at = 0usize;
        for c in part.chars() {
            if c.is_ascii_digit() || (Some(c) != None && c == '.' && fraction_on.is_some()) {
                number.push(c);
                continue;
            }
            let pos = units[unit_at..].iter().position(|(u, _)| *u == c)? + unit_at;
            let (unit, scale) = units[pos];
            if number.is_empty() || (number.contains('.') && Some(unit) != fraction_on) {
                return None;
            }
            let value: f64 = number.parse().ok()?;
            millis = millis.checked_add((value * scale as f64).round() as i64)?;
            any = true;
            number.clear();
            unit_at = pos + 1;
        }
        number.is_empty().then_some(())
    };
    take(date_part, &[('D', 86_400_000)], None)?;
    if let Some(time) = time_part {
        if time.is_empty() {
            return None;
        }
        take(
            time,
            &[('H', 3_600_000), ('M', 60_000), ('S', 1_000)],
            Some('S'),
        )?;
    }
    if !any {
        return None;
    }
    let d = Duration::milliseconds(millis);
    Some(if negative { -d } else { d })
}

fn parse_time_offset(text: &str) -> Result<Duration, DefinitionError> {
    parse_duration(text)
        .filter(|d| *d >= Duration::zero() && *d < Duration::hours(24))
        .ok_or_else(|| DefinitionError::BadTimeOffset(text.to_string()))
}

fn parse_month(text: &str) -> Result<u32, DefinitionError> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|m| (1..=12).contains(m))
        .ok_or_else(|| DefinitionError::BadMonth(text.to_string()))
}

fn parse_weekday(text: &str) -> Result<Weekday, DefinitionError> {
    match text.trim() {
        "Sunday" => Ok(Weekday::Sun),
        "Monday" => Ok(Weekday::Mon),
        "Tuesday" => Ok(Weekday::Tue),
        "Wednesday" => Ok(Weekday::Wed),
        "Thursday" => Ok(Weekday::Thu),
        "Friday" => Ok(Weekday::Fri),
        "Saturday" => Ok(Weekday::Sat),
        _ => Err(DefinitionError::BadDayOfWeek(text.to_string())),
    }
}

/// 1..4 from the month's start, -1..-4 from its end. A fifth weekday is the
/// month's last such weekday, as Windows reads it (decision 238).
fn parse_occurrence(text: &str) -> Result<i8, DefinitionError> {
    match text.trim().parse::<i8>() {
        Ok(5) => Ok(-1),
        Ok(n) if (1..=4).contains(&n) || (-4..=-1).contains(&n) => Ok(n),
        _ => Err(DefinitionError::BadOccurrence(text.to_string())),
    }
}

/// A naive `DateTime` (`2007-01-01T00:00:00`, with or without a fraction);
/// a value with a zone is refused.
fn parse_naive(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    if text.ends_with('Z')
        || text
            .get(10..)
            .is_some_and(|t| t.contains('+') || t.contains('-'))
    {
        return None;
    }
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn period(id: &str, bias: &str) -> PeriodDef {
        PeriodDef {
            id: id.into(),
            bias: bias.into(),
        }
    }

    fn day_change(to: &str, offset: &str, month: &str, dow: &str, occ: &str) -> TransitionDef {
        TransitionDef::RecurringDay {
            to_kind: "Period".into(),
            to: to.into(),
            time_offset: offset.into(),
            month: month.into(),
            day_of_week: dow.into(),
            occurrence: occ.into(),
        }
    }

    fn to_group(id: &str) -> TransitionDef {
        TransitionDef::Plain {
            to_kind: "Group".into(),
            to: id.into(),
        }
    }

    /// W. Europe Standard Time as Exchange 2016's GetServerTimeZones gives it.
    pub(crate) fn w_europe() -> ZoneDefinition {
        let std = "trule:Microsoft/Registry/W. Europe Standard Time/1-Standard";
        let dst = "trule:Microsoft/Registry/W. Europe Standard Time/1-Daylight";
        ZoneDefinition {
            periods: vec![period(std, "-PT1H"), period(dst, "-PT2H")],
            groups: vec![GroupDef {
                id: "0".into(),
                transitions: vec![
                    day_change(dst, "PT2H", "3", "Sunday", "-1"),
                    day_change(std, "PT3H", "10", "Sunday", "-1"),
                ],
            }],
            transitions: vec![to_group("0")],
        }
    }

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn w_europe_midnights_follow_the_clock_change() {
        let rules = ZoneRules::try_from(&w_europe()).unwrap();
        assert_eq!(
            rules.midnight(day("2026-10-12")),
            Some(utc("2026-10-11T22:00:00Z"))
        );
        assert_eq!(
            rules.midnight(day("2026-11-02")),
            Some(utc("2026-11-01T23:00:00Z"))
        );
        // The days of the changes themselves.
        assert_eq!(
            rules.midnight(day("2026-03-29")),
            Some(utc("2026-03-28T23:00:00Z"))
        );
        assert_eq!(
            rules.midnight(day("2026-10-25")),
            Some(utc("2026-10-24T22:00:00Z"))
        );
        assert_eq!(
            rules.midnight(day("2026-10-26")),
            Some(utc("2026-10-25T23:00:00Z"))
        );
    }

    #[test]
    fn the_order_of_a_groups_changes_does_not_matter() {
        let mut def = w_europe();
        def.groups[0].transitions.reverse();
        let rules = ZoneRules::try_from(&def).unwrap();
        assert_eq!(
            rules.midnight(day("2026-10-12")),
            Some(utc("2026-10-11T22:00:00Z"))
        );
        assert_eq!(
            rules.midnight(day("2026-11-02")),
            Some(utc("2026-11-01T23:00:00Z"))
        );
    }

    #[test]
    fn the_wall_clock_reads_back_the_midnight() {
        let rules = ZoneRules::try_from(&w_europe()).unwrap();
        for d in [
            "2026-01-15",
            "2026-03-29",
            "2026-07-01",
            "2026-10-25",
            "2026-12-31",
        ] {
            let m = rules.midnight(day(d)).unwrap();
            assert_eq!(rules.wall(m), day(d).and_hms_opt(0, 0, 0), "{d}");
        }
    }

    /// Sydney: daylight time opens the year, so the year's last change sets
    /// the bias on 1 January.
    #[test]
    fn a_southern_zone_opens_the_year_on_daylight_time() {
        let std = "aus/std";
        let dst = "aus/dst";
        let def = ZoneDefinition {
            periods: vec![period(std, "-PT10H"), period(dst, "-PT11H")],
            groups: vec![GroupDef {
                id: "0".into(),
                transitions: vec![
                    day_change(std, "PT3H", "4", "Sunday", "1"),
                    day_change(dst, "PT2H", "10", "Sunday", "1"),
                ],
            }],
            transitions: vec![to_group("0")],
        };
        let rules = ZoneRules::try_from(&def).unwrap();
        assert_eq!(
            rules.midnight(day("2026-01-01")),
            Some(utc("2025-12-31T13:00:00Z"))
        );
        assert_eq!(
            rules.midnight(day("2026-06-01")),
            Some(utc("2026-05-31T14:00:00Z"))
        );
    }

    /// Chile changes at midnight: 23:59:59.999 on the Saturday, so Sunday's
    /// midnight is skipped and the day starts at 01:00.
    #[test]
    fn a_skipped_midnight_starts_the_day_an_hour_later() {
        let std = "chile/std";
        let dst = "chile/dst";
        let def = ZoneDefinition {
            periods: vec![period(std, "PT4H"), period(dst, "PT3H")],
            groups: vec![GroupDef {
                id: "0".into(),
                transitions: vec![
                    day_change(std, "PT23H59M59.999S", "4", "Saturday", "1"),
                    day_change(dst, "PT23H59M59.999S", "9", "Saturday", "1"),
                ],
            }],
            transitions: vec![to_group("0")],
        };
        let rules = ZoneRules::try_from(&def).unwrap();
        assert_eq!(
            rules.midnight(day("2026-09-06")),
            Some(utc("2026-09-06T04:00:00Z"))
        );
    }

    #[test]
    fn a_fixed_zone_has_one_bias_all_year() {
        let def = ZoneDefinition {
            periods: vec![period("india", "-PT5H30M")],
            groups: vec![GroupDef {
                id: "0".into(),
                transitions: vec![TransitionDef::Plain {
                    to_kind: "Period".into(),
                    to: "india".into(),
                }],
            }],
            transitions: vec![to_group("0")],
        };
        let rules = ZoneRules::try_from(&def).unwrap();
        assert_eq!(
            rules.midnight(day("2026-10-12")),
            Some(utc("2026-10-11T18:30:00Z"))
        );
        let kiribati = ZoneDefinition {
            periods: vec![period("line", "-PT14H")],
            groups: vec![],
            transitions: vec![TransitionDef::Plain {
                to_kind: "Period".into(),
                to: "line".into(),
            }],
        };
        let rules = ZoneRules::try_from(&kiribati).unwrap();
        assert_eq!(
            rules.midnight(day("2026-10-12")),
            Some(utc("2026-10-11T10:00:00Z"))
        );
    }

    /// Microsoft's Eastern Standard Time sample: one rule until 2006, another
    /// from 2007.
    #[test]
    fn an_absolute_transition_switches_the_rule() {
        let p = |n: &str, b: &str| period(&format!("est/{n}"), b);
        let def = ZoneDefinition {
            periods: vec![
                p("2006-Standard", "PT5H"),
                p("2006-Daylight", "PT4H"),
                p("2007-Standard", "PT5H"),
                p("2007-Daylight", "PT4H"),
            ],
            groups: vec![
                GroupDef {
                    id: "0".into(),
                    transitions: vec![
                        day_change("est/2006-Daylight", "PT2H", "4", "Sunday", "1"),
                        day_change("est/2006-Standard", "PT2H", "10", "Sunday", "-1"),
                    ],
                },
                GroupDef {
                    id: "1".into(),
                    transitions: vec![
                        day_change("est/2007-Daylight", "PT2H", "3", "Sunday", "2"),
                        day_change("est/2007-Standard", "PT2H", "11", "Sunday", "1"),
                    ],
                },
            ],
            transitions: vec![
                to_group("0"),
                TransitionDef::Absolute {
                    to_kind: "Group".into(),
                    to: "1".into(),
                    date_time: "2007-01-01T00:00:00".into(),
                },
            ],
        };
        let rules = ZoneRules::try_from(&def).unwrap();
        // 20 March 2006: before the April change, standard time.
        assert_eq!(
            rules.midnight(day("2006-03-20")),
            Some(utc("2006-03-20T05:00:00Z"))
        );
        // 20 March 2007: after the second-Sunday change, daylight time.
        assert_eq!(
            rules.midnight(day("2007-03-20")),
            Some(utc("2007-03-20T04:00:00Z"))
        );
        // 2 November 2006 (after the last Sunday of October): standard.
        assert_eq!(
            rules.midnight(day("2006-11-02")),
            Some(utc("2006-11-02T05:00:00Z"))
        );
        // 2 November 2007 (before the first Sunday of November): daylight.
        assert_eq!(
            rules.midnight(day("2007-11-02")),
            Some(utc("2007-11-02T04:00:00Z"))
        );
    }

    #[test]
    fn a_fifth_weekday_reads_as_the_last() {
        let mut def = w_europe();
        if let TransitionDef::RecurringDay { occurrence, .. } = &mut def.groups[0].transitions[1] {
            *occurrence = "5".into();
        }
        let rules = ZoneRules::try_from(&def).unwrap();
        assert_eq!(
            rules.midnight(day("2026-11-02")),
            Some(utc("2026-11-01T23:00:00Z"))
        );
    }

    #[test]
    fn a_broken_definition_names_what_is_wrong() {
        let mut dangling = w_europe();
        dangling.transitions = vec![to_group("7")];
        assert_eq!(
            ZoneRules::try_from(&dangling),
            Err(DefinitionError::UnknownTarget("7".into()))
        );

        let mut zero = w_europe();
        if let TransitionDef::RecurringDay { occurrence, .. } = &mut zero.groups[0].transitions[0] {
            *occurrence = "0".into();
        }
        assert_eq!(
            ZoneRules::try_from(&zero),
            Err(DefinitionError::BadOccurrence("0".into()))
        );

        let mut weekday = w_europe();
        if let TransitionDef::RecurringDay { day_of_week, .. } =
            &mut weekday.groups[0].transitions[0]
        {
            *day_of_week = "Weekday".into();
        }
        assert_eq!(
            ZoneRules::try_from(&weekday),
            Err(DefinitionError::BadDayOfWeek("Weekday".into()))
        );

        let mut no_periods = w_europe();
        no_periods.periods.clear();
        assert_eq!(
            ZoneRules::try_from(&no_periods),
            Err(DefinitionError::NoPeriods)
        );

        let mut zoned = w_europe();
        zoned.transitions.push(TransitionDef::Absolute {
            to_kind: "Group".into(),
            to: "0".into(),
            date_time: "2007-01-01T00:00:00Z".into(),
        });
        assert_eq!(
            ZoneRules::try_from(&zoned),
            Err(DefinitionError::BadDateTime("2007-01-01T00:00:00Z".into()))
        );
    }

    #[test]
    fn durations_read_every_spelling() {
        assert_eq!(parse_duration("-PT1H"), Some(Duration::hours(-1)));
        assert_eq!(parse_duration("-PT60M"), Some(Duration::hours(-1)));
        assert_eq!(parse_duration("PT5H"), Some(Duration::hours(5)));
        assert_eq!(parse_duration("-PT5H30M"), Some(-Duration::minutes(330)));
        assert_eq!(
            parse_duration("PT23H59M59.999S"),
            Some(Duration::milliseconds(86_399_999))
        );
        assert_eq!(parse_duration("P1D"), Some(Duration::days(1)));
        assert_eq!(parse_duration("PT0S"), Some(Duration::zero()));
        for bad in ["", "P", "PT", "1H", "P1Y", "P1M", "PT1.5H", "PTH", "PT1H2X"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }
}
