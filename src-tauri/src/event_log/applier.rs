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

use cal_adapter_local::LocalAdapter;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use sync_core::{
    AccountPayload, CredentialPayload, CredentialSlotPayload, DeviceId, EventEnvelope,
    EventPayload, IdPayload, LogFile, PluginPayload, SettingsPayload, SyncError, SyncEvent,
    SyncResult,
};
use tracing::{debug, warn};

use crate::conflicts::{ConflictKind, ConflictsRepo, NewConflict};
use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// Fields the merge path treats as metadata — never user-surfaced
/// as conflicts. `updated_at` and `created_at` diverge mechanically
/// every time someone edits the row; `etag` is the remote
/// provider's bookkeeping. Showing the user a "your `etag` differs
/// from their `etag`" dialog would be noise.
const METADATA_FIELDS: &[&str] = &["updated_at", "created_at", "etag"];

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
    db: SharedConn,
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
    pub fn new(db: SharedConn, adapter: Arc<LocalAdapter>, local_device_id: DeviceId) -> Self {
        Self {
            db,
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
        let repo = ConflictsRepo::new(&self.db);
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
                match repo.record(new_conflict) {
                    Ok(_) => {
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
        let repo = UserPrefsRepo::new(&self.db);
        if payload.value.is_null() {
            repo.delete(&payload.key).map_err(|err| {
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
            repo.set(&payload.key, &stored).map_err(|err| {
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
        let repo = crate::remote_plugins::RemotePluginsRepo::new(&self.db);
        repo.upsert(
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
        let repo = crate::remote_plugins::RemotePluginsRepo::new(&self.db);
        repo.delete(&payload.id).map_err(|err| {
            SyncError::internal(format!("remote_plugins delete for {}: {err}", payload.id,))
        })
    }

    /// Insert-or-update the `accounts` row mirroring a
    /// `account.created` / `account.updated` event from another
    /// device. Same upsert shape as the snapshot path in
    /// `upsert_snapshot_account` — kept inline here so the
    /// applier doesn't take a dependency on snapshot.rs. Secrets
    /// are NOT included in the payload; the receiving device
    /// surfaces the existing `list_accounts_missing_credentials`
    /// wizard for the user to enter them locally. The implicit
    /// `local` account is skipped so a stray event from a peer
    /// can't overwrite its bootstrap timestamps.
    fn apply_account_upsert(&self, payload: &AccountPayload) -> SyncResult<()> {
        if payload.id == "local" {
            return Ok(());
        }
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO accounts
                (id, adapter_kind, display_name, config_json,
                 created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                adapter_kind = excluded.adapter_kind,
                display_name = excluded.display_name,
                config_json  = excluded.config_json,
                updated_at   = excluded.updated_at",
            params![
                payload.id,
                payload.adapter_kind,
                payload.display_name,
                payload.config_json,
                payload.created_at,
                payload.updated_at,
            ],
        )
        .map_err(|err| {
            SyncError::internal(format!("accounts upsert for {}: {err}", payload.id,))
        })?;
        Ok(())
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
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM accounts WHERE id = ?", params![payload.id])
            .map_err(|err| {
                SyncError::internal(format!("accounts delete for {}: {err}", payload.id,))
            })?;
        Ok(())
    }

    /// Apply a `credential.set` event by writing the synced secret into
    /// the local keychain. Only reached for events that arrived inside an
    /// E2E-encrypted log — a plaintext log can't legitimately carry these
    /// (the emit gate refuses to append them when E2E is off, and the
    /// E2E-disable downgrade strips them). The slot is validated against
    /// the syncable allowlist ([`crate::secrets::SecretSlot::syncable_from_wire`])
    /// so a stray `access_token` or the E2E key itself can never be
    /// written here. Best-effort: a keychain failure (locked /
    /// unavailable) is logged and skipped so the rest of the batch still
    /// integrates; the account just stays "credentials missing" until a
    /// later sync retries.
    fn apply_credential_set(&self, payload: &CredentialPayload) -> SyncResult<()> {
        if payload.account_id == "local" {
            return Ok(());
        }
        let Some(slot) = crate::secrets::SecretSlot::syncable_from_wire(&payload.slot) else {
            warn!(
                slot = %payload.slot,
                account_id = %payload.account_id,
                "credential.set: non-syncable slot rejected",
            );
            return Ok(());
        };
        if let Err(err) = crate::secrets::store(&payload.account_id, slot, &payload.secret) {
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
        let Some(slot) = crate::secrets::SecretSlot::syncable_from_wire(&payload.slot) else {
            return Ok(());
        };
        if let Err(err) = crate::secrets::delete(&payload.account_id, slot) {
            warn!(
                ?err,
                account_id = %payload.account_id,
                "credential.cleared: keychain delete failed",
            );
        }
        Ok(())
    }

    fn is_already_applied(&self, event_id: &str) -> SyncResult<bool> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT 1 FROM sync_applied_events WHERE event_id = ?")
            .map_err(|err| SyncError::internal(err.to_string()))?;
        let exists = stmt.query_row(params![event_id], |_| Ok(())).is_ok();
        Ok(exists)
    }

    fn mark_applied(&self, event_id: &str) -> SyncResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO sync_applied_events
                (event_id, applied_at) VALUES (?, ?)",
            params![event_id, now],
        )
        .map_err(|err| SyncError::internal(err.to_string()))?;
        Ok(())
    }
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
    use cal_core::{Calendar, ColorLabel, Event, Reminder};
    use chrono::TimeZone;
    use sync_core::{EventEnvelope, EventPayload, IdPayload, SyncEvent};

    /// Set up an in-memory DB + adapter for the test. The
    /// LocalAdapter's `open_test_db` already runs every migration
    /// including 0012 (sync_applied_events), so the applier can
    /// write its idempotency rows.
    fn fixture() -> (Arc<LocalAdapter>, SharedConn) {
        let shared = cal_adapter_local::test_support::open_test_db();
        let adapter = Arc::new(LocalAdapter::new(shared.clone()));
        (adapter, shared)
    }

    fn fixture_event(id: &str, calendar_id: &str) -> Event {
        Event {
            id: id.into(),
            calendar_id: calendar_id.into(),
            title: "Synced from elsewhere".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::<Reminder>::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            created_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
        }
    }

    fn fixture_envelope(
        device_id: DeviceId,
        event: SyncEvent,
        timestamp_secs: i64,
    ) -> EventEnvelope {
        EventEnvelope {
            id: format!("evt_{:013x}", timestamp_secs),
            device_id,
            timestamp: Utc.timestamp_opt(timestamp_secs, 0).unwrap(),
            event,
        }
    }

    fn fixture_calendar(id: &str) -> Calendar {
        Calendar {
            color_label: None,
            supports_scheduling: false,
            supports_event_color: false,
            id: id.into(),
            name: "From remote".into(),
            color: None,
            read_only: false,
            default_sound: None,
        }
    }

    #[test]
    fn apply_event_created_inserts_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        // Need a calendar locally for the FK to succeed —
        // apply CalendarCreated first.
        let cal = fixture_calendar("cal-x");
        let env_cal = fixture_envelope(
            other.clone(),
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            1000,
        );
        let env_ev = fixture_envelope(
            other,
            SyncEvent::EventCreated(EventPayload {
                id: "ev-1".into(),
                fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
            }),
            2000,
        );

        let report = applier
            .apply_envelopes(vec![env_ev, env_cal]) // out of order on purpose
            .unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.skipped_already_applied, 0);
        assert_eq!(report.failed, 0);

        // Row should be queryable from SQLite.
        let conn = db.lock().unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM events WHERE id = ?",
                params!["ev-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Synced from elsewhere");
    }

    #[test]
    fn applying_same_envelope_twice_is_idempotent() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-x");
        let envelopes = vec![
            fixture_envelope(
                other.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                other,
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
                }),
                2000,
            ),
        ];

        let first = applier.apply_envelopes(envelopes.clone()).unwrap();
        assert_eq!(first.applied, 2);

        let second = applier.apply_envelopes(envelopes).unwrap();
        // Second pass: both rows hit sync_applied_events.
        assert_eq!(second.applied, 0);
        assert_eq!(second.skipped_already_applied, 2);
    }

    #[test]
    fn applying_own_device_envelopes_skips_them() {
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me.clone());

        let cal = fixture_calendar("cal-x");
        // Both envelopes from this device — the applier should
        // count them as `skipped_own` and not touch the DB.
        let envelopes = vec![
            fixture_envelope(
                me.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                me,
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
                }),
                2000,
            ),
        ];
        let report = applier.apply_envelopes(envelopes).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.skipped_own, 2);
        // Calendar table is empty — own-device envelopes never
        // touched it.
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM calendars WHERE id = 'cal-x'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn force_own_re_applies_own_device_envelopes() {
        // §19.10 stale-resume invariant: after a snapshot apply
        // overwrites local rows, our own pending logs should be
        // replayable through the applier so offline edits come
        // back. `apply_envelopes_force_own` is the path that
        // makes this possible — bypass the skip_own filter that
        // a normal sync round honours.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me.clone());

        let cal = fixture_calendar("cal-x");
        let envelopes = vec![
            fixture_envelope(
                me.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                me,
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
                }),
                2000,
            ),
        ];
        let report = applier.apply_envelopes_force_own(envelopes).unwrap();
        // Both envelopes flowed through the dispatch instead of
        // being skipped — calendar + event landed in SQLite.
        assert_eq!(report.applied, 2);
        assert_eq!(report.skipped_own, 0);
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM calendars WHERE id = 'cal-x'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn force_own_honours_already_applied_check() {
        // A second `apply_envelopes_force_own` pass over the
        // same envelopes is idempotent — the first pass writes
        // to `sync_applied_events`, so the second sees them as
        // already-applied and counts them in the right bucket.
        // Important because stale-resume might re-run if the
        // user dismisses and re-triggers.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me.clone());
        let cal = fixture_calendar("cal-y");
        let env = fixture_envelope(
            me,
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            3000,
        );
        let first = applier
            .apply_envelopes_force_own(vec![env.clone()])
            .unwrap();
        assert_eq!(first.applied, 1);
        let second = applier.apply_envelopes_force_own(vec![env]).unwrap();
        assert_eq!(second.applied, 0);
        assert_eq!(second.skipped_already_applied, 1);
    }

    #[test]
    fn apply_event_deleted_removes_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-x");
        // Apply create + delete.
        applier
            .apply_envelopes(vec![
                fixture_envelope(
                    other.clone(),
                    SyncEvent::CalendarCreated(EventPayload {
                        id: cal.id.clone(),
                        fields: serde_json::to_value(&cal).unwrap(),
                    }),
                    1000,
                ),
                fixture_envelope(
                    other.clone(),
                    SyncEvent::EventCreated(EventPayload {
                        id: "ev-1".into(),
                        fields: serde_json::to_value(fixture_event("ev-1", "cal-x")).unwrap(),
                    }),
                    2000,
                ),
                fixture_envelope(
                    other,
                    SyncEvent::EventDeleted(IdPayload { id: "ev-1".into() }),
                    3000,
                ),
            ])
            .unwrap();

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE id = 'ev-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_color_label_created_inserts_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let label = ColorLabel {
            id: cal_core::ColorLabelId::new("lbl-a"),
            name: "Work".into(),
            hex: "#ff0000".into(),
            ad_hoc: false,
        };
        let env = fixture_envelope(
            other,
            SyncEvent::ColorLabelCreated(EventPayload {
                id: "lbl-a".into(),
                fields: serde_json::to_value(&label).unwrap(),
            }),
            1000,
        );
        applier.apply_envelopes(vec![env]).unwrap();

        let conn = db.lock().unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM color_labels WHERE id = ?",
                params!["lbl-a"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Work");
    }

    #[test]
    fn settings_updated_writes_user_prefs() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let env = fixture_envelope(
            other.clone(),
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "appearance.darkMode".into(),
                value: serde_json::json!(true),
            }),
            1000,
        );
        let env_delete = fixture_envelope(
            other,
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "appearance.colorScheme".into(),
                value: serde_json::Value::Null,
            }),
            2000,
        );
        // Pre-seed a value the delete event will remove.
        {
            let shared = db.clone();
            let repo = UserPrefsRepo::new(&shared);
            repo.set("appearance.colorScheme", "blue").unwrap();
        }

        applier.apply_envelopes(vec![env, env_delete]).unwrap();

        let shared = db.clone();
        let repo = UserPrefsRepo::new(&shared);
        // The set event arrived as a bool — encoded as "true".
        let dark = repo.get("appearance.darkMode").unwrap();
        assert_eq!(dark.as_deref(), Some("true"));
        // The delete event wiped the seeded row.
        assert!(repo.get("appearance.colorScheme").unwrap().is_none());
    }

    #[test]
    fn unsupported_variants_count_as_skipped_not_failed() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db, adapter, me);

        let env = fixture_envelope(
            other,
            SyncEvent::ShortcutSet(sync_core::ShortcutPayload {
                action: "event.save".into(),
                binding: "Mod+S".into(),
            }),
            1000,
        );
        let report = applier.apply_envelopes(vec![env]).unwrap();
        // We don't have a shortcut store yet — counted as
        // skipped_unsupported, not failed.
        assert_eq!(report.applied, 0);
        assert_eq!(report.skipped_unsupported, 1);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn apply_section_created_then_deleted() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db, adapter.clone(), me);

        // Seed the owning list so the section's FK is satisfied.
        let list = cal_core::TaskList {
            color_label: None,
            id: "list-1".into(),
            name: "Inbox".into(),
            color: None,
            default_sound: None,
            embedded_in_calendar: None,
            parent_id: None,
            read_only: false,
        };
        adapter.upsert_task_list_from_sync(&list).unwrap();

        let section = cal_core::Section {
            id: "sec-1".into(),
            list_id: "list-1".into(),
            name: "Doing".into(),
            color_label: None,
            order: 0,
        };
        let create = fixture_envelope(
            other.clone(),
            SyncEvent::SectionCreated(EventPayload {
                id: "sec-1".into(),
                fields: serde_json::to_value(&section).unwrap(),
            }),
            1000,
        );
        let report = applier.apply_envelopes(vec![create]).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(
            adapter.get_section_by_id("sec-1").unwrap().unwrap().name,
            "Doing",
        );

        let delete = fixture_envelope(
            other,
            SyncEvent::SectionDeleted(IdPayload { id: "sec-1".into() }),
            2000,
        );
        let report = applier.apply_envelopes(vec![delete]).unwrap();
        assert_eq!(report.applied, 1);
        assert!(adapter.get_section_by_id("sec-1").unwrap().is_none());
    }

    // -----------------------------------------------------------------
    // Phase Sh — field-level merge + conflict detection.
    // -----------------------------------------------------------------

    use crate::conflicts::{ConflictsRepo, ResolutionChoice};

    fn seed_event(adapter: &Arc<LocalAdapter>, id: &str) {
        let cal = fixture_calendar("cal-merge");
        adapter.upsert_calendar_from_sync(&cal).unwrap();
        let mut ev = fixture_event(id, "cal-merge");
        ev.title = "Original".into();
        ev.location = Some("Room A".into());
        // Seed a known updated_at so the merge timestamp math is
        // deterministic.
        ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
        adapter.upsert_event_from_sync(&ev).unwrap();
    }

    #[test]
    fn merge_auto_merges_when_local_was_updated_before_envelope() {
        // Local last edited at T1 = 09:00. Remote envelope at T2 = 10:00.
        // The remote update is "newer" — auto-merge takes the remote
        // value for the differing field; no conflict recorded.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let other = DeviceId::from_string("dev-other".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        seed_event(&adapter, "ev-merge-1");

        // Build the patch — only the title changed remotely.
        let env = fixture_envelope(
            other.clone(),
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-merge-1".into(),
                fields: serde_json::json!({
                    "title": "Updated remotely",
                }),
            }),
            // 10:00:00 UTC == 2026-05-12T10:00:00Z
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );

        let report = applier.apply_envelopes(vec![env]).unwrap();
        assert_eq!(report.applied, 1);

        // Local row reflects the remote title; location preserved.
        let row = adapter.get_event_by_id("ev-merge-1").unwrap().unwrap();
        assert_eq!(row.title, "Updated remotely");
        assert_eq!(row.location.as_deref(), Some("Room A"));

        // No conflict recorded.
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        assert_eq!(repo.unresolved_count().unwrap(), 0);
    }

    #[test]
    fn merge_records_conflict_when_local_was_updated_after_envelope() {
        // Local last edited at T2 = 11:00 (e.g. user just made a
        // change). Remote envelope arrives with timestamp T1 = 10:00
        // — divergent timelines on the same field. The merge keeps
        // the local value and records a conflict row.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let other = DeviceId::from_string("dev-other".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        // Seed with a row whose updated_at is at T2.
        let cal = fixture_calendar("cal-conflict");
        adapter.upsert_calendar_from_sync(&cal).unwrap();
        let mut ev = fixture_event("ev-conflict", "cal-conflict");
        ev.title = "Local title".into();
        ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
        adapter.upsert_event_from_sync(&ev).unwrap();

        // Remote envelope at T1 (older than local's updated_at).
        let env = fixture_envelope(
            other,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-conflict".into(),
                fields: serde_json::json!({
                    "title": "Remote title",
                }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );

        applier.apply_envelopes(vec![env]).unwrap();

        // Local row keeps its value.
        let row = adapter.get_event_by_id("ev-conflict").unwrap().unwrap();
        assert_eq!(row.title, "Local title");

        // A conflict row was recorded.
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        let conflicts = repo.list_unresolved().unwrap();
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.field, "title");
        assert_eq!(c.row_kind, ConflictKind::Event);
        assert_eq!(c.row_id, "ev-conflict");
        // Values are JSON-encoded strings, so `"Local title"` not
        // `Local title`.
        assert_eq!(c.local_value.as_deref(), Some("\"Local title\""));
        assert_eq!(c.remote_value.as_deref(), Some("\"Remote title\""));
    }

    #[test]
    fn merge_only_touches_changed_fields() {
        // Patch carries the full row but only the title differs from
        // local. Other fields (location, start, end) shouldn't
        // produce conflicts even when local's updated_at is newer —
        // they're equal, so the diff check short-circuits.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let other = DeviceId::from_string("dev-other".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-equal");
        adapter.upsert_calendar_from_sync(&cal).unwrap();
        let mut ev = fixture_event("ev-equal", "cal-equal");
        ev.title = "Local".into();
        ev.location = Some("Room X".into());
        ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
        adapter.upsert_event_from_sync(&ev).unwrap();

        // Remote patch carries the FULL row (which is how the
        // current writer emits) but only the title differs.
        let mut patched = ev.clone();
        patched.title = "Remote".into();
        let env = fixture_envelope(
            other,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-equal".into(),
                fields: serde_json::to_value(&patched).unwrap(),
            }),
            // Envelope older than local's updated_at → conflict
            // territory.
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );

        applier.apply_envelopes(vec![env]).unwrap();
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        let conflicts = repo.list_unresolved().unwrap();
        // Exactly one conflict: the title. Location / start / end
        // matched on both sides so no conflict for those.
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].field, "title");
    }

    #[test]
    fn merge_skips_conflict_detection_on_metadata_fields() {
        // `updated_at` / `created_at` / `etag` diverge mechanically.
        // The applier silently takes the remote value for these,
        // never as a user-facing conflict.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let other = DeviceId::from_string("dev-other".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-meta");
        adapter.upsert_calendar_from_sync(&cal).unwrap();
        let mut ev = fixture_event("ev-meta", "cal-meta");
        ev.updated_at = Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0).unwrap();
        ev.etag = Some("local-etag".into());
        adapter.upsert_event_from_sync(&ev).unwrap();

        // Remote patch: only `updated_at` differs (an "older" T1).
        let env = fixture_envelope(
            other,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-meta".into(),
                fields: serde_json::json!({
                    "updated_at": "2026-05-12T10:00:00Z",
                    "etag": "remote-etag",
                }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );
        applier.apply_envelopes(vec![env]).unwrap();

        // No conflict surfaced for metadata-only divergence.
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        assert_eq!(repo.unresolved_count().unwrap(), 0);
    }

    #[test]
    fn merge_falls_back_to_upsert_when_local_row_absent() {
        // Apply an `EventUpdated` before the corresponding
        // `EventCreated`. The merge path detects "no local row",
        // falls back to the regular upsert with the full payload —
        // same end state as if Created had arrived.
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let other = DeviceId::from_string("dev-other".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-absent");
        adapter.upsert_calendar_from_sync(&cal).unwrap();

        let new_event = fixture_event("ev-absent", "cal-absent");
        let env = fixture_envelope(
            other,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-absent".into(),
                fields: serde_json::to_value(&new_event).unwrap(),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );
        applier.apply_envelopes(vec![env]).unwrap();

        let row = adapter.get_event_by_id("ev-absent").unwrap().unwrap();
        assert_eq!(row.title, "Synced from elsewhere");
    }

    /// The canonical field-level-merge scenario from DESIGN.md
    /// §19.3: two devices each touch a different field of the
    /// same event; both edits arrive at this device and both
    /// must land without raising a conflict. This is the
    /// promise that distinguishes Aperio from last-write-wins
    /// designs.
    #[test]
    fn merge_concurrent_edits_to_different_fields_both_land() {
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let device_a = DeviceId::from_string("dev-a".into());
        let device_b = DeviceId::from_string("dev-b".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);
        seed_event(&adapter, "ev-multifield");

        // Device A pushes a title change at T1=10:00.
        let env_a = fixture_envelope(
            device_a,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-multifield".into(),
                fields: serde_json::json!({ "title": "Title from A" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );
        // Device B pushes a location change at T2=11:00 — touches
        // a *different* field than device A.
        let env_b = fixture_envelope(
            device_b,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-multifield".into(),
                fields: serde_json::json!({ "location": "Room from B" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
                .unwrap()
                .timestamp(),
        );

        applier.apply_envelopes(vec![env_a, env_b]).unwrap();

        // Both edits landed.
        let row = adapter.get_event_by_id("ev-multifield").unwrap().unwrap();
        assert_eq!(row.title, "Title from A");
        assert_eq!(row.location.as_deref(), Some("Room from B"));

        // No conflicts — the edits touched disjoint fields, so
        // the per-field "remote vs local" check never had a
        // reason to fire.
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        assert_eq!(repo.unresolved_count().unwrap(), 0);
    }

    /// Order-independence of the previous scenario. Applying
    /// device B's envelope first then device A's should produce
    /// the same end state. Two devices reaching cluster-wide
    /// consistency via the event log must not depend on the
    /// order envelopes happen to be downloaded in.
    #[test]
    fn merge_concurrent_edits_converge_regardless_of_apply_order() {
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let device_a = DeviceId::from_string("dev-a".into());
        let device_b = DeviceId::from_string("dev-b".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);
        seed_event(&adapter, "ev-order");

        let env_a = fixture_envelope(
            device_a,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-order".into(),
                fields: serde_json::json!({ "title": "Title from A" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );
        let env_b = fixture_envelope(
            device_b,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-order".into(),
                fields: serde_json::json!({ "location": "Room from B" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
                .unwrap()
                .timestamp(),
        );

        // Apply in REVERSE order — B then A.
        applier.apply_envelopes(vec![env_b, env_a]).unwrap();

        let row = adapter.get_event_by_id("ev-order").unwrap().unwrap();
        assert_eq!(row.title, "Title from A");
        assert_eq!(row.location.as_deref(), Some("Room from B"));
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        assert_eq!(repo.unresolved_count().unwrap(), 0);
    }

    /// Successive updates from the same device to the same
    /// field LWW correctly — the second envelope's value wins,
    /// no conflicts. Covers the simple "device A made two edits
    /// in a row to the title; we get both eventually" case.
    #[test]
    fn merge_sequential_updates_from_same_device_last_write_wins() {
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let device_a = DeviceId::from_string("dev-a".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);
        seed_event(&adapter, "ev-lww");

        let env_t1 = fixture_envelope(
            device_a.clone(),
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-lww".into(),
                fields: serde_json::json!({ "title": "First edit" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0)
                .unwrap()
                .timestamp(),
        );
        let env_t2 = fixture_envelope(
            device_a,
            SyncEvent::EventUpdated(EventPayload {
                id: "ev-lww".into(),
                fields: serde_json::json!({ "title": "Second edit" }),
            }),
            Utc.with_ymd_and_hms(2026, 5, 12, 11, 0, 0)
                .unwrap()
                .timestamp(),
        );

        applier.apply_envelopes(vec![env_t1, env_t2]).unwrap();

        let row = adapter.get_event_by_id("ev-lww").unwrap().unwrap();
        assert_eq!(row.title, "Second edit");
        let shared = db.clone();
        let repo = ConflictsRepo::new(&shared);
        assert_eq!(repo.unresolved_count().unwrap(), 0);
    }

    // Re-export `ResolutionChoice` so the warning compiler check
    // doesn't fire on the new conflicts use.
    #[allow(dead_code)]
    fn _exercise_resolution_choice() {
        let _ = ResolutionChoice::KeepLocal;
    }

    /// AccountCreated from another device should land as an
    /// upsert in the local `accounts` table; AccountUpdated
    /// on the same id mutates the row; AccountDeleted drops it.
    #[test]
    fn account_events_round_trip_through_applier() {
        use sync_core::AccountPayload;
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter, me);

        let envs = vec![
            fixture_envelope(
                other.clone(),
                SyncEvent::AccountCreated(AccountPayload {
                    id: "acc-1".into(),
                    adapter_kind: "caldav".into(),
                    display_name: "Work".into(),
                    config_json: r#"{"server_url":"https://dav.example.com"}"#.into(),
                    created_at: "2026-05-12T09:14:22Z".into(),
                    updated_at: "2026-05-12T09:14:22Z".into(),
                }),
                1000,
            ),
            fixture_envelope(
                other.clone(),
                SyncEvent::AccountUpdated(AccountPayload {
                    id: "acc-1".into(),
                    adapter_kind: "caldav".into(),
                    display_name: "Work (renamed)".into(),
                    config_json: r#"{"server_url":"https://dav.example.com"}"#.into(),
                    created_at: "2026-05-12T09:14:22Z".into(),
                    updated_at: "2026-05-12T09:20:00Z".into(),
                }),
                2000,
            ),
        ];
        let report = applier.apply_envelopes(envs).unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.failed, 0);

        let conn = db.lock().unwrap();
        let name: String = conn
            .query_row(
                "SELECT display_name FROM accounts WHERE id = ?",
                params!["acc-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "Work (renamed)");
        drop(conn);

        let env_del = fixture_envelope(
            other,
            SyncEvent::AccountDeleted(IdPayload { id: "acc-1".into() }),
            3000,
        );
        let report = applier.apply_envelopes(vec![env_del]).unwrap();
        assert_eq!(report.applied, 1);

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM accounts WHERE id = ?",
                params!["acc-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
