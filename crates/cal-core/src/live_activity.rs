//! Live Activities for events (`DESIGN-live-activities.md`).
//!
//! From the moment an appointment's LAST reminder fires until it ends, the
//! phone keeps its title, its time, its place and what comes after it in view:
//! an iOS Live Activity, an Android ongoing notification. This module decides
//! what that shows, and when. The platforms only draw the plan.
//!
//! # The rule
//!
//! - An appointment's **window** runs from its last reminder that fires before
//!   it ends, to its end. No reminder, no window. All-day, cancelled and hidden
//!   events have none, and neither has one whose last reminder is more than
//!   [`MAX_LEAD_MINUTES`] before its start.
//! - Copies of one appointment (members of a group that agree on the time, and
//!   a videoconference meeting whose invitation is in view) are ONE
//!   appointment with ONE window.
//! - At any instant the activity shows the running window that **began
//!   latest**: a later reminder takes over (the crossfade), and when a window
//!   nested inside another ends, the outer one is shown again.
//! - A **chain** is an unbroken run of such sections. A chain is one activity.
//! - A section that begins where its window begins is that appointment's last
//!   reminder. It names the reminders that are **absorbed** into it, and only
//!   those may be left out of the ordinary notifications: everything else
//!   still rings on its own. That keeps "one signal" from ever swallowing a
//!   reminder the activity does not show.
//!
//! # Pure
//!
//! Nothing here reads a clock, a database or a time zone. "Now" comes in, and
//! so does each occurrence's local day: the host knows the device's zone, the
//! rule only needs to know which day an occurrence is on.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;

use crate::event_group_fold::{collapse_event_groups, FoldableEvent};
use crate::group_suggestion::is_meeting_calendar;
use crate::meeting_events::{join_url_of, without_duplicate_meetings, MeetingFilterEvent};
use crate::EventGroup;

/// How long before an appointment starts Android may ask to PROMOTE its
/// notification (a Live Update, with a chip in the status bar).
///
/// Google's guidance names "upcoming calendar events" as a use Live Updates are
/// NOT for, and an event that is "about to start" as one they are for. A last
/// reminder an hour ahead is therefore an ordinary ongoing notification until
/// this point.
pub const PROMOTE_LEAD_MINUTES: i64 = 15;

/// How long before the start a last reminder may be and still become an
/// activity (Toni, 2026-11-06).
///
/// A reminder "one day before" is a reminder to plan, not one to watch a clock
/// by: an activity from there would sit on the lock screen all day, and iOS
/// ends any activity after eight hours anyway. Further ahead than this, the
/// reminder stays an ordinary notification and the appointment gets no
/// activity at all.
pub const MAX_LEAD_MINUTES: i64 = 120;

/// One occurrence of a timed or all-day event, expanded by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveOccurrence {
    pub item_id: String,
    pub calendar_id: String,
    /// The series master id group membership is keyed by (`EventGroupMember`).
    pub series_id: String,
    /// The occurrence's ORIGINAL start when it belongs to a series, so its key
    /// survives the occurrence being moved. `None` for a one-off event.
    pub original_start: Option<DateTime<Utc>>,
    pub title: String,
    pub location: Option<String>,
    /// Only read for the join link a meeting's invitation carries.
    pub description: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// The device-local day the occurrence starts on.
    pub local_day: NaiveDate,
    pub all_day: bool,
    pub cancelled: bool,
}

/// One reminder of one occurrence, as the host's reminder scan produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTrigger {
    pub item_id: String,
    pub calendar_id: String,
    /// The start of the occurrence it reminds of — the same instant as that
    /// occurrence's `start`.
    pub occurrence_start: DateTime<Utc>,
    pub fire_at: DateTime<Utc>,
    /// A reminder at a fixed time rather than relative to the start. On a
    /// series every occurrence reports the same instant for it.
    pub absolute: bool,
}

/// Everything [`live_activity_plan`] needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveActivityInput {
    pub now: DateTime<Utc>,
    /// Windows that begin at or after this are left out.
    pub horizon_end: DateTime<Utc>,
    /// Every occurrence of the days involved, with or without reminders: an
    /// appointment without one can still be what comes next.
    pub occurrences: Vec<LiveOccurrence>,
    pub triggers: Vec<LiveTrigger>,
    pub groups: Vec<EventGroup>,
    /// The device's hidden calendars. They are device-local, so they come in.
    pub hidden_calendars: Vec<String>,
}

/// Which reminder a signal section absorbs — the key the ordinary notification
/// list is filtered by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveTriggerKey {
    pub item_id: String,
    pub calendar_id: String,
    pub occurrence_start: DateTime<Utc>,
    pub fire_at: DateTime<Utc>,
}

/// The appointment a section shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveActivityEvent {
    /// Stable across plans: what a running activity is matched by, and what a
    /// dismissal is remembered under.
    pub key: String,
    pub item_id: String,
    pub calendar_id: String,
    pub original_start: Option<DateTime<Utc>>,
    pub title: String,
    /// A room or address. Never a bare join link: read aloud, a URL is not a
    /// place.
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    /// The latest end among the copies.
    pub end: DateTime<Utc>,
}

/// What comes after the appointment shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveActivityFollowUp {
    pub title: String,
    pub start: DateTime<Utc>,
}

/// A stretch of time over which nothing visible changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveActivitySection {
    pub from: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub event: LiveActivityEvent,
    pub follow_up: Option<LiveActivityFollowUp>,
    /// The appointment has begun ("Läuft bis" rather than "Beginnt um").
    pub started: bool,
    /// Android may promote the notification from here on.
    pub promoted: bool,
    /// Non-empty when this section IS the appointment's last reminder: the
    /// reminders absorbed into it. Empty for every other section, which must
    /// change the activity silently.
    pub signal: Vec<LiveTriggerKey>,
}

/// One activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveActivityChain {
    /// The keys of every appointment the chain shows, in order. A running
    /// activity belongs to the chain that shares ANY of them, so the match
    /// holds after the first appointment has ended and dropped out.
    pub keys: Vec<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sections: Vec<LiveActivitySection>,
}

/// The whole plan, chains in time order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LiveActivityPlan {
    pub chains: Vec<LiveActivityChain>,
}

impl LiveActivityPlan {
    /// Every reminder the plan absorbs. The ordinary notification list minus
    /// these is what still has to ring on its own — and only once the platform
    /// has accepted the plan.
    pub fn absorbed(&self) -> BTreeSet<LiveTriggerKey> {
        self.chains
            .iter()
            .flat_map(|chain| &chain.sections)
            .flat_map(|section| section.signal.iter().cloned())
            .collect()
    }
}

/// One appointment of one day: the occurrence shown plus its copies.
struct Appointment {
    shown: usize,
    members: Vec<usize>,
    key: String,
}

struct Window {
    appointment: usize,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    event_start: DateTime<Utc>,
    absorbed: Vec<LiveTriggerKey>,
}

fn occurrence_key(occ: &LiveOccurrence) -> String {
    match occ.original_start {
        Some(at) => format!(
            "{}|{}|{}",
            occ.calendar_id,
            occ.item_id,
            at.timestamp_millis()
        ),
        None => format!("{}|{}", occ.calendar_id, occ.item_id),
    }
}

/// The location to show: `None` when it is nothing but a join link.
fn place_of(occ: &LiveOccurrence) -> Option<String> {
    let location = occ.location.as_deref()?.trim();
    if location.is_empty() {
        return None;
    }
    match join_url_of(Some(location), None) {
        Some(url) if url == location => None,
        _ => Some(location.to_string()),
    }
}

/// The appointments of every day, copies folded together.
fn appointments(input: &LiveActivityInput) -> Vec<Appointment> {
    let occurrences = &input.occurrences;
    let hidden: HashSet<&str> = input.hidden_calendars.iter().map(String::as_str).collect();
    let member_group: HashMap<(&str, &str), &str> = input
        .groups
        .iter()
        .flat_map(|group| {
            group.members.iter().map(move |member| {
                (
                    (member.calendar_id.as_str(), member.event_id.as_str()),
                    group.id.as_str(),
                )
            })
        })
        .collect();
    let group_of = |occ: &LiveOccurrence| {
        member_group
            .get(&(occ.calendar_id.as_str(), occ.series_id.as_str()))
            .copied()
    };

    let mut by_day: BTreeMap<NaiveDate, Vec<usize>> = BTreeMap::new();
    for (i, occ) in occurrences.iter().enumerate() {
        if hidden.contains(occ.calendar_id.as_str())
            || occ.all_day
            || occ.cancelled
            || occ.end < occ.start
        {
            continue;
        }
        by_day.entry(occ.local_day).or_default().push(i);
    }

    let mut out = Vec::new();
    for rows in by_day.values_mut() {
        // A fixed order, so the same data always folds the same way.
        rows.sort_by(|&a, &b| {
            let (a, b) = (&occurrences[a], &occurrences[b]);
            a.start
                .cmp(&b.start)
                .then_with(|| a.calendar_id.cmp(&b.calendar_id))
                .then_with(|| a.item_id.cmp(&b.item_id))
        });

        // A meeting whose invitation is in view is that invitation's copy.
        let filter_rows: Vec<MeetingFilterEvent> = rows
            .iter()
            .map(|&i| MeetingFilterEvent {
                calendar_id: occurrences[i].calendar_id.clone(),
                location: occurrences[i].location.clone(),
                description: occurrences[i].description.clone(),
                grouped: group_of(&occurrences[i]).is_some(),
            })
            .collect();
        let kept: Vec<usize> = without_duplicate_meetings(&filter_rows)
            .into_iter()
            .map(|pos| rows[pos])
            .collect();
        let kept_set: HashSet<usize> = kept.iter().copied().collect();
        let link_of = |i: usize| {
            join_url_of(
                occurrences[i].location.as_deref(),
                occurrences[i].description.as_deref(),
            )
        };

        // Groups fold the rest, exactly as the views fold them.
        let foldable: Vec<FoldableEvent> = kept
            .iter()
            .map(|&i| FoldableEvent {
                calendar_id: occurrences[i].calendar_id.clone(),
                series_id: occurrences[i].series_id.clone(),
                start: Some(occurrences[i].start.to_rfc3339()),
                all_day: false,
                actionable: None,
            })
            .collect();
        let collapsed = collapse_event_groups(&foldable, &input.groups);
        let first = out.len();
        let mut folded_into: HashMap<&str, usize> = HashMap::new();
        let mut emitted: HashSet<usize> = HashSet::new();
        for row in &collapsed {
            let shown = kept[row.event];
            emitted.insert(shown);
            if let (Some(group), false) = (row.group_id.as_deref(), row.diverged) {
                folded_into.insert(group, out.len());
            }
            out.push(Appointment {
                shown,
                members: vec![shown],
                key: String::new(),
            });
        }
        for &i in &kept {
            if emitted.contains(&i) {
                continue;
            }
            if let Some(&at) = group_of(&occurrences[i]).and_then(|group| folded_into.get(group)) {
                out[at].members.push(i);
            }
        }
        for &i in rows.iter().filter(|&&i| !kept_set.contains(&i)) {
            let Some(url) = link_of(i) else { continue };
            let partner = kept.iter().copied().find(|&p| {
                !is_meeting_calendar(&occurrences[p].calendar_id)
                    && link_of(p).as_deref() == Some(url.as_str())
            });
            if let Some(partner) = partner {
                if let Some(appointment) = out[first..]
                    .iter_mut()
                    .find(|a| a.members.contains(&partner))
                {
                    appointment.members.push(i);
                }
            }
        }
        for appointment in &mut out[first..] {
            appointment.key = appointment
                .members
                .iter()
                .map(|&i| occurrence_key(&occurrences[i]))
                .min()
                .unwrap_or_default();
        }
    }
    out
}

/// The reminders each occurrence has, after a fixed-time reminder on a series
/// has been given to the one occurrence it belongs to.
fn triggers_by_occurrence(
    input: &LiveActivityInput,
) -> HashMap<(&str, &str, i64), Vec<&LiveTrigger>> {
    let end_of: HashMap<(&str, &str, i64), DateTime<Utc>> = input
        .occurrences
        .iter()
        .map(|occ| {
            (
                (
                    occ.item_id.as_str(),
                    occ.calendar_id.as_str(),
                    occ.start.timestamp_millis(),
                ),
                occ.end,
            )
        })
        .collect();
    // A fixed time on a series is reported for every occurrence. It belongs to
    // the first occurrence that ends after it — the same answer the reminder
    // planner gives — or every occurrence in view would open a window there.
    let mut owner: HashMap<(&str, &str, DateTime<Utc>), DateTime<Utc>> = HashMap::new();
    for trigger in input.triggers.iter().filter(|t| t.absolute) {
        let Some(&end) = end_of.get(&(
            trigger.item_id.as_str(),
            trigger.calendar_id.as_str(),
            trigger.occurrence_start.timestamp_millis(),
        )) else {
            continue;
        };
        if end <= trigger.fire_at {
            continue;
        }
        owner
            .entry((
                trigger.item_id.as_str(),
                trigger.calendar_id.as_str(),
                trigger.fire_at,
            ))
            .and_modify(|start| *start = (*start).min(trigger.occurrence_start))
            .or_insert(trigger.occurrence_start);
    }
    let mut out: HashMap<(&str, &str, i64), Vec<&LiveTrigger>> = HashMap::new();
    for trigger in &input.triggers {
        if trigger.absolute
            && owner.get(&(
                trigger.item_id.as_str(),
                trigger.calendar_id.as_str(),
                trigger.fire_at,
            )) != Some(&trigger.occurrence_start)
        {
            continue;
        }
        out.entry((
            trigger.item_id.as_str(),
            trigger.calendar_id.as_str(),
            trigger.occurrence_start.timestamp_millis(),
        ))
        .or_default()
        .push(trigger);
    }
    out
}

fn windows(input: &LiveActivityInput, appointments: &[Appointment]) -> Vec<Window> {
    let occurrences = &input.occurrences;
    let triggers = triggers_by_occurrence(input);
    let mut out = Vec::new();
    for (index, appointment) in appointments.iter().enumerate() {
        let end = appointment
            .members
            .iter()
            .map(|&i| occurrences[i].end)
            .max()
            .unwrap_or(occurrences[appointment.shown].end);
        let candidates: Vec<&LiveTrigger> = appointment
            .members
            .iter()
            .flat_map(|&i| {
                let occ = &occurrences[i];
                triggers
                    .get(&(
                        occ.item_id.as_str(),
                        occ.calendar_id.as_str(),
                        occ.start.timestamp_millis(),
                    ))
                    .into_iter()
                    .flatten()
                    .copied()
            })
            .filter(|trigger| trigger.fire_at < end)
            .collect();
        let Some(start) = candidates.iter().map(|trigger| trigger.fire_at).max() else {
            continue;
        };
        if end <= input.now || start >= input.horizon_end {
            continue;
        }
        let shown_start = occurrences[appointment.shown].start;
        if shown_start - start > Duration::minutes(MAX_LEAD_MINUTES) {
            continue;
        }
        let mut absorbed: Vec<LiveTriggerKey> = candidates
            .iter()
            .filter(|trigger| trigger.fire_at == start)
            .map(|trigger| LiveTriggerKey {
                item_id: trigger.item_id.clone(),
                calendar_id: trigger.calendar_id.clone(),
                occurrence_start: trigger.occurrence_start,
                fire_at: trigger.fire_at,
            })
            .collect();
        absorbed.sort();
        absorbed.dedup();
        out.push(Window {
            appointment: index,
            start,
            end,
            event_start: occurrences[appointment.shown].start,
            absorbed,
        });
    }
    out
}

/// The plan for the input: see the module docs.
pub fn live_activity_plan(input: &LiveActivityInput) -> LiveActivityPlan {
    let occurrences = &input.occurrences;
    let appointments = appointments(input);
    let windows = windows(input, &appointments);
    if windows.is_empty() {
        return LiveActivityPlan::default();
    }
    let promote_lead = Duration::minutes(PROMOTE_LEAD_MINUTES);

    let mut points: BTreeSet<DateTime<Utc>> = BTreeSet::new();
    for window in &windows {
        points.insert(window.start);
        points.insert(window.end);
        for at in [window.event_start, window.event_start - promote_lead] {
            if window.start < at && at < window.end {
                points.insert(at);
            }
        }
    }
    // Where a follow-up begins, the next one takes its place.
    for appointment in &appointments {
        points.insert(occurrences[appointment.shown].start);
    }

    // Follow-up candidates, earliest first.
    let mut by_start: Vec<usize> = (0..appointments.len()).collect();
    by_start.sort_by(|&a, &b| {
        let (sa, sb) = (
            occurrences[appointments[a].shown].start,
            occurrences[appointments[b].shown].start,
        );
        sa.cmp(&sb)
            .then_with(|| appointments[a].key.cmp(&appointments[b].key))
    });

    let points: Vec<DateTime<Utc>> = points.into_iter().collect();
    let mut chains: Vec<LiveActivityChain> = Vec::new();
    let mut open: Option<LiveActivityChain> = None;
    for pair in points.windows(2) {
        let (from, until) = (pair[0], pair[1]);
        // The running window that began latest; on a tie the appointment that
        // starts first, then the key — so the same data always shows the same.
        let shown = windows
            .iter()
            .filter(|window| window.start <= from && from < window.end)
            .max_by(|a, b| {
                a.start
                    .cmp(&b.start)
                    .then_with(|| b.event_start.cmp(&a.event_start))
                    .then_with(|| {
                        appointments[b.appointment]
                            .key
                            .cmp(&appointments[a.appointment].key)
                    })
            });
        let Some(window) = shown else {
            if let Some(chain) = open.take() {
                chains.push(chain);
            }
            continue;
        };
        let appointment = &appointments[window.appointment];
        let occ = &occurrences[appointment.shown];
        let after = from.max(occ.start);
        let follow_up = by_start
            .iter()
            .map(|&i| (i, &occurrences[appointments[i].shown]))
            .find(|&(i, next)| {
                i != window.appointment && next.local_day == occ.local_day && next.start > after
            })
            .map(|(_, next)| LiveActivityFollowUp {
                title: next.title.clone(),
                start: next.start,
            });
        let section = LiveActivitySection {
            from,
            until,
            event: LiveActivityEvent {
                key: appointment.key.clone(),
                item_id: occ.item_id.clone(),
                calendar_id: occ.calendar_id.clone(),
                original_start: occ.original_start,
                title: occ.title.clone(),
                location: place_of(occ),
                start: occ.start,
                end: window.end,
            },
            follow_up,
            started: from >= occ.start,
            promoted: from >= occ.start - promote_lead,
            signal: if from == window.start {
                window.absorbed.clone()
            } else {
                Vec::new()
            },
        };
        let chain = open.get_or_insert_with(|| LiveActivityChain {
            keys: Vec::new(),
            start: from,
            end: until,
            sections: Vec::new(),
        });
        chain.end = until;
        if !chain.keys.contains(&section.event.key) {
            chain.keys.push(section.event.key.clone());
        }
        match chain.sections.last_mut() {
            Some(last)
                if section.signal.is_empty()
                    && last.until == from
                    && last.event == section.event
                    && last.follow_up == section.follow_up
                    && last.started == section.started
                    && last.promoted == section.promoted =>
            {
                last.until = until;
            }
            _ => chain.sections.push(section),
        }
    }
    if let Some(chain) = open.take() {
        chains.push(chain);
    }
    LiveActivityPlan { chains }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventGroupMember;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 9, h, m, 0).unwrap()
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
    }

    fn event(id: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> LiveOccurrence {
        LiveOccurrence {
            item_id: id.to_string(),
            calendar_id: "work".to_string(),
            series_id: id.to_string(),
            original_start: None,
            title: id.to_string(),
            location: None,
            description: None,
            start,
            end,
            local_day: day(),
            all_day: false,
            cancelled: false,
        }
    }

    fn reminder(occ: &LiveOccurrence, fire_at: DateTime<Utc>) -> LiveTrigger {
        LiveTrigger {
            item_id: occ.item_id.clone(),
            calendar_id: occ.calendar_id.clone(),
            occurrence_start: occ.start,
            fire_at,
            absolute: false,
        }
    }

    fn input(occurrences: Vec<LiveOccurrence>, triggers: Vec<LiveTrigger>) -> LiveActivityInput {
        LiveActivityInput {
            now: at(8, 0),
            horizon_end: at(23, 59),
            occurrences,
            triggers,
            groups: Vec::new(),
            hidden_calendars: Vec::new(),
        }
    }

    /// Each section as "from-until title [flags]" — one line a failure can be
    /// read from.
    fn sketch(plan: &LiveActivityPlan) -> Vec<Vec<String>> {
        plan.chains
            .iter()
            .map(|chain| {
                chain
                    .sections
                    .iter()
                    .map(|s| {
                        let mut line = format!(
                            "{}-{} {}",
                            s.from.format("%H:%M"),
                            s.until.format("%H:%M"),
                            s.event.title
                        );
                        if !s.signal.is_empty() {
                            line.push_str(" signal");
                        }
                        if s.promoted {
                            line.push_str(" promoted");
                        }
                        if s.started {
                            line.push_str(" started");
                        }
                        if let Some(next) = &s.follow_up {
                            line.push_str(&format!(
                                " next={}@{}",
                                next.title,
                                next.start.format("%H:%M")
                            ));
                        }
                        line
                    })
                    .collect()
            })
            .collect()
    }

    fn group(id: &str, members: &[(&str, &str)]) -> EventGroup {
        EventGroup {
            id: id.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            members: members
                .iter()
                .map(|(calendar_id, event_id)| EventGroupMember {
                    calendar_id: calendar_id.to_string(),
                    event_id: event_id.to_string(),
                    title: String::new(),
                    starts_at: String::new(),
                    added_at: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_window_runs_from_the_last_reminder_to_the_end() {
        let a = event("A", at(10, 0), at(11, 0));
        let triggers = vec![reminder(&a, at(9, 0)), reminder(&a, at(9, 50))];
        let plan = live_activity_plan(&input(vec![a], triggers));
        assert_eq!(
            sketch(&plan),
            vec![vec![
                "09:50-10:00 A signal promoted".to_string(),
                "10:00-11:00 A promoted started".to_string(),
            ]]
        );
        // Only the last reminder is absorbed; the 09:00 one still rings.
        let absorbed: Vec<_> = plan.absorbed().into_iter().map(|k| k.fire_at).collect();
        assert_eq!(absorbed, vec![at(9, 50)]);
    }

    #[test]
    fn an_early_last_reminder_is_promoted_only_shortly_before_the_start() {
        let a = event("A", at(10, 0), at(11, 0));
        let plan = live_activity_plan(&input(vec![a.clone()], vec![reminder(&a, at(9, 0))]));
        assert_eq!(
            sketch(&plan),
            vec![vec![
                "09:00-09:45 A signal".to_string(),
                "09:45-10:00 A promoted".to_string(),
                "10:00-11:00 A promoted started".to_string(),
            ]]
        );
    }

    #[test]
    fn a_last_reminder_more_than_two_hours_ahead_makes_no_activity() {
        let a = event("A", at(12, 0), at(13, 0));
        let plan = live_activity_plan(&input(vec![a.clone()], vec![reminder(&a, at(9, 59))]));
        assert!(plan.chains.is_empty());
        // Exactly two hours is still an activity.
        let plan = live_activity_plan(&input(vec![a.clone()], vec![reminder(&a, at(10, 0))]));
        assert_eq!(plan.chains.len(), 1);
        // What counts is the LAST reminder: a day-ahead one beside a
        // quarter-hour one changes nothing.
        let day_ahead = reminder(&a, at(12, 0) - Duration::days(1));
        let plan = live_activity_plan(&input(
            vec![a.clone()],
            vec![day_ahead, reminder(&a, at(11, 45))],
        ));
        assert_eq!(plan.chains[0].start, at(11, 45));
    }

    #[test]
    fn no_reminder_no_window_and_reminders_at_or_after_the_end_do_not_count() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(12, 0), at(13, 0));
        let plan = live_activity_plan(&input(
            vec![a.clone(), b.clone()],
            vec![reminder(&b, at(13, 0)), reminder(&b, at(13, 5))],
        ));
        assert!(plan.chains.is_empty());

        // A reminder after the START is still a reminder.
        let plan = live_activity_plan(&input(vec![a.clone()], vec![reminder(&a, at(10, 5))]));
        assert_eq!(
            sketch(&plan),
            vec![vec!["10:05-11:00 A signal promoted started".to_string()]]
        );
    }

    #[test]
    fn all_day_cancelled_and_hidden_events_have_no_window() {
        let mut all_day = event("D", at(0, 0), at(23, 59));
        all_day.all_day = true;
        let mut cancelled = event("C", at(10, 0), at(11, 0));
        cancelled.cancelled = true;
        let mut hidden = event("H", at(12, 0), at(13, 0));
        hidden.calendar_id = "private".to_string();
        let triggers = vec![
            reminder(&all_day, at(8, 30)),
            reminder(&cancelled, at(9, 50)),
            reminder(&hidden, at(11, 50)),
        ];
        let mut input = input(vec![all_day, cancelled, hidden], triggers);
        input.hidden_calendars = vec!["private".to_string()];
        assert!(live_activity_plan(&input).chains.is_empty());
    }

    #[test]
    fn the_next_reminder_takes_over_and_the_chain_is_one_activity() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(10, 55), at(11, 30));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(10, 45))];
        let plan = live_activity_plan(&input(vec![a, b], triggers));
        assert_eq!(
            sketch(&plan),
            vec![vec![
                "09:50-10:00 A signal promoted next=B@10:55".to_string(),
                "10:00-10:45 A promoted started next=B@10:55".to_string(),
                "10:45-10:55 B signal promoted".to_string(),
                "10:55-11:30 B promoted started".to_string(),
            ]]
        );
        assert_eq!(plan.chains[0].keys, vec!["work|A", "work|B"]);
    }

    #[test]
    fn a_nested_appointment_hands_back_without_a_second_signal() {
        let a = event("A", at(10, 0), at(12, 0));
        let b = event("B", at(10, 30), at(11, 0));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(10, 25))];
        let plan = live_activity_plan(&input(vec![a, b], triggers));
        assert_eq!(
            sketch(&plan),
            vec![vec![
                "09:50-10:00 A signal promoted next=B@10:30".to_string(),
                "10:00-10:25 A promoted started next=B@10:30".to_string(),
                "10:25-10:30 B signal promoted".to_string(),
                "10:30-11:00 B promoted started".to_string(),
                "11:00-12:00 A promoted started".to_string(),
            ]]
        );
    }

    #[test]
    fn a_tie_shows_one_and_leaves_the_others_reminder_ringing() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(10, 5), at(10, 30));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(9, 50))];
        let plan = live_activity_plan(&input(vec![a, b], triggers));
        // A starts first, so A wins the tie; B's reminder is not absorbed.
        let absorbed: Vec<_> = plan.absorbed().into_iter().map(|k| k.item_id).collect();
        assert_eq!(absorbed, vec!["A"]);
        assert_eq!(plan.chains[0].sections[0].event.title, "A");
    }

    #[test]
    fn a_gap_makes_two_activities() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(14, 0), at(15, 0));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(13, 50))];
        let plan = live_activity_plan(&input(vec![a, b], triggers));
        assert_eq!(plan.chains.len(), 2);
        assert_eq!(plan.chains[1].start, at(13, 50));
    }

    #[test]
    fn the_follow_up_is_the_next_timed_event_that_day_and_moves_on() {
        let a = event("A", at(9, 0), at(12, 0));
        let x = event("X", at(10, 0), at(10, 15)); // no reminder of its own
        let y = event("Y", at(15, 0), at(16, 0));
        let same_start = event("S", at(9, 0), at(9, 30));
        let mut tomorrow = event("T", at(23, 0), at(23, 30));
        // Late in UTC but already tomorrow on the device.
        tomorrow.local_day = day().succ_opt().unwrap();
        let mut all_day = event("D", at(0, 0), at(23, 59));
        all_day.all_day = true;
        let triggers = vec![reminder(&a, at(8, 50))];
        let plan = live_activity_plan(&input(
            vec![a, x, y, same_start, tomorrow, all_day],
            triggers,
        ));
        assert_eq!(
            sketch(&plan),
            vec![vec![
                "08:50-09:00 A signal promoted next=X@10:00".to_string(),
                "09:00-10:00 A promoted started next=X@10:00".to_string(),
                "10:00-12:00 A promoted started next=Y@15:00".to_string(),
            ]]
        );
    }

    #[test]
    fn group_copies_are_one_window_from_the_latest_reminder() {
        let mut a = event("A", at(10, 0), at(11, 0));
        a.series_id = "A".to_string();
        let mut copy = event("A2", at(10, 0), at(11, 0));
        copy.calendar_id = "home".to_string();
        let triggers = vec![
            reminder(&a, at(9, 45)),
            reminder(&copy, at(9, 30)),
            reminder(&copy, at(9, 45)),
        ];
        let mut input = input(vec![a, copy], triggers);
        input.groups = vec![group("g", &[("work", "A"), ("home", "A2")])];
        let plan = live_activity_plan(&input);
        assert_eq!(plan.chains.len(), 1);
        assert_eq!(plan.chains[0].start, at(9, 45));
        // Both copies' 09:45 reminders become the one signal; 09:30 rings.
        let absorbed: Vec<_> = plan
            .absorbed()
            .into_iter()
            .map(|k| (k.calendar_id, k.fire_at))
            .collect();
        assert_eq!(
            absorbed,
            vec![
                ("work".to_string(), at(9, 45)),
                ("home".to_string(), at(9, 45))
            ]
        );
        // The key is the smallest member's, whichever copy is shown.
        assert_eq!(plan.chains[0].keys, vec!["home|A2"]);
    }

    #[test]
    fn a_group_whose_copies_drifted_apart_is_two_appointments() {
        let a = event("A", at(10, 0), at(11, 0));
        let mut moved = event("A2", at(10, 30), at(11, 30));
        moved.calendar_id = "home".to_string();
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&moved, at(10, 20))];
        let mut input = input(vec![a, moved], triggers);
        input.groups = vec![group("g", &[("work", "A"), ("home", "A2")])];
        let plan = live_activity_plan(&input);
        assert_eq!(plan.chains[0].keys, vec!["work|A", "home|A2"]);
        assert_eq!(plan.absorbed().len(), 2);
    }

    #[test]
    fn a_meeting_and_its_invitation_are_one_window() {
        let link = "https://x.webex.com/x/j.php?MTID=m1";
        let mut meeting = event("vc::m1", at(10, 0), at(11, 0));
        meeting.calendar_id = "acc::meetings".to_string();
        meeting.location = Some(link.to_string());
        let mut invitation = event("inv", at(10, 0), at(11, 0));
        invitation.description = Some(format!("Join: {link}"));
        invitation.location = Some("Raum 2.14".to_string());
        let triggers = vec![
            reminder(&meeting, at(9, 50)),
            reminder(&invitation, at(9, 50)),
        ];
        let plan = live_activity_plan(&input(vec![meeting, invitation], triggers));
        assert_eq!(plan.chains.len(), 1);
        let shown = &plan.chains[0].sections[0].event;
        assert_eq!(shown.item_id, "inv");
        assert_eq!(shown.location.as_deref(), Some("Raum 2.14"));
        assert_eq!(plan.absorbed().len(), 2);
    }

    #[test]
    fn a_join_link_is_not_a_place() {
        let mut meeting = event("vc::m1", at(10, 0), at(11, 0));
        meeting.calendar_id = "acc::meetings".to_string();
        meeting.location = Some("https://x.webex.com/x/j.php?MTID=m1".to_string());
        let triggers = vec![reminder(&meeting, at(9, 50))];
        let plan = live_activity_plan(&input(vec![meeting], triggers));
        assert_eq!(plan.chains[0].sections[0].event.location, None);
    }

    #[test]
    fn a_fixed_time_on_a_series_belongs_to_one_occurrence() {
        let mut first = event("S", at(10, 0), at(10, 30));
        first.original_start = Some(at(10, 0));
        let mut second = event("S", at(14, 0), at(14, 30));
        second.original_start = Some(at(14, 0));
        let fixed = |occ: &LiveOccurrence| LiveTrigger {
            absolute: true,
            ..reminder(occ, at(9, 0))
        };
        let triggers = vec![fixed(&first), fixed(&second)];
        let plan = live_activity_plan(&input(vec![first, second], triggers));
        assert_eq!(plan.chains.len(), 1);
        assert_eq!(
            plan.chains[0].keys,
            vec![format!("work|S|{}", at(10, 0).timestamp_millis())]
        );
    }

    #[test]
    fn the_horizon_and_now_bound_the_windows() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(22, 0), at(23, 0));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(21, 50))];
        let mut input = input(vec![a, b], triggers);
        input.horizon_end = at(21, 50);
        let plan = live_activity_plan(&input);
        assert_eq!(plan.chains.len(), 1);

        // A running window whose reminder is long past is still planned; one
        // that has ended is not.
        input.now = at(10, 30);
        assert_eq!(live_activity_plan(&input).chains.len(), 1);
        input.now = at(11, 0);
        assert!(live_activity_plan(&input).chains.is_empty());
    }

    #[test]
    fn the_running_appointments_key_survives_the_first_one_ending() {
        let a = event("A", at(10, 0), at(11, 0));
        let b = event("B", at(10, 55), at(11, 30));
        let triggers = vec![reminder(&a, at(9, 50)), reminder(&b, at(10, 45))];
        let mut input = input(vec![a, b], triggers);
        let before = live_activity_plan(&input);
        input.now = at(11, 10);
        let after = live_activity_plan(&input);
        assert_eq!(after.chains[0].keys, vec!["work|B"]);
        // An activity started for the chain at 09:50 shares "work|B" with it.
        assert!(before.chains[0].keys.contains(&after.chains[0].keys[0]));
    }

    #[test]
    fn a_moved_occurrence_keeps_its_key() {
        let mut moved = event("S", at(11, 0), at(12, 0));
        moved.original_start = Some(at(10, 0));
        let triggers = vec![reminder(&moved, at(10, 50))];
        let plan = live_activity_plan(&input(vec![moved], triggers));
        assert_eq!(
            plan.chains[0].keys,
            vec![format!("work|S|{}", at(10, 0).timestamp_millis())]
        );
    }
}
