//! On-demand / backlog recurrence spawning (DESIGN.md §9.12).
//!
//! Pure computation: given a completed recurring task, work out what its next
//! instance should look like — dated (`Schedule`) or undated-with-resurface
//! (`Backlog`), advanced by frequency/interval or by the next `fixed_dates`
//! trigger. No IO, no idempotency: the caller decides whether to actually
//! create the result (local adapter via SQL, the host orchestration via the
//! owning external adapter — DESIGN §9.12 "runs for external lists too").

use chrono::{Datelike, Days, Months, NaiveDate};

use crate::{
    MonthDay, NewTask, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
    Task, TaskRecurrence, TaskStatus, Weekday,
};

/// The next instance to spawn when `template` (a recurring task) is completed,
/// or `None` when nothing should be created — no recurrence, no anchor to
/// advance from, an exhausted `fixed_dates` rule, or a rule past its end date.
///
/// `completion_date` is the LOCAL calendar date the task was completed on — the
/// caller converts the UTC `completed_at` to the user's timezone. This matters:
/// a "+1 day backlog" rule completed at 00:30 LOCAL (still the previous day in
/// UTC) must resurface TOMORROW, not today; deriving the date from UTC here made
/// it land on today, so the task never left the active backlog.
///
/// This is the placement-aware core of the spawner; the caller adds
/// idempotency (don't spawn a second open instance of a series) and the actual
/// create.
pub fn next_recurrence_instance(template: &Task, completion_date: NaiveDate) -> Option<NewTask> {
    let rule = template.recurrence.as_ref()?;
    match rule.placement {
        RecurrencePlacement::Schedule => next_scheduled_instance(template, rule, completion_date),
        RecurrencePlacement::Backlog => next_backlog_instance(template, rule, completion_date),
    }
}

/// Build the COMPLETED snapshot ("completion record") to leave behind when a
/// recurring task is checked off on a provider whose NATIVE recurrence keeps no
/// completion history (Vikunja just advances the dates of the same task). The
/// provider advances the live task to its next occurrence; this record keeps
/// the just-completed turn visible under "Done".
///
/// It's a terminal, non-recurring copy: same content + date(s) + assignees,
/// status `Completed`, with `recurrence`/`series_id`/`resurface_date` cleared so
/// the provider can't repeat it and Aperio can't try to spawn from it. No
/// section (let the provider file it in its done state) and no reminders (a
/// finished task fires nothing).
pub fn completion_record_for(completed: &Task) -> NewTask {
    NewTask {
        title: completed.title.clone(),
        description: completed.description.clone(),
        status: TaskStatus::Completed,
        priority: completed.priority,
        effort: completed.effort,
        scheduled_date: completed.scheduled_date,
        scheduled_time: completed.scheduled_time,
        scheduled_end_time: completed.scheduled_end_time,
        deadline_date: completed.deadline_date,
        deadline_time: completed.deadline_time,
        deadline_reminder_days: completed.deadline_reminder_days,
        recurrence: None,
        resurface_date: None,
        series_id: None,
        parent_id: None,
        section_id: None,
        color_label: completed.color_label.clone(),
        reminders: Vec::new(),
        sound: None,
        assignees: completed.assignees.clone(),
    }
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

/// How many missed turns a single catch-up may skip over. 10 000 daily steps is
/// roughly 27 years — far past any real gap, and a hard stop for a rule whose
/// steps are pathologically small.
const MAX_CATCHUP_STEPS: u32 = 10_000;

/// The LATEST turn that is not after `not_before`, walking the rule forward
/// from `first` — or `first` itself when every turn is still ahead.
///
/// A daily task forgotten for five days used to hand back a task dated the day
/// AFTER the one that was forgotten — itself already in the past. Completing
/// that spawned the next missed day, and so on: one tick for every day gone by,
/// each new task born overdue, and to the user an endless queue of the same
/// chore on the same screen. A series carries at most one open turn (the
/// idempotency gate in the spawner's callers), so the days in between are not
/// turns waiting to be done.
///
/// Where it stops is the careful part. Running on to the first turn AFTER the
/// tick would be simpler and is right for the daily case — but it throws away
/// the turn of the period the user is standing in, and for a coarse rule that
/// period is the whole point: rent due on the 1st, paid late on the 2nd of the
/// next month, would skip a month's rent entirely and never mention it. So the
/// walk stops on the last turn that has not passed the tick. A daily rule lands
/// on the tick day itself, unchanged; a monthly one lands on this month's turn,
/// overdue by a day and visible. At worst that costs ONE more tick — never one
/// per period gone by, which is the loop this exists to close.
///
/// Returns `first` unchanged when it is not in the past. When the rule stops
/// advancing, or the walk runs out of budget, it returns the furthest point it
/// DID reach — still short of `not_before`, but never earlier than what the rule
/// produced on its own, so no input comes out worse than it went in.
fn catch_up(first: NaiveDate, rule: &TaskRecurrence, not_before: NaiveDate) -> NaiveDate {
    let mut date = first;
    let mut steps = 0;
    while date < not_before && steps < MAX_CATCHUP_STEPS {
        let Some(further) = next_trigger(date, rule) else {
            break;
        };
        // A rule that fails to move forward would loop here forever.
        if further <= date {
            break;
        }
        // The next turn is in the future: the one in hand is the last that was
        // due, and that is the one still owed.
        if further > not_before {
            break;
        }
        date = further;
        steps += 1;
    }
    date
}

/// Build the next `Schedule`-placement instance: a dated copy of the
/// template advanced to the next trigger. `None` when there's no anchor to
/// advance from or the rule has ended.
fn next_scheduled_instance(
    template: &Task,
    rule: &TaskRecurrence,
    completion_date: NaiveDate,
) -> Option<NewTask> {
    let base = match rule.anchor {
        RecurrenceAnchor::FromDate => template.scheduled_date.or(template.deadline_date)?,
        RecurrenceAnchor::FromCompletion => completion_date,
    };
    // Skip the turns that were missed while the task sat unchecked. A
    // `FromCompletion` rule already starts counting at the completion day, so
    // its first trigger is never in the past and this is a no-op there.
    let next_date = catch_up(next_trigger(base, rule)?, rule, completion_date);
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
fn next_backlog_instance(
    template: &Task,
    rule: &TaskRecurrence,
    completion_date: NaiveDate,
) -> Option<NewTask> {
    let from = completion_date;
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
/// content (title, description, priority, effort, deadline-reminder override,
/// section, color, reminders, sound, recurrence rule and `series_id`), resets
/// completion state, and takes the caller's date placement. Times survive only
/// alongside their date.
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
        effort: template.effort,
        scheduled_date,
        scheduled_time: scheduled_date.and(template.scheduled_time),
        // The block's length is part of the plan and repeats with it — but an
        // end with no start is not a block, so it travels only where the start
        // does.
        scheduled_end_time: scheduled_date
            .and(template.scheduled_time)
            .and(template.scheduled_end_time),
        deadline_date,
        deadline_time: deadline_date.and(template.deadline_time),
        deadline_reminder_days: deadline_date.and(template.deadline_reminder_days),
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

/// The next listed weekday after `anchor`: within the anchor's own week if one
/// is left there, otherwise the first listed day of the week `interval_weeks`
/// later.
///
/// The interval is why this counts WEEKS rather than days. Scanning the next
/// seven days for a listed weekday looks equivalent and is, for every-week
/// rules — but it can never return anything further out than seven days, so
/// "every 2 weeks on Monday" advanced by one week, every time, and the interval
/// was silently dropped for any rule that names its weekdays. The line meant to
/// catch that sat after a loop that always returned first, so it never ran.
///
/// Weeks start on MONDAY here, which is RFC 5545's default `WKST` and therefore
/// what the providers we round-trip RRULEs with assume. It is not the user's
/// `weekStartsOn` display setting: that one decides which column a calendar
/// starts on, and the core has no business reading it.
fn next_weekday_after(
    anchor: NaiveDate,
    days: &[Weekday],
    interval_weeks: u64,
) -> Option<NaiveDate> {
    let allowed: Vec<u32> = days.iter().map(|w| weekday_to_iso(*w)).collect();
    let first_listed = allowed.iter().min().copied()?;
    let week_start = anchor.checked_sub_days(Days::new(u64::from(
        anchor.weekday().number_from_monday() - 1,
    )))?;
    // A listed day still to come in this week wins, whatever the interval:
    // Mon+Thu every two weeks means BOTH days of every second week.
    let rest_of_week = allowed
        .iter()
        .filter_map(|iso| week_start.checked_add_days(Days::new(u64::from(iso - 1))))
        .filter(|cand| *cand > anchor)
        .min();
    if rest_of_week.is_some() {
        return rest_of_week;
    }
    let next_block = week_start.checked_add_days(Days::new(7 * interval_weeks.max(1)))?;
    next_block.checked_add_days(Days::new(u64::from(first_listed - 1)))
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
    use crate::{TaskEffort, TaskPriority};
    use chrono::{TimeZone, Utc};

    /// Spawn the next instance anchoring on the template's recorded completion
    /// date (the fixed-time test stamp makes the UTC date deterministic here —
    /// production injects the LOCAL date instead).
    fn spawn(t: &Task) -> Option<NewTask> {
        next_recurrence_instance(t, t.completed_at.unwrap().date_naive())
    }

    fn template(recurrence: TaskRecurrence, completed: NaiveDate) -> Task {
        Task {
            assignees: Vec::new(),
            id: "t".into(),
            list_id: "L".into(),
            title: "Chore".into(),
            description: None,
            status: TaskStatus::Completed,
            priority: TaskPriority::Medium,
            effort: TaskEffort::Medium,
            scheduled_date: None,
            scheduled_time: None,
            scheduled_end_time: None,
            deadline_date: None,
            deadline_time: None,
            deadline_reminder_days: None,
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
        assert!(spawn(&t).is_none());
    }

    #[test]
    fn completion_record_is_a_terminal_done_copy() {
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
        t.title = "Take pills".into();
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap());
        let rec = completion_record_for(&t);
        // Same content + date as the completed turn …
        assert_eq!(rec.title, "Take pills");
        assert_eq!(rec.status, TaskStatus::Completed);
        assert_eq!(
            rec.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap())
        );
        // … but a TERMINAL snapshot: no recurrence/series/resurface, so the
        // provider can't repeat it and Aperio can't try to spawn from it.
        assert!(rec.recurrence.is_none());
        assert!(rec.series_id.is_none());
        assert!(rec.resurface_date.is_none());
        assert!(rec.reminders.is_empty());
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
        let next = spawn(&t).unwrap();
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
        let next = spawn(&t).unwrap();
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
        );
    }

    #[test]
    fn backlog_interval_resurfaces_one_interval_after_completion() {
        // The user's case: a "recurring in backlog, in 1 day" task. The next
        // instance must resurface the day AFTER the LOCAL completion day, so it
        // leaves the active backlog — anchored on the injected date, NOT the UTC
        // date of completed_at (which at 00:30 local is still yesterday → +1
        // would land on today → never deferred).
        let t = template(
            rule(
                RecurrenceFrequency::Daily,
                1,
                RecurrenceAnchor::FromCompletion,
                RecurrencePlacement::Backlog,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 6, 21).unwrap(),
        );
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 6, 21).unwrap()).unwrap();
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 22).unwrap()),
        );
        assert_eq!(next.scheduled_date, None);
    }

    /// The reported case: a daily task forgotten for days. Ticking it off must
    /// leave TODAY'S turn — not the next of the days already gone, which would
    /// arrive overdue and demand another tick, and another.
    #[test]
    fn missed_days_collapse_into_todays_turn() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Daily,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        t.title = "Take pills".into();
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
        );
    }

    /// One day forgotten: yesterday's dose ticked today still leaves today's.
    /// The catch-up must not skip PAST the completion day.
    #[test]
    fn one_missed_day_still_leaves_the_completion_days_turn() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Daily,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 11).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()),
        );
    }

    /// Nothing was missed, so nothing is skipped — and a task finished EARLY
    /// still advances from its own day, not from the day it happened to be done.
    #[test]
    fn on_time_and_early_completions_are_untouched() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Daily,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap());
        let on_time =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            on_time.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 13).unwrap()),
        );

        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 20).unwrap());
        let early =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            early.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 21).unwrap()),
        );
    }

    /// A weekly rule keeps the turn of the week the user is standing in: the
    /// Monday two days ago was missed and is still owed, so that is what the
    /// tick leaves behind — not next Monday, which would drop a week silently.
    #[test]
    fn weekly_catch_up_keeps_the_current_periods_turn() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Weekly,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        // Monday 3 August, ticked off on Wednesday 12 August. The 10th was the
        // last Monday due; the 17th has not come round yet.
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 3).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()),
        );
    }

    /// The case the rule is written for: rent on the 1st, July's paid on the
    /// 2nd of August. August's rent is a day overdue and must be what comes
    /// next — stepping on to September would drop a month's rent in silence.
    #[test]
    fn monthly_catch_up_does_not_skip_the_month_just_started() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Monthly,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
        );
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 2).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()),
        );
    }

    /// And ticking THAT one settles the series: one extra tick, not one per
    /// period gone by. This is the bound the whole rule rests on.
    #[test]
    fn the_catch_up_costs_at_most_one_further_tick() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Monthly,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                None,
            ),
            NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
        );
        // Seven months behind: the first tick brings it to the current month …
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let first =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 2).unwrap()).unwrap();
        assert_eq!(
            first.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()),
        );
        // … and the second is already in the future.
        t.scheduled_date = first.scheduled_date;
        let second =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 2).unwrap()).unwrap();
        assert_eq!(
            second.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()),
        );
    }

    /// The walk goes through the WEEKDAY branch, not just the +7×interval one:
    /// a Monday/Thursday rule steps by named day, and the catch-up has to land
    /// on a listed weekday rather than a multiple of a week.
    #[test]
    fn catch_up_walks_named_weekdays() {
        let mut r = rule(
            RecurrenceFrequency::Weekly,
            1,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        );
        r.day_of_week = Some(vec![Weekday::Monday, Weekday::Thursday]);
        let mut t = template(r, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap());
        // Monday 3 August, ticked off on Wednesday 12 August. Thursday the 6th
        // and Monday the 10th were both due; the 10th is the later of them, and
        // Thursday the 13th has not come round yet.
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 3).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()),
        );
    }

    /// And through the FIXED-DATES branch: a seasonal trigger skips whole years
    /// at a time, so the walk must step by trigger rather than by interval.
    #[test]
    fn catch_up_walks_fixed_dates() {
        let mut t = template(
            rule(
                RecurrenceFrequency::Yearly,
                1,
                RecurrenceAnchor::FromDate,
                RecurrencePlacement::Schedule,
                Some(vec![MonthDay { month: 4, day: 1 }]),
            ),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        // Last done for the season of 2024; ticked off in August 2026, so April
        // 2025 is long gone but April 2026 is the season still owed.
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2024, 4, 1).unwrap());
        let next =
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).unwrap();
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
        );
    }

    /// "Every 2 weeks on Monday" used to advance ONE week: naming the weekdays
    /// dropped the interval entirely, because the day-by-day scan always found
    /// a match before the line that applies it.
    #[test]
    fn weekly_interval_survives_naming_the_weekdays() {
        let mut r = rule(
            RecurrenceFrequency::Weekly,
            2,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        );
        r.day_of_week = Some(vec![Weekday::Monday]);
        // Monday 3 August + 2 weeks = Monday 17 August, not the 10th.
        assert_eq!(
            advance(NaiveDate::from_ymd_opt(2026, 8, 3).unwrap(), &r),
            NaiveDate::from_ymd_opt(2026, 8, 17),
        );
    }

    /// Several named days belong to the SAME week, so an every-two-weeks Mon+Thu
    /// rule runs Mon, Thu, then skips a week — not Mon, Thu, Mon, Thu weekly.
    #[test]
    fn weekly_interval_keeps_every_listed_day_of_its_own_week() {
        let mut r = rule(
            RecurrenceFrequency::Weekly,
            2,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        );
        r.day_of_week = Some(vec![Weekday::Monday, Weekday::Thursday]);
        let mon = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let thu = advance(mon, &r).unwrap();
        assert_eq!(thu, NaiveDate::from_ymd_opt(2026, 8, 6).unwrap());
        // Nothing listed is left that week, so the next block starts on the
        // 17th and its first listed day is the Monday.
        assert_eq!(advance(thu, &r), NaiveDate::from_ymd_opt(2026, 8, 17),);
    }

    /// Every-week rules keep exactly the dates they always had — the fix must
    /// not move the common case by a day.
    #[test]
    fn weekly_interval_one_is_unchanged() {
        let mut r = rule(
            RecurrenceFrequency::Weekly,
            1,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        );
        r.day_of_week = Some(vec![Weekday::Monday, Weekday::Thursday]);
        // Mon → Thu of the same week …
        let mon = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        assert_eq!(advance(mon, &r), NaiveDate::from_ymd_opt(2026, 8, 6));
        // … Thu → the Monday after, crossing the week boundary.
        let thu = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        assert_eq!(advance(thu, &r), NaiveDate::from_ymd_opt(2026, 8, 10));
        // A Sunday anchor is the week's LAST day, so it always crosses.
        let sun = NaiveDate::from_ymd_opt(2026, 8, 9).unwrap();
        assert_eq!(advance(sun, &r), NaiveDate::from_ymd_opt(2026, 8, 10));
    }

    /// A rule whose end lies inside the skipped stretch spawns nothing at all —
    /// the catch-up must not carry a series past its own last day.
    #[test]
    fn catch_up_stops_at_the_rules_end() {
        let mut r = rule(
            RecurrenceFrequency::Daily,
            1,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        );
        r.end = Some(RecurrenceEnd::OnDate {
            date: NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(),
        });
        let mut t = template(r, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap());
        t.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
        assert!(
            next_recurrence_instance(&t, NaiveDate::from_ymd_opt(2026, 8, 12).unwrap()).is_none()
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
        let next = spawn(&t).unwrap();
        // Completed 10 May + 1 week → 17 May, landing in the scheduled slot.
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()),
        );
        assert_eq!(next.resurface_date, None);
    }
}
