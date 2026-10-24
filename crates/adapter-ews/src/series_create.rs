//! A new series that keeps its deleted occurrences (decisions 202-213, 225,
//! 263-266).
//!
//! Exchange's `CreateItem` takes a series' rule but not its deleted
//! occurrences — `DeletedOccurrences` is read-only (decision 140) —, so a
//! series created with deletions showed every one of them again. The adapter
//! now creates the series bare and silently, deletes each deleted occurrence
//! again by its slot, proves the result with a read of the series, and only
//! then invites (225). Where a step fails it deletes the new series again
//! (203): nothing stands and the old series is untouched. Where even that
//! fails, it says the series may stand (210).
//!
//! What the steps may take, which deletions are written again and how the
//! outcome is told are cal-core's ([`cal_core::SERIES_WITH_DELETIONS`],
//! [`cal_core::deletions_from_today`], [`cal_core::step_detail`]); the clock,
//! the pacing and the requests are the adapter's.
//!
//! An occurrence is found by its slot, never by an index alone: the index
//! Aperio computes from the rule is a guess ([`crate::mapping::nominal_occurrence_index`]
//! expands it in UTC), and only the occurrence Exchange answers at that index
//! says where it stands. A slot Exchange's stored rule makes no occurrence at
//! — before the series' start, after its end, or off its pattern — has nothing
//! to delete and is skipped (207, 266); one it does make, but where no live
//! occurrence is found, takes everything back.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use cal_core::{Event, NewEvent, SettleBudget};
use chrono::{DateTime, TimeZone, Utc};
use tokio::time::Instant;

use crate::api::EwsClient;
use crate::error::{EwsError, EwsResult};
use crate::mapping::{EventIdKind, ParsedItem};
use crate::windows_tz::ServerTimeZones;

/// How the adapter sends its requests. [`EwsClient`] posts to the server; the
/// tests post to a mailbox they script. The answer is the raw body: a
/// per-item error is read here, not turned into an error on the way.
pub(crate) trait Mailbox {
    fn post(&self, body: String) -> impl Future<Output = EwsResult<String>> + Send;
}

impl Mailbox for EwsClient {
    fn post(&self, body: String) -> impl Future<Output = EwsResult<String>> + Send {
        self.post_soap_raw(body)
    }
}

/// How the adapter paces the steps on Exchange's clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pacing {
    /// The first wait after a busy answer that names no back-off, doubled for
    /// each one after it…
    pub first_wait: Duration,
    /// …up to this.
    pub max_wait: Duration,
    /// What the reads that prove the result may take past the budget's end,
    /// out of the time kept to take the series back (critique 9): the proof
    /// read, and with invitees the read after the invitation; no write. They
    /// must not throw finished work away.
    pub proof_borrow: Duration,
    /// The least time the first request of a take-back, and the read that
    /// confirms it, are given, whatever is left (226).
    pub undo_floor: Duration,
}

impl Pacing {
    pub(crate) const EXCHANGE: Self = Self {
        first_wait: Duration::from_millis(500),
        max_wait: Duration::from_secs(4),
        proof_borrow: Duration::from_secs(5),
        undo_floor: Duration::from_secs(5),
    };
}

/// Two instants this close name the same slot: the tolerance the single
/// occurrence delete has always used ([`crate::api::delete_series_occurrence`]),
/// well under any real gap between two occurrences (daily = 24 h).
const SLOT_TOLERANCE: chrono::Duration = chrono::Duration::hours(6);

/// The SOAP codes with which a silent `CreateItem` may have saved the series
/// and lost the answer: the server failed inside the store. Any other code is
/// Exchange turning the request down before it saved anything — a
/// `SendToNone` create has no send stage that could fail after the save.
const UNSURE_CREATE_CODES: &[&str] = &[
    "ErrorInternalServerError",
    "ErrorInternalServerTransientError",
    "ErrorTimeoutExpired",
    "ErrorMailboxStoreUnavailable",
    "Unknown",
];

/// The codes with which Exchange turns down a write to an item whose
/// ChangeKey moved on: each occurrence delete changes the series' key, and
/// the deletes send none, which Exchange accepted (live round 7); should it
/// ever ask for one, the series is read for its key and the delete sent once
/// more (critique 7).
const CHANGE_KEY_CODES: &[&str] = &[
    "ErrorChangeKeyRequiredForWriteOperations",
    "ErrorStaleObject",
    "ErrorIrresolvableConflict",
];

/// Create `event` — a series with deleted occurrences — as Exchange can: bare,
/// then each deletion from today on again, proved, then invited. See the
/// module documentation. `device`, `now`, `budget` and `pacing` are seams for
/// the tests; [`crate::api::create_event`] passes the device's clock,
/// [`cal_core::SERIES_WITH_DELETIONS`] and [`Pacing::EXCHANGE`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_keeping_deletions<M, D>(
    mailbox: &M,
    calendar_id: &str,
    event: NewEvent,
    server_zones: Option<&ServerTimeZones>,
    device: &D,
    now: DateTime<Utc>,
    budget: SettleBudget,
    pacing: Pacing,
) -> EwsResult<Event>
where
    M: Mailbox + Sync,
    D: TimeZone + Sync,
{
    let asked = event
        .recurrence
        .as_ref()
        .map(|r| r.exceptions.clone())
        .unwrap_or_default();
    let parted = cal_core::deletions_from_today(&asked, event.all_day, now, device);
    // One slot per day or instant, in order, as the series' occurrences come.
    let mut wanted: Vec<Wanted> = parted
        .kept
        .iter()
        .map(|&asked| {
            let day = event
                .all_day
                .then(|| cal_core::all_day_deletion_day(asked, device));
            Wanted {
                asked,
                slot: day
                    .and_then(|day| crate::mapping::local_midnight_in(day, device))
                    .unwrap_or(asked),
                day,
            }
        })
        .collect();
    wanted.sort_by_key(|w| w.slot);
    wanted.dedup_by_key(|w| w.slot);

    let (folder_id, folder_change_key) = crate::mapping::split_calendar_id(calendar_id);
    let item_xml = crate::mapping::new_event_to_calendar_item_xml_in(&event, server_zones, device)?;
    // Invited only where the request names someone to invite: the attendee
    // block the create writes (critique 12).
    let attendees_xml = crate::mapping::required_attendees_xml(&event.attendees);
    let invite = event.send_invitations && !attendees_xml.is_empty();

    if wanted.is_empty() {
        // Every deletion lies in the past (263): the series is created as
        // every series is, invitations with it, and shows them again.
        let created = plain_create(
            mailbox,
            &folder_id,
            folder_change_key.as_deref(),
            &item_xml,
            invite,
        )
        .await?;
        tracing::info!(
            target: "adapter_ews::write",
            past = parted.past.len(),
            "exchange series created; its deleted occurrences all lie in the past and show again"
        );
        return Ok(created_event(
            &event,
            calendar_id,
            &created.id,
            created.change_key,
            Vec::new(),
        ));
    }

    let mut run = Run::new(
        mailbox,
        &event,
        &wanted,
        &parted.past,
        invite,
        budget,
        pacing,
    );
    let created = run
        .create(&folder_id, folder_change_key.as_deref(), &item_xml)
        .await?;
    run.master = created.id.clone();
    match run.complete(device, &attendees_xml).await {
        Ok(change_key) => {
            run.log_success();
            let written = run
                .deleted
                .values()
                .map(|done| done.asked)
                .collect::<Vec<_>>();
            Ok(created_event(
                &event,
                calendar_id,
                &created.id,
                change_key,
                written,
            ))
        }
        Err(failure) => Err(run.take_back(failure).await),
    }
}

/// A deletion the series is to keep: as the request spelled it, and the slot
/// it is compared by — for an all-day series the device's midnight of the day
/// it names, whichever zone's midnight spelled it.
#[derive(Debug, Clone, Copy)]
struct Wanted {
    asked: DateTime<Utc>,
    slot: DateTime<Utc>,
    /// The day an all-day deletion names on the device; `None` for a timed one.
    day: Option<chrono::NaiveDate>,
}

/// An occurrence this run deleted: where it was asked for, and where Exchange
/// said it stood.
#[derive(Debug, Clone, Copy)]
struct Deleted {
    asked: DateTime<Utc>,
    stood: DateTime<Utc>,
}

/// Why the steps stopped, for the log and the detail.
#[derive(Debug, Clone)]
struct Failure {
    why: String,
    step: &'static str,
}

impl Failure {
    fn new(step: &'static str, why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            step,
        }
    }
}

/// What a probe of one occurrence found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// An occurrence, at this slot (an exception's `OriginalStart`, else its
    /// `Start`).
    At(DateTime<Utc>),
    /// Exchange says it is deleted from the series.
    Deleted,
    /// The index lies past the series' end.
    PastEnd,
    /// The server is busy; the back-off it names, if any.
    Busy(Option<Duration>),
    /// An answer that says nothing.
    Unreadable,
}

/// [`Answer`] of a GetItem on one occurrence, read from its raw body.
pub(crate) fn probe_answer(xml: &str) -> Answer {
    if let Some(back_off) = busy_in(xml) {
        return Answer::Busy(back_off);
    }
    let start = crate::mapping::parse_get_calendar_items_response(xml)
        .ok()
        .and_then(|items| {
            items
                .into_iter()
                .find_map(|it| it.original_start.or(it.start))
        });
    match start {
        Some(start) => Answer::At(start),
        None if xml.contains("ErrorCalendarOccurrenceIsDeletedFromRecurrence") => Answer::Deleted,
        None if xml.contains("ErrorCalendarOccurrenceIndexIsOutOfRecurrenceRange") => {
            Answer::PastEnd
        }
        None => Answer::Unreadable,
    }
}

/// Whether an answer says the server is busy (`ErrorServerBusy` as a
/// response code or a fault, or HTTP 429, [`crate::busy_detail`]), and the
/// back-off it names (`BackOffMilliseconds`), read from the raw body: a fault
/// sent with HTTP 500 keeps its body as the error's message.
pub(crate) fn busy_of(answer: &EwsResult<String>) -> Option<Option<Duration>> {
    match answer {
        Ok(xml) => busy_in(xml),
        Err(EwsError::Http {
            status: 500,
            message,
        }) => busy_in(message),
        // HTTP 429 names no back-off: the step backs off on its own clock.
        Err(err) => crate::busy_detail(err).map(|_| None),
    }
}

fn busy_in(body: &str) -> Option<Option<Duration>> {
    // The answer's own code says so, never text somebody wrote: a subject, a
    // place or a description may name the code as well.
    let busy = matches!(
        crate::soap::check_for_fault(body),
        Err(EwsError::Soap { code, .. }) if bare_code(&code) == "ErrorServerBusy"
    );
    if !busy {
        return None;
    }
    let back_off = body.find("BackOffMilliseconds").and_then(|at| {
        let rest = &body[at..];
        let value = &rest[rest.find('>')? + 1..];
        let digits: String = value
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse::<u64>().ok().map(Duration::from_millis)
    });
    Some(back_off)
}

/// What a write's answer says: done only where a response message says
/// `Success`; a fault or an error message is that error. Anything else — an
/// empty body, a 200 whose body was lost on the way, a proxy's page — proves
/// nothing and reads as a lost answer, which the caller decides by reading.
fn write_outcome(answer: EwsResult<String>) -> EwsResult<()> {
    let xml = answer?;
    match crate::soap::check_for_fault(&xml) {
        // A body no XML reader takes says nothing either.
        Err(EwsError::Protocol(_)) => {
            return Err(EwsError::Network("an answer that says nothing".into()))
        }
        other => other?,
    }
    if xml.contains("ResponseClass=\"Success\"") {
        Ok(())
    } else {
        Err(EwsError::Network("an answer that says nothing".into()))
    }
}

/// A SOAP code without the namespace prefix a fault's `faultcode` carries.
fn bare_code(code: &str) -> &str {
    code.rsplit(':').next().unwrap_or(code)
}

/// The machine word for an error, for the log and the detail.
fn why_of(err: &EwsError) -> String {
    match err {
        EwsError::Http { status, .. } => format!("http-{status}"),
        EwsError::Soap { code, .. } => format!("ews-{}", bare_code(code)),
        EwsError::Network(_) | EwsError::Unreached(_) => "network".to_string(),
        EwsError::Protocol(_) => "unreadable".to_string(),
        EwsError::Config(_) | EwsError::DiscoveryFailed(_) => "config".to_string(),
        EwsError::NotKept { .. } => "not-kept".to_string(),
        EwsError::PartlySaved { step, .. } => format!("partly-saved-{step}"),
    }
}

/// The created series: its id and key.
struct Created {
    id: String,
    change_key: Option<String>,
}

/// A create without deletions to write: exactly today's single `CreateItem`.
async fn plain_create<M: Mailbox + Sync>(
    mailbox: &M,
    folder_id: &str,
    folder_change_key: Option<&str>,
    item_xml: &str,
    invite: bool,
) -> EwsResult<Created> {
    let response = mailbox
        .post(crate::soap::create_calendar_item(
            folder_id,
            folder_change_key,
            item_xml,
            invite,
        ))
        .await?;
    crate::soap::check_for_fault(&response)?;
    let item = crate::mapping::parse_first_item_id(&response)?;
    Ok(Created {
        id: item.id,
        change_key: item.change_key,
    })
}

/// The event a create returns, minted from the request (as
/// [`crate::api::create_event`] always has), with the deletions really
/// written: one skipped or past shows on the next read, and the series says
/// what Exchange holds.
fn created_event(
    event: &NewEvent,
    calendar_id: &str,
    item_id: &str,
    change_key: Option<String>,
    written: Vec<DateTime<Utc>>,
) -> Event {
    let mut written_event = event.clone();
    if let Some(recurrence) = written_event.recurrence.as_mut() {
        recurrence.exceptions = written;
    }
    crate::api::build_event_from_new(
        &written_event,
        calendar_id,
        EventIdKind::RecurringMaster,
        item_id,
        change_key,
    )
}

/// One create with deletions, from its first request to its outcome.
struct Run<'a, M> {
    mailbox: &'a M,
    event: &'a NewEvent,
    wanted: &'a [Wanted],
    past: &'a [DateTime<Utc>],
    invite: bool,
    budget: SettleBudget,
    pacing: Pacing,
    master: String,
    /// When Exchange confirmed the create: the budget starts here (226).
    confirmed: Option<Instant>,
    started: Instant,
    /// The occurrences deleted, by index.
    deleted: BTreeMap<u32, Deleted>,
    /// The answers read, by index, in this run.
    answers: BTreeMap<u32, Answer>,
    skipped: usize,
    requests: usize,
    retries: usize,
    waited: Duration,
    /// The part of `waited` spent on answers that asked to wait (276).
    busy_waited: Duration,
    slowest: Duration,
    /// Whether an invitation may have gone out.
    invited: bool,
}

impl<'a, M: Mailbox + Sync> Run<'a, M> {
    fn new(
        mailbox: &'a M,
        event: &'a NewEvent,
        wanted: &'a [Wanted],
        past: &'a [DateTime<Utc>],
        invite: bool,
        budget: SettleBudget,
        pacing: Pacing,
    ) -> Self {
        Self {
            mailbox,
            event,
            wanted,
            past,
            invite,
            budget,
            pacing,
            master: String::new(),
            confirmed: None,
            started: Instant::now(),
            deleted: BTreeMap::new(),
            answers: BTreeMap::new(),
            skipped: 0,
            requests: 0,
            retries: 0,
            waited: Duration::ZERO,
            busy_waited: Duration::ZERO,
            slowest: Duration::ZERO,
            invited: false,
        }
    }

    /// The silent `CreateItem` (225), never cut short by the clock (226).
    /// Turned down before anything was saved, it is that error; where the
    /// series may have been saved but its id never arrived, nothing can be
    /// taken back and it may stand.
    async fn create(
        &mut self,
        folder_id: &str,
        folder_change_key: Option<&str>,
        item_xml: &str,
    ) -> EwsResult<Created> {
        self.requests += 1;
        let answer = self
            .mailbox
            .post(crate::soap::create_calendar_item(
                folder_id,
                folder_change_key,
                item_xml,
                false,
            ))
            .await;
        let lost = |why: &str| EwsError::NotKept {
            undone: false,
            detail: cal_core::step_detail(why, false),
        };
        let response = match answer {
            Ok(response) => response,
            // Nothing reached the server.
            Err(err @ EwsError::Unreached(_)) => return Err(err),
            // Turned down whole — a busy server or a refusal —, or the sign-in
            // or the folder failed: nothing was saved.
            Err(err @ EwsError::Http { status, .. })
                if cal_core::WriteRefusal::turned_down_by_status(status).is_some()
                    || matches!(status, 401 | 403 | 404) =>
            {
                return Err(err)
            }
            Err(EwsError::Http {
                status: 500,
                message,
            }) => match crate::soap::check_for_fault(&message) {
                Err(EwsError::Soap { code, .. })
                    if !UNSURE_CREATE_CODES.contains(&bare_code(&code))
                        && bare_code(&code).starts_with("Error") =>
                {
                    return Err(EwsError::Http {
                        status: 500,
                        message,
                    })
                }
                fault => {
                    let why = match fault {
                        Err(err @ EwsError::Soap { .. }) => why_of(&err),
                        _ => "http-500".to_string(),
                    };
                    self.log_lost(&why);
                    return Err(lost("create-answer-lost"));
                }
            },
            Err(err) => {
                self.log_lost(&why_of(&err));
                return Err(lost("create-answer-lost"));
            }
        };
        if let Err(err) = crate::soap::check_for_fault(&response) {
            return match &err {
                EwsError::Soap { code, .. } if UNSURE_CREATE_CODES.contains(&bare_code(code)) => {
                    self.log_lost(&why_of(&err));
                    Err(lost("create-answer-lost"))
                }
                // An answer that came but cannot be read: its id never
                // arrived, and the series may have been saved.
                EwsError::Protocol(_) => {
                    self.log_lost("unreadable");
                    Err(lost("create-answer-lost"))
                }
                _ => Err(err),
            };
        }
        let Ok(item) = crate::mapping::parse_first_item_id(&response) else {
            self.log_lost("unreadable");
            return Err(lost("create-answer-lost"));
        };
        self.confirmed = Some(Instant::now());
        Ok(Created {
            id: item.id,
            change_key: item.change_key,
        })
    }

    /// When the steps must be done: the budget from the confirmed create.
    fn settle_until(&self) -> Instant {
        self.confirmed.unwrap_or(self.started) + self.budget.settle
    }

    /// Delete, prove, invite, prove: the master's ChangeKey at the end.
    async fn complete<D: TimeZone + Sync>(
        &mut self,
        device: &D,
        attendees_xml: &str,
    ) -> Result<Option<String>, Failure> {
        let settle_until = self.settle_until();
        // The control before: the series as Exchange stored it, with nothing
        // deleted yet.
        let stored = self.read_master(settle_until, "control").await?;
        if !stored.deleted_occurrence_starts.is_empty() {
            return Err(Failure::new("control", "control"));
        }
        let (Some(rule), Some(first)) = (stored.recurrence.clone(), stored.start) else {
            return Err(Failure::new("control", "control"));
        };
        let day_zone = if stored.is_all_day {
            crate::mapping::all_day_zone(&stored)
        } else {
            None
        };
        let slot_of = |at: DateTime<Utc>| match &day_zone {
            Some(zone) => crate::mapping::all_day_anchor_in(at, Some(zone), device),
            None if stored.is_all_day => crate::mapping::all_day_anchor_in(at, None, device),
            None => at,
        };
        let first_slot = slot_of(first);
        for wanted in self.wanted {
            // Before the series' first occurrence: nothing to delete (207).
            if wanted.slot < first_slot - SLOT_TOLERANCE {
                self.skip(wanted, "before-start");
                continue;
            }
            let candidate =
                crate::mapping::nominal_occurrence_index(&rule, first, wanted.slot).unwrap_or(1);
            match self
                .find(candidate, wanted.slot, &slot_of, settle_until)
                .await?
            {
                Found::At(index, stood) => {
                    self.delete(index, wanted, stood, &slot_of, settle_until)
                        .await?
                }
                Found::NotMade(where_) => self.skip(wanted, where_),
                // A slot this close to one deleted already: the same
                // occurrence, spelled apart.
                Found::Already => {}
            }
        }
        // The proof (208, 230): the series' deletions are exactly the ones
        // this run made. Silent when invitations follow, so a mismatch is
        // taken back before anyone hears of the series (225).
        let proof_until = settle_until + self.pacing.proof_borrow;
        let proved = self.read_master(proof_until, "after-control").await?;
        self.prove(&proved, &slot_of)?;
        if !self.invite {
            return Ok(proved.change_key);
        }
        // A write that mails the invitees: only inside the steps' own time,
        // never in the time borrowed for the proof's read.
        self.send_invitation(attendees_xml, settle_until).await?;
        // The invitation wrote only the invitees; read once more that it
        // kept the deletions, and for the key it left.
        let final_read = self.read_master(proof_until, "after-control").await?;
        self.prove(&final_read, &slot_of)?;
        Ok(final_read.change_key)
    }

    fn skip(&mut self, wanted: &Wanted, where_: &str) {
        self.skipped += 1;
        tracing::info!(
            target: "adapter_ews::write",
            slot = %describe_slot(wanted),
            r#where = where_,
            "a deleted occurrence the stored rule makes no occurrence at is skipped",
        );
    }

    /// Post one request inside the time left until `until`, waiting out busy
    /// answers while they fit; a request that cannot be sent in time fails
    /// with [`cal_core::OUT_OF_TIME`].
    async fn send(
        &mut self,
        body: &str,
        until: Instant,
        step: &'static str,
        must_send: bool,
    ) -> Result<EwsResult<String>, Failure> {
        let mut attempt = 0u32;
        let mut first = true;
        loop {
            let remaining = until.saturating_duration_since(Instant::now());
            let floor = if must_send && first {
                self.pacing.undo_floor
            } else {
                Duration::ZERO
            };
            if !(first && must_send) && !cal_core::may_send(remaining, Duration::ZERO, self.slowest)
            {
                return Err(Failure::new(step, self.out_of_time(attempt > 0)));
            }
            first = false;
            let window = remaining.max(floor);
            self.requests += 1;
            let sent = Instant::now();
            let answer =
                match tokio::time::timeout(window, self.mailbox.post(body.to_string())).await {
                    Ok(answer) => answer,
                    Err(_) => Err(EwsError::Network("timed out".into())),
                };
            self.slowest = self.slowest.max(sent.elapsed());
            let Some(back_off) = busy_of(&answer) else {
                return Ok(answer);
            };
            let wait = back_off.unwrap_or_else(|| {
                self.pacing
                    .first_wait
                    .saturating_mul(1 << attempt.min(8))
                    .min(self.pacing.max_wait)
            });
            attempt += 1;
            let remaining = until.saturating_duration_since(Instant::now());
            if !cal_core::may_send(remaining, wait, self.slowest) {
                return Err(Failure::new(step, cal_core::PROVIDER_BUSY));
            }
            self.retries += 1;
            self.waited += wait;
            self.busy_waited += wait;
            tokio::time::sleep(wait).await;
        }
    }

    /// Why a step that cannot be sent in time stops: the provider kept asking
    /// to wait — right before it, or for half the time to settle or more —,
    /// or the run was slow (decision 276).
    fn out_of_time(&self, after_busy: bool) -> &'static str {
        cal_core::out_of_time_reason(after_busy, self.busy_waited, self.budget)
    }

    /// The master as Exchange holds it now.
    async fn read_master(
        &mut self,
        until: Instant,
        step: &'static str,
    ) -> Result<ParsedItem, Failure> {
        let body = crate::soap::get_calendar_items_with_recurrence(&[(self.master.clone(), None)]);
        let answer = self.send(&body, until, step, false).await?;
        let xml = answer.map_err(|err| Failure::new(step, why_of(&err)))?;
        crate::soap::check_for_fault(&xml).map_err(|err| Failure::new(step, why_of(&err)))?;
        crate::mapping::parse_get_calendar_items_response(&xml)
            .ok()
            .and_then(|items| items.into_iter().find(|it| it.item_id == self.master))
            .ok_or_else(|| Failure::new(step, "unreadable"))
    }

    /// The answer at `index`, read once per run.
    async fn probe(&mut self, index: u32, until: Instant) -> Result<Answer, Failure> {
        if let Some(&answer) = self.answers.get(&index) {
            return Ok(answer);
        }
        let body = crate::soap::get_occurrence_item(&self.master, None, index);
        let answer = match self.send(&body, until, "probe", false).await? {
            Ok(xml) => probe_answer(&xml),
            Err(_) => Answer::Unreadable,
        };
        if !matches!(answer, Answer::Busy(_) | Answer::Unreadable) {
            self.answers.insert(index, answer);
        }
        Ok(answer)
    }

    /// Find the occurrence at `slot`. What every answer of the run says is
    /// kept as two bounds: the last index known to stand before the slot and
    /// the first known to stand after it, or past the series' end. The next
    /// index read lies between them, guessed from the spacing of the
    /// occurrences read so far: the UTC guess `candidate` can be off by a
    /// step, or by many where Exchange's pattern counts occurrences the RFC
    /// expansion skips (a series on the 31st has one on the last day of every
    /// shorter month). Two bounds next to each other with none at the slot, or
    /// the series' first occurrence after it, mean the stored rule makes none
    /// there (207, 266).
    async fn find(
        &mut self,
        candidate: u32,
        slot: DateTime<Utc>,
        slot_of: &impl Fn(DateTime<Utc>) -> DateTime<Utc>,
        until: Instant,
    ) -> Result<Found, Failure> {
        /// The most occurrences one slot reads; the budget ends it sooner.
        const MAX_PROBES: usize = 64;
        let mut bounds = Bounds::default();
        // What this run read already, for other slots, counts too.
        let known: Vec<(u32, Answer)> = self.answers.iter().map(|(&i, &a)| (i, a)).collect();
        for (index, answer) in known {
            match self.place_of(index, answer, slot, slot_of)? {
                Place::Match(start) => return Ok(Found::At(index, start)),
                Place::Already => return Ok(Found::Already),
                place => bounds.note(index, place),
            }
        }
        for _ in 0..MAX_PROBES {
            if let Some(found) = bounds.decided()? {
                return Ok(found);
            }
            let index = bounds.next(candidate, slot);
            let answer = self.probe(index, until).await?;
            match self.place_of(index, answer, slot, slot_of)? {
                Place::Match(start) => return Ok(Found::At(index, start)),
                Place::Already => return Ok(Found::Already),
                place => bounds.note(index, place),
            }
        }
        Err(Failure::new("probe", "no-occurrence"))
    }

    /// Where the occurrence at `index` stands against `slot`, by its answer.
    fn place_of(
        &self,
        index: u32,
        answer: Answer,
        slot: DateTime<Utc>,
        slot_of: &impl Fn(DateTime<Utc>) -> DateTime<Utc>,
    ) -> Result<Place, Failure> {
        let side = |stood: DateTime<Utc>| {
            if stood < slot {
                Place::Before(stood)
            } else {
                Place::After(Some(stood))
            }
        };
        Ok(match answer {
            Answer::At(start) => {
                let stood = slot_of(start);
                if (stood - slot).abs() <= SLOT_TOLERANCE {
                    Place::Match(start)
                } else {
                    side(stood)
                }
            }
            Answer::Deleted => match self.deleted.get(&index) {
                // Deleted by this run, for a slot this close: done already.
                Some(done) if (slot_of(done.stood) - slot).abs() <= SLOT_TOLERANCE => {
                    Place::Already
                }
                // Deleted by this run: it stood where Exchange said.
                Some(done) => side(slot_of(done.stood)),
                // A fresh series has no deletion this run did not make.
                None => return Err(Failure::new("probe", "foreign-deletion")),
            },
            Answer::PastEnd => Place::After(None),
            Answer::Busy(_) | Answer::Unreadable => {
                return Err(Failure::new("probe", "unreadable"))
            }
        })
    }

    /// Delete the occurrence at `index` again, silently (225).
    async fn delete(
        &mut self,
        index: u32,
        wanted: &Wanted,
        stood: DateTime<Utc>,
        slot_of: &impl Fn(DateTime<Utc>) -> DateTime<Utc>,
        until: Instant,
    ) -> Result<(), Failure> {
        let mut change_key: Option<String> = None;
        let mut resent = false;
        loop {
            let body = crate::soap::delete_occurrence_item(
                &self.master,
                change_key.as_deref(),
                index,
                false,
            );
            let answer = self.send(&body, until, "delete", false).await?;
            match write_outcome(answer) {
                Ok(()) => break,
                Err(EwsError::Soap { code, .. })
                    if CHANGE_KEY_CODES.contains(&bare_code(&code)) && !resent =>
                {
                    // Read the series for its key and send it once more.
                    resent = true;
                    self.retries += 1;
                    let master = self.read_master(until, "delete").await?;
                    change_key = master.change_key;
                }
                Err(err @ (EwsError::Network(_) | EwsError::Http { .. })) => {
                    // The answer may be lost after the delete was made: the
                    // occurrence itself says.
                    self.answers.remove(&index);
                    match self.probe(index, until).await? {
                        Answer::Deleted => break,
                        Answer::At(start) if slot_of(start) == slot_of(stood) && !resent => {
                            resent = true;
                            self.retries += 1;
                            self.answers.remove(&index);
                        }
                        _ => return Err(Failure::new("delete", why_of(&err))),
                    }
                }
                Err(err) => return Err(Failure::new("delete", why_of(&err))),
            }
        }
        self.answers.insert(index, Answer::Deleted);
        self.deleted.insert(
            index,
            Deleted {
                asked: wanted.asked,
                stood,
            },
        );
        Ok(())
    }

    /// The series' deletions are exactly the ones this run made, slot for
    /// slot: no more (a wrong date deleted, one someone else made) and no
    /// fewer (a delete Exchange answered but did not make).
    fn prove(
        &self,
        master: &ParsedItem,
        slot_of: &impl Fn(DateTime<Utc>) -> DateTime<Utc>,
    ) -> Result<(), Failure> {
        let mut made: Vec<DateTime<Utc>> =
            self.deleted.values().map(|d| slot_of(d.stood)).collect();
        let mut held: Vec<DateTime<Utc>> = master
            .deleted_occurrence_starts
            .iter()
            .map(|&at| slot_of(at))
            .collect();
        made.sort();
        held.sort();
        let same = made.len() == held.len()
            && made
                .iter()
                .zip(&held)
                .all(|(a, b)| (*a - *b).abs() <= SLOT_TOLERANCE);
        if same {
            Ok(())
        } else {
            tracing::warn!(
                target: "adapter_ews::write",
                made = made.len(),
                held = held.len(),
                "the series' deleted occurrences are not the ones deleted",
            );
            Err(Failure::new("control", "after-control"))
        }
    }

    /// The one invitation (225): the invitees written again as they are,
    /// sent to all of them. Nothing else is written — a new start and end,
    /// zone or pattern would make Exchange drop the deletions (246-253).
    async fn send_invitation(
        &mut self,
        attendees_xml: &str,
        until: Instant,
    ) -> Result<(), Failure> {
        let set = format!(
            "            <t:SetItemField>\n              <t:FieldURI FieldURI=\"calendar:RequiredAttendees\"/>\n              <t:CalendarItem>\n{attendees_xml}              </t:CalendarItem>\n            </t:SetItemField>\n"
        );
        let body = crate::soap::update_calendar_item(&self.master, None, &set, "", true);
        // Not sent in time, or only ever answered busy: Exchange processed
        // nothing, and no invitation went out.
        let answer = self.send(&body, until, "invitation", false).await?;
        match write_outcome(answer) {
            Ok(()) => {
                self.invited = true;
                Ok(())
            }
            Err(err) => {
                // Turned down whole, never sent, or refused for the sign-in or
                // a gone item: nothing went out (264). Any other failure may
                // come after the mail left — a meeting update can save, send
                // and then fail, and a lost answer says nothing either way.
                self.invited = !(crate::turned_down_whole(&err)
                    || crate::names_sign_in_or_gone(&err)
                    || matches!(err, EwsError::Unreached(_)));
                Err(Failure::new("invitation", "invitation"))
            }
        }
    }

    /// Delete the new series again (203), inside the time kept for it, and
    /// say what stands.
    async fn take_back(&mut self, failure: Failure) -> EwsError {
        let failed_at = Instant::now();
        let confirmed = self.confirmed.unwrap_or(self.started);
        let until =
            (failed_at + self.budget.undo).min(confirmed + self.budget.settle + self.budget.undo);
        // The invitees hear of the take-back exactly when they may have heard
        // of the series (212, 264).
        let body = crate::soap::delete_calendar_item(&self.master, None, self.invited);
        let read = crate::soap::get_calendar_items_with_recurrence(&[(self.master.clone(), None)]);
        let mut undone = false;
        // The first delete and the first read that confirms it are sent
        // whatever the budget says (226); a busy answer is waited out inside
        // it (critique 6).
        let mut first_delete = true;
        let mut first_read = true;
        for _ in 0..3 {
            let Ok(answer) = self.send(&body, until, "take-back", first_delete).await else {
                break;
            };
            first_delete = false;
            match write_outcome(answer) {
                Ok(()) => {
                    undone = true;
                    break;
                }
                Err(EwsError::Soap { code, .. }) if bare_code(&code) == "ErrorItemNotFound" => {
                    undone = true;
                    break;
                }
                Err(_) => {
                    // The answer may be lost after the delete was made: the
                    // series says. Exchange gives a series moved to Deleted
                    // Items a new id, so the old one is not found.
                    let Ok(answer) = self.send(&read, until, "take-back", first_read).await else {
                        break;
                    };
                    first_read = false;
                    match answer.and_then(|xml| crate::soap::check_for_fault(&xml)) {
                        Err(EwsError::Soap { code, .. })
                            if bare_code(&code) == "ErrorItemNotFound" =>
                        {
                            undone = true;
                            break;
                        }
                        // Still there: delete it once more.
                        Ok(()) => continue,
                        Err(_) => break,
                    }
                }
            }
        }
        let detail = cal_core::step_detail(&failure.why, self.invited);
        tracing::warn!(
            target: "adapter_ews::write",
            asked = self.wanted.len(),
            past = self.past.len(),
            deleted = self.deleted.len(),
            skipped = self.skipped,
            step = failure.step,
            why = %failure.why,
            invited = self.invited,
            undo = if undone { "done" } else { "failed" },
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "exchange series taken back: a deletion was not kept",
        );
        EwsError::NotKept { undone, detail }
    }

    /// The create's answer was lost: nothing is taken back, so this is the
    /// line a "may stand" is read by. `why` in the grammar of the take-back's.
    fn log_lost(&self, why: &str) {
        tracing::warn!(
            target: "adapter_ews::write",
            step = "create",
            why,
            asked = self.wanted.len(),
            past = self.past.len(),
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "exchange series may stand: the answer to its creation was lost",
        );
    }

    fn log_success(&self) {
        let slots = self
            .wanted
            .iter()
            .map(|w| {
                let done = self.deleted.values().any(|d| d.asked == w.asked);
                format!(
                    "{} {}",
                    describe_slot(w),
                    if done { "deleted" } else { "skipped" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!(
            target: "adapter_ews::write",
            asked = self.wanted.len(),
            past = self.past.len(),
            deleted = self.deleted.len(),
            skipped = self.skipped,
            notify = self.invite,
            all_day = self.event.all_day,
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            %slots,
            "exchange series created; deletions removed one by one",
        );
    }
}

/// Where [`Run::find`] found an occurrence, or why there is none.
enum Found {
    At(u32, DateTime<Utc>),
    NotMade(&'static str),
    /// Deleted already in this run, for another slot as close.
    Already,
}

/// Where an occurrence stands against the slot [`Run::find`] looks for.
#[derive(Debug, Clone, Copy)]
enum Place {
    /// At the slot: this is it.
    Match(DateTime<Utc>),
    /// Deleted by this run already, for a slot as close.
    Already,
    Before(DateTime<Utc>),
    /// After the slot, or past the series' end (`None`).
    After(Option<DateTime<Utc>>),
}

/// What [`Run::find`] knows of the occurrences around a slot.
#[derive(Debug, Default)]
struct Bounds {
    /// The last index known to stand before the slot, and where.
    before: Option<(u32, DateTime<Utc>)>,
    /// The first index known to stand after it, and where (`None`: past the
    /// series' end).
    after: Option<(u32, Option<DateTime<Utc>>)>,
    /// Every occurrence read and where it stood, for the spacing.
    points: Vec<(u32, DateTime<Utc>)>,
    /// Every index answered, past the end included.
    read: std::collections::BTreeSet<u32>,
}

impl Bounds {
    fn note(&mut self, index: u32, place: Place) {
        self.read.insert(index);
        match place {
            Place::Before(stood) => {
                self.points.push((index, stood));
                if self.before.is_none_or(|(at, _)| index > at) {
                    self.before = Some((index, stood));
                }
            }
            Place::After(stood) => {
                if let Some(stood) = stood {
                    self.points.push((index, stood));
                }
                if self.after.is_none_or(|(at, _)| index < at) {
                    self.after = Some((index, stood));
                }
            }
            Place::Match(_) | Place::Already => {}
        }
    }

    /// The answer, where the bounds give one: next to each other with none
    /// at the slot, the rule makes none there.
    fn decided(&self) -> Result<Option<Found>, Failure> {
        let low = self.before.map_or(0, |(at, _)| at);
        let Some((high, stood)) = self.after else {
            return Ok(None);
        };
        if high <= low {
            // Answers that contradict each other: nothing can be trusted.
            return Err(Failure::new("probe", "no-occurrence"));
        }
        if high != low + 1 {
            return Ok(None);
        }
        Ok(Some(Found::NotMade(if low == 0 {
            "before-start"
        } else if stood.is_none() {
            "after-end"
        } else {
            "off-pattern"
        })))
    }

    /// The average time between occurrences around `index`, from the points
    /// read: between the two nearest on one side of it.
    fn spacing_near(&self, index: u32, upward: bool) -> Option<i64> {
        let mut side: Vec<&(u32, DateTime<Utc>)> = self
            .points
            .iter()
            .filter(|(at, _)| if upward { *at <= index } else { *at >= index })
            .collect();
        side.sort_by_key(|(at, _)| if upward { u32::MAX - *at } else { *at });
        let [(a, sa), (b, sb), ..] = side.as_slice() else {
            return None;
        };
        let steps = i64::from(a.abs_diff(*b));
        let seconds = (*sa - *sb).num_seconds().abs();
        (steps > 0 && seconds > 0).then(|| seconds / steps)
    }

    /// The index to read next: strictly between the bounds, guessed from the
    /// spacing of the occurrences read; `candidate` while nothing is known.
    fn next(&self, candidate: u32, slot: DateTime<Utc>) -> u32 {
        let low = self.before.map_or(0, |(at, _)| at);
        let high = self.after.map(|(at, _)| at);
        let clamp = |index: i64| -> u32 {
            let floor = i64::from(low) + 1;
            let ceiling = high.map_or(i64::from(u32::MAX), |h| i64::from(h) - 1);
            u32::try_from(index.clamp(floor, ceiling.max(floor))).unwrap_or(u32::MAX)
        };
        // The guess for this slot first, while it lies between the bounds and
        // no answer said anything about it yet.
        let candidate = candidate.max(1);
        if candidate > low && high.is_none_or(|h| candidate < h) && !self.read.contains(&candidate)
        {
            return candidate;
        }
        match (self.before, self.after) {
            (None, None) => candidate,
            // Only what comes before is known: on from the last one, by as
            // many occurrences as fit between it and the slot.
            (Some((at, stood)), None) => {
                let gap = self
                    .spacing_near(at, true)
                    .map_or(1, |spacing| ((slot - stood).num_seconds() / spacing).max(1));
                clamp(i64::from(at) + gap)
            }
            // Only what comes after is known: back from the first one.
            (None, Some((at, Some(stood)))) => {
                let gap = self
                    .spacing_near(at, false)
                    .map_or(1, |spacing| ((stood - slot).num_seconds() / spacing).max(1));
                clamp(i64::from(at) - gap)
            }
            // The end is past, nothing before known: halfway back.
            (None, Some((at, None))) => clamp(i64::from(at / 2)),
            // Both known: where the slot falls between them, by their times.
            (Some((lo, lo_at)), Some((hi, Some(hi_at)))) => {
                let span = (hi_at - lo_at).num_seconds().max(1);
                let into = (slot - lo_at).num_seconds().max(0);
                clamp(i64::from(lo) + (i64::from(hi - lo) * into / span).max(1))
            }
            // The end is past and the last one before known: halfway between
            // them. The spacing put the read past the end in the first place,
            // and from there it would only step back one at a time.
            (Some((lo, _)), Some((hi, None))) => clamp(i64::from(lo + (hi - lo) / 2)),
        }
    }
}

/// A slot as the log names it: an instant, or for an all-day series the day
/// the run read it as, on the device. No id ever: the log is the user's to
/// share.
fn describe_slot(wanted: &Wanted) -> String {
    match wanted.day {
        Some(day) => day.to_string(),
        None => wanted
            .asked
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}

#[cfg(test)]
mod tests {
    //! The flow against a scripted mailbox: a model of one series that
    //! answers each request as Exchange would, and faults a test injects by
    //! the kind of request and how many of that kind came before.
    use super::*;
    use cal_core::EventRecurrence;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    /// What one request does instead of being answered.
    #[derive(Debug, Clone)]
    enum Fault {
        /// Answer that the server is busy, with this back-off in ms.
        Busy(Option<u64>),
        /// The same, as a SOAP fault sent with HTTP 500.
        BusyAs500(Option<u64>),
        /// Make the change, then lose the answer.
        LostAfter,
        /// Never reach the server.
        Unreached,
        /// Answer a per-item SOAP error with this code.
        Soap(&'static str),
        /// Answer this HTTP status, without making the change.
        Http(u16),
        /// Answer only after this long.
        Slow(Duration),
        /// Answer success, but delete nothing.
        Pretend,
        /// Answer HTTP 200 with an empty body, without making the change: a
        /// body lost on the way.
        Empty,
        /// Make the change, then answer HTTP 200 with a body no XML reader
        /// takes: a proxy's page.
        Garbled,
        /// Delete this index instead of the one asked.
        Other(u32),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Create,
        Read,
        Probe,
        DeleteOccurrence,
        Invite,
        DeleteSeries,
    }

    fn kind_of(body: &str) -> Kind {
        if body.contains("<m:CreateItem") {
            Kind::Create
        } else if body.contains("<m:UpdateItem") {
            Kind::Invite
        } else if body.contains("<m:DeleteItem") && body.contains("OccurrenceItemId") {
            Kind::DeleteOccurrence
        } else if body.contains("<m:DeleteItem") {
            Kind::DeleteSeries
        } else if body.contains("OccurrenceItemId") {
            Kind::Probe
        } else {
            Kind::Read
        }
    }

    /// The series Exchange stores, as a GetItem shows it.
    #[derive(Debug, Clone)]
    struct Shape {
        /// Its occurrences' slots, InstanceIndex 1 first.
        slots: Vec<DateTime<Utc>>,
        length: chrono::Duration,
        all_day: bool,
        pattern: &'static str,
        start_date: &'static str,
        start_zone: &'static str,
        end_zone: &'static str,
    }

    /// Mondays at 09:00 UTC from 2 November 2026, six of them, in W. Europe.
    fn mondays() -> Shape {
        Shape {
            slots: weekly(at("2026-11-02T09:00:00Z"), 6),
            length: chrono::Duration::hours(1),
            all_day: false,
            pattern: "<t:WeeklyRecurrence><t:Interval>1</t:Interval><t:DaysOfWeek>Monday</t:DaysOfWeek><t:FirstDayOfWeek>Monday</t:FirstDayOfWeek></t:WeeklyRecurrence>",
            start_date: "2026-11-02+01:00",
            start_zone: "W. Europe Standard Time",
            end_zone: "W. Europe Standard Time",
        }
    }

    fn weekly(first: DateTime<Utc>, count: usize) -> Vec<DateTime<Utc>> {
        (0..count)
            .map(|i| first + chrono::Duration::weeks(i as i64))
            .collect()
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn zulu(at: DateTime<Utc>) -> String {
        at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    fn envelope(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            xmlns:m="http://schemas.microsoft.com/exchange/services/2006/messages"
            xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
  <s:Body>{inner}</s:Body>
</s:Envelope>"#
        )
    }

    fn item_error(response: &str, code: &str) -> String {
        envelope(&format!(
            r#"<m:{response}><m:ResponseMessages>
    <m:{response}Message ResponseClass="Error">
      <m:MessageText>{code}</m:MessageText>
      <m:ResponseCode>{code}</m:ResponseCode>
    </m:{response}Message>
  </m:ResponseMessages></m:{response}>"#
        ))
    }

    fn busy(back_off: Option<u64>) -> String {
        let detail = back_off
            .map(|ms| {
                format!(r#"<m:MessageXml><t:Value Name="BackOffMilliseconds">{ms}</t:Value></m:MessageXml>"#)
            })
            .unwrap_or_default();
        envelope(&format!(
            r#"<m:GetItemResponse><m:ResponseMessages>
    <m:GetItemResponseMessage ResponseClass="Error">
      <m:MessageText>The server cannot service this request right now.</m:MessageText>
      <m:ResponseCode>ErrorServerBusy</m:ResponseCode>{detail}
    </m:GetItemResponseMessage>
  </m:ResponseMessages></m:GetItemResponse>"#
        ))
    }

    fn busy_fault(back_off: Option<u64>) -> String {
        let detail = back_off
            .map(|ms| format!(r#"<t:Value Name="BackOffMilliseconds">{ms}</t:Value>"#))
            .unwrap_or_default();
        envelope(&format!(
            r#"<s:Fault><faultcode xmlns:a="http://schemas.microsoft.com/exchange/services/2006/types">a:ErrorServerBusy</faultcode>
      <faultstring>The server cannot service this request right now.</faultstring>
      <detail><e:ResponseCode xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">ErrorServerBusy</e:ResponseCode>
      <e:MessageXml xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">{detail}</e:MessageXml></detail></s:Fault>"#
        ))
    }

    fn success(response: &str, items: &str) -> String {
        envelope(&format!(
            r#"<m:{response}><m:ResponseMessages>
    <m:{response}Message ResponseClass="Success">
      <m:ResponseCode>NoError</m:ResponseCode>{items}
    </m:{response}Message>
  </m:ResponseMessages></m:{response}>"#
        ))
    }

    struct State {
        shape: Shape,
        exists: bool,
        /// Bumped by every change, as Exchange bumps the ChangeKey.
        version: u32,
        deleted: BTreeSet<u32>,
        faults: Vec<(Kind, usize, Fault)>,
        seen: Vec<(Kind, String)>,
    }

    struct Fake {
        state: Mutex<State>,
    }

    impl Fake {
        fn new(shape: Shape, faults: Vec<(Kind, usize, Fault)>) -> Self {
            Self {
                state: Mutex::new(State {
                    shape,
                    exists: false,
                    version: 0,
                    deleted: BTreeSet::new(),
                    faults,
                    seen: Vec::new(),
                }),
            }
        }

        fn kinds(&self) -> Vec<Kind> {
            self.state
                .lock()
                .unwrap()
                .seen
                .iter()
                .map(|(k, _)| *k)
                .collect()
        }

        fn bodies(&self, kind: Kind) -> Vec<String> {
            self.state
                .lock()
                .unwrap()
                .seen
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, b)| b.clone())
                .collect()
        }

        fn deleted(&self) -> Vec<u32> {
            self.state.lock().unwrap().deleted.iter().copied().collect()
        }

        fn exists(&self) -> bool {
            self.state.lock().unwrap().exists
        }

        fn master(state: &State) -> String {
            let shape = &state.shape;
            let start = shape.slots[0];
            let deleted: String = state
                .deleted
                .iter()
                .map(|i| {
                    format!(
                        "<t:DeletedOccurrence><t:Start>{}</t:Start></t:DeletedOccurrence>",
                        zulu(shape.slots[*i as usize - 1])
                    )
                })
                .collect();
            let deleted = if deleted.is_empty() {
                String::new()
            } else {
                format!("<t:DeletedOccurrences>{deleted}</t:DeletedOccurrences>")
            };
            success(
                "GetItemResponse",
                &format!(
                    r#"<m:Items><t:CalendarItem>
        <t:ItemId Id="NEW" ChangeKey="CK{version}"/>
        <t:Subject>Weekly</t:Subject>
        <t:Start>{start}</t:Start>
        <t:End>{end}</t:End>
        <t:IsAllDayEvent>{all_day}</t:IsAllDayEvent>
        <t:CalendarItemType>RecurringMaster</t:CalendarItemType>
        <t:Recurrence>{pattern}<t:NumberedRecurrence><t:StartDate>{start_date}</t:StartDate><t:NumberOfOccurrences>{count}</t:NumberOfOccurrences></t:NumberedRecurrence></t:Recurrence>{deleted}
        <t:StartTimeZone Id="{start_zone}"/>
        <t:EndTimeZone Id="{end_zone}"/>
      </t:CalendarItem></m:Items>"#,
                    version = state.version,
                    start = zulu(start),
                    end = zulu(start + shape.length),
                    all_day = shape.all_day,
                    pattern = shape.pattern,
                    start_date = shape.start_date,
                    count = shape.slots.len(),
                    start_zone = shape.start_zone,
                    end_zone = shape.end_zone,
                ),
            )
        }

        fn index_of(body: &str) -> u32 {
            let at = body.find("InstanceIndex=\"").unwrap() + "InstanceIndex=\"".len();
            body[at..].split('"').next().unwrap().parse().unwrap()
        }

        /// Make the change a request asks for, and the answer it gets.
        fn apply(
            state: &mut State,
            kind: Kind,
            body: &str,
            other: Option<u32>,
            pretend: bool,
        ) -> String {
            match kind {
                Kind::Create => {
                    state.exists = true;
                    state.version += 1;
                    success(
                        "CreateItemResponse",
                        &format!(
                            r#"<m:Items><t:CalendarItem><t:ItemId Id="NEW" ChangeKey="CK{}"/></t:CalendarItem></m:Items>"#,
                            state.version
                        ),
                    )
                }
                _ if !state.exists => item_error("GetItemResponse", "ErrorItemNotFound"),
                Kind::Read => Self::master(state),
                Kind::Probe => {
                    let index = Self::index_of(body);
                    if index as usize > state.shape.slots.len() {
                        item_error(
                            "GetItemResponse",
                            "ErrorCalendarOccurrenceIndexIsOutOfRecurrenceRange",
                        )
                    } else if state.deleted.contains(&index) {
                        item_error(
                            "GetItemResponse",
                            "ErrorCalendarOccurrenceIsDeletedFromRecurrence",
                        )
                    } else {
                        let start = state.shape.slots[index as usize - 1];
                        success(
                            "GetItemResponse",
                            &format!(
                                r#"<m:Items><t:CalendarItem>
        <t:ItemId Id="OCC{index}" ChangeKey="OCK"/>
        <t:Subject>Weekly</t:Subject>
        <t:Start>{}</t:Start>
        <t:End>{}</t:End>
        <t:CalendarItemType>Occurrence</t:CalendarItemType>
      </t:CalendarItem></m:Items>"#,
                                zulu(start),
                                zulu(start + state.shape.length),
                            ),
                        )
                    }
                }
                Kind::DeleteOccurrence => {
                    let index = other.unwrap_or_else(|| Self::index_of(body));
                    if !pretend {
                        state.deleted.insert(index);
                        state.version += 1;
                    }
                    success("DeleteItemResponse", "")
                }
                Kind::Invite => {
                    state.version += 1;
                    success(
                        "UpdateItemResponse",
                        &format!(
                            r#"<m:Items><t:CalendarItem><t:ItemId Id="NEW" ChangeKey="CK{}"/></t:CalendarItem></m:Items>"#,
                            state.version
                        ),
                    )
                }
                Kind::DeleteSeries => {
                    state.exists = false;
                    success("DeleteItemResponse", "")
                }
            }
        }
    }

    impl Mailbox for Fake {
        fn post(&self, body: String) -> impl Future<Output = EwsResult<String>> + Send {
            let (answer, delay) = {
                let mut state = self.state.lock().unwrap();
                let kind = kind_of(&body);
                let nth = state.seen.iter().filter(|(k, _)| *k == kind).count();
                state.seen.push((kind, body.clone()));
                let fault = state
                    .faults
                    .iter()
                    .find(|(k, n, _)| *k == kind && *n == nth)
                    .map(|(_, _, f)| f.clone());
                match fault {
                    None => (Ok(Fake::apply(&mut state, kind, &body, None, false)), None),
                    Some(Fault::Busy(ms)) => (Ok(busy(ms)), None),
                    Some(Fault::BusyAs500(ms)) => (
                        Err(EwsError::Http {
                            status: 500,
                            message: busy_fault(ms),
                        }),
                        None,
                    ),
                    Some(Fault::LostAfter) => {
                        Fake::apply(&mut state, kind, &body, None, false);
                        (Err(EwsError::Network("connection reset".into())), None)
                    }
                    Some(Fault::Unreached) => {
                        (Err(EwsError::Unreached("connection refused".into())), None)
                    }
                    Some(Fault::Soap(code)) => (Ok(item_error("GetItemResponse", code)), None),
                    Some(Fault::Http(status)) => (
                        Err(EwsError::Http {
                            status,
                            message: "server error".into(),
                        }),
                        None,
                    ),
                    Some(Fault::Slow(delay)) => (
                        Ok(Fake::apply(&mut state, kind, &body, None, false)),
                        Some(delay),
                    ),
                    Some(Fault::Empty) => (Ok(String::new()), None),
                    Some(Fault::Garbled) => {
                        Fake::apply(&mut state, kind, &body, None, false);
                        (Ok("<html><body>gateway</head></html>".to_string()), None)
                    }
                    Some(Fault::Pretend) => {
                        (Ok(Fake::apply(&mut state, kind, &body, None, true)), None)
                    }
                    Some(Fault::Other(index)) => (
                        Ok(Fake::apply(&mut state, kind, &body, Some(index), false)),
                        None,
                    ),
                }
            };
            async move {
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                answer
            }
        }
    }

    const BUDGET: SettleBudget = SettleBudget {
        settle: Duration::from_secs(5),
        undo: Duration::from_secs(5),
    };

    const PACING: Pacing = Pacing {
        first_wait: Duration::from_millis(1),
        max_wait: Duration::from_millis(5),
        proof_borrow: Duration::from_millis(500),
        undo_floor: Duration::from_millis(500),
    };

    /// 18 October 2026, before every slot of [`mondays`].
    fn today() -> DateTime<Utc> {
        at("2026-10-18T10:00:00Z")
    }

    fn series(deleted: Vec<DateTime<Utc>>, all_day: bool) -> NewEvent {
        NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Weekly".into(),
            description: None,
            location: None,
            start: if all_day {
                at("2026-11-02T00:00:00Z")
            } else {
                at("2026-11-02T09:00:00Z")
            },
            end: if all_day {
                at("2026-11-03T00:00:00Z")
            } else {
                at("2026-11-02T10:00:00Z")
            },
            all_day,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=6".into(),
                exceptions: deleted,
                tzid: if all_day {
                    None
                } else {
                    Some("Europe/Berlin".into())
                },
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    fn invited(mut event: NewEvent) -> NewEvent {
        event.attendees = vec!["Bob <bob@example.org>".into()];
        event.send_invitations = true;
        event
    }

    async fn run_on<D: TimeZone + Sync>(
        fake: &Fake,
        event: NewEvent,
        device: &D,
        now: DateTime<Utc>,
        budget: SettleBudget,
    ) -> EwsResult<Event> {
        create_keeping_deletions(fake, "FOLDER|FCK", event, None, device, now, budget, PACING).await
    }

    async fn run(fake: &Fake, event: NewEvent) -> EwsResult<Event> {
        run_on(fake, event, &Utc, today(), BUDGET).await
    }

    fn not_kept(result: EwsResult<Event>) -> (bool, String) {
        match result {
            Err(EwsError::NotKept { undone, detail }) => (undone, detail),
            other => panic!("expected NotKept, got {other:?}"),
        }
    }

    use Kind::*;

    #[tokio::test]
    async fn a_series_with_deletions_is_created_bare_then_each_slot_deleted() {
        let shape = mondays();
        let (second, fourth) = (shape.slots[1], shape.slots[3]);
        let fake = Fake::new(shape, Vec::new());
        let created = run(&fake, series(vec![fourth, second], false))
            .await
            .unwrap();
        assert_eq!(
            fake.kinds(),
            vec![
                Create,
                Read,
                Probe,
                DeleteOccurrence,
                Probe,
                DeleteOccurrence,
                Read
            ],
        );
        assert_eq!(fake.deleted(), vec![2, 4]);
        // Silent throughout, and no delete carries a key (225, live round 7).
        assert!(fake.bodies(Create)[0].contains(r#"SendMeetingInvitations="SendToNone""#));
        for body in fake.bodies(DeleteOccurrence) {
            assert!(
                body.contains(r#"SendMeetingCancellations="SendToNone""#),
                "{body}"
            );
            assert!(!body.contains("ChangeKey"), "{body}");
        }
        // What Exchange now holds, and the key the proof read.
        assert_eq!(created.recurrence.unwrap().exceptions, vec![second, fourth]);
        assert_eq!(created.etag.as_deref(), Some("CK3"));
    }

    #[tokio::test]
    async fn deletions_before_today_are_not_written_again_and_none_left_is_one_create() {
        let shape = mondays();
        let (first, second) = (shape.slots[0], shape.slots[1]);
        let fake = Fake::new(shape, Vec::new());
        // On 10 November the first two Mondays lie in the past (263).
        let created = run_on(
            &fake,
            invited(series(vec![first, second], false)),
            &Utc,
            at("2026-11-10T08:00:00Z"),
            BUDGET,
        )
        .await
        .unwrap();
        assert_eq!(fake.kinds(), vec![Create]);
        // Nothing to complete: the invitation goes with the create, as for
        // every series.
        assert!(fake.bodies(Create)[0].contains(r#"SendMeetingInvitations="SendToAllAndSaveCopy""#));
        assert!(created.recurrence.unwrap().exceptions.is_empty());
    }

    #[tokio::test]
    async fn invitations_go_out_once_after_the_proof_and_only_where_there_are_invitees() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape.clone(), Vec::new());
        let created = run(&fake, invited(series(vec![second], false)))
            .await
            .unwrap();
        assert_eq!(
            fake.kinds(),
            vec![Create, Read, Probe, DeleteOccurrence, Read, Invite, Read],
        );
        assert!(fake.bodies(Create)[0].contains("SendToNone"));
        assert!(fake.bodies(DeleteOccurrence)[0].contains("SendToNone"));
        let invite = &fake.bodies(Invite)[0];
        assert!(invite.contains(r#"SendMeetingInvitationsOrCancellations="SendToAllAndSaveCopy""#));
        assert!(invite.contains("calendar:RequiredAttendees"), "{invite}");
        assert!(invite.contains("bob@example.org"), "{invite}");
        // Nothing that would make Exchange drop the deletions (246-253).
        for field in [
            "calendar:Start",
            "calendar:End",
            "calendar:Recurrence",
            "TimeZone",
        ] {
            assert!(!invite.contains(field), "{field}: {invite}");
        }
        assert_eq!(created.etag.as_deref(), Some("CK3"));

        // Asked to invite, but nobody to invite: silent, no update.
        let fake = Fake::new(shape, Vec::new());
        let mut nobody = series(vec![second], false);
        nobody.send_invitations = true;
        nobody.attendees = vec!["".into()];
        run(&fake, nobody).await.unwrap();
        assert!(!fake.kinds().contains(&Invite));
    }

    #[tokio::test]
    async fn the_slot_decides_not_the_index() {
        // Mondays and Wednesdays at 00:30 in Berlin: Sunday and Tuesday 23:30
        // UTC. The rule's UTC expansion puts every one a day later, a whole
        // day from every real occurrence — so a skip judged by it would skip
        // them all (266 allows a skip only where Exchange makes none), and the
        // index it guesses for the Tuesday is the Sunday's. The occurrences
        // Exchange answers say which it is.
        let first = at("2026-11-01T23:30:00Z");
        let shape = Shape {
            slots: [0i64, 2, 7, 9, 14, 16]
                .iter()
                .map(|days| first + chrono::Duration::days(*days))
                .collect(),
            pattern: "<t:WeeklyRecurrence><t:Interval>1</t:Interval><t:DaysOfWeek>Monday Wednesday</t:DaysOfWeek><t:FirstDayOfWeek>Monday</t:FirstDayOfWeek></t:WeeklyRecurrence>",
            ..mondays()
        };
        let (tuesday, sunday) = (shape.slots[1], shape.slots[4]);
        let fake = Fake::new(shape, Vec::new());
        let mut event = series(vec![tuesday, sunday], false);
        event.start = first;
        event.end = first + chrono::Duration::hours(1);
        event.recurrence.as_mut().unwrap().rrule = "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6".into();
        run(&fake, event).await.unwrap();
        assert_eq!(fake.deleted(), vec![2, 5]);
    }

    #[tokio::test]
    async fn an_occurrence_read_for_a_stray_slot_is_still_found_for_its_own() {
        // Mondays and Wednesdays at 00:30 in Berlin. A stray Tuesday 00:30 is
        // bracketed by the Sunday and Tuesday UTC occurrences, which the walk
        // reads and keeps; the real Wednesday's walk then meets the Tuesday
        // UTC occurrence as its neighbour. It is the occurrence, not a bound.
        let first = at("2026-11-01T23:30:00Z");
        let shape = Shape {
            slots: [0i64, 2, 7, 9, 14, 16]
                .iter()
                .map(|days| first + chrono::Duration::days(*days))
                .collect(),
            pattern: "<t:WeeklyRecurrence><t:Interval>1</t:Interval><t:DaysOfWeek>Monday Wednesday</t:DaysOfWeek><t:FirstDayOfWeek>Monday</t:FirstDayOfWeek></t:WeeklyRecurrence>",
            ..mondays()
        };
        let stray = at("2026-11-02T23:30:00Z");
        let wednesday = shape.slots[1];
        let fake = Fake::new(shape, Vec::new());
        let mut event = series(vec![stray, wednesday], false);
        event.start = first;
        event.end = first + chrono::Duration::hours(1);
        event.recurrence.as_mut().unwrap().rrule = "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6".into();
        let created = run(&fake, event).await.unwrap();
        assert_eq!(fake.deleted(), vec![2]);
        assert_eq!(created.recurrence.unwrap().exceptions, vec![wednesday]);
    }

    #[tokio::test]
    async fn a_series_on_the_31st_is_walked_past_its_short_months() {
        // Exchange puts a 31st series' occurrence on the last day of each
        // shorter month; the RFC expansion has none there, so its guess for
        // December is five occurrences short.
        let days = [
            "2027-01-31",
            "2027-02-28",
            "2027-03-31",
            "2027-04-30",
            "2027-05-31",
            "2027-06-30",
            "2027-07-31",
            "2027-08-31",
            "2027-09-30",
            "2027-10-31",
            "2027-11-30",
            "2027-12-31",
        ];
        let shape = Shape {
            slots: days.iter().map(|d| at(&format!("{d}T09:00:00Z"))).collect(),
            pattern: "<t:AbsoluteMonthlyRecurrence><t:Interval>1</t:Interval><t:DayOfMonth>31</t:DayOfMonth></t:AbsoluteMonthlyRecurrence>",
            start_date: "2027-01-31+01:00",
            ..mondays()
        };
        let december = shape.slots[11];
        let fake = Fake::new(shape, Vec::new());
        let mut event = series(vec![december], false);
        event.start = at("2027-01-31T09:00:00Z");
        event.end = at("2027-01-31T10:00:00Z");
        event.recurrence.as_mut().unwrap().rrule = "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=12".into();
        run(&fake, event).await.unwrap();
        assert_eq!(fake.deleted(), vec![12]);
    }

    #[tokio::test]
    async fn a_31st_series_years_old_is_found_by_its_spacing() {
        // Seven years of a series on the 31st: the RFC expansion's guess for
        // its last December is about 35 occurrences short.
        let mut slots = Vec::new();
        for year in 2020..2027 {
            for month in 1..=12u32 {
                let last = (28..=31)
                    .rev()
                    .find(|d| chrono::NaiveDate::from_ymd_opt(year, month, *d).is_some())
                    .unwrap();
                slots.push(at(&format!("{year}-{month:02}-{last:02}T09:00:00Z")));
            }
        }
        let december = *slots.last().unwrap();
        let count = slots.len();
        let shape = Shape {
            slots,
            pattern: "<t:AbsoluteMonthlyRecurrence><t:Interval>1</t:Interval><t:DayOfMonth>31</t:DayOfMonth></t:AbsoluteMonthlyRecurrence>",
            start_date: "2020-01-31+01:00",
            ..mondays()
        };
        let fake = Fake::new(shape, Vec::new());
        let mut event = series(vec![december], false);
        event.start = at("2020-01-31T09:00:00Z");
        event.end = at("2020-01-31T10:00:00Z");
        event.recurrence.as_mut().unwrap().rrule =
            format!("FREQ=MONTHLY;BYMONTHDAY=31;COUNT={count}");
        run(&fake, event).await.unwrap();
        assert_eq!(fake.deleted(), vec![count as u32]);
        // Found by its spacing, not one occurrence at a time.
        assert!(
            fake.bodies(Probe).len() < 10,
            "{} probes",
            fake.bodies(Probe).len()
        );
    }

    #[tokio::test]
    async fn an_unreadable_answer_to_a_delete_is_decided_by_the_occurrence() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape.clone(), vec![(DeleteOccurrence, 0, Fault::Garbled)]);
        run(&fake, series(vec![second], false)).await.unwrap();
        // Read again, found deleted: no second delete.
        assert_eq!(fake.bodies(Probe).len(), 2);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 1);
        assert_eq!(fake.deleted(), vec![2]);

        // An empty answer before the delete was made: read again, it stands,
        // and the delete goes once more.
        let fake = Fake::new(shape, vec![(DeleteOccurrence, 0, Fault::Empty)]);
        run(&fake, series(vec![second], false)).await.unwrap();
        assert_eq!(fake.bodies(Probe).len(), 2);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 2);
        assert_eq!(fake.deleted(), vec![2]);
    }

    #[tokio::test]
    async fn a_slot_past_the_stored_end_is_skipped_in_a_few_reads() {
        // Monthly on the 31st, 120 times: Exchange's 120 occurrences end in
        // December 2036, the RFC expansion's 31sts run to 2044. A deletion
        // on its 110th, August 2042, lies past Exchange's end.
        let mut slots = Vec::new();
        for year in 2027..2037 {
            for month in 1..=12u32 {
                let last = (28..=31)
                    .rev()
                    .find(|d| chrono::NaiveDate::from_ymd_opt(year, month, *d).is_some())
                    .unwrap();
                slots.push(at(&format!("{year}-{month:02}-{last:02}T09:00:00Z")));
            }
        }
        let march = slots[2];
        let shape = Shape {
            slots,
            pattern: "<t:AbsoluteMonthlyRecurrence><t:Interval>1</t:Interval><t:DayOfMonth>31</t:DayOfMonth></t:AbsoluteMonthlyRecurrence>",
            start_date: "2027-01-31+01:00",
            ..mondays()
        };
        let fake = Fake::new(shape, Vec::new());
        let far = at("2042-08-31T09:00:00Z");
        let mut event = series(vec![march, far], false);
        event.start = at("2027-01-31T09:00:00Z");
        event.end = at("2027-01-31T10:00:00Z");
        event.recurrence.as_mut().unwrap().rrule = "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=120".into();
        let created = run(&fake, event).await.unwrap();
        assert_eq!(fake.deleted(), vec![3]);
        assert_eq!(created.recurrence.unwrap().exceptions, vec![march]);
        assert!(
            fake.bodies(Probe).len() < 16,
            "{} probes",
            fake.bodies(Probe).len()
        );
    }

    #[tokio::test]
    async fn a_slot_the_stored_rule_makes_no_occurrence_at_is_skipped() {
        let shape = mondays();
        let second = shape.slots[1];
        let before = at("2026-10-26T09:00:00Z");
        let tuesday = at("2026-11-10T09:00:00Z");
        let after = at("2026-12-14T09:00:00Z");
        let fake = Fake::new(shape, Vec::new());
        let created = run(&fake, series(vec![before, second, tuesday, after], false))
            .await
            .unwrap();
        // Only the Monday was deleted; the others cost a probe or nothing.
        assert_eq!(fake.deleted(), vec![2]);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 1);
        assert_eq!(created.recurrence.unwrap().exceptions, vec![second]);
        assert!(fake.exists());
    }

    #[tokio::test]
    async fn an_occurrence_that_cannot_be_read_or_was_deleted_by_another_takes_everything_back() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape.clone(),
            vec![(Probe, 0, Fault::Soap("ErrorAccessDenied"))],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(undone);
        assert_eq!(detail, "unreadable");
        assert!(fake.bodies(DeleteOccurrence).is_empty());
        assert_eq!(fake.bodies(DeleteSeries).len(), 1);
        assert!(!fake.exists());

        // An occurrence a fresh series shows deleted, which this run did not
        // delete: something else is writing to it.
        let fake = Fake::new(shape, Vec::new());
        fake.state.lock().unwrap().deleted.insert(2);
        let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(undone);
        assert_eq!(detail, "control");
    }

    #[tokio::test]
    async fn the_proof_catches_a_delete_that_was_not_made_or_hit_another_date() {
        let shape = mondays();
        let second = shape.slots[1];
        for fault in [Fault::Pretend, Fault::Other(3)] {
            let fake = Fake::new(shape.clone(), vec![(DeleteOccurrence, 0, fault.clone())]);
            let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
            assert!(undone, "{fault:?}");
            assert_eq!(detail, "after-control", "{fault:?}");
            assert!(!fake.exists(), "{fault:?}");
        }
    }

    #[tokio::test]
    async fn a_busy_server_is_waited_out_in_all_its_shapes() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape,
            vec![
                (Probe, 0, Fault::Busy(Some(2))),
                (DeleteOccurrence, 0, Fault::BusyAs500(None)),
                (Read, 1, Fault::Busy(None)),
            ],
        );
        run(&fake, series(vec![second], false)).await.unwrap();
        assert_eq!(fake.deleted(), vec![2]);
        assert_eq!(fake.bodies(Probe).len(), 2);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 2);
    }

    /// HTTP 429 is a busy server too (decision 270): the step waits it out
    /// as it waits out `ErrorServerBusy`, and the run completes.
    #[tokio::test]
    async fn a_429_is_waited_out_as_a_busy_server() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape, vec![(DeleteOccurrence, 0, Fault::Http(429))]);
        run(&fake, series(vec![second], false)).await.unwrap();
        assert_eq!(fake.deleted(), vec![2]);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 2);
        // Sent again after the wait: never read as a lost answer and probed.
        assert_eq!(fake.bodies(Probe).len(), 1);
    }

    #[tokio::test]
    async fn a_back_off_beyond_the_budget_takes_back_at_once_waiting_out_its_own() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape,
            vec![
                (Probe, 0, Fault::Busy(Some(60_000))),
                // The take-back meets the same throttle, and waits it out.
                (DeleteSeries, 0, Fault::Busy(Some(5))),
            ],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(undone);
        // The provider was busy, not the run slow (276).
        assert_eq!(detail, cal_core::PROVIDER_BUSY);
        assert_eq!(fake.bodies(DeleteSeries).len(), 2);
        assert!(!fake.exists());
    }

    #[tokio::test]
    async fn a_lost_delete_answer_is_decided_by_the_occurrence() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape, vec![(DeleteOccurrence, 0, Fault::LostAfter)]);
        run(&fake, series(vec![second], false)).await.unwrap();
        // The occurrence, read again, said it was deleted: no second delete.
        assert_eq!(fake.bodies(Probe).len(), 2);
        assert_eq!(fake.bodies(DeleteOccurrence).len(), 1);
        assert_eq!(fake.deleted(), vec![2]);
    }

    #[tokio::test]
    async fn the_create_is_never_sent_twice_and_a_lost_answer_may_stand() {
        let shape = mondays();
        let second = shape.slots[1];
        for fault in [
            Fault::LostAfter,
            Fault::Http(502),
            Fault::Soap("ErrorInternalServerError"),
            Fault::Garbled,
        ] {
            let fake = Fake::new(shape.clone(), vec![(Create, 0, fault.clone())]);
            let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
            assert!(!undone, "{fault:?}");
            assert_eq!(detail, "create-answer-lost", "{fault:?}");
            assert_eq!(fake.kinds(), vec![Create], "{fault:?}");
        }
        // Never sent, or turned down before saving: that error, nothing stands.
        for fault in [
            Fault::Unreached,
            Fault::Http(429),
            Fault::Soap("ErrorQuotaExceeded"),
        ] {
            let fake = Fake::new(shape.clone(), vec![(Create, 0, fault.clone())]);
            let result = run(&fake, series(vec![second], false)).await;
            assert!(
                matches!(
                    result,
                    Err(EwsError::Unreached(_) | EwsError::Http { .. } | EwsError::Soap { .. })
                ),
                "{fault:?}: {result:?}"
            );
            assert_eq!(fake.kinds(), vec![Create], "{fault:?}");
        }
    }

    #[tokio::test]
    async fn an_empty_answer_is_no_proof_a_series_was_deleted() {
        // The take-back's delete is answered 200 with nothing in it: the
        // series is read, still stands, and is deleted once more.
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape,
            vec![
                (Probe, 0, Fault::Soap("ErrorAccessDenied")),
                (DeleteSeries, 0, Fault::Empty),
            ],
        );
        let (undone, _) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(undone);
        assert_eq!(fake.bodies(DeleteSeries).len(), 2);
        assert!(!fake.exists());
    }

    #[tokio::test]
    async fn an_invitation_that_never_left_takes_back_silently() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape, vec![(Invite, 0, Fault::Unreached)]);
        let (undone, detail) = not_kept(run(&fake, invited(series(vec![second], false))).await);
        assert!(undone);
        assert_eq!(detail, "invitation");
        assert!(fake.bodies(DeleteSeries)[0].contains(r#"SendMeetingCancellations="SendToNone""#));
    }

    #[tokio::test]
    async fn no_invitation_goes_out_once_the_steps_time_is_spent() {
        // The proof's read may borrow time past the budget; the invitation,
        // which mails the invitees, may not.
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape,
            vec![(Read, 1, Fault::Slow(Duration::from_millis(300)))],
        );
        let budget = SettleBudget {
            settle: Duration::from_millis(200),
            undo: Duration::from_secs(2),
        };
        let (undone, detail) = not_kept(
            run_on(
                &fake,
                invited(series(vec![second], false)),
                &Utc,
                today(),
                budget,
            )
            .await,
        );
        assert!(undone);
        assert_eq!(detail, cal_core::OUT_OF_TIME);
        assert!(!fake.kinds().contains(&Invite));
        assert!(fake.bodies(DeleteSeries)[0].contains(r#"SendMeetingCancellations="SendToNone""#));
    }

    #[tokio::test]
    async fn the_take_back_decides_what_stands() {
        let shape = mondays();
        let second = shape.slots[1];
        // Its answer lost after it was made: the series is gone.
        let fake = Fake::new(
            shape.clone(),
            vec![
                (Probe, 0, Fault::Soap("ErrorAccessDenied")),
                (DeleteSeries, 0, Fault::LostAfter),
            ],
        );
        let (undone, _) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(undone);
        // Refused throughout: it may stand.
        let fake = Fake::new(
            shape,
            vec![
                (Probe, 0, Fault::Soap("ErrorAccessDenied")),
                (DeleteSeries, 0, Fault::Http(500)),
                (Read, 1, Fault::Http(500)),
            ],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![second], false)).await);
        assert!(!undone);
        assert_eq!(detail, "unreadable");
        assert!(fake.exists());
    }

    #[tokio::test]
    async fn the_clock_counts_the_time_requests_take() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(shape, vec![(Probe, 0, Fault::Slow(Duration::from_secs(5)))]);
        let budget = SettleBudget {
            settle: Duration::from_millis(150),
            undo: Duration::from_secs(2),
        };
        let started = std::time::Instant::now();
        let (undone, detail) =
            not_kept(run_on(&fake, series(vec![second], false), &Utc, today(), budget).await);
        assert!(undone);
        // The probe was cut off at the budget, read as unanswered, long before
        // its answer would have come.
        assert_eq!(detail, "unreadable");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_all_day_series_is_matched_by_its_day_east_and_west() {
        // Aperio writes an all-day series without a zone: Exchange stores UTC
        // midnights (ews.md). The views send the device's midnights.
        let shape = Shape {
            slots: weekly(at("2026-11-02T00:00:00Z"), 6),
            length: chrono::Duration::days(1),
            all_day: true,
            start_zone: "Greenwich Standard Time",
            end_zone: "tzone://Microsoft/Utc",
            ..mondays()
        };
        for hours in [-11, -8, -5, 1, 9, 12, 14] {
            let device = chrono::FixedOffset::east_opt(hours * 3600).unwrap();
            let midnight = |day: &str| {
                chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(device)
                    .unwrap()
                    .with_timezone(&Utc)
            };
            let fake = Fake::new(shape.clone(), Vec::new());
            let deleted = vec![midnight("2026-11-09"), at("2026-11-23T00:00:00Z")];
            run_on(&fake, series(deleted, true), &device, today(), BUDGET)
                .await
                .unwrap_or_else(|err| panic!("UTC{hours:+}: {err}"));
            assert_eq!(fake.deleted(), vec![2, 4], "UTC{hours:+}");
        }
    }

    #[tokio::test]
    async fn an_invitation_that_may_have_gone_out_is_cancelled_with_the_take_back() {
        let shape = mondays();
        let second = shape.slots[1];
        // Lost: it may have gone out. The take-back cancels.
        let fake = Fake::new(shape.clone(), vec![(Invite, 0, Fault::Http(503))]);
        let (undone, detail) = not_kept(run(&fake, invited(series(vec![second], false))).await);
        assert!(undone);
        assert_eq!(detail, "invited invitation");
        assert!(fake.bodies(DeleteSeries)[0]
            .contains(r#"SendMeetingCancellations="SendToAllAndSaveCopy""#));
        // Turned down whole: nothing went out, the take-back is silent.
        let fake = Fake::new(
            shape,
            vec![(Invite, 0, Fault::Soap("ErrorSchemaValidation"))],
        );
        let (undone, detail) = not_kept(run(&fake, invited(series(vec![second], false))).await);
        assert!(undone);
        assert_eq!(detail, "invitation");
        assert!(fake.bodies(DeleteSeries)[0].contains(r#"SendMeetingCancellations="SendToNone""#));
    }

    #[tokio::test]
    async fn a_delete_refused_for_its_key_is_sent_once_more_with_the_series_key() {
        let shape = mondays();
        let second = shape.slots[1];
        let fake = Fake::new(
            shape,
            vec![(
                DeleteOccurrence,
                0,
                Fault::Soap("ErrorChangeKeyRequiredForWriteOperations"),
            )],
        );
        run(&fake, series(vec![second], false)).await.unwrap();
        let deletes = fake.bodies(DeleteOccurrence);
        assert_eq!(deletes.len(), 2);
        assert!(!deletes[0].contains("ChangeKey"));
        assert!(deletes[1].contains(r#"ChangeKey="CK1""#), "{}", deletes[1]);
    }

    #[test]
    fn answers_are_read_as_exchange_gives_them() {
        assert_eq!(
            probe_answer(&item_error(
                "GetItemResponse",
                "ErrorCalendarOccurrenceIsDeletedFromRecurrence"
            )),
            Answer::Deleted
        );
        assert_eq!(
            probe_answer(&item_error(
                "GetItemResponse",
                "ErrorCalendarOccurrenceIndexIsOutOfRecurrenceRange"
            )),
            Answer::PastEnd
        );
        assert_eq!(
            probe_answer(&item_error("GetItemResponse", "ErrorAccessDenied")),
            Answer::Unreadable
        );
        assert_eq!(
            probe_answer(&busy(Some(1500))),
            Answer::Busy(Some(Duration::from_millis(1500)))
        );
        assert_eq!(busy_of(&Ok(busy(None))), Some(None));
        assert_eq!(
            busy_of(&Err(EwsError::Http {
                status: 500,
                message: busy_fault(Some(250)),
            })),
            Some(Some(Duration::from_millis(250)))
        );
        assert_eq!(
            busy_of(&Err(EwsError::Soap {
                code: "a:ErrorServerBusy".into(),
                message: String::new(),
            })),
            Some(None)
        );
        assert_eq!(busy_of(&Err(EwsError::Network("reset".into()))), None);
        // HTTP 429 names no back-off.
        assert_eq!(
            busy_of(&Err(EwsError::Http {
                status: 429,
                message: String::new(),
            })),
            Some(None)
        );
        // A subject that names the code is no busy answer.
        let named = success(
            "GetItemResponse",
            "<m:Items><t:CalendarItem><t:ItemId Id=\"NEW\"/><t:Subject>ErrorServerBusy BackOffMilliseconds 60000</t:Subject></t:CalendarItem></m:Items>",
        );
        assert_eq!(busy_of(&Ok(named.clone())), None);
        assert!(matches!(
            probe_answer(&named),
            Answer::Unreadable | Answer::At(_)
        ));
    }

    #[test]
    fn a_slot_is_logged_as_an_instant_or_a_day_never_with_an_id() {
        let timed = at("2026-11-09T09:00:00Z");
        assert_eq!(
            describe_slot(&Wanted {
                asked: timed,
                slot: timed,
                day: None,
            }),
            "2026-11-09T09:00:00Z"
        );
        // Pago Pago's midnight of the 9th, 11:00 UTC: the day the run reads.
        let asked = at("2026-11-09T11:00:00Z");
        let device = chrono::FixedOffset::east_opt(-11 * 3600).unwrap();
        let day = cal_core::all_day_deletion_day(asked, &device);
        assert_eq!(
            describe_slot(&Wanted {
                asked,
                slot: asked,
                day: Some(day),
            }),
            "2026-11-09"
        );
    }
}
