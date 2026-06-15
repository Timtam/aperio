//! On-demand / backlog recurrence spawning (DESIGN.md §9.12).
//!
//! Pure computation: given a completed recurring task, work out what its next
//! instance should look like — dated (`Schedule`) or undated-with-resurface
//! (`Backlog`), advanced by frequency/interval or by the next `fixed_dates`
//! trigger. No IO, no idempotency: the caller decides whether to actually
//! create the result (local adapter via SQL, the host orchestration via the
//! owning external adapter — DESIGN §9.12 "runs for external lists too").

use chrono::{Datelike, Days, Months, NaiveDate, Utc};

use crate::{
    MonthDay, NewTask, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
    Task, TaskRecurrence, TaskStatus, Weekday,
};

/// The next instance to spawn when `template` (a recurring task) is completed,
/// or `None` when nothing should be created — no recurrence, no anchor to
/// advance from, an exhausted `fixed_dates` rule, or a rule past its end date.
///
/// This is the placement-aware core of the spawner; the caller adds
/// idempotency (don't spawn a second open instance of a series) and the actual
/// create.
pub fn next_recurrence_instance(template: &Task) -> Option<NewTask> {
    let rule = template.recurrence.as_ref()?;
    match rule.placement {
        RecurrencePlacement::Schedule => next_scheduled_instance(template, rule),
        RecurrencePlacement::Backlog => next_backlog_instance(template, rule),
    }
}

/// The calendar date a task was completed on. Falls back to today when the
/// row carries no `completed_at` (a status flip without a stamp), so the
/// backlog/`FromCompletion` math always has an anchor.
fn completion_date(template: &Task) -> NaiveDate {
    template
        .completed_at
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive())
}

/// True when a date-ended rule (`OnDate`) has run past its boundary.
/// `After { occurrences }` isn't tracked here (no per-row counter) and
/// `Never`/absent never end.
fn recurrence_ended(rule: &TaskRecurrence, date: NaiveDate) -> bool {
    matches!(&rule.end, Some(RecurrenceEnd::OnDate { date: end }) if date > *end)
}

/// Next trigger date for a `Schedule`-placement rule: the next `fixed_dates`
/// entry when set, otherwise `advance` by frequency × interval.
fn next_trigger(base: NaiveDate, rule: &TaskRecurrence) -> Option<NaiveDate> {
    match rule.fixed_dates.as_ref().filter(|d| !d.is_empty()) {
        Some(dates) => next_fixed_date_after(base, dates),
        None => advance(base, rule),
    }
}

/// The earliest `MonthDay` strictly after `from`, scanning the current year
/// then the next so wrap-around (e.g. completing in November with an April
/// trigger) lands on next April. Out-of-range months are skipped; a day past
/// the month's length is clamped (Feb 30 → Feb 28/29).
fn next_fixed_date_after(from: NaiveDate, dates: &[MonthDay]) -> Option<NaiveDate> {
    let mut best: Option<NaiveDate> = None;
    for year in [from.year(), from.year() + 1] {
        for md in dates {
            if !(1..=12).contains(&md.month) {
                continue;
            }
            let day = u32::from(md.day).max(1);
            let Some(cand) = clamp_to_month(year, u32::from(md.month), day) else {
                continue;
            };
            if cand > from && best.is_none_or(|b| cand < b) {
                best = Some(cand);
            }
        }
    }
    best
}

/// Build the next `Schedule`-placement instance: a dated copy of the
/// template advanced to the next trigger. `None` when there's no anchor to
/// advance from or the rule has ended.
fn next_scheduled_instance(template: &Task, rule: &TaskRecurrence) -> Option<NewTask> {
    let base = match rule.anchor {
        RecurrenceAnchor::FromDate => template.scheduled_date.or(template.deadline_date)?,
        RecurrenceAnchor::FromCompletion => completion_date(template),
    };
    let next_date = next_trigger(base, rule)?;
    if recurrence_ended(rule, next_date) {
        return None;
    }
    // Preserve which date field(s) the template used.
    let scheduled_date = template.scheduled_date.map(|_| next_date);
    let deadline_date = template.deadline_date.map(|_| next_date);
    // A `FromCompletion` rule on an otherwise-undated task still needs a
    // date to land on; default it to the scheduled slot.
    let (scheduled_date, deadline_date) = if scheduled_date.is_none() && deadline_date.is_none() {
        (Some(next_date), None)
    } else {
        (scheduled_date, deadline_date)
    };
    Some(instance_skeleton(
        template,
        rule,
        scheduled_date,
        deadline_date,
        None,
    ))
}

/// Build the next `Backlog`-placement instance: an undated copy whose
/// `resurface_date` decides when it re-enters the active backlog. `None`
/// when a `fixed_dates` rule has no valid trigger or the rule has ended.
fn next_backlog_instance(template: &Task, rule: &TaskRecurrence) -> Option<NewTask> {
    let from = completion_date(template);
    let resurface: Option<NaiveDate> = match rule.fixed_dates.as_ref().filter(|d| !d.is_empty()) {
        Some(dates) => Some(next_fixed_date_after(from, dates)?),
        // No interval ⇒ surface immediately (the dishwasher case):
        // `None` resurface_date means "visible now".
        None if rule.interval == 0 => None,
        None => Some(advance(from, rule)?),
    };
    if let Some(d) = resurface {
        if recurrence_ended(rule, d) {
            return None;
        }
    }
    Some(instance_skeleton(template, rule, None, None, resurface))
}

/// Shared constructor for a spawned instance: inherits the template's
/// content (title, description, priority, section, color, reminders, sound,
/// recurrence rule and `series_id`), resets completion state, and takes the
/// caller's date placement. Times survive only alongside their date.
fn instance_skeleton(
    template: &Task,
    rule: &TaskRecurrence,
    scheduled_date: Option<NaiveDate>,
    deadline_date: Option<NaiveDate>,
    resurface_date: Option<NaiveDate>,
) -> NewTask {
    NewTask {
        assignees: Vec::new(),
        title: template.title.clone(),
        description: template.description.clone(),
        status: TaskStatus::Open,
        priority: template.priority,
        scheduled_date,
        scheduled_time: scheduled_date.and(template.scheduled_time),
        deadline_date,
        deadline_time: deadline_date.and(template.deadline_time),
        recurrence: Some(rule.clone()),
        resurface_date,
        // The next instance stays in the same series for idempotent spawning.
        series_id: template.series_id.clone(),
        parent_id: None,
        // Keep the next occurrence in the same section as its template.
        section_id: template.section_id.clone(),
        color_label: template.color_label.clone(),
        reminders: template.reminders.clone(),
        sound: template.sound.clone(),
    }
}

/// Compute the next occurrence date for a recurring task.
///
/// Honours `interval` (every N days/weeks/months/years) and, for
/// weekly rules with `day_of_week` set, snaps forward to the next
/// listed weekday relative to the anchor. `day_of_month` for monthly
/// rules is respected verbatim, clamped to the target month's length
/// (e.g. the 31st in February becomes the last day of February).
pub fn advance(anchor: NaiveDate, rule: &TaskRecurrence) -> Option<NaiveDate> {
    let interval = rule.interval.max(1) as i64;
    match rule.frequency {
        RecurrenceFrequency::Daily => anchor.checked_add_days(Days::new(interval as u64)),
        RecurrenceFrequency::Weekly => {
            if let Some(days) = rule.day_of_week.as_ref().filter(|d| !d.is_empty()) {
                next_weekday_after(anchor, days, interval as u64)
            } else {
                anchor.checked_add_days(Days::new(7 * interval as u64))
            }
        }
        RecurrenceFrequency::Monthly => {
            let next = anchor.checked_add_months(Months::new(interval as u32))?;
            if let Some(d) = rule.day_of_month {
                clamp_to_month(next.year(), next.month(), d.into())
            } else {
                Some(next)
            }
        }
        RecurrenceFrequency::Yearly => anchor.checked_add_months(Months::new(12 * interval as u32)),
    }
}

/// Within the same week (or the next interval-week block), find the
/// first weekday listed in `days` after the anchor.
fn next_weekday_after(
    anchor: NaiveDate,
    days: &[Weekday],
    interval_weeks: u64,
) -> Option<NaiveDate> {
    let allowed: Vec<u32> = days.iter().map(|w| weekday_to_iso(*w)).collect();
    if allowed.is_empty() {
        return None;
    }
    // Step day by day up to 7 days; if none of the next 7 days match,
    // jump to the start of the interval-week block after.
    for offset in 1..=7 {
        let candidate = anchor.checked_add_days(Days::new(offset))?;
        let iso = candidate.weekday().number_from_monday();
        if allowed.contains(&iso) {
            return Some(candidate);
        }
    }
    // Fallback for interval > 1: skip the whole gap.
    anchor.checked_add_days(Days::new(7 * interval_weeks.max(1)))
}

fn weekday_to_iso(w: Weekday) -> u32 {
    match w {
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
        Weekday::Sunday => 7,
    }
}

fn clamp_to_month(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    let last = last_day_of_month(year, month);
    NaiveDate::from_ymd_opt(year, month, day.min(last))
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    // First day of the next month minus one.
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    first_next.pred_opt().unwrap().day()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskPriority;
    use chrono::TimeZone;

    fn template(recurrence: TaskRecurrence, completed: NaiveDate) -> Task {
        Task {
            assignees: Vec::new(),
            id: "t".into(),
            list_id: "L".into(),
            title: "Chore".into(),
            description: None,
            status: TaskStatus::Completed,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: Some(recurrence),
            resurface_date: None,
            series_id: Some("series-1".into()),
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            completed_at: Some(completed.and_hms_opt(9, 0, 0).unwrap().and_utc()),
            etag: None,
        }
    }

    fn rule(
        frequency: RecurrenceFrequency,
        interval: u32,
        anchor: RecurrenceAnchor,
        placement: RecurrencePlacement,
        fixed_dates: Option<Vec<MonthDay>>,
    ) -> TaskRecurrence {
        TaskRecurrence {
            frequency,
            interval,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor,
            placement,
            fixed_dates,
        }
    }

    #[test]
    fn no_recurrence_spawns_nothing() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Daily,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(),
        );
        t.recurrence = None;
        assert!(next_recurrence_instance(&t).is_none());
    }

    #[test]
    fn backlog_immediate_is_undated_and_visible_now() {
        let t = template(
            rule(
                RecurrenceFrequency::Daily,
                0,
                RecurrenceAnchor::FromCompletion,
                RecurrencePlacement::Backlog,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(),
        );
        let next = next_recurrence_instance(&t).unwrap();
        assert_eq!(next.scheduled_date, None);
        assert_eq!(next.resurface_date, None);
        assert_eq!(next.series_id.as_deref(), Some("series-1"));
    }

    #[test]
    fn backlog_fixed_dates_resurfaces_on_next_season() {
        let t = template(
            rule(
                RecurrenceFrequency::Yearly,
                1,
                RecurrenceAnchor::FromCompletion,
                RecurrencePlacement::Backlog,
                Some(vec![
                    MonthDay { month: 4, day: 1 },
                    MonthDay { month: 10, day: 1 },
                ]),
            ),
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(),
        );
        let next = next_recurrence_instance(&t).unwrap();
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
        );
    }

    #[test]
    fn scheduled_from_completion_advances_by_interval() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Weekly,
                1,
                RecurrenceAnchor::FromCompletion,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(),
        );
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 5, 1).unwrap());
        let next = next_recurrence_instance(&t).unwrap();
        // Completed 10 May + 1 week → 17 May, landing in the scheduled slot.
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()),
        );
        assert_eq!(next.resurface_date, None);
    }
}
