//! A write that settles in steps: one request the provider confirms, then
//! more that complete it — or that take it back (decisions 202-213).
//!
//! A provider may not take a new series' deleted occurrences with it, or
//! Aperio may not trust how it takes them: the adapter creates the series
//! bare, removes them again one by one and checks the result, and where that
//! fails it deletes the series again. A provider may invite with the series or
//! only once it is proved. What such a write may take, which deletions it
//! writes again and how its outcome names the invitees is decided once, here,
//! for every adapter that writes a series this way. Each adapter keeps its own requests, its pacing and its
//! clock: this crate reads no clock (it is compiled to WebAssembly too), so
//! everything here takes the durations and days the adapter measured.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};

use crate::event_anchor::all_day_slot_day;

/// How long the steps after the first write may take, and the time kept back
/// to take that write back (decision 213).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettleBudget {
    /// For the steps that complete the write. It starts when the provider
    /// confirms the first write, which is never cut short itself (226): a slow
    /// first write must not leave nothing for the steps.
    pub settle: Duration,
    /// For taking the first write back, from the moment a step fails.
    pub undo: Duration,
}

/// A new series and its deleted occurrences: fifteen seconds to delete them
/// and prove it, fifteen more to delete the series again (213).
pub const SERIES_WITH_DELETIONS: SettleBudget = SettleBudget {
    settle: Duration::from_secs(15),
    undo: Duration::from_secs(15),
};

/// Whether one more request fits in what is left of a budget: the wait before
/// it and the slowest request of these steps so far. One that does not fit is
/// not sent, so a budget runs out between requests rather than inside one the
/// adapter would have to abandon halfway.
///
/// The first request of a take-back is sent whatever this says (226): without
/// it nothing can be taken back at all.
pub fn may_send(remaining: Duration, wait: Duration, slowest: Duration) -> bool {
    wait.checked_add(slowest)
        .is_some_and(|needed| needed <= remaining)
}

/// The word the detail of a step-wise write's outcome starts with when the
/// invitees may have heard of the write: an invitation may have gone out, so
/// the take-back sent its cancellation too (212, 264), or a series that may
/// stand may stand in their calendars as well (228).
pub const INVITED: &str = "invited";

/// The detail of [`crate::WriteRefusal::DeletionsNotKept`] and
/// [`crate::WriteLeftover::CreatedMayStand`]: the reason, a machine token for
/// the log, after [`INVITED`] when the invitees may have heard of the write.
pub fn step_detail(why: &str, invited: bool) -> String {
    let why = why.trim();
    match (invited, why.is_empty()) {
        (true, true) => INVITED.to_string(),
        (true, false) => format!("{INVITED} {why}"),
        (false, _) => why.to_string(),
    }
}

/// Whether a detail [`step_detail`] wrote says the invitees may have heard of
/// the write.
pub fn step_detail_invited(detail: &str) -> bool {
    detail.split_whitespace().next() == Some(INVITED)
}

/// The reason a detail [`step_detail`] wrote names, without [`INVITED`].
pub fn step_detail_why(detail: &str) -> &str {
    let detail = detail.trim();
    match detail.strip_prefix(INVITED) {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest.trim(),
        _ => detail,
    }
}

/// The reason a step-wise write gives when its steps did not fit the budget
/// ([`SERIES_WITH_DELETIONS`]). With [`crate::WriteRefusal::DeletionsNotKept`]
/// the surfaces say that it took too long: the provider answered slowly or
/// asked to wait, or there are very many deletions to write, and a later
/// retry may well fit. With [`crate::WriteLeftover::CreatedMayStand`] it is
/// for the log only: the surfaces say the series may stand, and that it must
/// not be written again.
pub const OUT_OF_TIME: &str = "time";

/// The reason a step-wise write gives when it gave up because the provider
/// kept asking it to wait (decision 276): the wait it asked for last did not
/// fit the budget, or waiting for it used up half the time to settle or more
/// ([`busy_used_up`]). With [`crate::WriteRefusal::DeletionsNotKept`] the
/// surfaces say the provider was busy and to try again later; with
/// [`crate::WriteLeftover::CreatedMayStand`] it is for the log only.
/// [`OUT_OF_TIME`] stays for a slow provider and very many deletions.
pub const PROVIDER_BUSY: &str = "busy";

/// Whether waiting for a busy provider used up a decisive share of a
/// step-wise write's time to settle: half of it or more (decision 276). A run
/// that then runs out of time gives [`PROVIDER_BUSY`], not [`OUT_OF_TIME`].
pub fn busy_used_up(waited: Duration, budget: SettleBudget) -> bool {
    waited.saturating_mul(2) >= budget.settle
}

/// The reason a step-wise write gives for a step it cannot send in time
/// (decision 276): [`PROVIDER_BUSY`] right after an answer that asked to wait,
/// or once waiting for such answers took half the time to settle or more
/// ([`busy_used_up`]); [`OUT_OF_TIME`] otherwise. `busy_waited` is what the
/// run waited for those answers.
pub fn out_of_time_reason(
    after_busy: bool,
    busy_waited: Duration,
    budget: SettleBudget,
) -> &'static str {
    if after_busy || busy_used_up(busy_waited, budget) {
        PROVIDER_BUSY
    } else {
        OUT_OF_TIME
    }
}

/// The day an all-day deletion names on `device`: its own date where it is
/// the device's midnight — the spelling the views and the device's own reads
/// give —, else the day the 13:45 rule reads ([`all_day_slot_day`]), for a
/// provider's UTC midnight or another zone's. The rule alone misreads a
/// device's own midnight outside its window (Niue, Pago Pago at −11,
/// Kiritimati at +14) by a day; the device's own date never does.
pub fn all_day_deletion_day<D: TimeZone>(at: DateTime<Utc>, device: &D) -> chrono::NaiveDate {
    let local = at.with_timezone(device);
    if local.time() == chrono::NaiveTime::MIN {
        local.date_naive()
    } else {
        all_day_slot_day(at)
    }
}

/// A new series' deleted occurrences, parted by [`deletions_from_today`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionsFromToday {
    /// From today on: deleted again in the new series.
    pub kept: Vec<DateTime<Utc>>,
    /// Before today: they show again in the new series, as every deletion did
    /// before decision 263. A long history would outrun the budget and send
    /// cancellations for occurrences nobody can attend any more.
    pub past: Vec<DateTime<Utc>>,
}

/// Which of a new series' deleted occurrences an adapter deletes again one by
/// one (decision 263), in their order: those that fall on the device's day
/// `now` falls on, or later. A timed one falls on its date on the `device`'s
/// clock; an all-day one on the day it names there ([`all_day_deletion_day`]:
/// its own date where it is the device's midnight, else the 13:45 rule), so
/// the same day spelled as the device's midnight or as a provider's UTC
/// midnight lands on the same side on every device. An occurrence deleted
/// earlier today is kept: it is today's.
///
/// Only an adapter that deletes a new series' occurrences one by one asks, and
/// passes the device's zone: a provider that takes them with the series —
/// CalDAV, the local store — keeps every one at no cost, and the shells send
/// every one (decision 259).
pub fn deletions_from_today<D: TimeZone>(
    deleted: &[DateTime<Utc>],
    all_day: bool,
    now: DateTime<Utc>,
    device: &D,
) -> DeletionsFromToday {
    let today = now.with_timezone(device).date_naive();
    let day_of = |at: DateTime<Utc>| {
        if all_day {
            all_day_deletion_day(at, device)
        } else {
            at.with_timezone(device).date_naive()
        }
    };
    let mut parted = DeletionsFromToday::default();
    for &gone in deleted {
        if day_of(gone) < today {
            parted.past.push(gone);
        } else {
            parted.kept.push(gone);
        }
    }
    parted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_out_of_time_names_a_busy_provider_where_it_was() {
        let budget = SERIES_WITH_DELETIONS;
        assert_eq!(
            out_of_time_reason(true, Duration::ZERO, budget),
            PROVIDER_BUSY
        );
        assert_eq!(
            out_of_time_reason(false, Duration::from_millis(7_500), budget),
            PROVIDER_BUSY
        );
        assert_eq!(
            out_of_time_reason(false, Duration::from_millis(7_499), budget),
            OUT_OF_TIME
        );
        assert_eq!(
            out_of_time_reason(false, Duration::ZERO, budget),
            OUT_OF_TIME
        );
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn zone(hours: i32) -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(hours * 3600).unwrap()
    }

    #[test]
    fn a_request_is_sent_only_while_its_wait_and_the_slowest_fit() {
        let ms = Duration::from_millis;
        assert!(may_send(ms(1000), ms(400), ms(600)));
        assert!(!may_send(ms(999), ms(400), ms(600)));
        // The first step has no slowest request yet.
        assert!(may_send(ms(1), Duration::ZERO, Duration::ZERO));
        assert!(!may_send(Duration::ZERO, ms(1), Duration::ZERO));
        // A wait no clock could add up does not fit anything.
        assert!(!may_send(Duration::MAX, Duration::MAX, ms(1)));
    }

    #[test]
    fn the_budget_is_fifteen_and_fifteen_seconds() {
        assert_eq!(SERIES_WITH_DELETIONS.settle, Duration::from_secs(15));
        assert_eq!(SERIES_WITH_DELETIONS.undo, Duration::from_secs(15));
    }

    #[test]
    fn a_detail_says_whether_the_invitees_may_have_heard() {
        assert_eq!(step_detail("no-occurrence", false), "no-occurrence");
        assert_eq!(step_detail(" time ", true), "invited time");
        assert_eq!(step_detail("", true), "invited");
        assert!(step_detail_invited("invited time"));
        assert!(step_detail_invited("invited"));
        assert!(!step_detail_invited("no-occurrence"));
        assert!(!step_detail_invited("invitation"));
        assert!(!step_detail_invited(""));
        assert_eq!(step_detail_why("invited time"), OUT_OF_TIME);
        assert_eq!(step_detail_why("time"), OUT_OF_TIME);
        assert_eq!(step_detail_why("invited"), "");
        assert_eq!(step_detail_why("invitation"), "invitation");
    }

    #[test]
    fn timed_deletions_before_the_device_s_today_are_left() {
        let deleted = [
            at("2026-10-16T08:00:00Z"),
            at("2026-10-18T05:00:00Z"),
            at("2026-10-19T08:00:00Z"),
            at("2026-10-17T22:30:00Z"),
            at("2026-10-17T21:30:00Z"),
        ];
        // 10:00 on the 18th, two hours east of UTC: 22:30 UTC on the 17th is
        // 00:30 on the 18th there, today; 21:30 UTC is 23:30 yesterday.
        let parted = deletions_from_today(&deleted, false, at("2026-10-18T08:00:00Z"), &zone(2));
        assert_eq!(
            parted.kept,
            vec![deleted[1], deleted[2], deleted[3]],
            "today, later, and one that is today on the device's clock"
        );
        assert_eq!(parted.past, vec![deleted[0], deleted[4]]);
    }

    #[test]
    fn an_occurrence_deleted_earlier_today_is_kept() {
        let deleted = [at("2026-10-18T06:00:00Z")];
        let parted = deletions_from_today(&deleted, false, at("2026-10-18T20:00:00Z"), &zone(0));
        assert_eq!(parted.kept, deleted.to_vec());
    }

    #[test]
    fn an_all_day_deletion_is_read_by_the_day_it_names_east_and_west() {
        // The 17th and the 18th, each spelled as a UTC midnight and as the
        // device's own midnight.
        for hours in [-11, -8, -5, 0, 2, 9, 12, 14] {
            let device = zone(hours);
            let midnight = |d: &str| {
                chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(device)
                    .unwrap()
                    .with_timezone(&Utc)
            };
            let deleted = [
                at("2026-10-17T00:00:00Z"),
                midnight("2026-10-17"),
                at("2026-10-18T00:00:00Z"),
                midnight("2026-10-18"),
            ];
            // Noon of the 18th on the device.
            let now = midnight("2026-10-18") + chrono::Duration::hours(12);
            let parted = deletions_from_today(&deleted, true, now, &device);
            assert_eq!(parted.past, deleted[..2].to_vec(), "UTC{hours:+}");
            assert_eq!(parted.kept, deleted[2..].to_vec(), "UTC{hours:+}");
        }
    }
}
