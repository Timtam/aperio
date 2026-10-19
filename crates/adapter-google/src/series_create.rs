//! A new series that keeps its deleted occurrences (decisions 202-213,
//! 263-269).
//!
//! Google takes deletion lines with a new series, but whether it heeds them
//! was never measured (137); its own deletions are cancelled occurrences. So
//! the adapter creates the series without a deletion line, under an id of its
//! own (268), lists the occurrences Google made, cancels each deleted one by
//! its slot, and checks the listing again. The invitees hear of each step as
//! the notify switch says (204, 267): the invitation with the series, a
//! cancellation with each occurrence. Where a step fails it deletes the new
//! series again (203): nothing stands and the old series is untouched. Where
//! even that fails, it says the series may stand (210).
//!
//! What the steps may take, which deletions are written again and how the
//! outcome is told are cal-core's ([`cal_core::SERIES_WITH_DELETIONS`],
//! [`cal_core::deletions_from_today`], [`cal_core::step_detail`]); the clock,
//! the pacing and the requests are the adapter's.
//!
//! Google's own listing of the new series says where its rule makes an
//! occurrence (269): a deleted slot with no occurrence there, after a second
//! listing, is skipped and logged (207, 266). An occurrence counts as the one
//! at a slot when it starts there, or at most an hour off — what an outdated
//! zone table can put between Aperio's slot and Google's (tzdata 2025b) — and
//! never past half the way to its neighbours.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use cal_core::{Event, NewEvent, SettleBudget};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use reqwest::Method;
use tokio::time::Instant;

use crate::api::{error_reason, urlencoding, Answer, ApiState};
use crate::error::{GoogleError, GoogleResult};
use crate::mapping::{
    bare_series_body, map_event, written_attendees, EventEntry, EventListResponse,
};

/// How the adapter sends its requests. [`ApiState`] sends them to Google; the
/// tests send them to a series they script. Any status comes back as an
/// [`Answer`]; only a request that failed on its way is an error.
pub(crate) trait Calendar {
    fn send(
        &self,
        method: Method,
        path: String,
        body: Option<serde_json::Value>,
    ) -> impl Future<Output = GoogleResult<Answer>> + Send;
}

impl Calendar for ApiState {
    async fn send(
        &self,
        method: Method,
        path: String,
        body: Option<serde_json::Value>,
    ) -> GoogleResult<Answer> {
        self.send_raw(method, &path, body.as_ref()).await
    }
}

/// How the adapter paces the steps on Google's clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pacing {
    /// The first wait after an answer that asks to slow down without saying
    /// how long, doubled for each one after it…
    pub first_wait: Duration,
    /// …up to this.
    pub max_wait: Duration,
    /// What the listing that checks the result may take past the budget's
    /// end, out of the time kept to take the series back: a read, no write;
    /// it must not throw finished work away.
    pub proof_borrow: Duration,
    /// The least time the first request of a take-back, and the read that
    /// confirms it, are given, whatever is left (226).
    pub undo_floor: Duration,
}

impl Pacing {
    /// Google's own advice for a rate limit: 1 s, 2 s, 4 s.
    pub(crate) const GOOGLE: Self = Self {
        first_wait: Duration::from_secs(1),
        max_wait: Duration::from_secs(4),
        proof_borrow: Duration::from_secs(5),
        undo_floor: Duration::from_secs(5),
    };
}

/// How far from a deleted slot an occurrence may start and still be the one
/// it names (269): what an outdated zone table puts between the two.
const MATCH_TOLERANCE: chrono::Duration = chrono::Duration::hours(1);

/// How many times a create Google asked to slow down is sent again, after
/// waits of [`Pacing::first_wait`] doubling: the create is not on the clock
/// (226), and Google answering so means it made nothing.
const CREATE_BUSY_TRIES: u32 = 3;

/// What Google's reasons for asking to slow down are called; a quota used up
/// for the day is no such answer — waiting seconds does not help it.
const RATE_LIMIT_REASONS: &[&str] = &["rateLimitExceeded", "userRateLimitExceeded"];

/// Create `event` — a series with deleted occurrences — as Google can: bare,
/// under `id`, then each deletion from today on cancelled. See the module
/// documentation. `device`, `now`, `budget`, `pacing` and `id` are seams for
/// the tests; [`crate::api::create_event`] passes the device's clock,
/// [`cal_core::SERIES_WITH_DELETIONS`], [`Pacing::GOOGLE`] and a fresh id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_keeping_deletions<C, D>(
    cal: &C,
    calendar_id: &str,
    event: NewEvent,
    device: &D,
    now: DateTime<Utc>,
    budget: SettleBudget,
    pacing: Pacing,
    id: String,
) -> GoogleResult<Event>
where
    C: Calendar + Sync,
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
        .map(|&asked| Wanted {
            asked,
            slot: if event.all_day {
                Slot::Day(cal_core::all_day_deletion_day(asked, device))
            } else {
                Slot::At(asked)
            },
        })
        .collect();
    wanted.sort_by_key(|w| w.slot);
    wanted.dedup_by_key(|w| w.slot);
    let invite = event.send_invitations && written_attendees(&event);

    let mut run = Run::new(
        cal,
        calendar_id,
        &event,
        &wanted,
        parted.past.len(),
        invite,
        budget,
        pacing,
    );
    if wanted.is_empty() {
        // Every deletion lies in the past (263): the series is created as
        // every series is, invitations with it, and shows them again.
        let created = run.create_plain().await?;
        tracing::info!(
            target: "adapter_google::write",
            past = parted.past.len(),
            "google series created; its deleted occurrences all lie in the past and show again"
        );
        return Ok(created);
    }
    let created = run.create(id).await?;
    match run.complete().await {
        Ok(()) => {
            run.log_success();
            let mut created = created;
            let mut written: Vec<DateTime<Utc>> =
                run.cancelled.iter().map(|done| done.asked).collect();
            written.sort();
            if let Some(recurrence) = created.recurrence.as_mut() {
                recurrence.exceptions = written;
            }
            Ok(created)
        }
        Err(failure) => Err(run.take_back(failure).await),
    }
}

/// Where an occurrence stands: an instant for a timed series, a day for an
/// all-day one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Slot {
    At(DateTime<Utc>),
    Day(NaiveDate),
}

/// A deletion the series is to keep: as the request spelled it, and the slot
/// it is compared by.
#[derive(Debug, Clone, Copy)]
struct Wanted {
    asked: DateTime<Utc>,
    slot: Slot,
}

/// An occurrence of the new series as Google listed it.
#[derive(Debug, Clone)]
struct Occurrence {
    id: String,
    slot: Slot,
    cancelled: bool,
}

/// An occurrence this run cancelled.
#[derive(Debug, Clone)]
struct Cancelled {
    asked: DateTime<Utc>,
    id: String,
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

/// One create with deletions, from its first request to its outcome.
struct Run<'a, C> {
    cal: &'a C,
    calendar_id: &'a str,
    /// The calendar's id as a path segment.
    calendar: String,
    event: &'a NewEvent,
    wanted: &'a [Wanted],
    past: usize,
    invite: bool,
    budget: SettleBudget,
    pacing: Pacing,
    /// The new series' id, once Google confirmed it.
    series: String,
    /// When Google confirmed the create: the budget starts here (226).
    confirmed: Option<Instant>,
    started: Instant,
    cancelled: Vec<Cancelled>,
    skipped: usize,
    requests: usize,
    retries: usize,
    waited: Duration,
    /// The part of `waited` spent on answers that asked to slow down (276).
    busy_waited: Duration,
    slowest: Duration,
    /// Whether the invitees may have heard of the series: from a create sent
    /// with them on (204, 267).
    invited: bool,
}

impl<'a, C: Calendar + Sync> Run<'a, C> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cal: &'a C,
        calendar_id: &'a str,
        event: &'a NewEvent,
        wanted: &'a [Wanted],
        past: usize,
        invite: bool,
        budget: SettleBudget,
        pacing: Pacing,
    ) -> Self {
        Self {
            cal,
            calendar_id,
            calendar: urlencoding(calendar_id),
            event,
            wanted,
            past,
            invite,
            budget,
            pacing,
            series: String::new(),
            confirmed: None,
            started: Instant::now(),
            cancelled: Vec::new(),
            skipped: 0,
            requests: 0,
            retries: 0,
            waited: Duration::ZERO,
            busy_waited: Duration::ZERO,
            slowest: Duration::ZERO,
            invited: false,
        }
    }

    fn send_updates(&self, notify: bool) -> &'static str {
        if notify {
            "all"
        } else {
            "none"
        }
    }

    /// A series with no deletion left to cancel: one create, as any.
    async fn create_plain(&mut self) -> GoogleResult<Event> {
        let path = format!(
            "/calendars/{}/events?sendUpdates={}",
            self.calendar,
            self.send_updates(self.invite)
        );
        let body = serde_json::to_value(bare_series_body(self.event, None))?;
        self.requests += 1;
        let answer = self.cal.send(Method::POST, path, Some(body)).await?;
        if !is_success(answer.status) {
            return Err(answer_error(&answer));
        }
        let entry: EventEntry = serde_json::from_str(&answer.body)
            .map_err(|e| GoogleError::Protocol(format!("json: {e}")))?;
        map_event(entry, self.calendar_id)?
            .ok_or_else(|| GoogleError::Protocol("create returned cancelled event".into()))
    }

    /// The create, under `id` (268), never cut short by the clock (226).
    /// Turned down whole, it is that error; where its answer was lost, the
    /// series is read back by its id, and where it cannot be found it may
    /// stand. Google refusing the id itself is answered by one create under
    /// an id of Google's. A create asked to slow down made nothing and is
    /// sent again after a wait, a few times.
    async fn create(&mut self, id: String) -> GoogleResult<Event> {
        let path = format!(
            "/calendars/{}/events?sendUpdates={}",
            self.calendar,
            self.send_updates(self.invite)
        );
        let mut own = Some(id);
        let mut busy = 0u32;
        let entry = loop {
            let body = serde_json::to_value(bare_series_body(self.event, own.as_deref()))?;
            self.requests += 1;
            let sent = Instant::now();
            let answer = self.cal.send(Method::POST, path.clone(), Some(body)).await;
            self.slowest = self.slowest.max(sent.elapsed());
            let answer = match answer {
                Ok(answer) => answer,
                // Sent, and its answer lost: it may have been made.
                Err(GoogleError::Network(_)) => break self.read_back(own.as_deref()).await?,
                // Never left, or the sign-in was refused: nothing was made.
                Err(err) => return Err(err),
            };
            if is_success(answer.status) {
                match serde_json::from_str::<EventEntry>(&answer.body) {
                    Ok(entry) if entry.status.as_deref() != Some("cancelled") => break entry,
                    // An answer that says nothing readable: it may have been made.
                    _ => break self.read_back(own.as_deref()).await?,
                }
            }
            if rate_limited(&answer) && busy + 1 < CREATE_BUSY_TRIES {
                let wait = answer.retry_after.unwrap_or_else(|| self.back_off(busy));
                busy += 1;
                self.retries += 1;
                self.waited += wait;
                tokio::time::sleep(wait).await;
                continue;
            }
            if answer.status == 400 && own.is_some() {
                // Google refusing the id Aperio chose: once more under its own.
                tracing::info!(
                    target: "adapter_google::write",
                    "google refused the series' id; creating it under one of google's"
                );
                own = None;
                continue;
            }
            if answer.status >= 500 {
                // The server failed, maybe after making it.
                break self.read_back(own.as_deref()).await?;
            }
            return Err(answer_error(&answer));
        };
        self.confirmed = Some(Instant::now());
        self.invited = self.invite;
        if own.as_deref().is_some_and(|own| own != entry.id) {
            tracing::info!(
                target: "adapter_google::write",
                "google created the series under an id of its own"
            );
        }
        self.series = entry.id.clone();
        // The series as Google stored it carries its rule and no deletion: a
        // line of its own would be taken for what the steps are to make.
        let lines = entry.recurrence.clone().unwrap_or_default();
        let rules = lines.iter().filter(|l| l.starts_with("RRULE")).count();
        let others = lines
            .iter()
            .any(|l| l.starts_with("EXDATE") || l.starts_with("RDATE") || l.starts_with("EXRULE"));
        let mapped = map_event(entry, self.calendar_id);
        match mapped {
            Ok(Some(created)) if rules == 1 && !others => Ok(created),
            _ => {
                // Not the series that was asked for, or not readable: it stands,
                // so it is taken back like any step that failed.
                Err(self.take_back(Failure::new("create", "control")).await)
            }
        }
    }

    /// The series a create whose answer was lost may have made, read by the
    /// id Aperio gave it: there, it was made; not found, it may still stand.
    async fn read_back(&mut self, id: Option<&str>) -> GoogleResult<EventEntry> {
        // The invitees may have heard of it: the create was sent.
        self.invited = self.invite;
        if let Some(id) = id {
            let path = format!("/calendars/{}/events/{}", self.calendar, urlencoding(id));
            for attempt in 0..2 {
                if attempt > 0 {
                    tokio::time::sleep(self.pacing.first_wait).await;
                }
                self.requests += 1;
                let Ok(answer) = self.cal.send(Method::GET, path.clone(), None).await else {
                    continue;
                };
                if !is_success(answer.status) {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<EventEntry>(&answer.body) {
                    if entry.status.as_deref() != Some("cancelled") {
                        return Ok(entry);
                    }
                }
            }
        }
        self.log_lost("create-answer-lost");
        Err(GoogleError::NotKept {
            undone: false,
            detail: cal_core::step_detail("create-answer-lost", self.invited),
        })
    }

    /// The steps after the confirmed create: list, judge, cancel, check.
    async fn complete(&mut self) -> Result<(), Failure> {
        let confirmed = self.confirmed.unwrap_or(self.started);
        let until = confirmed + self.budget.settle;
        let mut before = self.list(until, "list").await?;
        if before.iter().any(|o| o.cancelled) {
            // A series just made has nothing cancelled: someone else did it.
            return Err(Failure::new("list", "foreign-deletion"));
        }
        if before.is_empty() {
            self.pause(until, "list").await?;
            before = self.list(until, "list").await?;
        }
        if before.is_empty() {
            // Nothing in the deletions' window: the series ended before them,
            // or Google lists nothing at all.
            if self.any_occurrence(until).await? {
                for wanted in self.wanted {
                    self.skip(wanted, "past-end");
                }
                return Ok(());
            }
            return Err(Failure::new("list", "control"));
        }
        let mut matched = judge(self.wanted, &before);
        if matched.iter().any(Option::is_none) {
            // One listing may lag behind the create (269): once more.
            self.pause(until, "list").await?;
            let again = self.list(until, "list").await?;
            if again.iter().any(|o| o.cancelled) {
                return Err(Failure::new("list", "foreign-deletion"));
            }
            before = again;
            matched = judge(self.wanted, &before);
        }
        for (wanted, found) in self.wanted.iter().zip(&matched) {
            let Some(index) = found else {
                let reason = skip_reason(wanted.slot, self.event, &before);
                self.skip(wanted, reason);
                continue;
            };
            let occurrence = before[*index].clone();
            self.cancel(&occurrence, until).await?;
            self.cancelled.push(Cancelled {
                asked: wanted.asked,
                id: occurrence.id,
            });
        }
        self.check(&before, until + self.pacing.proof_borrow).await
    }

    /// Wait the first wait, where it fits.
    async fn pause(&mut self, until: Instant, step: &'static str) -> Result<(), Failure> {
        let remaining = until.saturating_duration_since(Instant::now());
        if !cal_core::may_send(remaining, self.pacing.first_wait, self.slowest) {
            return Err(Failure::new(step, self.out_of_time(false)));
        }
        self.waited += self.pacing.first_wait;
        tokio::time::sleep(self.pacing.first_wait).await;
        Ok(())
    }

    /// The new series' occurrences in the deletions' window, cancelled ones
    /// too, every page.
    async fn list(
        &mut self,
        until: Instant,
        step: &'static str,
    ) -> Result<Vec<Occurrence>, Failure> {
        let (from, to) = self.window();
        let base = format!(
            "/calendars/{}/events/{}/instances?showDeleted=true&timeMin={}&timeMax={}&maxResults=2500",
            self.calendar,
            urlencoding(&self.series),
            urlencoding(&from.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            urlencoding(&to.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        );
        let mut found = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let path = match &page {
                Some(token) => format!("{base}&pageToken={}", urlencoding(token)),
                None => base.clone(),
            };
            let answer = self
                .send(Method::GET, path, None, until, step, false)
                .await?;
            let answer = answer.map_err(|err| Failure::new(step, why_of_error(&err)))?;
            if !is_success(answer.status) {
                return Err(Failure::new(step, why_of(&answer)));
            }
            let listed: EventListResponse =
                serde_json::from_str(&answer.body).map_err(|_| Failure::new(step, "unreadable"))?;
            for entry in listed.items {
                if let Some(occurrence) = occurrence_of(entry, self.event.all_day) {
                    found.push(occurrence);
                }
            }
            match listed.next_page_token {
                Some(token) if !token.is_empty() => page = Some(token),
                _ => break,
            }
        }
        found.sort_by_key(|o| o.slot);
        Ok(found)
    }

    /// Whether the series has any occurrence at all, wherever it stands.
    async fn any_occurrence(&mut self, until: Instant) -> Result<bool, Failure> {
        let path = format!(
            "/calendars/{}/events/{}/instances?showDeleted=true&maxResults=1",
            self.calendar,
            urlencoding(&self.series),
        );
        let answer = self
            .send(Method::GET, path, None, until, "list", false)
            .await?;
        let answer = answer.map_err(|err| Failure::new("list", why_of_error(&err)))?;
        if !is_success(answer.status) {
            return Err(Failure::new("list", why_of(&answer)));
        }
        let listed: EventListResponse =
            serde_json::from_str(&answer.body).map_err(|_| Failure::new("list", "unreadable"))?;
        Ok(!listed.items.is_empty())
    }

    /// The window the deletions lie in, wide enough that an occurrence
    /// starting or ending at its edge is listed: Google filters by an
    /// occurrence's end at the window's start.
    fn window(&self) -> (DateTime<Utc>, DateTime<Utc>) {
        let first = self
            .wanted
            .iter()
            .map(|w| w.asked)
            .min()
            .unwrap_or(self.event.start);
        let last = self
            .wanted
            .iter()
            .map(|w| w.asked)
            .max()
            .unwrap_or(self.event.start);
        let length = (self.event.end - self.event.start).max(chrono::Duration::zero());
        let pad = chrono::Duration::days(2);
        (first - length - pad, last + pad)
    }

    /// Cancel one occurrence, as Google itself deletes one, and take its
    /// answer as the proof (208): this occurrence, cancelled, of this series,
    /// at this slot — where the answer names them. An answer lost is decided
    /// by reading the occurrence.
    async fn cancel(&mut self, occurrence: &Occurrence, until: Instant) -> Result<(), Failure> {
        let path = format!(
            "/calendars/{}/events/{}?sendUpdates={}",
            self.calendar,
            urlencoding(&occurrence.id),
            self.send_updates(self.invite)
        );
        let read = format!(
            "/calendars/{}/events/{}",
            self.calendar,
            urlencoding(&occurrence.id)
        );
        for _ in 0..2 {
            let body = serde_json::json!({ "status": "cancelled" });
            let answer = self
                .send(
                    Method::PATCH,
                    path.clone(),
                    Some(body),
                    until,
                    "cancel",
                    false,
                )
                .await?;
            match answer {
                Ok(answer) if is_success(answer.status) => {
                    return match self.proves(&answer.body, occurrence) {
                        Some(true) => Ok(()),
                        Some(false) => Err(Failure::new("cancel", "control")),
                        // Unreadable: the occurrence says.
                        None => match self.read_occurrence(&read, occurrence, until).await? {
                            Some(true) => Ok(()),
                            Some(false) => continue,
                            None => Err(Failure::new("cancel", "unreadable")),
                        },
                    };
                }
                Ok(answer) if answer.status >= 500 => {
                    match self.read_occurrence(&read, occurrence, until).await? {
                        Some(true) => return Ok(()),
                        Some(false) => continue,
                        None => return Err(Failure::new("cancel", why_of(&answer))),
                    }
                }
                Ok(answer) => return Err(Failure::new("cancel", why_of(&answer))),
                Err(GoogleError::Network(_)) => {
                    match self.read_occurrence(&read, occurrence, until).await? {
                        Some(true) => return Ok(()),
                        Some(false) => continue,
                        None => return Err(Failure::new("cancel", "network")),
                    }
                }
                Err(err) => return Err(Failure::new("cancel", why_of_error(&err))),
            }
        }
        Err(Failure::new("cancel", "control"))
    }

    /// Whether a cancel's answer proves it: `None` when it cannot be read.
    fn proves(&self, body: &str, occurrence: &Occurrence) -> Option<bool> {
        let entry: EventEntry = serde_json::from_str(body).ok()?;
        let cancelled = entry.status.as_deref() == Some("cancelled");
        let same_series = entry
            .recurring_event_id
            .as_deref()
            .is_none_or(|series| series == self.series);
        let same_slot = entry
            .original_start_time
            .as_ref()
            .and_then(|at| slot_of(at, self.event.all_day))
            .is_none_or(|slot| slot == occurrence.slot);
        Some(entry.id == occurrence.id && cancelled && same_series && same_slot)
    }

    /// One occurrence as Google holds it now: `Some(true)` cancelled,
    /// `Some(false)` still there, `None` not to be told.
    async fn read_occurrence(
        &mut self,
        path: &str,
        occurrence: &Occurrence,
        until: Instant,
    ) -> Result<Option<bool>, Failure> {
        let answer = self
            .send(Method::GET, path.to_string(), None, until, "cancel", false)
            .await?;
        let Ok(answer) = answer else {
            return Ok(None);
        };
        if !is_success(answer.status) {
            return Ok(None);
        }
        let Ok(entry) = serde_json::from_str::<EventEntry>(&answer.body) else {
            return Ok(None);
        };
        if entry.id != occurrence.id {
            return Ok(None);
        }
        Ok(Some(entry.status.as_deref() == Some("cancelled")))
    }

    /// The listing after the cancels (208): every occurrence that stood
    /// before and was not cancelled here still stands, and each one cancelled
    /// here is cancelled or gone. One still standing is read once more.
    async fn check(&mut self, before: &[Occurrence], until: Instant) -> Result<(), Failure> {
        let ours: HashSet<String> = self.cancelled.iter().map(|c| c.id.clone()).collect();
        let mut tries = 0;
        loop {
            let after = self.list(until, "after-control").await?;
            let standing: HashSet<String> = after
                .into_iter()
                .filter(|o| !o.cancelled)
                .map(|o| o.id)
                .collect();
            let foreign = before
                .iter()
                .filter(|o| !o.cancelled && !ours.contains(&o.id))
                .any(|o| !standing.contains(&o.id));
            if foreign {
                return Err(Failure::new("after-control", "foreign-deletion"));
            }
            if !ours.iter().any(|id| standing.contains(id)) {
                return Ok(());
            }
            tries += 1;
            if tries > 1 {
                return Err(Failure::new("after-control", "after-control"));
            }
            self.pause(until, "after-control").await?;
        }
    }

    /// Send one request inside the time left until `until`, waiting out
    /// answers that ask to slow down while they fit; a request that cannot be
    /// sent in time fails with [`cal_core::OUT_OF_TIME`].
    async fn send(
        &mut self,
        method: Method,
        path: String,
        body: Option<serde_json::Value>,
        until: Instant,
        step: &'static str,
        must_send: bool,
    ) -> Result<GoogleResult<Answer>, Failure> {
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
            let answer = match tokio::time::timeout(
                window,
                self.cal.send(method.clone(), path.clone(), body.clone()),
            )
            .await
            {
                Ok(answer) => answer,
                Err(_) => Err(GoogleError::Network("timed out".into())),
            };
            self.slowest = self.slowest.max(sent.elapsed());
            let busy = match &answer {
                Ok(answer) => {
                    rate_limited(answer) || (method == Method::GET && answer.status >= 500)
                }
                Err(_) => false,
            };
            if !busy {
                return Ok(answer);
            }
            let asked = answer.as_ref().ok().and_then(|a| a.retry_after);
            let wait = asked.unwrap_or_else(|| self.back_off(attempt));
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

    /// The wait before the `attempt`th repeat: the first wait, doubled, up
    /// to the longest.
    fn back_off(&self, attempt: u32) -> Duration {
        self.pacing
            .first_wait
            .saturating_mul(1 << attempt.min(8))
            .min(self.pacing.max_wait)
    }

    fn skip(&mut self, wanted: &Wanted, where_: &str) {
        self.skipped += 1;
        tracing::info!(
            target: "adapter_google::write",
            slot = %describe_slot(wanted.slot),
            r#where = where_,
            "a deleted occurrence the series makes no occurrence at is skipped",
        );
    }

    /// Delete the new series again (203), inside the time kept for it, and
    /// say what stands.
    async fn take_back(&mut self, failure: Failure) -> GoogleError {
        let failed_at = Instant::now();
        let confirmed = self.confirmed.unwrap_or(self.started);
        let until =
            (failed_at + self.budget.undo).min(confirmed + self.budget.settle + self.budget.undo);
        // The invitees hear of the take-back exactly when they may have heard
        // of the series (212, 264).
        let delete = format!(
            "/calendars/{}/events/{}?sendUpdates={}",
            self.calendar,
            urlencoding(&self.series),
            self.send_updates(self.invited)
        );
        let read = format!(
            "/calendars/{}/events/{}",
            self.calendar,
            urlencoding(&self.series)
        );
        let mut undone = false;
        // The first delete and the first read that confirms it are sent
        // whatever the budget says (226); an answer asking to slow down is
        // waited out inside it.
        let mut first_delete = true;
        let mut first_read = true;
        for _ in 0..3 {
            let Ok(answer) = self
                .send(
                    Method::DELETE,
                    delete.clone(),
                    None,
                    until,
                    "take-back",
                    first_delete,
                )
                .await
            else {
                break;
            };
            first_delete = false;
            if let Ok(answer) = &answer {
                if is_success(answer.status) || matches!(answer.status, 404 | 410) {
                    undone = true;
                    break;
                }
                if answer.status < 500 {
                    // Turned down: it stands.
                    break;
                }
            }
            // The answer may be lost after the delete was made: the series
            // says. A deleted event still reads, as cancelled.
            let Ok(read) = self
                .send(
                    Method::GET,
                    read.clone(),
                    None,
                    until,
                    "take-back",
                    first_read,
                )
                .await
            else {
                break;
            };
            first_read = false;
            match read {
                Ok(answer) if matches!(answer.status, 404 | 410) => {
                    undone = true;
                    break;
                }
                Ok(answer) if is_success(answer.status) => {
                    let gone = serde_json::from_str::<EventEntry>(&answer.body)
                        .is_ok_and(|entry| entry.status.as_deref() == Some("cancelled"));
                    if gone {
                        undone = true;
                        break;
                    }
                    // Still there: delete it once more.
                }
                _ => break,
            }
        }
        let detail = cal_core::step_detail(&failure.why, self.invited);
        tracing::warn!(
            target: "adapter_google::write",
            asked = self.wanted.len(),
            past = self.past,
            cancelled = self.cancelled.len(),
            skipped = self.skipped,
            step = failure.step,
            why = %failure.why,
            invited = self.invited,
            undo = if undone { "done" } else { "failed" },
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "google series taken back: a deletion was not kept",
        );
        GoogleError::NotKept { undone, detail }
    }

    /// The create's answer was lost: nothing is taken back, so this is the
    /// line a "may stand" is read by.
    fn log_lost(&self, why: &str) {
        tracing::warn!(
            target: "adapter_google::write",
            step = "create",
            why,
            asked = self.wanted.len(),
            past = self.past,
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "google series may stand: the answer to its creation was lost",
        );
    }

    fn log_success(&self) {
        let slots = self
            .wanted
            .iter()
            .map(|w| {
                let done = self.cancelled.iter().any(|c| c.asked == w.asked);
                format!(
                    "{} {}",
                    describe_slot(w.slot),
                    if done { "cancelled" } else { "skipped" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!(
            target: "adapter_google::write",
            asked = self.wanted.len(),
            past = self.past,
            cancelled = self.cancelled.len(),
            skipped = self.skipped,
            notify = self.invite,
            all_day = self.event.all_day,
            requests = self.requests,
            retries = self.retries,
            waited_ms = self.waited.as_millis() as u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            %slots,
            "google series created; deletions cancelled one by one",
        );
    }
}

/// Which listed occurrence each deleted slot names (269), in the order of
/// `wanted`: the one starting there, else the nearest one not yet taken that
/// starts at most [`MATCH_TOLERANCE`] off and closer to the slot than half
/// the way to its neighbours; an all-day slot by its day only. Cancelled
/// occurrences are no candidates.
fn judge(wanted: &[Wanted], listed: &[Occurrence]) -> Vec<Option<usize>> {
    let mut taken: HashSet<usize> = HashSet::new();
    let mut found: Vec<Option<usize>> = vec![None; wanted.len()];
    // Exact first, so a near one never takes an occurrence another slot names.
    for (w, wanted) in wanted.iter().enumerate() {
        if let Some(index) = listed
            .iter()
            .position(|o| !o.cancelled && o.slot == wanted.slot)
        {
            if taken.insert(index) {
                found[w] = Some(index);
            }
        }
    }
    let live: Vec<usize> = (0..listed.len())
        .filter(|&i| !listed[i].cancelled)
        .collect();
    for (w, wanted) in wanted.iter().enumerate() {
        if found[w].is_some() {
            continue;
        }
        let Slot::At(at) = wanted.slot else {
            continue;
        };
        let mut best: Option<(chrono::Duration, usize)> = None;
        for (k, &index) in live.iter().enumerate() {
            if taken.contains(&index) {
                continue;
            }
            let Slot::At(start) = listed[index].slot else {
                continue;
            };
            let off = (start - at).abs();
            if off > MATCH_TOLERANCE {
                continue;
            }
            // Closer than half the way to either neighbour.
            let neighbour_gap = [k.checked_sub(1), Some(k + 1)]
                .into_iter()
                .flatten()
                .filter_map(|n| live.get(n))
                .filter_map(|&n| match listed[n].slot {
                    Slot::At(other) => Some((other - start).abs()),
                    Slot::Day(_) => None,
                })
                .min();
            if neighbour_gap.is_some_and(|gap| off * 2 >= gap) {
                continue;
            }
            if best.is_none_or(|(best_off, _)| off < best_off) {
                best = Some((off, index));
            }
        }
        if let Some((_, index)) = best {
            taken.insert(index);
            found[w] = Some(index);
        }
    }
    found
}

/// Why a deleted slot has no occurrence, for the log: before the series'
/// start, after the last occurrence listed, or between two off the pattern.
fn skip_reason(slot: Slot, event: &NewEvent, listed: &[Occurrence]) -> &'static str {
    let start = if event.all_day {
        Slot::Day(event.start.date_naive())
    } else {
        Slot::At(event.start)
    };
    if slot < start {
        "before-start"
    } else if listed.iter().all(|o| o.slot < slot) {
        "past-end"
    } else {
        "off-pattern"
    }
}

/// An occurrence as Google listed it, by the slot it was made for.
fn occurrence_of(entry: EventEntry, all_day: bool) -> Option<Occurrence> {
    let slot = entry
        .original_start_time
        .as_ref()
        .and_then(|at| slot_of(at, all_day))
        .or_else(|| slot_of(&entry.start, all_day))?;
    Some(Occurrence {
        cancelled: entry.status.as_deref() == Some("cancelled"),
        id: entry.id,
        slot,
    })
}

fn slot_of(at: &crate::mapping::EventDateTime, all_day: bool) -> Option<Slot> {
    if all_day {
        at.date.map(Slot::Day)
    } else {
        at.date_time.map(Slot::At)
    }
}

/// A slot as the log names it: an instant, or a day. No id ever: the log is
/// the user's to share.
fn describe_slot(slot: Slot) -> String {
    match slot {
        Slot::At(at) => at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        Slot::Day(day) => day.to_string(),
    }
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Whether Google asked to slow down: 429, or 403 for a rate limit; not a
/// quota used up for the day, which waiting seconds does not help.
fn rate_limited(answer: &Answer) -> bool {
    answer.status == 429
        || (answer.status == 403
            && error_reason(&answer.body)
                .is_some_and(|reason| RATE_LIMIT_REASONS.contains(&reason.as_str())))
}

/// The reason an answer gives, for the log: Google's own where it names one.
fn why_of(answer: &Answer) -> String {
    match error_reason(&answer.body) {
        Some(reason) => format!("google-{reason}"),
        None => format!("http-{}", answer.status),
    }
}

fn why_of_error(err: &GoogleError) -> String {
    match err {
        GoogleError::Http { status, .. } => format!("http-{status}"),
        GoogleError::Network(_) | GoogleError::Unreached(_) => "network".to_string(),
        GoogleError::Protocol(_) => "unreadable".to_string(),
        GoogleError::Config(_) => "config".to_string(),
        _ => "other".to_string(),
    }
}

/// An answer that is no success, as the error a create gives back: its
/// status, and the start of what Google said.
fn answer_error(answer: &Answer) -> GoogleError {
    GoogleError::Http {
        status: answer.status,
        message: answer.body.chars().take(300).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::EventRecurrence;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// What a request asked Google to do.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Kind {
        Insert,
        List,
        Cancel,
        GetSeries,
        GetOccurrence,
        Delete,
    }
    use Kind::*;

    /// What goes wrong with the `nth` request of a kind.
    #[derive(Debug, Clone)]
    enum Fault {
        /// 429, with what it asks to wait.
        Busy429(Option<Duration>),
        /// 403 with this reason.
        Busy403(&'static str),
        /// This status, nothing done.
        Http(u16),
        /// Done, and the answer lost.
        LostAfter,
        /// Not done, and the answer lost.
        LostBefore,
        /// Never left.
        Unreached,
        /// Answered after this long.
        Slow(Duration),
        /// A cancel answered as done, but nothing done.
        Pretend,
        /// Done, answered with something no reader takes.
        Garbled,
        /// A cancel that also cancels the occurrence at this index.
        Foreign(usize),
        /// A cancel answered as another series' occurrence.
        OtherSeries,
        /// A cancel answered as another slot's.
        OtherSlot,
        /// A cancel answered with the id and the status only.
        Bare,
        /// A create under Aperio's id refused for its id.
        RefuseId,
        /// The sign-in could not be renewed.
        RefreshRefused,
        /// A listing that shows the occurrences as before the last cancel.
        StaleList,
    }

    #[derive(Debug, Clone)]
    struct Occ {
        id: String,
        slot: Slot,
        cancelled: bool,
    }

    #[derive(Debug, Clone)]
    struct Series {
        id: String,
        deleted: bool,
        lines: Vec<String>,
        /// What the create sent: the series as Google stores it.
        stored: serde_json::Value,
        occurrences: Vec<Occ>,
    }

    #[derive(Debug, Default)]
    struct State {
        series: Option<Series>,
        /// Every request, in order: its kind, its path and query, its body.
        log: Vec<(Kind, String, Option<serde_json::Value>)>,
        faults: Vec<(Kind, usize, Fault)>,
        counts: HashMap<Kind, usize>,
        /// Listings leave cancelled occurrences out.
        hide_cancelled: bool,
        /// This many listings show the occurrences as before the last change.
        stale: usize,
        before_last: Option<Vec<Occ>>,
        /// Occurrences per page of a listing.
        page: usize,
        /// The create stores a deletion line of its own.
        extra_line: bool,
        /// The create stores its rule spelled another way.
        normalized: bool,
    }

    /// One Google calendar, holding the series a run creates: the occurrences
    /// its rule makes are `shape`.
    struct Fake {
        shape: Vec<Slot>,
        state: Mutex<State>,
    }

    impl Fake {
        fn new(shape: Vec<Slot>, faults: Vec<(Kind, usize, Fault)>) -> Self {
            Self {
                shape,
                state: Mutex::new(State {
                    faults,
                    page: 2500,
                    ..State::default()
                }),
            }
        }

        fn with(self, change: impl FnOnce(&mut State)) -> Self {
            change(&mut self.state.lock().unwrap());
            self
        }

        fn requests(&self, kind: Kind) -> Vec<(String, Option<serde_json::Value>)> {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .filter(|(k, _, _)| *k == kind)
                .map(|(_, path, body)| (path.clone(), body.clone()))
                .collect()
        }

        fn kinds(&self) -> Vec<Kind> {
            self.state
                .lock()
                .unwrap()
                .log
                .iter()
                .map(|(k, _, _)| *k)
                .collect()
        }

        /// The series as Google holds it: standing, and its occurrences.
        fn standing(&self) -> Option<Series> {
            self.state
                .lock()
                .unwrap()
                .series
                .clone()
                .filter(|s| !s.deleted)
        }

        fn cancelled_slots(&self) -> Vec<Slot> {
            self.standing()
                .map(|s| {
                    s.occurrences
                        .iter()
                        .filter(|o| o.cancelled)
                        .map(|o| o.slot)
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    fn occurrence_id(series: &str, slot: Slot) -> String {
        match slot {
            Slot::At(at) => format!("{series}_{}", at.format("%Y%m%dT%H%M%SZ")),
            Slot::Day(day) => format!("{series}_{}", day.format("%Y%m%d")),
        }
    }

    fn slot_json(slot: Slot) -> serde_json::Value {
        match slot {
            Slot::At(at) => serde_json::json!({ "dateTime": at.to_rfc3339() }),
            Slot::Day(day) => serde_json::json!({ "date": day.to_string() }),
        }
    }

    fn instant(slot: Slot) -> DateTime<Utc> {
        match slot {
            Slot::At(at) => at,
            Slot::Day(day) => day.and_hms_opt(12, 0, 0).unwrap().and_utc(),
        }
    }

    fn query<'q>(path: &'q str, key: &str) -> Option<&'q str> {
        let (_, query) = path.split_once('?')?;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
    }

    fn decoded(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn answer(status: u16, body: serde_json::Value) -> GoogleResult<Answer> {
        Ok(Answer {
            status,
            retry_after: None,
            body: body.to_string(),
        })
    }

    fn refused(status: u16, reason: &str) -> GoogleResult<Answer> {
        answer(
            status,
            serde_json::json!({ "error": { "code": status, "errors": [{ "reason": reason }] } }),
        )
    }

    impl Fake {
        fn kind_of(&self, method: &Method, path: &str) -> Kind {
            let bare = path.split('?').next().unwrap();
            let segments: Vec<&str> = bare.split('/').collect();
            // ["", "calendars", cal, "events", id?, "instances"?]
            match (method.clone(), segments.len()) {
                (Method::POST, 4) => Insert,
                (Method::GET, 6) => List,
                (Method::GET, 5) => {
                    let id = decoded(segments[4]);
                    let series = self
                        .state
                        .lock()
                        .unwrap()
                        .series
                        .as_ref()
                        .map(|s| s.id.clone());
                    if series.as_deref() == Some(id.as_str()) {
                        GetSeries
                    } else {
                        GetOccurrence
                    }
                }
                (Method::PATCH, 5) => Cancel,
                (Method::DELETE, 5) => Delete,
                other => panic!("unexpected request {other:?} {path}"),
            }
        }

        /// Do what the request asks, and answer as Google does.
        fn handle(
            &self,
            kind: Kind,
            path: &str,
            body: Option<&serde_json::Value>,
        ) -> GoogleResult<Answer> {
            let mut state = self.state.lock().unwrap();
            match kind {
                Insert => {
                    let body = body.unwrap();
                    let id = body
                        .get("id")
                        .and_then(|id| id.as_str())
                        .unwrap_or("googlechosen1")
                        .to_string();
                    let mut lines: Vec<String> = body["recurrence"]
                        .as_array()
                        .map(|a| a.iter().map(|l| l.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default();
                    if state.normalized {
                        lines = lines.into_iter().map(|l| format!("{l};WKST=SU")).collect();
                    }
                    if state.extra_line {
                        lines.push("EXDATE:20261109T090000Z".into());
                    }
                    let occurrences = self
                        .shape
                        .iter()
                        .map(|&slot| Occ {
                            id: occurrence_id(&id, slot),
                            slot,
                            cancelled: false,
                        })
                        .collect();
                    let stored = serde_json::json!({
                        "id": id,
                        "status": "confirmed",
                        "summary": body["summary"],
                        "start": body["start"],
                        "end": body["end"],
                        "recurrence": lines,
                    });
                    state.series = Some(Series {
                        id: id.clone(),
                        deleted: false,
                        lines: lines.clone(),
                        stored: stored.clone(),
                        occurrences,
                    });
                    answer(200, stored)
                }
                List => {
                    let Some(series) = state.series.clone() else {
                        return refused(404, "notFound");
                    };
                    let mut occurrences = series.occurrences.clone();
                    if state.stale > 0 {
                        state.stale -= 1;
                        if let Some(before) = state.before_last.clone() {
                            occurrences = before;
                        }
                    }
                    let from = query(path, "timeMin")
                        .map(|v| decoded(v).parse::<DateTime<Utc>>().unwrap());
                    let to = query(path, "timeMax")
                        .map(|v| decoded(v).parse::<DateTime<Utc>>().unwrap());
                    let limit: usize = query(path, "maxResults").unwrap().parse().unwrap();
                    let listed: Vec<&Occ> = occurrences
                        .iter()
                        .filter(|o| !(state.hide_cancelled && o.cancelled))
                        .filter(|o| from.is_none_or(|from| instant(o.slot) >= from))
                        .filter(|o| to.is_none_or(|to| instant(o.slot) < to))
                        .collect();
                    let offset: usize = query(path, "pageToken")
                        .map(|t| decoded(t).parse().unwrap())
                        .unwrap_or(0);
                    let size = limit.min(state.page);
                    let items: Vec<serde_json::Value> = listed
                        .iter()
                        .skip(offset)
                        .take(size)
                        .map(|o| {
                            serde_json::json!({
                                "id": o.id,
                                "status": if o.cancelled { "cancelled" } else { "confirmed" },
                                "recurringEventId": series.id,
                                "originalStartTime": slot_json(o.slot),
                                "start": slot_json(o.slot),
                            })
                        })
                        .collect();
                    let mut page = serde_json::json!({ "items": items });
                    if offset + size < listed.len() {
                        page["nextPageToken"] = serde_json::json!((offset + size).to_string());
                    }
                    answer(200, page)
                }
                GetSeries => {
                    let series = state.series.as_ref().unwrap();
                    let mut stored = series.stored.clone();
                    stored["recurrence"] = serde_json::json!(series.lines);
                    stored["status"] = serde_json::json!(if series.deleted {
                        "cancelled"
                    } else {
                        "confirmed"
                    });
                    answer(200, stored)
                }
                GetOccurrence => {
                    let id = decoded(path.split('?').next().unwrap().rsplit('/').next().unwrap());
                    let found = state
                        .series
                        .as_ref()
                        .and_then(|s| s.occurrences.iter().find(|o| o.id == id).cloned());
                    match found {
                        Some(o) => answer(
                            200,
                            serde_json::json!({
                                "id": o.id,
                                "status": if o.cancelled { "cancelled" } else { "confirmed" },
                            }),
                        ),
                        None => refused(404, "notFound"),
                    }
                }
                Cancel => {
                    let id = decoded(path.split('?').next().unwrap().rsplit('/').next().unwrap());
                    let before = state.series.as_ref().map(|s| s.occurrences.clone());
                    let Some(series) = state.series.as_mut() else {
                        return refused(404, "notFound");
                    };
                    let Some(occurrence) = series.occurrences.iter_mut().find(|o| o.id == id)
                    else {
                        return refused(404, "notFound");
                    };
                    occurrence.cancelled = true;
                    let (slot, series_id) = (occurrence.slot, series.id.clone());
                    state.before_last = before;
                    answer(
                        200,
                        serde_json::json!({
                            "id": id,
                            "status": "cancelled",
                            "recurringEventId": series_id,
                            "originalStartTime": slot_json(slot),
                        }),
                    )
                }
                Delete => match state.series.as_mut() {
                    Some(series) if !series.deleted => {
                        series.deleted = true;
                        Ok(Answer {
                            status: 204,
                            retry_after: None,
                            body: String::new(),
                        })
                    }
                    Some(_) => refused(410, "deleted"),
                    None => refused(404, "notFound"),
                },
            }
        }
    }

    impl Calendar for Fake {
        async fn send(
            &self,
            method: Method,
            path: String,
            body: Option<serde_json::Value>,
        ) -> GoogleResult<Answer> {
            let kind = self.kind_of(&method, &path);
            let fault = {
                let mut state = self.state.lock().unwrap();
                state.log.push((kind, path.clone(), body.clone()));
                let nth = *state.counts.get(&kind).unwrap_or(&0);
                state.counts.insert(kind, nth + 1);
                state
                    .faults
                    .iter()
                    .find(|(k, n, _)| *k == kind && *n == nth)
                    .map(|(_, _, fault)| fault.clone())
            };
            match fault {
                None => self.handle(kind, &path, body.as_ref()),
                Some(Fault::Busy429(after)) => Ok(Answer {
                    status: 429,
                    retry_after: after,
                    body: r#"{"error":{"code":429,"errors":[{"reason":"rateLimitExceeded"}]}}"#
                        .into(),
                }),
                Some(Fault::Busy403(reason)) => refused(403, reason),
                Some(Fault::Http(status)) => {
                    answer(status, serde_json::json!({ "error": { "code": status } }))
                }
                Some(Fault::LostAfter) => {
                    let _ = self.handle(kind, &path, body.as_ref());
                    Err(GoogleError::Network("connection reset".into()))
                }
                Some(Fault::LostBefore) => Err(GoogleError::Network("connection reset".into())),
                Some(Fault::Unreached) => Err(GoogleError::Unreached("refused".into())),
                Some(Fault::Slow(delay)) => {
                    tokio::time::sleep(delay).await;
                    self.handle(kind, &path, body.as_ref())
                }
                // A cancel's own faults; any other request goes as asked.
                Some(Fault::Pretend | Fault::OtherSeries | Fault::OtherSlot | Fault::Bare)
                    if kind != Cancel =>
                {
                    self.handle(kind, &path, body.as_ref())
                }
                Some(Fault::Pretend) => {
                    let id = decoded(path.split('?').next().unwrap().rsplit('/').next().unwrap());
                    answer(200, serde_json::json!({ "id": id, "status": "cancelled" }))
                }
                Some(Fault::StaleList) => {
                    self.state.lock().unwrap().stale = 1;
                    self.handle(kind, &path, body.as_ref())
                }
                Some(Fault::Garbled) => {
                    let _ = self.handle(kind, &path, body.as_ref());
                    answer(200, serde_json::json!("<<garbled")).map(|mut a| {
                        a.body = "<<garbled".into();
                        a
                    })
                }
                // Someone else cancels an occurrence, before this request is
                // answered (after it, for the create that makes the series).
                Some(Fault::Foreign(index)) => {
                    let foreign = |fake: &Fake| {
                        if let Some(series) = fake.state.lock().unwrap().series.as_mut() {
                            series.occurrences[index].cancelled = true;
                        }
                    };
                    if kind == Insert {
                        let done = self.handle(kind, &path, body.as_ref());
                        foreign(self);
                        done
                    } else {
                        foreign(self);
                        self.handle(kind, &path, body.as_ref())
                    }
                }
                Some(Fault::OtherSeries) | Some(Fault::OtherSlot) | Some(Fault::Bare) => {
                    let done = self.handle(kind, &path, body.as_ref())?;
                    let mut value: serde_json::Value = serde_json::from_str(&done.body).unwrap();
                    match fault {
                        Some(Fault::OtherSeries) => {
                            value["recurringEventId"] = "someoneelse".into()
                        }
                        Some(Fault::OtherSlot) => {
                            value["originalStartTime"] =
                                serde_json::json!({ "dateTime": "2030-01-01T00:00:00Z" })
                        }
                        _ => {
                            let id = value["id"].clone();
                            value = serde_json::json!({ "id": id, "status": "cancelled" });
                        }
                    }
                    answer(done.status, value)
                }
                Some(Fault::RefuseId) => {
                    if body.as_ref().is_some_and(|b| b.get("id").is_some()) {
                        refused(400, "invalid")
                    } else {
                        self.handle(kind, &path, body.as_ref())
                    }
                }
                Some(Fault::RefreshRefused) => Err(GoogleError::Http {
                    status: 401,
                    message: "token refresh refused (HTTP 400): invalid_grant".into(),
                }),
            }
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    /// Mondays at 09:00 UTC, six of them from 2 November 2026.
    fn mondays() -> Vec<Slot> {
        (0..6)
            .map(|week| Slot::At(at("2026-11-02T09:00:00Z") + chrono::Duration::weeks(week)))
            .collect()
    }

    fn slot_at(slots: &[Slot], index: usize) -> DateTime<Utc> {
        match slots[index] {
            Slot::At(at) => at,
            Slot::Day(_) => unreachable!(),
        }
    }

    /// A weekly series from 2 November 2026, 09:00 UTC, with `deleted`.
    fn series(deleted: Vec<DateTime<Utc>>, notify: bool) -> NewEvent {
        NewEvent {
            title: "Teamrunde".into(),
            description: None,
            location: None,
            start: at("2026-11-02T09:00:00Z"),
            end: at("2026-11-02T10:00:00Z"),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=6".into(),
                exceptions: deleted,
                tzid: Some("Europe/Berlin".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: if notify {
                vec!["Lea <lea@example.com>".into()]
            } else {
                vec![]
            },
            send_invitations: notify,
            organizer: None,
            organized_elsewhere: false,
        }
    }

    const NOW: &str = "2026-10-18T10:00:00Z";
    const OWN_ID: &str = "0123456789abcdef0123456789abcdef";

    fn budget() -> SettleBudget {
        SettleBudget {
            settle: Duration::from_millis(600),
            undo: Duration::from_millis(400),
        }
    }

    fn pace() -> Pacing {
        Pacing {
            first_wait: Duration::from_millis(10),
            max_wait: Duration::from_millis(40),
            proof_borrow: Duration::from_millis(100),
            undo_floor: Duration::from_millis(200),
        }
    }

    async fn run(fake: &Fake, event: NewEvent) -> GoogleResult<Event> {
        run_on(fake, event, &Utc).await
    }

    async fn run_on<D: TimeZone + Sync>(
        fake: &Fake,
        event: NewEvent,
        device: &D,
    ) -> GoogleResult<Event> {
        create_keeping_deletions(
            fake,
            "cal@example.com",
            event,
            device,
            at(NOW),
            budget(),
            pace(),
            OWN_ID.to_string(),
        )
        .await
    }

    fn not_kept(result: GoogleResult<Event>) -> (bool, String) {
        match result {
            Err(GoogleError::NotKept { undone, detail }) => (undone, detail),
            other => panic!("expected NotKept, got {other:?}"),
        }
    }

    // ── success ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_series_is_created_bare_then_its_deletions_cancelled() {
        for (notify, hide) in [(false, false), (true, false), (false, true)] {
            let shape = mondays();
            let deleted = vec![slot_at(&shape, 1), slot_at(&shape, 3)];
            // Listings with or without the cancelled occurrences.
            let fake = Fake::new(shape.clone(), vec![]).with(|s| s.hide_cancelled = hide);
            let created = run(&fake, series(deleted.clone(), notify)).await.unwrap();
            assert_eq!(
                fake.kinds(),
                [Insert, List, Cancel, Cancel, List],
                "{notify}"
            );
            // Created under Aperio's id, with its rule and no deletion line.
            let (_, body) = &fake.requests(Insert)[0];
            let body = body.as_ref().unwrap();
            assert_eq!(body["id"], OWN_ID);
            assert_eq!(
                body["recurrence"],
                serde_json::json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=6"])
            );
            // Every write tells the invitees as the switch says (204, 267).
            let su = if notify {
                "sendUpdates=all"
            } else {
                "sendUpdates=none"
            };
            for (path, _) in fake.requests(Insert).iter().chain(&fake.requests(Cancel)) {
                assert!(path.ends_with(su), "{path}");
            }
            assert_eq!(fake.cancelled_slots(), [shape[1], shape[3]]);
            assert_eq!(created.recurrence.unwrap().exceptions, deleted);
        }
    }

    #[tokio::test]
    async fn the_listing_reads_every_page() {
        // Three thousand days: three pages of a thousand.
        let start = at("2026-11-02T09:00:00Z");
        let shape: Vec<Slot> = (0..3000)
            .map(|d| Slot::At(start + chrono::Duration::days(d)))
            .collect();
        let deleted = vec![slot_at(&shape, 5), slot_at(&shape, 2900)];
        let fake = Fake::new(shape.clone(), vec![]).with(|s| s.page = 1000);
        let mut event = series(deleted, false);
        event.recurrence.as_mut().unwrap().rrule = "FREQ=DAILY;COUNT=3000".into();
        run(&fake, event).await.unwrap();
        let pages = fake.requests(List);
        assert_eq!(pages.len(), 6, "two listings of three pages");
        assert!(pages[1].0.contains("pageToken=1000"));
        assert_eq!(fake.cancelled_slots(), [shape[5], shape[2900]]);
    }

    #[tokio::test]
    async fn all_day_deletions_are_cancelled_by_the_day_they_name() {
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        let shape: Vec<Slot> = (2..8).map(|d| Slot::Day(day(d))).collect();
        for hours in [-8, 13] {
            let device = chrono::FixedOffset::east_opt(hours * 3600).unwrap();
            let midnight = |d: u32| {
                device
                    .from_local_datetime(&day(d).and_hms_opt(0, 0, 0).unwrap())
                    .unwrap()
                    .with_timezone(&Utc)
            };
            // The 4th as the device's own midnight, the 6th as a UTC midnight
            // another provider spells, and the 4th once more.
            let deleted = vec![midnight(4), at("2026-11-06T00:00:00Z"), midnight(4)];
            let mut event = series(deleted, false);
            event.all_day = true;
            event.start = midnight(2);
            event.end = midnight(3);
            event.recurrence.as_mut().unwrap().rrule = "FREQ=DAILY;COUNT=6".into();
            event.recurrence.as_mut().unwrap().tzid = None;
            let fake = Fake::new(shape.clone(), vec![]);
            run_on(&fake, event, &device).await.unwrap();
            assert_eq!(
                fake.cancelled_slots(),
                [Slot::Day(day(4)), Slot::Day(day(6))],
                "{hours}"
            );
            assert_eq!(fake.requests(Cancel).len(), 2, "one cancel per day");
            // Both spellings of the 4th are one slot: nothing left to list again.
            assert_eq!(fake.requests(List).len(), 2, "{hours}");
        }
    }

    #[tokio::test]
    async fn an_occurrence_an_hour_off_is_the_one_deleted_but_not_one_further() {
        // Google puts the 16th at 10:00 where Aperio's zone table says 09:00.
        let mut shape = mondays();
        shape[2] = Slot::At(slot_at(&shape, 2) + chrono::Duration::hours(1));
        let deleted = vec![at("2026-11-16T09:00:00Z")];
        let fake = Fake::new(shape.clone(), vec![]);
        run(&fake, series(deleted, false)).await.unwrap();
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
        // A deletion at 15:00 names no occurrence at 09:00.
        let fake = Fake::new(mondays(), vec![]);
        run(&fake, series(vec![at("2026-11-16T15:00:00Z")], false))
            .await
            .unwrap();
        assert!(fake.cancelled_slots().is_empty());
        assert!(fake.requests(Cancel).is_empty());
    }

    #[tokio::test]
    async fn a_slot_the_series_makes_nothing_at_is_skipped_after_a_second_listing() {
        let shape = mondays();
        // Before its start, a Tuesday, after its end.
        let deleted = vec![
            at("2026-10-26T09:00:00Z"),
            at("2026-11-10T09:00:00Z"),
            slot_at(&shape, 1),
            at("2026-12-21T09:00:00Z"),
        ];
        let fake = Fake::new(shape.clone(), vec![]);
        let created = run(&fake, series(deleted, false)).await.unwrap();
        assert_eq!(fake.kinds(), [Insert, List, List, Cancel, List]);
        assert_eq!(fake.cancelled_slots(), [shape[1]]);
        assert_eq!(created.recurrence.unwrap().exceptions, [slot_at(&shape, 1)]);
    }

    #[tokio::test]
    async fn deletions_only_in_the_past_create_the_series_as_any() {
        let fake = Fake::new(mondays(), vec![]);
        let created = run(&fake, series(vec![at("2026-10-12T09:00:00Z")], true))
            .await
            .unwrap();
        assert_eq!(fake.kinds(), [Insert]);
        let (path, body) = &fake.requests(Insert)[0];
        let body = body.as_ref().unwrap();
        assert!(body.get("id").is_none());
        assert!(!body.to_string().contains("EXDATE"));
        assert!(path.ends_with("sendUpdates=all"));
        assert!(created.recurrence.unwrap().exceptions.is_empty());
    }

    #[tokio::test]
    async fn a_deletion_earlier_today_is_kept() {
        let mut shape = mondays();
        shape.insert(0, Slot::At(at("2026-10-18T08:00:00Z")));
        let fake = Fake::new(shape.clone(), vec![]);
        let mut event = series(vec![at("2026-10-18T08:00:00Z")], false);
        event.start = at("2026-10-18T08:00:00Z");
        event.end = at("2026-10-18T09:00:00Z");
        run(&fake, event).await.unwrap();
        assert_eq!(fake.cancelled_slots(), [shape[0]]);
    }

    #[tokio::test]
    async fn a_rule_google_spells_otherwise_still_passes_and_a_line_of_its_own_does_not() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![]).with(|s| s.normalized = true);
        run(&fake, series(vec![slot_at(&shape, 1)], false))
            .await
            .unwrap();
        let fake = Fake::new(shape.clone(), vec![]).with(|s| s.extra_line = true);
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 1)], false)).await);
        assert!(undone);
        assert_eq!(detail, "control");
        assert!(fake.standing().is_none());
    }

    #[tokio::test]
    async fn a_series_that_ends_before_its_deletions_skips_them_all() {
        // Google lists nothing in the window; the series has occurrences.
        let fake = Fake::new(mondays(), vec![]);
        let created = run(
            &fake,
            series(
                vec![at("2027-03-01T09:00:00Z"), at("2027-03-08T09:00:00Z")],
                false,
            ),
        )
        .await
        .unwrap();
        assert_eq!(fake.kinds(), [Insert, List, List, List]);
        assert!(fake.requests(List)[2].0.contains("maxResults=1"));
        assert!(created.recurrence.unwrap().exceptions.is_empty());
        // Nothing listed at all: not to be told.
        let fake = Fake::new(vec![], vec![]);
        let (undone, detail) =
            not_kept(run(&fake, series(vec![at("2026-11-09T09:00:00Z")], false)).await);
        assert!(undone);
        assert_eq!(detail, "control");
    }

    #[tokio::test]
    async fn a_cancel_answered_with_less_is_taken_as_it_is() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::Bare)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
    }

    // ── failures ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_cancel_answered_as_another_one_takes_everything_back() {
        for fault in [Fault::OtherSeries, Fault::OtherSlot] {
            let shape = mondays();
            let fake = Fake::new(shape.clone(), vec![(Cancel, 0, fault.clone())]);
            let (undone, detail) =
                not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
            assert!(undone, "{fault:?}");
            assert_eq!(detail, "control", "{fault:?}");
            assert!(fake.standing().is_none());
        }
    }

    #[tokio::test]
    async fn an_answer_asking_to_slow_down_is_waited_out() {
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![
                (Cancel, 0, Fault::Busy429(Some(Duration::from_millis(30)))),
                (Cancel, 1, Fault::Busy403("rateLimitExceeded")),
                (List, 0, Fault::Busy403("userRateLimitExceeded")),
            ],
        );
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Cancel).len(), 3);
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
    }

    #[tokio::test]
    async fn a_quota_used_up_is_not_waited_out() {
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Busy403("quotaExceeded"))],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(undone);
        assert_eq!(detail, "google-quotaExceeded");
        assert_eq!(fake.requests(Cancel).len(), 1);
    }

    #[tokio::test]
    async fn slowing_down_past_the_budget_takes_back_in_time() {
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Busy429(Some(Duration::from_secs(30))))],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], true)).await);
        assert!(undone);
        // The provider was busy, not the run slow (276).
        assert_eq!(detail, "invited busy");
        // The take-back tells the invitees who heard of the series (212).
        assert!(fake.requests(Delete)[0].0.ends_with("sendUpdates=all"));
    }

    #[tokio::test]
    async fn waiting_half_the_time_for_a_busy_provider_names_it_when_time_runs_out() {
        // Two answers asking to wait 160 ms each, then a slow cancel: the run
        // is out of time at the next step, and half its 600 ms went to waiting.
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![
                (Cancel, 0, Fault::Busy429(Some(Duration::from_millis(160)))),
                (Cancel, 1, Fault::Busy429(Some(Duration::from_millis(160)))),
                (Cancel, 2, Fault::Slow(Duration::from_millis(200))),
            ],
        );
        let deleted = vec![slot_at(&shape, 1), slot_at(&shape, 3)];
        let (undone, detail) = not_kept(run(&fake, series(deleted, false)).await);
        assert!(undone);
        assert_eq!(detail, cal_core::PROVIDER_BUSY);
        // Without the waits, the same slow run is just out of time.
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Slow(Duration::from_millis(560)))],
        );
        let deleted = vec![slot_at(&shape, 1), slot_at(&shape, 3)];
        let (undone, detail) = not_kept(run(&fake, series(deleted, false)).await);
        assert!(undone);
        assert_eq!(detail, cal_core::OUT_OF_TIME);
    }

    #[tokio::test]
    async fn a_create_whose_answer_was_lost_is_read_back_by_its_id() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::LostAfter)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Insert).len(), 1, "never created twice");
        assert!(fake.requests(GetSeries)[0].0.ends_with(OWN_ID));
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
        // An answer no reader takes, the same.
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::Garbled)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Insert).len(), 1);
        // A server failing, the same: read back, not found, may stand.
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::Http(503))]);
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], true)).await);
        assert!(!undone);
        assert_eq!(detail, "invited create-answer-lost");
    }

    #[tokio::test]
    async fn a_create_lost_and_not_found_may_stand() {
        for notify in [false, true] {
            let shape = mondays();
            let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::LostBefore)]);
            let (undone, detail) =
                not_kept(run(&fake, series(vec![slot_at(&shape, 2)], notify)).await);
            assert!(!undone);
            assert_eq!(cal_core::step_detail_why(&detail), "create-answer-lost");
            assert_eq!(cal_core::step_detail_invited(&detail), notify);
            assert_eq!(fake.requests(Insert).len(), 1, "never created twice");
            assert_eq!(
                fake.requests(GetSeries).len() + fake.requests(GetOccurrence).len(),
                2
            );
        }
    }

    #[tokio::test]
    async fn a_create_that_never_left_or_was_turned_down_is_that_error() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::Unreached)]);
        let err = run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap_err();
        assert!(matches!(err, GoogleError::Unreached(_)), "{err:?}");
        assert_eq!(fake.kinds(), [Insert]);
        for status in [403, 413] {
            let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::Http(status))]);
            let err = run(&fake, series(vec![slot_at(&shape, 2)], false))
                .await
                .unwrap_err();
            assert!(
                matches!(err, GoogleError::Http { status: s, .. } if s == status),
                "{err:?}"
            );
            assert_eq!(fake.kinds(), [Insert]);
        }
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::RefreshRefused)]);
        let err = run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap_err();
        assert!(
            matches!(err, GoogleError::Http { status: 401, .. }),
            "{err:?}"
        );
        assert_eq!(fake.kinds(), [Insert]);
    }

    #[tokio::test]
    async fn a_create_asked_to_slow_down_is_sent_again_a_few_times() {
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![
                (Insert, 0, Fault::Busy429(None)),
                (Insert, 1, Fault::Busy403("rateLimitExceeded")),
            ],
        );
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Insert).len(), 3);
        let fake = Fake::new(
            shape.clone(),
            (0..3).map(|n| (Insert, n, Fault::Busy429(None))).collect(),
        );
        let err = run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap_err();
        assert!(
            matches!(err, GoogleError::Http { status: 429, .. }),
            "{err:?}"
        );
        assert_eq!(fake.requests(Insert).len(), 3);
    }

    #[tokio::test]
    async fn an_id_google_refuses_is_left_to_google() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Insert, 0, Fault::RefuseId)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        let inserts = fake.requests(Insert);
        assert_eq!(inserts.len(), 2);
        assert!(inserts[1].1.as_ref().unwrap().get("id").is_none());
        assert_eq!(fake.standing().unwrap().id, "googlechosen1");
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
    }

    #[tokio::test]
    async fn a_cancel_that_did_nothing_takes_everything_back() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::Pretend)]);
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(undone);
        assert_eq!(detail, "after-control");
        assert_eq!(
            fake.requests(List).len(),
            3,
            "one more listing before giving up"
        );
    }

    #[tokio::test]
    async fn a_listing_that_lags_once_is_read_again() {
        let shape = mondays();
        // The listing after the cancel shows the series as before it, once.
        let fake = Fake::new(shape.clone(), vec![(List, 1, Fault::StaleList)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(List).len(), 3);
        // Twice: the cancel did not hold.
        let fake = Fake::new(
            shape.clone(),
            vec![(List, 1, Fault::StaleList), (List, 2, Fault::StaleList)],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(undone);
        assert_eq!(detail, "after-control");
    }

    #[tokio::test]
    async fn someone_elses_cancel_takes_everything_back() {
        for hide in [false, true] {
            let shape = mondays();
            // Another occurrence in the window, cancelled by someone else.
            let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::Foreign(2))])
                .with(|s| s.hide_cancelled = hide);
            let deleted = vec![slot_at(&shape, 1), slot_at(&shape, 3)];
            let (undone, detail) = not_kept(run(&fake, series(deleted, false)).await);
            assert!(undone, "{hide}");
            assert_eq!(detail, "foreign-deletion", "{hide}");
        }
    }

    #[tokio::test]
    async fn a_cancel_whose_answer_was_lost_is_decided_by_the_occurrence() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::LostAfter)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Cancel).len(), 1, "no second cancel");
        assert_eq!(fake.requests(GetOccurrence).len(), 1);
        // Lost before it was made: cancelled once more.
        let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::LostBefore)]);
        run(&fake, series(vec![slot_at(&shape, 2)], false))
            .await
            .unwrap();
        assert_eq!(fake.requests(Cancel).len(), 2);
        assert_eq!(fake.cancelled_slots(), [shape[2]]);
    }

    #[tokio::test]
    async fn a_slow_cancel_is_cut_at_the_budget() {
        let shape = mondays();
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Slow(Duration::from_secs(5)))],
        );
        let started = std::time::Instant::now();
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert!(undone);
        assert_eq!(detail, "time");
    }

    #[tokio::test]
    async fn a_sign_in_refused_mid_run_takes_everything_back() {
        let shape = mondays();
        let fake = Fake::new(shape.clone(), vec![(Cancel, 0, Fault::RefreshRefused)]);
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(undone);
        assert_eq!(detail, "http-401");
    }

    #[tokio::test]
    async fn the_take_back_decides_what_stands() {
        let shape = mondays();
        // Its answer lost after the delete: the series reads deleted.
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Http(404)), (Delete, 0, Fault::LostAfter)],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], false)).await);
        assert!(undone);
        assert_eq!(detail, "http-404");
        assert!(fake.standing().is_none());
        // Turned down: it stands, with the invitees told of it (228).
        let fake = Fake::new(
            shape.clone(),
            vec![(Cancel, 0, Fault::Http(404)), (Delete, 0, Fault::Http(403))],
        );
        let (undone, detail) = not_kept(run(&fake, series(vec![slot_at(&shape, 2)], true)).await);
        assert!(!undone);
        assert_eq!(detail, "invited http-404");
        assert!(fake.standing().is_some());
    }

    /// Whatever goes wrong with whichever request, the outcome says what
    /// stands: never an error the shells read as "never made" while the series
    /// stands, never "taken back" while it stands.
    #[tokio::test]
    async fn every_fault_ends_in_what_stands() {
        let faults = || {
            vec![
                Fault::Busy429(None),
                Fault::Busy403("rateLimitExceeded"),
                Fault::Busy403("quotaExceeded"),
                Fault::Http(400),
                Fault::Http(404),
                Fault::Http(500),
                Fault::LostAfter,
                Fault::LostBefore,
                Fault::Unreached,
                Fault::Garbled,
                Fault::Pretend,
                Fault::Foreign(2),
                Fault::StaleList,
                Fault::OtherSeries,
                Fault::RefreshRefused,
            ]
        };
        let shape = mondays();
        let deleted = vec![slot_at(&shape, 1), slot_at(&shape, 3)];
        for kind in [Insert, List, Cancel, GetSeries, GetOccurrence, Delete] {
            for nth in 0..3 {
                for fault in faults() {
                    for notify in [false, true] {
                        let fake = Fake::new(shape.clone(), vec![(kind, nth, fault.clone())]);
                        let result = run(&fake, series(deleted.clone(), notify)).await;
                        let case = format!("{kind:?} {nth} {fault:?} {notify}");
                        let inserted = !fake.requests(Insert).is_empty();
                        match result {
                            Ok(created) => {
                                let standing = fake.standing().expect(&case);
                                assert!(!standing.deleted, "{case}");
                                // Exactly the deletions, cancelled.
                                assert_eq!(fake.cancelled_slots(), [shape[1], shape[3]], "{case}");
                                assert_eq!(
                                    created.recurrence.unwrap().exceptions,
                                    deleted,
                                    "{case}"
                                );
                            }
                            Err(GoogleError::NotKept { undone, detail }) => {
                                if undone {
                                    assert!(fake.standing().is_none(), "{case}");
                                }
                                // Invited exactly where a create told them.
                                assert_eq!(
                                    cal_core::step_detail_invited(&detail),
                                    notify && inserted,
                                    "{case}"
                                );
                            }
                            Err(err) => {
                                // Only the create's own refusal: nothing stands.
                                assert_eq!(kind, Insert, "{case}: {err:?}");
                                assert!(fake.standing().is_none(), "{case}: {err:?}");
                            }
                        }
                    }
                }
            }
        }
    }
}
