//! Event-log applier — the consumer side of Phase Sc.
//!
//! Reads [`sync_core::EventEnvelope`]s out of a [`LogFile`]
//! (typically one fetched from a remote sync adapter), and
//! integrates them into the local SQLite cache. The pipeline is:
//!
//! ```text
//!   LogFile bytes ──► EventEnvelope[]
//!                     │
//!                     ▼  sort chronologically
//!                     │
//!                     ├─ skip envelopes from this device's id (we
//!                     │  already applied them when we minted them)
//!                     ├─ skip envelopes whose `id` is already in
//!                     │  sync_applied_events (idempotency)
//!                     │
//!                     ▼  dispatch on SyncEvent variant
//!                     │
//!         ┌───────────┼───────────────┐
//!         ▼           ▼               ▼
//!     events/      tasks/         color_labels/
//!     calendars    task_lists     settings (user_prefs)
//!         │           │               │
//!         └─ upsert helpers on LocalAdapter (`*_from_sync`)
//!         └─ idempotent INSERT OR DO UPDATE
//!         └─ record event_id in sync_applied_events
//! ```
//!
//! ## What the applier deliberately does NOT do (yet)
//!
//! - **Field-level merge with conflict detection.** Phase Sb's
//!   writer emits the full row on update, so the applier
//!   currently does last-write-wins by event timestamp. Real
//!   diff-based merge + conflict surfacing lands with the
//!   conflict-UI work in Phase Sh.
//! - **Cross-device cursor management.** That's the sync
//!   scheduler's job (Phase Se / Sd) — knowing which files we
//!   already fetched. The applier just acts on whatever envelope
//!   list it's handed.
//! - **Snapshot application.** Snapshots are a Phase-Sg artefact;
//!   the applier handles the per-event path only.
//! - **plugin.* / shortcut.*** event variants. Phase Sb defines
//!   them in the SyncEvent enum but doesn't yet emit them
//!   (plugin manager + shortcut overrides aren't wired). The
//!   applier mirrors that — those variants log a debug line and
//!   no-op rather than failing, so a forward-compat dataset
//!   from a future Aperio doesn't break us.

use std::sync::Arc;

use adapter_local::LocalAdapter;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sync_core::{
    AccountPayload, CredentialPayload, CredentialSlotPayload, DeviceId, EventEnvelope,
    EventPayload, IdPayload, LogFile, PluginPayload, SettingsPayload, SyncError, SyncEvent,
    SyncResult,
};
use tracing::{debug, warn};

use crate::{
    ConflictKind, NewConflict, SecretSlot, SecretStore, SnapshotAccount, StoreError, SyncStore,
};

/// Fields the merge path treats as metadata — never user-surfaced
/// as conflicts. `updated_at` and `created_at` diverge mechanically
/// every time someone edits the row; `etag` is the remote
/// provider's bookkeeping. Showing the user a "your `etag` differs
/// from their `etag`" dialog would be noise.
const METADATA_FIELDS: &[&str] = &["updated_at", "created_at", "etag"];

/// Round-trip a `recurrence` field value through its typed model so two
/// semantically-identical recurrences that serialize differently compare equal.
/// The §9.12 `#[serde(default)]` axes (anchor/placement/fixed_dates) are always
/// emitted by the current model but omitted by older payloads; without this,
/// "default present" vs "absent" raises a spurious conflict. Returns the value
/// unchanged if it can't be parsed as the expected type (so a genuinely
/// malformed/foreign value still compares by its raw form).
fn canonicalize_recurrence(kind: ConflictKind, value: &Value) -> Value {
    let canon = match kind {
        ConflictKind::Task => {
            serde_json::from_value::<Option<cal_core::TaskRecurrence>>(value.clone())
                .ok()
                .and_then(|r| serde_json::to_value(r).ok())
        }
        ConflictKind::Event => {
            serde_json::from_value::<Option<cal_core::EventRecurrence>>(value.clone())
                .ok()
                .and_then(|r| serde_json::to_value(r).ok())
        }
        _ => None,
    };
    canon.unwrap_or_else(|| value.clone())
}

/// Whether a STORED conflict is still a genuine field difference under the
/// CURRENT comparison rules. The live merge path (below) already skips metadata
/// fields and canonicalizes `recurrence` before raising a conflict — but it only
/// guards NEW envelopes, so a conflict RECORDED by an older build (before those
/// rules existed) lingers in `sync_conflicts` forever even after the user
/// updates. This is the retroactive counterpart: `false` means the stored
/// conflict is spurious under today's rules and can be auto-resolved. Mirrors the
/// exact predicates the merge loop uses, so the two never disagree.
pub fn conflict_still_genuine(
    kind: ConflictKind,
    field: &str,
    local: &Value,
    remote: &Value,
) -> bool {
    if METADATA_FIELDS.contains(&field) {
        return false;
    }
    if field == "recurrence" {
        return canonicalize_recurrence(kind, local) != canonicalize_recurrence(kind, remote);
    }
    local != remote
}

/// Per-call summary the applier hands back so callers (the sync
/// scheduler, settings dialog "Reapply log" actions, tests) can
/// surface what happened without grovelling through tracing
/// output.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// Envelopes whose `event` actually wrote to SQLite.
    pub applied: usize,
    /// Envelopes whose `device_id` matched the local one — we
    /// minted them and applied them at write time, so the
    /// loopback pass through the applier is a no-op.
    pub skipped_own: usize,
    /// Envelopes whose `id` was already in
    /// `sync_applied_events`. Re-fetches and overlapping log
    /// files both end up here.
    pub skipped_already_applied: usize,
    /// Variants we don't have a handler for yet (plugin.*,
    /// shortcut.*). Logged at debug.
    pub skipped_unsupported: usize,
    /// Per-envelope failures. Doesn't sink the run — we keep
    /// applying the remaining envelopes; a single bad row
    /// shouldn't strand a 1000-row log file. The number here
    /// signals whether we should warn the user that some data
    /// didn't make it.
    pub failed: usize,
    /// The `user_prefs` keys this pass actually wrote (DESIGN.md §19.2.1
    /// settings). Named rather than counted, because the frontend needs to
    /// know WHICH setting arrived: a running app holds these in memory and
    /// would otherwise show a value another device has already changed until
    /// the next launch. Empty for the overwhelming majority of rounds.
    pub settings_keys: Vec<String>,
    /// Whether this pass wrote anything day-marker shaped — the vocabulary or
    /// a day's log. A flag rather than a list of ids because every reader of
    /// this data reads all of it: the vocabulary is a handful of rows and a
    /// view's summaries come from one range query, so "something moved" is the
    /// whole of what a frontend needs to act.
    ///
    /// Same reason as `settings_keys`: both frontends hold this in memory, and
    /// without the signal a day ticked on the phone would sit in SQLite while
    /// the desktop went on showing an unmarked day until the next launch.
    pub day_markers_touched: bool,
    /// Field-level conflicts the applier wrote into
    /// `sync_conflicts` during this pass (DESIGN.md §19.3).
    /// Surfaced through `SyncRoundReport` so the scheduler can
    /// fire a §19.9 system notification when the count goes up
    /// in a single round — the user shouldn't have to
    /// rediscover unresolved conflicts by checking the status
    /// indicator after a quiet sync.
    pub conflicts: usize,
}

/// The applier itself.
pub struct EventLogApplier {
    /// Local store seam — idempotency (`sync_applied_events`), conflict
    /// recording, settings, accounts and remote-plugin announcements.
    store: Arc<dyn SyncStore>,
    /// Credential store seam — used by the `credential.*` handlers to
    /// write/clear synced secrets in the platform keychain.
    secrets: Arc<dyn SecretStore>,
    /// Reference to the local adapter so we can call its
    /// `*_from_sync` upsert helpers. Wrapped in `Arc` because the
    /// applier is constructed once and held by the sync scheduler.
    adapter: Arc<LocalAdapter>,
    /// This device's id. Envelopes carrying this id originated
    /// here and have already been applied locally during their
    /// emit — skipping them in the applier prevents re-running
    /// the same insert.
    local_device_id: DeviceId,
    /// Per-`apply_log_file` conflict counter. Reset to 0 on
    /// entry to `apply_envelopes`; bumped by `merge_fields` on
    /// every successful `repo.record(...)`. Read into the
    /// `ApplyReport` before returning.
    ///
    /// Interior mutability via `AtomicUsize` lets `merge_fields`
    /// stay `&self`, avoiding a refactor of the whole apply
    /// dispatch to thread mutable counters through. The
    /// `InFlightGuard` on the orchestrator guarantees only one
    /// `apply_envelopes` runs at a time, so the relaxed
    /// ordering is sufficient — we don't actually rely on
    /// cross-thread synchronisation.
    pending_conflicts: std::sync::atomic::AtomicUsize,
}

impl EventLogApplier {
    pub fn new(
        store: Arc<dyn SyncStore>,
        secrets: Arc<dyn SecretStore>,
        adapter: Arc<LocalAdapter>,
        local_device_id: DeviceId,
    ) -> Self {
        Self {
            store,
            secrets,
            adapter,
            local_device_id,
            pending_conflicts: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Apply every envelope in one log file. Convenience wrapper
    /// around `apply_envelopes` that handles the JSONL → envelope
    /// decoding via `LogFile::into_envelopes`.
    pub fn apply_log_file(&self, log: &LogFile) -> SyncResult<ApplyReport> {
        let envelopes = log.into_envelopes()?;
        self.apply_envelopes(envelopes)
    }

    /// Apply a batch of envelopes. The caller can hand in
    /// envelopes from any source (one log file, several, or a
    /// snapshot's appended tail) — the applier sorts them by
    /// (timestamp, id) before dispatching, so the same input set
    /// produces the same final state regardless of source order.
    pub fn apply_envelopes(&self, envelopes: Vec<EventEnvelope>) -> SyncResult<ApplyReport> {
        self.apply_envelopes_inner(envelopes, false)
    }

    /// Like [`apply_envelopes`] but ignores the local-device-id
    /// filter — every envelope goes through the apply pipeline
    /// even if it was minted by this device.
    ///
    /// Normal sync rounds always skip own-device envelopes
    /// because they were already applied at write time. The
    /// §19.10 stale-resume flow needs the opposite: after a
    /// snapshot apply has overwritten local SQLite to the
    /// snapshot state, our own pending logs (which carry edits
    /// the user made while offline) must be replayed to bring
    /// those edits back. The applier's existing field-level
    /// merge — using `local_updated_at vs env.timestamp` —
    /// handles the snapshot-vs-edit ordering correctly without
    /// any special "force" semantics.
    ///
    /// Callers MUST limit this to the stale-resume path. Using
    /// it during a steady-state round would re-apply own events
    /// twice (once at write time, once via the applier) and
    /// could trigger spurious conflicts via the merge_fields
    /// timestamp comparison.
    pub fn apply_envelopes_force_own(
        &self,
        envelopes: Vec<EventEnvelope>,
    ) -> SyncResult<ApplyReport> {
        self.apply_envelopes_inner(envelopes, true)
    }

    fn apply_envelopes_inner(
        &self,
        mut envelopes: Vec<EventEnvelope>,
        force_own: bool,
    ) -> SyncResult<ApplyReport> {
        // Reset the per-pass conflict counter. `merge_fields`
        // bumps it on every recorded conflict; we fold the final
        // value into the report below.
        self.pending_conflicts
            .store(0, std::sync::atomic::Ordering::Relaxed);
        // Chronological order. ULID-prefixed ids sort
        // lexicographically by timestamp too — so the secondary
        // key is just for ties at the same wall-clock moment.
        envelopes.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));

        let mut report = ApplyReport::default();
        for env in envelopes {
            if !force_own && env.device_id == self.local_device_id {
                report.skipped_own += 1;
                continue;
            }
            match self.is_already_applied(&env.id) {
                Ok(true) => {
                    report.skipped_already_applied += 1;
                    continue;
                }
                Ok(false) => {}
                Err(err) => {
                    warn!(
                        event_id = %env.id,
                        ?err,
                        "could not check sync_applied_events; skipping envelope",
                    );
                    report.failed += 1;
                    continue;
                }
            }

            match self.apply_one(&env) {
                Ok(true) => {
                    // Mark as applied. If marking itself fails we
                    // log + count as `failed` so callers know to
                    // raise a "sync state inconsistent" alert —
                    // re-running the applier would re-apply the
                    // event, which our handlers tolerate (idempotent
                    // upserts) but it's still worth visibility.
                    if let Err(err) = self.mark_applied(&env.id) {
                        warn!(
                            event_id = %env.id,
                            ?err,
                            "applied envelope but couldn't write idempotency row",
                        );
                        report.failed += 1;
                    } else {
                        report.applied += 1;
                        // Remember the key of a settings write. Collected
                        // here rather than inside the handler so the dispatch
                        // keeps its plain `Ok(bool)` shape, and only for
                        // envelopes that actually landed — an event that was
                        // skipped or failed changed nothing to re-read.
                        if let SyncEvent::SettingsUpdated(payload) = &env.event {
                            report.settings_keys.push(payload.key.clone());
                        }
                        if matches!(
                            &env.event,
                            SyncEvent::DayMarkerWritten(_)
                                | SyncEvent::DayMarkerDeleted(_)
                                | SyncEvent::DayLogSet(_)
                        ) {
                            report.day_markers_touched = true;
                        }
                    }
                }
                Ok(false) => {
                    report.skipped_unsupported += 1;
                }
                Err(err) => {
                    warn!(
                        event_id = %env.id,
                        device_id = %env.device_id,
                        ?err,
                        "failed to apply envelope; skipping",
                    );
                    report.failed += 1;
                }
            }
        }
        // Fold the per-pass conflict counter into the report so
        // the orchestrator can decide whether to fire a §19.9
        // system notification after the round.
        report.conflicts = self
            .pending_conflicts
            .load(std::sync::atomic::Ordering::Relaxed);
        Ok(report)
    }

    /// Dispatch one envelope to its variant handler. Returns
    /// `Ok(true)` when something was actually applied,
    /// `Ok(false)` when the variant is one we don't handle yet
    /// (plugin.*, shortcut.*), `Err` on a real failure.
    fn apply_one(&self, env: &EventEnvelope) -> SyncResult<bool> {
        match &env.event {
            SyncEvent::EventCreated(payload) => {
                self.apply_event_upsert(payload)?;
                Ok(true)
            }
            // Phase Sh: `*Updated` events go through the field-level
            // merge path. The envelope timestamp + device id steer
            // conflict detection.
            SyncEvent::EventUpdated(payload) => {
                self.apply_event_merge(payload, env)?;
                Ok(true)
            }
            SyncEvent::EventDeleted(payload) => {
                self.apply_event_delete(payload)?;
                Ok(true)
            }
            SyncEvent::TaskCreated(payload) => {
                self.apply_task_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::TaskUpdated(payload) => {
                self.apply_task_merge(payload, env)?;
                Ok(true)
            }
            SyncEvent::TaskDeleted(payload) => {
                self.apply_task_delete(payload)?;
                Ok(true)
            }
            SyncEvent::TaskListCreated(payload) => {
                self.apply_task_list_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::TaskListUpdated(payload) => {
                self.apply_task_list_merge(payload, env)?;
                Ok(true)
            }
            SyncEvent::TaskListDeleted(payload) => {
                self.apply_task_list_delete(payload)?;
                Ok(true)
            }
            // Sections are simple metadata: both create and update take
            // the full row last-write-wins (no field-level merge path).
            SyncEvent::SectionCreated(payload) | SyncEvent::SectionUpdated(payload) => {
                self.apply_section_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::SectionDeleted(payload) => {
                self.apply_section_delete(payload)?;
                Ok(true)
            }
            SyncEvent::CalendarCreated(payload) => {
                self.apply_calendar_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::CalendarUpdated(payload) => {
                self.apply_calendar_merge(payload, env)?;
                Ok(true)
            }
            SyncEvent::CalendarDeleted(payload) => {
                self.apply_calendar_delete(payload)?;
                Ok(true)
            }
            SyncEvent::EventGroupUpdated(payload) => {
                self.apply_event_group_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::EventGroupSuggestionDeclined(payload) => {
                let decline: cal_core::SuggestionDecline =
                    serde_json::from_value(payload.fields.clone()).map_err(|err| {
                        SyncError::protocol(format!("suggestion decline payload not valid: {err}",))
                    })?;
                self.adapter
                    .upsert_suggestion_decline_from_sync(&decline)
                    .map_err(core_to_sync)?;
                Ok(true)
            }
            SyncEvent::EventLocalRemindersSet(payload) => {
                let row: cal_core::EventLocalReminders =
                    serde_json::from_value(payload.fields.clone()).map_err(|err| {
                        SyncError::protocol(format!("local reminders payload not valid: {err}"))
                    })?;
                self.adapter
                    .upsert_event_local_reminders_from_sync(&row)
                    .map_err(core_to_sync)?;
                Ok(true)
            }
            SyncEvent::EventGroupDissolved(payload) => {
                // The envelope's timestamp, not ours: it says WHEN the other
                // device decided, which is what an arriving update has to be
                // compared against.
                self.adapter
                    .delete_event_group_from_sync(&payload.id, &env.timestamp.to_rfc3339())
                    .map_err(core_to_sync)?;
                Ok(true)
            }
            SyncEvent::ColorLabelCreated(payload) => {
                self.apply_color_label_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::ColorLabelUpdated(payload) => {
                self.apply_color_label_merge(payload, env)?;
                Ok(true)
            }
            SyncEvent::ColorLabelDeleted(payload) => {
                self.apply_color_label_delete(payload)?;
                Ok(true)
            }
            SyncEvent::DayMarkerWritten(payload) => {
                self.apply_day_marker_written(payload)?;
                Ok(true)
            }
            SyncEvent::DayMarkerDeleted(payload) => {
                self.apply_day_marker_deleted(payload)?;
                Ok(true)
            }
            SyncEvent::DayLogSet(payload) => {
                self.apply_day_log_set(payload)?;
                Ok(true)
            }
            SyncEvent::SettingsUpdated(payload) => {
                self.apply_settings_updated(payload)?;
                Ok(true)
            }
            SyncEvent::PluginInstalled(payload) | SyncEvent::PluginUpdated(payload) => {
                self.apply_plugin_announcement(payload, env)?;
                Ok(true)
            }
            SyncEvent::PluginUninstalled(payload) => {
                self.apply_plugin_uninstall(payload)?;
                Ok(true)
            }
            SyncEvent::AccountCreated(payload) | SyncEvent::AccountUpdated(payload) => {
                self.apply_account_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::AccountDeleted(payload) => {
                self.apply_account_delete(payload)?;
                Ok(true)
            }
            SyncEvent::CredentialSet(payload) => {
                self.apply_credential_set(payload)?;
                Ok(true)
            }
            SyncEvent::CredentialCleared(payload) => {
                self.apply_credential_clear(payload)?;
                Ok(true)
            }
            SyncEvent::ShortcutSet(_)
            | SyncEvent::ShortcutReset(_)
            | SyncEvent::ShortcutCleared(_) => {
                // Forward-compat: variants without local handlers
                // log + skip. Once the shortcut overrides land
                // they'll grow handlers here.
                debug!(
                    event_id = %env.id,
                    "skipping envelope: variant not handled yet",
                );
                Ok(false)
            }
        }
    }

    fn apply_event_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let event: cal_core::Event =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!("event upsert payload not a valid Event: {err}",))
            })?;
        // Pin the wire id even if the deserialised payload's
        // `id` differs (defensive: shouldn't happen, but the
        // envelope's `id` is the canonical one for sync).
        let mut event = event;
        event.id = payload.id.clone();
        self.adapter
            .upsert_event_from_sync(&event)
            .map_err(core_to_sync)
    }

    fn apply_event_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_event_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_task_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let task: cal_core::Task =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!("task upsert payload not a valid Task: {err}",))
            })?;
        let mut task = task;
        task.id = payload.id.clone();
        self.adapter
            .upsert_task_from_sync(&task)
            .map_err(core_to_sync)
    }

    fn apply_task_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_task_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_task_list_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let list: cal_core::TaskList =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "task_list upsert payload not a valid TaskList: {err}",
                ))
            })?;
        let mut list = list;
        list.id = payload.id.clone();
        self.adapter
            .upsert_task_list_from_sync(&list)
            .map_err(core_to_sync)
    }

    fn apply_task_list_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_task_list_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_section_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let section: cal_core::Section =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!("section upsert payload not a valid Section: {err}",))
            })?;
        let mut section = section;
        section.id = payload.id.clone();
        self.adapter
            .upsert_section_from_sync(&section)
            .map_err(core_to_sync)
    }

    fn apply_section_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_section_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_calendar_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let cal: cal_core::Calendar =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "calendar upsert payload not a valid Calendar: {err}",
                ))
            })?;
        let mut cal = cal;
        cal.id = payload.id.clone();
        self.adapter
            .upsert_calendar_from_sync(&cal)
            .map_err(core_to_sync)
    }

    fn apply_calendar_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_calendar_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_event_group_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let group: cal_core::EventGroup =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "event_group upsert payload not a valid EventGroup: {err}",
                ))
            })?;
        self.adapter
            .upsert_event_group_from_sync(&group)
            .map_err(core_to_sync)
    }

    fn apply_color_label_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let label: cal_core::ColorLabel =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "color_label upsert payload not a valid ColorLabel: {err}",
                ))
            })?;
        self.adapter
            .upsert_color_label_from_sync(&label)
            .map_err(core_to_sync)
    }

    fn apply_color_label_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_color_label_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    /// A day-marker vocabulary entry, written whole.
    ///
    /// Whole-row rather than a field merge: a marker is four small fields the
    /// user edits together in one dialog, and the merge machinery next door
    /// exists for rows several devices touch different parts of. This is not
    /// one of those.
    fn apply_day_marker_written(&self, payload: &EventPayload) -> SyncResult<()> {
        let marker: cal_core::DayMarker =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!("day_marker payload not a valid DayMarker: {err}",))
            })?;
        self.adapter.write_day_marker(&marker).map_err(core_to_sync)
    }

    fn apply_day_marker_deleted(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_day_marker(&payload.id)
            .map_err(core_to_sync)
    }

    /// One day's log, written whole and keyed by the day.
    ///
    /// A log that arrives with nothing on it means the day was emptied — the
    /// store's own `set_day_log` deletes the row for exactly that case, so no
    /// separate deletion event is needed.
    fn apply_day_log_set(&self, payload: &EventPayload) -> SyncResult<()> {
        let log: cal_core::DayLog =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!("day_log payload not a valid DayLog: {err}"))
            })?;
        self.adapter.set_day_log(&log).map_err(core_to_sync)
    }

    // -----------------------------------------------------------------
    // Phase Sh: field-level merge handlers for `*Updated` variants.
    //
    // Each handler:
    //   1. Loads the live local row.
    //   2. If absent: falls through to the full-row upsert path —
    //      the patch is treated as the initial seed.
    //   3. Otherwise computes a per-field merge against the patch:
    //      - Fields where both sides agree: no-op.
    //      - Fields where only one side changed: auto-merge — take
    //        whichever value differs from the merged baseline.
    //      - Fields where local was modified after the envelope's
    //        timestamp AND values diverge: record a conflict and
    //        keep the local value pending user resolution.
    //   4. Writes the merged row back via the same `upsert_*_from_sync`
    //      helper the full-row path uses, so cascading FKs are
    //      preserved exactly as before.
    //
    // The §19.3 design promises field-level merge "when possible";
    // this heuristic is the conservative pass. True concurrent edits
    // (both devices touch the same field within a small window
    // without seeing each other) still resolve last-write-wins
    // because we lack vector clocks here — see §19.3 caveats. The
    // common single-user-on-multiple-devices pattern (where one
    // device's edit travels first, then the other device edits the
    // same row) IS caught.
    // -----------------------------------------------------------------

    fn apply_event_merge(&self, payload: &EventPayload, env: &EventEnvelope) -> SyncResult<()> {
        let local = self
            .adapter
            .get_event_by_id(&payload.id)
            .map_err(core_to_sync)?;
        let Some(local) = local else {
            return self.apply_event_upsert(payload);
        };
        let merged = self.merge_fields(
            &local,
            &payload.fields,
            local.updated_at,
            env,
            ConflictKind::Event,
            &payload.id,
        )?;
        let mut event: cal_core::Event = serde_json::from_value(merged).map_err(|err| {
            SyncError::protocol(format!("merged event row not deserialisable: {err}",))
        })?;
        event.id = payload.id.clone();
        self.adapter
            .upsert_event_from_sync(&event)
            .map_err(core_to_sync)
    }

    fn apply_task_merge(&self, payload: &EventPayload, env: &EventEnvelope) -> SyncResult<()> {
        let local = self
            .adapter
            .get_task_by_id(&payload.id)
            .map_err(core_to_sync)?;
        let Some(local) = local else {
            return self.apply_task_upsert(payload);
        };
        let merged = self.merge_fields(
            &local,
            &payload.fields,
            local.updated_at,
            env,
            ConflictKind::Task,
            &payload.id,
        )?;
        let mut task: cal_core::Task = serde_json::from_value(merged).map_err(|err| {
            SyncError::protocol(format!("merged task row not deserialisable: {err}",))
        })?;
        task.id = payload.id.clone();
        self.adapter
            .upsert_task_from_sync(&task)
            .map_err(core_to_sync)
    }

    fn apply_task_list_merge(&self, payload: &EventPayload, env: &EventEnvelope) -> SyncResult<()> {
        let local = self
            .adapter
            .get_task_list_by_id(&payload.id)
            .map_err(core_to_sync)?;
        let Some(local) = local else {
            return self.apply_task_list_upsert(payload);
        };
        // TaskList has no `updated_at` column. The merge still runs
        // — auto-merge for fields only one side changed — but
        // conflict detection falls back to "value differs" without
        // a temporal anchor. That over-flags on rare TaskList
        // edits; acceptable trade-off for v1.
        let merged = self.merge_fields(
            &local,
            &payload.fields,
            env.timestamp,
            env,
            ConflictKind::TaskList,
            &payload.id,
        )?;
        let mut list: cal_core::TaskList = serde_json::from_value(merged).map_err(|err| {
            SyncError::protocol(format!("merged task_list row not deserialisable: {err}",))
        })?;
        list.id = payload.id.clone();
        self.adapter
            .upsert_task_list_from_sync(&list)
            .map_err(core_to_sync)
    }

    fn apply_calendar_merge(&self, payload: &EventPayload, env: &EventEnvelope) -> SyncResult<()> {
        let local = self
            .adapter
            .get_calendar_by_id(&payload.id)
            .map_err(core_to_sync)?;
        let Some(local) = local else {
            return self.apply_calendar_upsert(payload);
        };
        let merged = self.merge_fields(
            &local,
            &payload.fields,
            env.timestamp,
            env,
            ConflictKind::Calendar,
            &payload.id,
        )?;
        let mut cal: cal_core::Calendar = serde_json::from_value(merged).map_err(|err| {
            SyncError::protocol(format!("merged calendar row not deserialisable: {err}",))
        })?;
        cal.id = payload.id.clone();
        self.adapter
            .upsert_calendar_from_sync(&cal)
            .map_err(core_to_sync)
    }

    fn apply_color_label_merge(
        &self,
        payload: &EventPayload,
        env: &EventEnvelope,
    ) -> SyncResult<()> {
        // Try to extract the id from the payload — color labels use
        // the wire id from the payload directly since `EventPayload`
        // requires it.
        let local = self
            .adapter
            .get_color_label_by_id(&payload.id)
            .map_err(core_to_sync)?;
        let Some(local) = local else {
            return self.apply_color_label_upsert(payload);
        };
        let merged = self.merge_fields(
            &local,
            &payload.fields,
            env.timestamp,
            env,
            ConflictKind::ColorLabel,
            &payload.id,
        )?;
        let label: cal_core::ColorLabel = serde_json::from_value(merged).map_err(|err| {
            SyncError::protocol(format!("merged color_label row not deserialisable: {err}",))
        })?;
        self.adapter
            .upsert_color_label_from_sync(&label)
            .map_err(core_to_sync)
    }

    /// Compute a field-level merge of `patch` over the serialised
    /// `local` row. Returns the merged JSON; side-effect records
    /// conflicts to the `sync_conflicts` table.
    ///
    /// The `local_updated_at` argument is the live row's
    /// `updated_at` (or a stand-in like `env.timestamp` for tables
    /// without one). It steers the per-field "did local change
    /// after the remote?" decision.
    fn merge_fields<L: Serialize>(
        &self,
        local: &L,
        patch: &Value,
        local_updated_at: DateTime<Utc>,
        env: &EventEnvelope,
        kind: ConflictKind,
        row_id: &str,
    ) -> SyncResult<Value> {
        let local_val = serde_json::to_value(local)
            .map_err(|err| SyncError::internal(format!("serialise local row: {err}")))?;
        let Some(patch_obj) = patch.as_object() else {
            // Patch isn't a JSON object — fall back to the patch
            // verbatim. The applier's upsert path will fail the
            // deserialise step downstream and surface a clear
            // protocol error.
            return Ok(patch.clone());
        };
        let mut merged = local_val.as_object().cloned().unwrap_or_default();
        for (field, patch_val) in patch_obj {
            // Skip the row id — it's pinned by the envelope and the
            // upsert helper overrides any payload-side value.
            if field == "id" {
                continue;
            }
            let local_field = merged.get(field).cloned().unwrap_or(Value::Null);
            if local_field == *patch_val {
                continue; // already aligned, no-op
            }
            // The `recurrence` field carries `#[serde(default)]` axes
            // (anchor/placement/fixed_dates, §9.12) that the CURRENT model always
            // serializes but older payloads omit. A raw-JSON compare then reads
            // "key present at its default" vs "key absent" as a difference and
            // raises a SPURIOUS conflict for a recurrence that is in fact
            // identical. Compare the typed values so a serialization difference
            // alone can't conflict.
            if field == "recurrence"
                && canonicalize_recurrence(kind, &local_field)
                    == canonicalize_recurrence(kind, patch_val)
            {
                continue;
            }
            // Metadata fields (`updated_at`, `created_at`, `etag`) are
            // bookkeeping — silently take the remote so the merged
            // row is consistent. They're never user-surfaced as
            // conflicts.
            if METADATA_FIELDS.contains(&field.as_str()) {
                merged.insert(field.clone(), patch_val.clone());
                continue;
            }
            if local_updated_at > env.timestamp {
                // Local was edited AFTER the remote envelope was
                // minted — divergent timelines on this field.
                // Record a conflict, keep local value.
                let new_conflict = NewConflict {
                    row_kind: kind,
                    row_id: row_id.to_string(),
                    field: field.clone(),
                    local_value: serde_json::to_string(&local_field).ok(),
                    remote_value: serde_json::to_string(patch_val).ok(),
                    remote_device_id: env.device_id.as_str().to_string(),
                    remote_timestamp: env.timestamp,
                };
                match self.store.record_conflict(&new_conflict) {
                    Ok(()) => {
                        // Bump the per-pass counter; the
                        // outer `apply_envelopes` folds the
                        // total into the ApplyReport.
                        self.pending_conflicts
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(err) => {
                        warn!(
                            field = %field,
                            ?err,
                            "couldn't persist sync conflict; falling back to last-write-wins for this field",
                        );
                        // Don't let a conflict-table write failure
                        // block the merge — better to apply silently
                        // than to stall the sync. The local row stays
                        // as it was.
                    }
                }
                // Keep local value (merged already holds it).
            } else {
                // Auto-merge: remote is newer for this field.
                merged.insert(field.clone(), patch_val.clone());
            }
        }
        Ok(Value::Object(merged))
    }

    /// Settings live in `user_prefs`. Phase Sb's whitelist
    /// already gates which keys propagate; the applier writes
    /// whatever it receives (the whitelist on the writer side is
    /// the producer's responsibility, not the consumer's). Value
    /// = JSON null encodes "delete the row" — see the
    /// `delete_user_pref` hook for the symmetric write side.
    fn apply_settings_updated(&self, payload: &SettingsPayload) -> SyncResult<()> {
        if payload.value.is_null() {
            self.store.delete_pref(&payload.key).map_err(|err| {
                SyncError::internal(format!(
                    "user_prefs delete failed for {}: {err}",
                    payload.key,
                ))
            })?;
        } else {
            // Encode the JSON value back to a string. The
            // user_prefs table holds opaque strings; the
            // frontend re-parses on read. A bare string value
            // round-trips as a JSON-quoted string ("foo") to
            // match the writer's emission semantics — the writer
            // tries `from_str` first so this re-quoting stays
            // symmetric.
            let stored = match &payload.value {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other)?,
            };
            self.store.set_pref(&payload.key, &stored).map_err(|err| {
                SyncError::internal(format!("user_prefs set failed for {}: {err}", payload.key,))
            })?;
        }
        Ok(())
    }

    /// Mirror a remote plugin announcement (`plugin.installed`
    /// or `plugin.updated`) into the local `remote_plugins`
    /// table. The Settings → Plugins panel reads from there to
    /// render the "Plugin benötigt" section (DESIGN.md §20.8).
    /// We unconditionally upsert; the panel decides whether to
    /// show the row by cross-checking against the local
    /// PluginManager's list.
    fn apply_plugin_announcement(
        &self,
        payload: &PluginPayload,
        env: &EventEnvelope,
    ) -> SyncResult<()> {
        self.store
            .upsert_remote_plugin(
                &payload.id,
                payload.name.as_deref(),
                &payload.version,
                payload.plugin_type.as_deref(),
                payload.source.as_deref(),
                env.device_id.as_str(),
            )
            .map_err(|err| {
                SyncError::internal(format!("remote_plugins upsert for {}: {err}", payload.id,))
            })
    }

    /// Drop a remote plugin announcement when the
    /// corresponding `plugin.uninstalled` event arrives.
    fn apply_plugin_uninstall(&self, payload: &IdPayload) -> SyncResult<()> {
        self.store.delete_remote_plugin(&payload.id).map_err(|err| {
            SyncError::internal(format!("remote_plugins delete for {}: {err}", payload.id,))
        })
    }

    /// Insert-or-update the `accounts` row mirroring a
    /// `account.created` / `account.updated` event from another
    /// device. Reuses the store's `upsert_account` (the same upsert the
    /// snapshot path uses). Secrets are NOT included in the payload; the
    /// receiving device surfaces the existing
    /// `list_accounts_missing_credentials` wizard for the user to enter
    /// them locally. The implicit `local` account is skipped so a stray
    /// event from a peer can't overwrite its bootstrap timestamps.
    fn apply_account_upsert(&self, payload: &AccountPayload) -> SyncResult<()> {
        // Both halves of the same rule. The id check protects the built-in
        // store's bootstrap row; the kind check refuses anything that describes
        // one machine — a peer's device calendar has no meaning here, and
        // applying it creates an account whose plugin will never load.
        if payload.id == "local" || sync_core::event::is_host_internal_kind(&payload.adapter_kind) {
            return Ok(());
        }
        let account = SnapshotAccount {
            id: payload.id.clone(),
            adapter_kind: payload.adapter_kind.clone(),
            display_name: payload.display_name.clone(),
            config_json: payload.config_json.clone(),
            created_at: payload.created_at.clone(),
            updated_at: payload.updated_at.clone(),
        };
        self.store.upsert_account(&account).map_err(|err| {
            SyncError::internal(format!("accounts upsert for {}: {err}", payload.id,))
        })
    }

    /// Remove the `accounts` row matching an `account.deleted`
    /// event. Refuses to touch `local` for the same reason
    /// `apply_account_upsert` does. Secret cleanup is a host
    /// concern (we don't have keychain access at this layer);
    /// the originating device already cleaned its own keychain
    /// in `delete_account`.
    fn apply_account_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        if payload.id == "local" {
            return Ok(());
        }
        self.store.delete_account(&payload.id).map_err(|err| {
            SyncError::internal(format!("accounts delete for {}: {err}", payload.id,))
        })
    }

    /// Apply a `credential.set` event by writing the synced secret into
    /// the local keychain. Only reached for events that arrived inside an
    /// E2E-encrypted log — a plaintext log can't legitimately carry these
    /// (the emit gate refuses to append them when E2E is off, and the
    /// E2E-disable downgrade strips them). The slot is validated against
    /// the syncable allowlist ([`SecretSlot::syncable_from_wire`]) so a
    /// stray `access_token` or the E2E key itself can never be written
    /// here. Best-effort: a keychain failure (locked / unavailable) is
    /// logged and skipped so the rest of the batch still integrates; the
    /// account just stays "credentials missing" until a later sync retries.
    fn apply_credential_set(&self, payload: &CredentialPayload) -> SyncResult<()> {
        if payload.account_id == "local" {
            return Ok(());
        }
        let Some(slot) = SecretSlot::syncable_from_wire(&payload.slot) else {
            warn!(
                slot = %payload.slot,
                account_id = %payload.account_id,
                "credential.set: non-syncable slot rejected",
            );
            return Ok(());
        };
        if let Err(err) = self
            .secrets
            .store(&payload.account_id, slot, &payload.secret)
        {
            warn!(
                ?err,
                account_id = %payload.account_id,
                "credential.set: keychain write failed (account stays credentials-missing)",
            );
        }
        Ok(())
    }

    /// Apply a `credential.cleared` event by removing that slot from the
    /// local keychain. Same allowlist + best-effort semantics as
    /// [`Self::apply_credential_set`].
    fn apply_credential_clear(&self, payload: &CredentialSlotPayload) -> SyncResult<()> {
        if payload.account_id == "local" {
            return Ok(());
        }
        let Some(slot) = SecretSlot::syncable_from_wire(&payload.slot) else {
            return Ok(());
        };
        if let Err(err) = self.secrets.delete(&payload.account_id, slot) {
            warn!(
                ?err,
                account_id = %payload.account_id,
                "credential.cleared: keychain delete failed",
            );
        }
        Ok(())
    }

    fn is_already_applied(&self, event_id: &str) -> SyncResult<bool> {
        self.store.is_event_applied(event_id).map_err(store_to_sync)
    }

    fn mark_applied(&self, event_id: &str) -> SyncResult<()> {
        self.store
            .mark_event_applied(event_id)
            .map_err(store_to_sync)
    }
}

/// Map a store-seam error into the apply path's `SyncError`. The store
/// methods (idempotency, conflicts, settings, accounts, plugins) surface
/// backend failures as [`StoreError`]; the applier reports them as
/// `Internal` — "the sync hit an unexpected local-storage state".
fn store_to_sync(err: StoreError) -> SyncError {
    SyncError::Internal(err.to_string())
}

/// Convert a cal-core error from an adapter call into a SyncError
/// flavour appropriate for the apply path. Most cal-core variants
/// map to `Internal` because they signal "the row payload doesn't
/// match local invariants" — exactly the case where the user
/// needs to know "the sync hit an unexpected state" without us
/// claiming a particular root cause.
fn core_to_sync(err: cal_core::Error) -> SyncError {
    SyncError::Internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recurrence_serialization_drift_is_not_a_conflict() {
        // The exact shapes from a real cross-device conflict: the remote payload
        // was written by an older build that predates the §9.12 axes, so it omits
        // `anchor`/`placement`/`fixed_dates`; the local row, re-serialized through
        // the current model, always emits them at their defaults. The two are the
        // SAME daily recurrence.
        let legacy_remote = json!({
            "day_of_month": null,
            "day_of_week": null,
            "end": { "type": "never" },
            "frequency": "daily",
            "interval": 1
        });
        let current_local = json!({
            "anchor": "from_date",
            "day_of_month": null,
            "day_of_week": null,
            "end": { "type": "never" },
            "fixed_dates": null,
            "frequency": "daily",
            "interval": 1,
            "placement": "schedule"
        });

        // Raw JSON differs only by the default-valued keys the older build omitted.
        assert_ne!(legacy_remote, current_local);

        // Canonicalized through the typed model they are identical, so merge_fields
        // treats them as aligned and records NO conflict (the bug was a spurious
        // recurrence conflict surfacing in the sync-conflicts dialog).
        assert_eq!(
            canonicalize_recurrence(ConflictKind::Task, &legacy_remote),
            canonicalize_recurrence(ConflictKind::Task, &current_local),
        );
    }

    #[test]
    fn genuinely_different_recurrence_still_differs_after_canonicalize() {
        // A real difference (daily vs weekly) must survive canonicalization so a
        // true recurrence conflict is still detected.
        let daily = json!({ "end": { "type": "never" }, "frequency": "daily", "interval": 1 });
        let weekly = json!({ "end": { "type": "never" }, "frequency": "weekly", "interval": 1 });
        assert_ne!(
            canonicalize_recurrence(ConflictKind::Task, &daily),
            canonicalize_recurrence(ConflictKind::Task, &weekly),
        );
    }

    #[test]
    fn conflict_still_genuine_prunes_spurious_keeps_real() {
        // Recurrence serialization drift (the §9.12 default axes present vs
        // absent) is NOT a genuine conflict — so a stale record like this can be
        // pruned retroactively.
        let with_axes = json!({
            "anchor": "from_date", "end": { "type": "never" }, "fixed_dates": null,
            "frequency": "daily", "interval": 1, "placement": "schedule"
        });
        let without = json!({ "end": { "type": "never" }, "frequency": "daily", "interval": 1 });
        assert!(!conflict_still_genuine(
            ConflictKind::Task,
            "recurrence",
            &with_axes,
            &without
        ));
        // A real recurrence difference (daily vs weekly) stays genuine.
        let weekly = json!({ "end": { "type": "never" }, "frequency": "weekly", "interval": 1 });
        assert!(conflict_still_genuine(
            ConflictKind::Task,
            "recurrence",
            &without,
            &weekly
        ));
        // Metadata fields are never genuine conflicts.
        assert!(!conflict_still_genuine(
            ConflictKind::Task,
            "updated_at",
            &json!("2026-06-01T00:00:00Z"),
            &json!("2026-06-02T00:00:00Z"),
        ));
        // A normal field with different values IS genuine (kept for the user).
        assert!(conflict_still_genuine(
            ConflictKind::Task,
            "status",
            &json!("completed"),
            &json!("open"),
        ));
    }

    #[test]
    fn non_recurrence_kinds_pass_values_through_unchanged() {
        // For kinds without a recurrence type the helper is a no-op, so it can't
        // accidentally normalize an unrelated field.
        let v = json!({ "frequency": "daily" });
        assert_eq!(canonicalize_recurrence(ConflictKind::Calendar, &v), v);
    }
}
