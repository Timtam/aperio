//! Onboarding for the cross-device sync layer (Phase Sf, DESIGN.md
//! §19.11).
//!
//! Phase Sd configured a working adapter and Phase Se gave us
//! automatic sync triggers. Phase Sf is the missing first step:
//! **what happens when a brand-new device points at a remote that
//! already contains another device's data.**
//!
//! The user-facing decision tree is from §19.11:
//!
//! ```text
//! [device boots, has zero or some local data]
//!         │
//!         ▼
//! configure_sync_adapter → preview_sync_target(config)
//!         │
//!         ├── empty remote ─────► adopt_local_dataset
//!         │                       (push our state up — we become
//!         │                        device 1 of N)
//!         │
//!         └── existing remote ──► UI asks the user:
//!                                 "Datensatz übernehmen" → accept_remote_dataset
//!                                 "Neu beginnen"           → adopt_local_dataset
//!                                                            (with overwrite warning)
//! ```
//!
//! The [`OnboardingService`] keeps the adapter-agnostic logic in one
//! place; the Tauri command layer is a thin wiring around it. Tests
//! can drive the service against a fake `SyncAdapter` without
//! involving the Tauri runtime.
//!
//! ## What this module deliberately does NOT do
//!
//! - **E2E password prompt.** §19.11 Step 4. Lives in Phase Sk; the
//!   meta.json `e2e_enabled` flag is surfaced to the frontend so it
//!   can refuse to onboard if E2E is on and we don't have the key.
//! - **Snapshot consumption.** §19.11 Step 5. Phase Sg will both
//!   produce and consume snapshots; until then, the onboarding flow
//!   falls back to "fetch every log and apply chronologically",
//!   which is correct (just slower) for any dataset that hasn't
//!   been compacted yet.
//! - **Sound asset pull.** §19.11 Step 7. Sounds aren't synced yet
//!   anyway (Phase Sk dependency).
//! - **Account re-connect prompt.** §19.11 Step 8. UI in Phase Si.
//! - **Plugin gap detection.** §19.11 Step 9. Plugins land in §20.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sync_core::{
    DeviceCursor, DeviceId, DeviceRecord, EventEnvelope, LogFile, LogFileName, MetaJson,
    SyncAdapter, SyncError, SyncResult,
};
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::{
    ApplyReport, EventLogApplier, SnapshotBuilder, SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
    SYNC_CURSOR_PREF_KEY,
};
use crate::user_prefs::UserPrefsRepo;
use sync_engine::SYNC_UNREADABLE_LOGS_PREF_KEY;

/// `user_prefs` key holding the user-chosen device name. Surfaces in
/// other devices' "known devices" lists via `meta.json`. Optional —
/// when unset the meta record falls back to the bare device id. The
/// canonical definition lives in `sync-engine` (the compactor writes it
/// into meta too); re-exported here because onboarding owns the UI that
/// sets it.
pub use sync_engine::PREF_DEVICE_NAME;

/// `user_prefs` key flagging that onboarding has completed at least
/// once. Lets the frontend tell "first launch, no adapter ever
/// chosen" apart from "adapter currently disconnected".
pub const PREF_ONBOARDED: &str = "sync.onboarded";

/// How stale this device's `last_seen` stamp may get before the heartbeat
/// re-publishes it for that reason alone.
///
/// The trade this number makes: every round that has nothing else to say skips
/// its meta push entirely, and this is the one thing that would spoil that, so
/// it has to be far coarser than the sync interval. Twelve hours means at most
/// two extra pushes a day on a completely idle dataset, and a device list that
/// can be up to twelve hours out about a device that IS syncing.
///
/// Twelve hours of imprecision is invisible in what the list is read for.
/// Nobody prunes a registry by asking which device was here this morning; they
/// ask which of these eight has not been here since March.
const LAST_SEEN_REFRESH: chrono::Duration = chrono::Duration::hours(12);

/// Outcome of [`OnboardingService::preview`]. Mirrors the three
/// cases the §19.11 onboarding dialog distinguishes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncPreview {
    /// `meta.json` doesn't exist at the remote root — this is the
    /// "you're the first device" case. UI offers a single
    /// "Connect & push" action.
    Empty,
    /// `meta.json` exists and (optionally) `snapshot.json` does too.
    /// UI offers "Datensatz übernehmen" + "Neu beginnen (überschreibt)".
    Existing {
        schema_version: u32,
        min_app_version: String,
        /// RFC 3339 timestamp of the current snapshot, or `null`
        /// when the dataset has never been compacted.
        snapshot_timestamp: Option<String>,
        e2e_enabled: bool,
        devices: Vec<DeviceSummary>,
        /// Phase Sl: how the running build relates to the
        /// dataset's version requirements. `Ok` is the happy
        /// path; `AppTooOld` / `SchemaAhead` let the frontend
        /// gate the accept button and pop the §19.13 update
        /// modal.
        compatibility: sync_core::Compatibility,
    },
}

/// One row in the `Existing` preview's device list. Pre-formatted
/// for direct display in the frontend's "known devices" table.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceSummary {
    /// Bare device-id string. Stable identifier the frontend can
    /// key React lists on.
    pub id: String,
    pub name: Option<String>,
    /// The device's CONTENT horizon, as RFC 3339. Not when it was here — see
    /// [`sync_core::DeviceRecord::last_seen_log`], which explains why the two
    /// are different questions and why this one cannot answer the other.
    pub last_seen_log: String,
    /// When this device last completed a round, as RFC 3339, or `None` on a
    /// dataset whose registry predates the field — which reads honestly as
    /// "unknown" rather than as a very old date.
    pub last_seen: Option<String>,
    pub app_version: String,
    pub stale: bool,
    /// `true` when this entry refers to the current device. Lets
    /// the dialog highlight it ("Dieses Gerät") even though we
    /// also list it in the table.
    pub is_this_device: bool,
}

/// Summary returned to the frontend by [`OnboardingService::accept_remote`]
/// and [`OnboardingService::adopt_local`]. Same shape as the
/// orchestrator's `SyncRoundReport` minus push counts (onboarding
/// never pushes, it adopts).
#[derive(Debug, Default, Clone, Serialize)]
pub struct OnboardingReport {
    pub fetched_logs: usize,
    pub applied: usize,
    pub skipped_already_applied: usize,
    pub skipped_unsupported: usize,
    pub apply_failures: usize,
    /// `true` when the remote was empty before we touched it. The
    /// frontend can tell the user "Connected as device 1" vs
    /// "Connected, joined N existing devices".
    pub remote_was_empty: bool,
    /// Number of devices in `meta.json` AFTER this device's entry
    /// was upserted. Includes us.
    pub device_count: usize,
    /// What the pass over every log applied, whole — for a sync round that
    /// resumed or caught up, so the platform re-reads settings and reloads
    /// views from its report.
    #[serde(skip)]
    pub detail: ApplyReport,
}

impl OnboardingReport {
    fn merge_apply(&mut self, report: ApplyReport) {
        self.applied += report.applied;
        self.skipped_already_applied += report.skipped_already_applied;
        self.skipped_unsupported += report.skipped_unsupported;
        self.apply_failures += report.failed;
    }
}

/// What [`OnboardingService::apply_every_log`] found.
struct Pass {
    /// The newest foreign file's name time, if any.
    newest_foreign: Option<DateTime<Utc>>,
    /// Every file read: the device is exact.
    complete: bool,
}

/// Onboarding helper. The Tauri commands hold one of these in State
/// and dispatch the user-facing verbs through it.
///
/// Independent of [`SyncOrchestrator`]: the orchestrator is the
/// "steady state" sync engine, the onboarding service is the
/// "first connection" engine. They share the applier so an
/// onboarded log isn't re-applied during the first scheduled
/// round.
pub struct OnboardingService {
    db: SharedConn,
    local_device_id: DeviceId,
    applier: Arc<EventLogApplier>,
    /// Phase Sg: used to consume `snapshot.json` before pulling
    /// logs so a freshly-onboarded device pays one snapshot read
    /// + a tiny log backlog instead of replaying months of logs
    /// from epoch.
    snapshot_builder: Arc<SnapshotBuilder>,
    /// §19.10 stale-resume: where the writer queues
    /// not-yet-pushed log files. The resume path reads from
    /// here AFTER applying the snapshot so the user's offline
    /// edits get re-established on top of the snapshot state.
    /// Shares the same path the orchestrator pushes from; no
    /// cross-component locking needed because resume is
    /// single-threaded inside the orchestrator's in-flight
    /// guard.
    pending_dir: PathBuf,
    /// §19.11.7: where custom notification sound files live
    /// locally. `accept_remote` and `resume_from_stale` invoke
    /// `sound_assets::sync_assets` after applying the snapshot
    /// so any newly-referenced hashes are downloaded before
    /// the user sees the dataset.
    sounds_dir: PathBuf,
    app_version: String,
}

impl OnboardingService {
    pub fn new(
        db: SharedConn,
        local_device_id: DeviceId,
        applier: Arc<EventLogApplier>,
        snapshot_builder: Arc<SnapshotBuilder>,
        pending_dir: PathBuf,
        sounds_dir: PathBuf,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            db,
            local_device_id,
            applier,
            snapshot_builder,
            pending_dir,
            sounds_dir,
            app_version: app_version.into(),
        }
    }

    /// Borrow the device id this service tags meta.json entries
    /// with. Used by [`SyncOrchestrator::heartbeat_meta`] so the
    /// scheduler can refresh its own record without re-loading the
    /// id from user_prefs.
    pub fn device_id(&self) -> &DeviceId {
        &self.local_device_id
    }

    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    /// Non-mutating probe. Reads `meta.json` (if any), classifies
    /// it as Empty vs Existing, returns enough detail for the
    /// onboarding dialog to render the device list + warnings.
    pub async fn preview(&self, adapter: &dyn SyncAdapter) -> SyncResult<SyncPreview> {
        adapter.test_connection().await?;
        let meta = adapter.fetch_meta().await?;
        Ok(match meta {
            None => SyncPreview::Empty,
            Some(meta) => {
                let compatibility = sync_core::check_compatibility(
                    meta.schema_version,
                    &meta.min_app_version,
                    &self.app_version,
                    sync_core::SCHEMA_VERSION,
                );
                SyncPreview::Existing {
                    schema_version: meta.schema_version,
                    min_app_version: meta.min_app_version.clone(),
                    // A freshly-minted dataset carries the MIN_UTC sentinel
                    // (no snapshot yet) — surface that as None so the UI shows
                    // "noch kein Snapshot" instead of an empty-state
                    // pseudo-timestamp.
                    snapshot_timestamp: snapshot_ts_if_real(&meta),
                    e2e_enabled: meta.e2e_enabled,
                    devices: self.summarise_devices(&meta),
                    compatibility,
                }
            }
        })
    }

    /// Every device the dataset has a record for, this one included and marked.
    ///
    /// A live read: `meta.json` is the registry, and the answer to "who else is
    /// on this dataset" is only ever as good as the last fetch. Cheap — one
    /// small file — and the alternative, a cached copy, would answer a question
    /// about right now with something from the last round.
    pub async fn list_devices(&self, adapter: &dyn SyncAdapter) -> SyncResult<Vec<DeviceSummary>> {
        Ok(adapter
            .fetch_meta()
            .await?
            .as_ref()
            .map(|meta| self.summarise_devices(meta))
            .unwrap_or_default())
    }

    /// Drop another device's registry entry.
    ///
    /// ## What the user is actually doing
    ///
    /// Not revoking access, and not deleting anything the device wrote. A
    /// registry entry is a claim about who is still participating, and after
    /// enough reinstalls and test devices the claim stops being true. Every
    /// entry holds the GC floor down at its own held horizon
    /// (`min_device_held` in the compactor), so leftovers are not merely
    /// clutter in a list — they keep logs alive that nothing will ever read.
    ///
    /// The log FILES stay. They are dataset content: the compactor deletes each
    /// one once the snapshot covers it, and taking them out from under it here
    /// would delete events no device has folded in yet. Dropping the record is
    /// enough for the floor, because the floor is computed from the records.
    ///
    /// ## Why it is safe to be wrong
    ///
    /// A device that is still running re-registers itself on its next round —
    /// nothing here revokes a credential, and the registry is not a permission
    /// list. The worst outcome of removing a live device is that the compactor
    /// stops holding logs for it in the meantime, and it consumes the snapshot
    /// on return instead of replaying logs. Slower, not lossy: its own
    /// un-uploaded events are in its own database, and the logs it did upload
    /// are only ever GC'd once some device has folded them into the snapshot.
    ///
    /// ## The one it refuses
    ///
    /// This device. Removing our own record would be undone by our own next
    /// heartbeat, so the gesture would read as broken rather than as refused —
    /// and a user reaching for it means something else ("stop syncing", which
    /// is Disconnect, and which does clean up after itself).
    pub async fn forget_device(
        &self,
        adapter: &dyn SyncAdapter,
        device_id: &str,
    ) -> SyncResult<()> {
        let device_id = device_id.trim();
        if device_id == self.local_device_id.as_str() {
            return Err(SyncError::internal(
                "this device cannot remove its own registry entry; disconnect instead",
            ));
        }
        // Fetched fresh rather than taken from the round: the PUT below rewrites
        // the whole file, so basing it on anything older would revert whatever a
        // peer wrote in between — the same reasoning as the heartbeat's
        // re-fetch, and here the window is a user staring at a dialog rather
        // than a round, so it is wider.
        let Some(mut meta) = adapter.fetch_meta().await? else {
            return Err(SyncError::not_found(
                "the target has no meta.json, so it has no device registry",
            ));
        };
        if !meta.remove_device(&sync_core::DeviceId::from_string(device_id.to_string())) {
            // Already gone. Two devices pruning the same leftover is an
            // ordinary race and the second one has got what it asked for.
            debug!(device = device_id, "forget_device: no such record");
            return Ok(());
        }
        adapter.push_meta(&meta).await?;
        info!(device = device_id, "removed a device from the registry");
        Ok(())
    }

    /// The registry as the frontends render it, newest-seen first so the
    /// entries a user is deciding about — the ones that have not been here for
    /// months — collect at the end rather than scattering through the list.
    fn summarise_devices(&self, meta: &MetaJson) -> Vec<DeviceSummary> {
        // Sorted on the TIMESTAMPS, before they become strings. Comparing the
        // RFC 3339 forms would happen to work today — `to_rfc3339` on a UTC
        // value always writes the same offset, so lexical order is chronological
        // — and would quietly stop working the day anything renders one in a
        // local zone.
        let mut rows: Vec<(&String, &sync_core::DeviceRecord)> = meta.devices.iter().collect();
        // This device first, whatever its stamp: it is the entry the user
        // orients by, and the one row they must not act on by accident. Then by
        // stamp, newest first — an entry with no stamp sorts last, because
        // "never reported" is the strongest form of "not recently".
        rows.sort_by(|(a_id, a), (b_id, b)| {
            let a_is_self = *a_id == self.local_device_id.as_str();
            let b_is_self = *b_id == self.local_device_id.as_str();
            b_is_self
                .cmp(&a_is_self)
                .then_with(|| b.last_seen.cmp(&a.last_seen))
                .then_with(|| a_id.cmp(b_id))
        });
        rows.into_iter()
            .map(|(id, rec)| DeviceSummary {
                id: id.clone(),
                name: rec.name.clone(),
                last_seen_log: rec.last_seen_log.to_rfc3339(),
                last_seen: rec.last_seen.map(|ts| ts.to_rfc3339()),
                app_version: rec.app_version.clone(),
                stale: rec.stale,
                is_this_device: id == self.local_device_id.as_str(),
            })
            .collect()
    }

    /// "Datensatz übernehmen" path. Applies the snapshot and every log
    /// on top ([`Self::apply_every_log`]), registers this device in
    /// meta.json, advances the local sync cursor; the device's applied
    /// record is exact from here on.
    ///
    /// `device_name` is optional; when `None`, the device record
    /// goes in without a friendly name and other devices show the
    /// bare id until the user names it later (or it falls back to a
    /// short id prefix in the UI).
    ///
    /// The orchestrator is NOT configured here — the caller (the
    /// Tauri command) installs the adapter into the orchestrator
    /// after a successful onboard. That keeps a half-finished
    /// onboarding from leaving the scheduler running against an
    /// inconsistent remote.
    pub async fn accept_remote(
        &self,
        adapter: &dyn SyncAdapter,
        device_name: Option<&str>,
    ) -> SyncResult<OnboardingReport> {
        adapter.test_connection().await?;
        let meta = adapter.fetch_meta().await?.ok_or_else(|| {
            SyncError::not_found("remote has no meta.json — use adopt_local for a fresh dataset")
        })?;

        // Phase Sl: refuse to pull from a dataset our build can't
        // safely read. Surfaces as `SchemaTooOld` so the frontend
        // can render the §19.13 "update required" modal instead of
        // a generic error.
        sync_core::ensure_compatible(&meta, &self.app_version)?;

        // Phase Sg: consume the snapshot first if one exists, then every log
        // file on top. A snapshot that names the events it holds records them
        // as applied (DESIGN-sync-snapshot-coverage.md).
        let mut starting_cursor: DateTime<Utc> = DateTime::<Utc>::MIN_UTC;
        let mut snapshot_applied = false;
        let mut holds = None;
        // Everything is fetched first: a join that fails on the way leaves
        // this device as it was, still syncing with what it had (review
        // round 3 of #130).
        let snapshot = adapter.fetch_snapshot().await?;
        let logs = self.fetch_every_log(adapter).await?;
        // Whatever this device knew of another dataset says nothing here; the
        // pass over every file below makes the record exact again. Nor do its
        // row clocks: they would let that dataset's times keep this one's
        // changes out (review round 2 of #130). The snapshot brings its own.
        self.clear_coverage_exact()?;
        self.clear_row_clocks()?;
        match snapshot {
            Some(snapshot) => {
                info!(
                    snapshot_ts = %snapshot.metadata.snapshot_timestamp,
                    "consuming remote snapshot during onboarding",
                );
                match self.snapshot_builder.apply(&snapshot) {
                    Ok(outcome) => {
                        starting_cursor = snapshot.metadata.snapshot_timestamp;
                        snapshot_applied = true;
                        holds = Some(SnapshotHolds::of(&snapshot, &outcome));
                        if let Some(count) = outcome.covered_events {
                            info!(count, "the snapshot names the events it holds");
                        }
                    }
                    Err(err) => {
                        // Refuse to silently fall back to log-only
                        // replay — a partial snapshot apply would
                        // leave an inconsistent dataset. Surface
                        // so the user can retry.
                        return Err(SyncError::internal(format!("snapshot apply: {err}")));
                    }
                }
            }
            None => {
                debug!("no remote snapshot present; applying every log");
            }
        }

        // Joining a (possibly different) dataset invalidates every
        // remote-missing sound verdict from the previous backend.
        crate::sound_assets::reset_missing_cache();
        let mut report = OnboardingReport {
            remote_was_empty: false,
            ..Default::default()
        };
        // Every log file, not only those named after the snapshot: a session
        // named before it may have grown after it (Toni's laptop,
        // 2026-10-27).
        let pass = self.apply_every_log(logs, &mut report, holds).await?;
        info!(
            count = report.fetched_logs,
            snapshot_applied = snapshot_applied,
            "accept_remote applied every log",
        );

        // Persist the cursor so the next scheduler round doesn't re-pull the
        // same backlog: the snapshot's time, or the newest foreign file's. If
        // neither moved it past MIN_UTC (no snapshot AND no logs), don't
        // touch the existing cursor.
        let cursor = starting_cursor.max(pass.newest_foreign.unwrap_or(DateTime::<Utc>::MIN_UTC));
        if cursor > DateTime::<Utc>::MIN_UTC {
            self.save_cursor(cursor)?;
        }
        if pass.complete {
            self.save_coverage_exact_since(Utc::now())?;
        }

        // Persist + push the device registration — into a fresh copy: the
        // pass above took a while, and a compaction meanwhile may have raised
        // the GC horizon or locked older builds out (review round 4 of #130).
        let mut meta = adapter.fetch_meta().await?.unwrap_or(meta);
        self.register_self_in_meta(&mut meta, device_name).await;
        adapter.push_meta(&meta).await?;
        report.device_count = meta.devices.len();

        // Save the chosen device name + the "we've onboarded" flag.
        if let Some(name) = device_name {
            self.save_device_name(name)?;
        }
        self.mark_onboarded()?;

        // §19.11.7: pull every custom sound the freshly-applied
        // snapshot + logs reference. Best-effort: a failure here
        // means some reminders will be silent until the next
        // periodic sound-asset sync round; the rest of the
        // dataset has already converged.
        match crate::sound_assets::sync_assets(&self.db, &self.sounds_dir, adapter).await {
            Ok(asset_report) => info!(
                pushed = asset_report.pushed,
                fetched = asset_report.fetched,
                "accept_remote sound asset sync",
            ),
            Err(err) => warn!(?err, "accept_remote sound asset sync failed"),
        }

        info!(
            applied = report.applied,
            devices = report.device_count,
            "accept_remote complete",
        );
        Ok(report)
    }

    /// §19.10 stale-device resume.
    ///
    /// Called when the user clicks Fortfahren in the §19.10 "this
    /// device was offline for a while" dialog, and by the round's
    /// auto-resume. Sequence:
    ///
    /// 1. Re-pull the current `snapshot.json` + apply via
    ///    `SnapshotBuilder` (upserts rows from the snapshot
    ///    body; local-only rows untouched).
    /// 2. Every log file still present and our own pending ones, as one
    ///    pass ([`Self::apply_every_log`]): a change wins only where it is
    ///    newer than its row. That brings back what the snapshot apply
    ///    reverted — our offline edits, and changes this device applied
    ///    before — wherever the change is newer than the snapshot's row.
    /// 3. Save the cursor, mark the record exact, clear the device's
    ///    `stale` flag in a freshly fetched meta + push it.
    ///
    /// Pending logs stay on disk after the pass; the next
    /// scheduled sync round picks them up and pushes to the
    /// remote so other devices receive them too.
    pub async fn resume_from_stale(
        &self,
        adapter: &dyn SyncAdapter,
    ) -> SyncResult<OnboardingReport> {
        adapter.test_connection().await?;
        let meta = adapter.fetch_meta().await?.ok_or_else(|| {
            SyncError::not_found("remote meta.json disappeared between stale detection and resume")
        })?;

        // Fetch + apply the current snapshot. Usually one exists
        // (a compaction is what flags devices stale), but a MISSING
        // snapshot must NOT wedge the device: a dataset can carry a
        // `gc_horizon` (logs deleted) with no `snapshot.json` if the
        // snapshot was removed out-of-band, and a legacy/transient
        // latch can fire too. So fall back to a log-only catch-up,
        // but advance PAST `gc_horizon`: the logs below it are gone,
        // so there is nothing to fetch there, and leaving the cursor
        // below it would just re-trip the backstop next round (the
        // wedge). The alternative — erroring here — left the device
        // permanently stuck, since the inline compaction that mints a
        // snapshot sits behind the bailing round.
        let snapshot = adapter.fetch_snapshot().await?;
        let logs = self.fetch_every_log(adapter).await?;
        // Exact again only once this pass read every file (review round 3
        // of #130): an earlier mark says nothing after the snapshot apply.
        self.clear_coverage_exact()?;
        let mut holds = None;
        let base = match snapshot {
            Some(snapshot) => {
                info!(
                    snapshot_ts = %snapshot.metadata.snapshot_timestamp,
                    "applying snapshot during stale-device resume",
                );
                let outcome = self.snapshot_builder.apply(&snapshot).map_err(|err| {
                    SyncError::internal(format!("stale resume snapshot apply: {err}"))
                })?;
                holds = Some(SnapshotHolds::of(&snapshot, &outcome));
                snapshot.metadata.snapshot_timestamp
            }
            None => {
                warn!(
                    "stale resume found no snapshot.json; proceeding with log-only catch-up \
                     past the GC horizon",
                );
                self.read_cursor().max(meta.gc_horizon_or_min())
            }
        };

        let mut report = OnboardingReport {
            remote_was_empty: false,
            ..Default::default()
        };
        let pass = self.apply_every_log(logs, &mut report, holds).await?;
        info!(
            count = report.fetched_logs,
            "stale resume applied every log",
        );

        // Persist the cursor before mutating meta. If we crash
        // between cursor + meta push the next sync round just
        // re-pulls the post-snapshot logs (idempotent applier).
        let cursor = base.max(pass.newest_foreign.unwrap_or(DateTime::<Utc>::MIN_UTC));
        self.save_cursor(cursor)?;
        if pass.complete {
            self.save_coverage_exact_since(Utc::now())?;
        }

        // Clear our device's `stale` flag in meta.json + bump
        // `last_seen_log` so other devices see us as current.
        // This is the bit that lets a future compactor round
        // include our cursor in its retention math. On a fresh copy:
        // the pass above took a while, and a peer may have pushed since.
        let mut meta = adapter.fetch_meta().await?.unwrap_or(meta);
        if let Some(entry) = meta.devices.get_mut(self.local_device_id.as_str()) {
            entry.stale = false;
            entry.last_seen_log = cursor;
            entry.exact = pass.complete;
        }
        // A write of this build, as every other (review round 7 of #130).
        lock_out_older_builds(&mut meta, &self.app_version);
        adapter.push_meta(&meta).await?;
        report.device_count = meta.devices.len();

        // §19.11.7: same sound-asset sync as `accept_remote`.
        // The snapshot pull above may have referenced new sound
        // hashes; pull them now so the user hears reminders
        // correctly after the resume.
        match crate::sound_assets::sync_assets(&self.db, &self.sounds_dir, adapter).await {
            Ok(asset_report) => info!(
                pushed = asset_report.pushed,
                fetched = asset_report.fetched,
                "stale resume sound asset sync",
            ),
            Err(err) => warn!(?err, "stale resume sound asset sync failed"),
        }

        info!(
            applied = report.applied,
            devices = report.device_count,
            "stale resume complete",
        );
        Ok(report)
    }

    /// The round's one-time catch-up for a device whose applied record is
    /// not exact (DESIGN-sync-snapshot-coverage.md; Toni, 2026-10-30): it
    /// joined or resumed from a snapshot before this existed, so a file it
    /// skipped by name may hold events it never applied. One pass over every
    /// log file ([`Self::apply_every_log`]); afterwards the record is exact.
    pub async fn catch_up(&self, adapter: &dyn SyncAdapter) -> SyncResult<OnboardingReport> {
        let mut report = OnboardingReport {
            remote_was_empty: false,
            ..Default::default()
        };
        let logs = self.fetch_every_log(adapter).await?;
        // No snapshot here: the rows hold every own change, or a newer one.
        let pass = self.apply_every_log(logs, &mut report, None).await?;
        if let Some(newest) = pass.newest_foreign {
            self.save_cursor(self.read_cursor().max(newest))?;
        }
        if pass.complete {
            self.save_coverage_exact_since(Utc::now())?;
        }
        info!(
            fetched = report.fetched_logs,
            applied = report.applied,
            "catch-up over every log file complete",
        );
        Ok(report)
    }

    /// Every log file still present — this device's own included — and its
    /// own changes not uploaded yet, applied as ONE pass in time order
    /// (DESIGN-sync-snapshot-coverage.md; Toni, 2026-10-30). Not file by
    /// file: sessions of different devices overlap in time, and a file's
    /// late change must not win over another file's newer one. A change wins
    /// only where it is newer than its row, without a conflict; what this
    /// device applied before is weighed again, so a row a snapshot apply
    /// reverted gets its newer change back
    /// (`EventLogApplier::catch_up_envelopes`). Joining, resuming and the
    /// round's catch-up all end here.
    ///
    /// Everything is fetched before anything changes, so a failed fetch
    /// leaves the device as it was. Records the foreign files' lengths for
    /// the growth refetch and returns the newest foreign file's name time,
    /// and whether every file read: only then is the device exact.
    ///
    /// `holds` is what a snapshot the same pass applied holds (a join, a
    /// resume): an own change not on the remote yet is weighed, as the peers
    /// will weigh it once it is, and so is any own or applied change the
    /// snapshot does not hold (review rounds 5 and 6 of #130). `None` where
    /// no snapshot came first (the round's catch-up): the rows hold every own
    /// change, or a newer one, so each is held.
    async fn apply_every_log(
        &self,
        logs: Vec<LogFile>,
        report: &mut OnboardingReport,
        holds: Option<SnapshotHolds>,
    ) -> SyncResult<Pass> {
        let unreadable_before = self.read_unreadable_logs();
        let mut unreadable: HashMap<String, u64> = HashMap::new();
        let mut envelopes = Vec::new();
        let mut on_remote: HashSet<String> = HashSet::new();
        let mut lengths: Vec<(String, u64)> = Vec::new();
        let mut newest_foreign: Option<DateTime<Utc>> = None;
        let mut complete = true;
        for log in &logs {
            let foreign = log.name.device_id != self.local_device_id;
            if foreign {
                report.fetched_logs += 1;
                newest_foreign = newest_foreign.max(Some(log.name.timestamp));
            }
            match log.into_envelopes() {
                Ok(mut parsed) => {
                    if !foreign {
                        on_remote.extend(parsed.iter().map(|envelope| envelope.id.clone()));
                    }
                    envelopes.append(&mut parsed);
                    if foreign {
                        lengths.push((log.name.to_filename(), log.bytes.len() as u64));
                    }
                }
                Err(err) => {
                    // A torn read of a file being written: not recorded, and
                    // the pass is not complete — the device is not exact, and
                    // the next catch-up takes the file again, under its rules
                    // rather than a round's (review round 2 of #130). Torn
                    // again at the same size, it is no read in progress
                    // (review round 3). A line this build does not know is
                    // no torn read either, at any size: nothing here can
                    // apply it, and it keeps no device from being exact; its
                    // length is recorded, and an update that changed the
                    // parser reads it again (review round 5).
                    let filename = log.name.to_filename();
                    let len = log.bytes.len() as u64;
                    report.apply_failures += 1;
                    if !log.is_torn() || unreadable_before.get(&filename) == Some(&len) {
                        warn!(
                            log = %filename,
                            ?err,
                            "log file this build cannot read; nothing to apply from it",
                        );
                        if foreign {
                            lengths.push((filename.clone(), len));
                        }
                    } else {
                        warn!(
                            log = %filename,
                            ?err,
                            "unreadable log file; the device catches up again later",
                        );
                        complete = false;
                    }
                    // Own files too, for the rule above (review round 7);
                    // only another device's are named in meta.json.
                    unreadable.insert(filename, len);
                }
            }
        }
        // Our own changes not on the remote yet are weighed, not held.
        let pending_files = self.pending_files().await;
        let pending = envelopes_of(&pending_files);
        let unpushed: HashSet<String> = match &holds {
            Some(holds) => pending
                .iter()
                .filter(|envelope| !on_remote.contains(&envelope.id))
                .chain(envelopes.iter().filter(|envelope| !holds.holds(envelope)))
                .map(|envelope| envelope.id.clone())
                .collect(),
            None => HashSet::new(),
        };
        envelopes.extend(pending);
        // A pending file may already be uploaded: each change once.
        let mut seen = HashSet::new();
        envelopes.retain(|envelope| seen.insert(envelope.id.clone()));
        let applied = self
            .applier
            .catch_up_envelopes(envelopes, &sync_engine::PassContext { unpushed })?;
        report.detail = applied.clone();
        report.merge_apply(applied);
        self.record_applied_lengths(&lengths);
        self.save_unreadable_logs(&unreadable);
        Ok(Pass {
            newest_foreign,
            complete,
        })
    }

    /// Every log file on the remote, this device's own included.
    async fn fetch_every_log(&self, adapter: &dyn SyncAdapter) -> SyncResult<Vec<LogFile>> {
        adapter
            .fetch_new_logs(&DeviceCursor {
                last_seen_log: DateTime::<Utc>::MIN_UTC,
                exclude_device: None,
                known_lengths: Vec::new(),
                fetch_unknown: false,
            })
            .await
    }

    /// The log files the last pass over every log could not read, with
    /// their sizes ([`SYNC_UNREADABLE_LOGS_PREF_KEY`]) — only if this build's
    /// parser tried: another one may read what that one could not. A record
    /// that does not read is empty. Either way every unreadable file then
    /// counts as a first sighting.
    fn read_unreadable_logs(&self) -> HashMap<String, u64> {
        let raw = UserPrefsRepo::new(&self.db)
            .get(SYNC_UNREADABLE_LOGS_PREF_KEY)
            .ok()
            .flatten();
        sync_engine::UnreadableLogs::of_this_parser(raw.as_deref())
            .map(|record| record.files)
            .unwrap_or_default()
    }

    /// The files this device names as unreadable in `meta.json`.
    fn unreadable_names(&self) -> Vec<String> {
        let raw = UserPrefsRepo::new(&self.db)
            .get(SYNC_UNREADABLE_LOGS_PREF_KEY)
            .ok()
            .flatten();
        sync_engine::UnreadableLogs::names(raw.as_deref(), &self.local_device_id)
    }

    fn save_unreadable_logs(&self, unreadable: &HashMap<String, u64>) {
        let prefs = UserPrefsRepo::new(&self.db);
        let saved = if unreadable.is_empty() {
            prefs.delete(SYNC_UNREADABLE_LOGS_PREF_KEY)
        } else {
            let record = sync_engine::UnreadableLogs {
                parser: sync_core::PARSER_FINGERPRINT,
                files: unreadable.clone(),
            };
            match serde_json::to_string(&record) {
                Ok(raw) => prefs.set(SYNC_UNREADABLE_LOGS_PREF_KEY, &raw),
                Err(err) => {
                    warn!(?err, "couldn't encode the unreadable log files");
                    return;
                }
            }
        };
        if let Err(err) = saved {
            warn!(?err, "couldn't record the unreadable log files");
        }
    }

    /// This device's own changes in `pending_dir` — written, maybe not
    /// uploaded yet. Per-file failures degrade gracefully: a parse error or
    /// read error gets logged + skipped, the rest of the directory is still
    /// read. The files themselves stay on disk so the next sync round can
    /// still push them to the remote.
    async fn pending_files(&self) -> Vec<LogFile> {
        let mut logs = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.pending_dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // No writer has flushed anything this install.
                debug!("no pending dir; nothing of our own to add");
                return logs;
            }
            Err(err) => {
                warn!(?err, "couldn't open the pending dir");
                return logs;
            }
        };
        let mut files = 0usize;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(err) => {
                    warn!(?err, "read_dir entry error in the pending dir");
                    break;
                }
            };
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let parsed = match LogFileName::from_filename(name) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = name, "skipping pending entry: not a log file");
                    continue;
                }
            };
            // Only OUR own-device logs belong here — anything else in the
            // pending dir is suspicious (the writer only emits our own
            // session files). Guard explicitly.
            if parsed.device_id != self.local_device_id {
                debug!(name = name, "skipping pending entry: foreign device");
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(err) => {
                    warn!(name = name, ?err, "couldn't read pending log");
                    continue;
                }
            };
            files += 1;
            logs.push(LogFile {
                name: parsed,
                bytes,
            });
        }
        debug!(files, "read our own pending logs");
        logs
    }

    /// "Neu beginnen" path. Deletes the old dataset's log files, pushes a
    /// snapshot of this device's state and a fresh meta.json with only this
    /// device, then leaves the rest to the scheduler's next round (which
    /// will push whatever was already queued in `pending/`).
    ///
    /// Caller is responsible for warning the user that any existing
    /// remote data is overwritten — its log files are deleted and its
    /// snapshot replaced.
    pub async fn adopt_local(
        &self,
        adapter: &dyn SyncAdapter,
        device_name: Option<&str>,
        e2e_params: Option<sync_core::EncryptionParams>,
    ) -> SyncResult<OnboardingReport> {
        adapter.test_connection().await?;
        // Pointing at a (possibly different) remote invalidates every
        // remote-missing sound verdict from the previous backend.
        crate::sound_assets::reset_missing_cache();

        // Best-effort peek so we can log what was there before. A
        // missing meta means the remote was already empty and the
        // overwrite is a no-op against prior data.
        let prior = adapter.fetch_meta().await.unwrap_or(None);
        let remote_was_empty = prior.is_none();
        if let Some(prior) = &prior {
            info!(
                devices = prior.devices.len(),
                "adopt_local overwriting existing meta.json",
            );
        }

        // "Neu beginnen" overwrites the dataset (Toni, 2026-10-30): the old
        // one's log files go, so neither this device's rounds nor a device
        // joining later take them in, and its snapshot gives way to this
        // device's state. Names only: the old files may be under another key.
        // A delete that fails stops here, before the new meta, and the user
        // can start over again.
        let old_logs = adapter.list_log_names().await?;
        for name in &old_logs {
            adapter.delete_log(name).await?;
        }
        if !old_logs.is_empty() {
            info!(
                count = old_logs.len(),
                "adopt_local deleted the old dataset's log files"
            );
        }
        let snapshot_ts = Utc::now();
        let mut snapshot = self.snapshot_builder.build_at(snapshot_ts, None)?;
        // A dataset started without a passphrase is plaintext: no secret in
        // its snapshot, whatever E2E mark this device kept from an earlier
        // one (review round 3 of #130).
        if e2e_params.is_none() {
            crate::credential_sync::strip_credentials_from_snapshot(&mut snapshot);
        }
        adapter.push_snapshot(&snapshot).await?;
        // Its own state is the dataset now: nothing to catch up over.
        self.save_coverage_exact_since(snapshot_ts)?;

        let mut meta = MetaJson::fresh(&self.app_version);
        meta.snapshot_timestamp = snapshot_ts;
        // Phase Sk: bake the E2E flag + params into the fresh
        // meta so devices joining later can derive the same key
        // from the user's passphrase. The key itself is NEVER
        // written here — it lives only in each device's keychain.
        if let Some(params) = e2e_params {
            meta.e2e_enabled = true;
            meta.e2e_params = Some(params);
        }
        self.register_self_in_meta(&mut meta, device_name).await;
        adapter.push_meta(&meta).await?;

        if let Some(name) = device_name {
            self.save_device_name(name)?;
        }
        self.mark_onboarded()?;

        Ok(OnboardingReport {
            remote_was_empty,
            device_count: meta.devices.len(),
            ..Default::default()
        })
    }

    /// Refresh this device's `last_seen_log` + `app_version` in
    /// meta.json. Called by the orchestrator after every successful
    /// sync round so other devices see a heartbeat and the
    /// compaction algorithm (Phase Sg) has accurate cursors.
    ///
    /// `round_meta` — the copy the round fetched at its START — is used
    /// ONLY to decide whether a push is needed at all. When our device
    /// record is already current (same held horizon, name and app
    /// version, not flagged stale) the push is skipped entirely:
    /// `last_seen_log` means "the point up to which I hold every event",
    /// which a no-change round doesn't move, so re-writing the identical
    /// record was one wasted GET+PUT per round. (Round liveness for the
    /// UI rides `SYNC_LAST_ROUND_PREF_KEY`, not this file.)
    ///
    /// When a push IS needed, the base copy is RE-FETCHED fresh — never
    /// the round-start copy. The PUT rewrites the whole file, and a peer
    /// can have compacted DURING our round (raising the monotonic
    /// `gc_horizon`, stamping `snapshot_timestamp`, flagging behind
    /// devices stale); pushing a copy as old as the entire round would
    /// silently revert all of that, un-flagging a stale device that
    /// would then skip GC'd logs forever. Re-fetching narrows the
    /// lost-update window back to the single GET→PUT gap §19.5's
    /// last-write-wins already tolerates for the device registry.
    pub async fn heartbeat_meta(
        &self,
        adapter: &dyn SyncAdapter,
        last_seen_log: DateTime<Utc>,
        round_meta: Option<&MetaJson>,
    ) -> SyncResult<()> {
        let name = UserPrefsRepo::new(&self.db)
            .get(PREF_DEVICE_NAME)
            .ok()
            .flatten();
        let exact = self.coverage_exact();
        let unreadable = self.unreadable_names();
        let pending = self.pending_names().await;
        let now = Utc::now();
        let is_current = |meta: &MetaJson| {
            meta.devices
                .get(self.local_device_id.as_str())
                .is_some_and(|d| {
                    d.last_seen_log == last_seen_log
                        && d.name == name
                        && d.app_version == self.app_version
                        && d.exact == exact
                        && d.unreadable == unreadable
                        && pending.as_ref().is_none_or(|names| d.pending == *names)
                        // Deliberately COARSE. Every other term here is an
                        // equality, and treating the wall clock the same way
                        // would make the skip below unreachable — the stamp
                        // differs by definition on every round, so a quiet
                        // dataset would go back to one GET plus one PUT per
                        // device per round, which is exactly the traffic the
                        // skip was added to remove.
                        //
                        // So the record counts as current while its stamp is
                        // younger than the refresh window, and the push
                        // happens at most twice a day for this reason alone.
                        // What that costs is precision the device list does
                        // not use: it says "yesterday" or "in March", and
                        // being up to twelve hours out never changes which of
                        // those it says.
                        //
                        // `None` is not current. That is how the stamp first
                        // appears on a dataset that predates the field.
                        && d.last_seen
                            .is_some_and(|seen| now - seen < LAST_SEEN_REFRESH)
                })
        };
        // A dataset still on an older format is locked out for older builds
        // at this build's first write (review round 6 of #130): from then on
        // no compactor deletes by name, ignoring what devices name pending.
        let locked_out = |meta: &MetaJson| meta.schema_version >= sync_core::SCHEMA_VERSION;
        if round_meta.is_some_and(|meta| locked_out(meta) && is_current(meta)) {
            debug!("heartbeat_meta: device record already current; skipping push");
            return Ok(());
        }
        let mut meta = match adapter.fetch_meta().await? {
            Some(m) => m,
            None => {
                // The remote has lost its meta.json since onboarding
                // (someone deleted it, or onboarding was incomplete).
                // Mint a fresh one with us as the only known device.
                debug!("heartbeat_meta found no remote meta; reseeding with this device",);
                MetaJson::fresh(&self.app_version)
            }
        };
        // The fresh copy can already carry the update (a fast concurrent
        // round of our own, or the round-start copy was merely stale) —
        // re-check before writing.
        if locked_out(&meta) && is_current(&meta) {
            return Ok(());
        }
        lock_out_older_builds(&mut meta, &self.app_version);
        // A flag a peer set stands: only a resume clears it, once it took the
        // snapshot (review round 7 of #130). A push between rounds names its
        // files here too.
        let stale = meta
            .device(&self.local_device_id)
            .is_some_and(|record| record.stale);
        let pending = match pending {
            Some(names) => names,
            // pending/ did not read: what the record named stands.
            None => meta
                .device(&self.local_device_id)
                .map(|record| record.pending.clone())
                .unwrap_or_default(),
        };
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name,
                last_seen_log,
                last_seen: Some(now),
                app_version: self.app_version.clone(),
                stale,
                exact,
                unreadable,
                pending,
            },
        );
        adapter.push_meta(&meta).await?;
        Ok(())
    }

    async fn register_self_in_meta(&self, meta: &mut MetaJson, device_name: Option<&str>) {
        lock_out_older_builds(meta, &self.app_version);
        let pending = self.pending_names().await.unwrap_or_default();
        let name = device_name.map(str::to_string).or_else(|| {
            UserPrefsRepo::new(&self.db)
                .get(PREF_DEVICE_NAME)
                .ok()
                .flatten()
        });
        let now = Utc::now();
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name,
                last_seen_log: now,
                last_seen: Some(now),
                app_version: self.app_version.clone(),
                stale: false,
                exact: self.coverage_exact(),
                unreadable: self.unreadable_names(),
                pending,
            },
        );
    }

    /// The log files in `pending/` that hold anything, for this device's
    /// record; `None` when the directory does not read.
    async fn pending_names(&self) -> Option<Vec<String>> {
        let mut entries = match tokio::fs::read_dir(&self.pending_dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
            Err(err) => {
                warn!(?err, "couldn't open the pending dir to name its files");
                return None;
            }
        };
        let mut names = Vec::new();
        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // An empty file never goes up (review round 7 of #130).
                    let holds_any = entry
                        .metadata()
                        .await
                        .is_ok_and(|metadata| metadata.len() > 0);
                    if holds_any && LogFileName::from_filename(&name).is_ok() {
                        names.push(name);
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    warn!(?err, "read_dir entry error naming the pending files");
                    return None;
                }
            }
        }
        names.sort_unstable();
        Some(names)
    }

    fn save_device_name(&self, name: &str) -> SyncResult<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        UserPrefsRepo::new(&self.db)
            .set(PREF_DEVICE_NAME, trimmed)
            .map_err(|err| SyncError::internal(format!("save device name: {err}")))?;
        Ok(())
    }

    fn save_cursor(&self, ts: DateTime<Utc>) -> SyncResult<()> {
        UserPrefsRepo::new(&self.db)
            .set(SYNC_CURSOR_PREF_KEY, &ts.to_rfc3339())
            .map_err(|err| SyncError::internal(format!("save cursor: {err}")))?;
        Ok(())
    }

    /// Record that this device's applied record is exact from `ts` on
    /// (`sync_engine::SYNC_COVERAGE_EXACT_SINCE_PREF_KEY`).
    fn save_coverage_exact_since(&self, ts: DateTime<Utc>) -> SyncResult<()> {
        UserPrefsRepo::new(&self.db)
            .set(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &ts.to_rfc3339())
            .map_err(|err| SyncError::internal(format!("save coverage mark: {err}")))?;
        Ok(())
    }

    /// Forget every row's clock (`sync_row_clock`): this device joins a
    /// dataset, maybe another one.
    fn clear_row_clocks(&self) -> SyncResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM sync_row_clock", [])
            .map_err(|err| SyncError::internal(format!("clear row clocks: {err}")))?;
        Ok(())
    }

    /// Whether this device's applied record is exact, for its entry in
    /// meta.json. A mark that does not read is not: the compactors then wait.
    fn coverage_exact(&self) -> bool {
        matches!(
            UserPrefsRepo::new(&self.db).get(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY),
            Ok(Some(_))
        )
    }

    /// This device's record is no longer known to be exact: it joins a
    /// dataset, maybe another one, and is exact again once it applied every
    /// log file there.
    fn clear_coverage_exact(&self) -> SyncResult<()> {
        UserPrefsRepo::new(&self.db)
            .delete(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY)
            .map_err(|err| SyncError::internal(format!("clear coverage mark: {err}")))?;
        Ok(())
    }

    /// Merge freshly applied log byte lengths into the shared
    /// growth-refetch map (same pref + semantics as the sync round —
    /// `sync_engine::merge_applied_log_lengths`). Best-effort: a failed
    /// write only costs a re-fetch.
    ///
    /// The get-merge-set here is NOT serialized against a concurrently
    /// running round (only the round's own auto-resume sits inside the
    /// orchestrator's in-flight guard; the manual resume command and a
    /// re-onboard don't). A lost update would drop a filename's entry —
    /// meaning that one file isn't growth-refetched until its peer
    /// rotates — through a window that requires a round and a
    /// user-triggered onboarding to interleave their final writes.
    /// Accepted: rare, self-healing, and strictly no worse than before
    /// this recording existed.
    fn record_applied_lengths(&self, applied: &[(String, u64)]) {
        let prefs = UserPrefsRepo::new(&self.db);
        let existing = prefs
            .get(sync_engine::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
            .ok()
            .flatten();
        if let Some(raw) = sync_engine::merge_applied_log_lengths(existing.as_deref(), applied) {
            if let Err(err) = prefs.set(sync_engine::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY, &raw) {
                warn!(?err, "couldn't persist applied-log-length map");
            }
        }
    }

    /// Read the persisted fetch cursor, or the `MIN_UTC` "fetch
    /// everything" floor when none has been written yet. Used by the
    /// tolerant stale-resume path when there's no snapshot to anchor on,
    /// and by the catch-up, which never moves it back.
    fn read_cursor(&self) -> DateTime<Utc> {
        UserPrefsRepo::new(&self.db)
            .get(SYNC_CURSOR_PREF_KEY)
            .ok()
            .flatten()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    fn mark_onboarded(&self) -> SyncResult<()> {
        UserPrefsRepo::new(&self.db)
            .set(PREF_ONBOARDED, "true")
            .map_err(|err| SyncError::internal(format!("save onboarded flag: {err}")))?;
        Ok(())
    }
}

/// Decide whether `meta.snapshot_timestamp` represents a real
/// compaction or just the [`MetaJson::fresh`] "no snapshot yet"
/// sentinel. Delegates to [`MetaJson::has_real_snapshot`], which checks
/// the snapshot timestamp against the `MIN_UTC` sentinel a fresh dataset
/// carries. (The old "older than 1 second from now" heuristic was a
/// stopgap from when `fresh()` stamped `Utc::now()`; the sentinel is
/// exact.)
fn snapshot_ts_if_real(meta: &MetaJson) -> Option<String> {
    if meta.has_real_snapshot() {
        Some(meta.snapshot_timestamp.to_rfc3339())
    } else {
        None
    }
}

/// Lock builds before this format out of `meta` (review round 6 of #130):
/// they delete by name and ignore what devices name pending.
fn lock_out_older_builds(meta: &mut MetaJson, app_version: &str) {
    if meta.schema_version < sync_core::SCHEMA_VERSION {
        meta.schema_version = sync_core::SCHEMA_VERSION;
        meta.min_app_version = app_version.to_string();
    }
}

/// What a snapshot the same pass applied holds (review round 6 of #130): the
/// events it names, or, naming none, every change up to its time. The rows
/// hold its values now, so an own or earlier applied change it does not hold
/// is weighed against them, never held.
enum SnapshotHolds {
    Named(HashSet<String>),
    Until(DateTime<Utc>),
}

impl SnapshotHolds {
    fn of(snapshot: &sync_core::Snapshot, outcome: &sync_engine::SnapshotApplyOutcome) -> Self {
        let named: Option<HashSet<String>> = outcome.covered_events.and_then(|_| {
            snapshot
                .body
                .get("covered_event_ids")
                .and_then(|ids| serde_json::from_value::<Vec<String>>(ids.clone()).ok())
                .map(|ids| ids.into_iter().collect())
        });
        match named {
            Some(named) => SnapshotHolds::Named(named),
            None => SnapshotHolds::Until(snapshot.metadata.snapshot_timestamp),
        }
    }

    fn holds(&self, envelope: &EventEnvelope) -> bool {
        match self {
            SnapshotHolds::Named(named) => named.contains(&envelope.id),
            SnapshotHolds::Until(at) => envelope.timestamp <= *at,
        }
    }
}

/// The envelopes of `files`; one that does not parse is left out.
fn envelopes_of(files: &[LogFile]) -> Vec<EventEnvelope> {
    let mut envelopes = Vec::new();
    for file in files {
        match file.into_envelopes() {
            Ok(mut parsed) => envelopes.append(&mut parsed),
            Err(err) => warn!(name = %file.name.to_filename(), ?err, "couldn't parse pending log"),
        }
    }
    envelopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use adapter_local::LocalAdapter;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use sync_core::{LogFile, LogFileName, Snapshot};
    use tempfile::TempDir;

    /// Minimal in-memory `SyncAdapter` for unit testing the
    /// onboarding flow. Stores its bytes in `Mutex` so the test
    /// doesn't need a temp directory.
    struct FakeAdapter {
        meta: Mutex<Option<MetaJson>>,
        logs: Mutex<Vec<LogFile>>,
        snapshot: Mutex<Option<Snapshot>>,
        /// Fetching the logs fails, as offline.
        fail_fetch: Mutex<bool>,
        /// The meta.json a compaction elsewhere writes while the logs are
        /// fetched.
        meta_while_fetching: Mutex<Option<MetaJson>>,
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self::with_logs(Vec::new())
        }

        fn with_logs(logs: Vec<LogFile>) -> Self {
            Self {
                meta: Mutex::new(None),
                logs: Mutex::new(logs),
                snapshot: Mutex::new(None),
                fail_fetch: Mutex::new(false),
                meta_while_fetching: Mutex::new(None),
            }
        }

        fn install_meta(&self, meta: MetaJson) {
            *self.meta.lock().unwrap() = Some(meta);
        }

        fn install_snapshot(&self, snapshot: Snapshot) {
            *self.snapshot.lock().unwrap() = Some(snapshot);
        }
    }

    #[async_trait]
    impl SyncAdapter for FakeAdapter {
        async fn test_connection(&self) -> SyncResult<()> {
            Ok(())
        }
        async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
            Ok(self.meta.lock().unwrap().clone())
        }
        async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
            *self.meta.lock().unwrap() = Some(meta.clone());
            Ok(())
        }
        async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
            if *self.fail_fetch.lock().unwrap() {
                return Err(SyncError::internal("offline"));
            }
            if let Some(meta) = self.meta_while_fetching.lock().unwrap().take() {
                *self.meta.lock().unwrap() = Some(meta);
            }
            Ok(self
                .logs
                .lock()
                .unwrap()
                .iter()
                // The filter every real adapter applies, exclusion included.
                .filter(|l| {
                    since.wants_sized(&l.name, &l.name.to_filename(), Some(l.bytes.len() as u64))
                })
                .cloned()
                .collect())
        }
        async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
            self.logs.lock().unwrap().push(log.clone());
            Ok(())
        }
        async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
            Ok(self.snapshot.lock().unwrap().clone())
        }
        async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
            *self.snapshot.lock().unwrap() = Some(snapshot.clone());
            Ok(())
        }
        async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
            self.logs.lock().unwrap().retain(|log| log.name != *name);
            Ok(())
        }
        async fn push_sound_asset(
            &self,
            _hash: &str,
            _extension: &str,
            _bytes: &[u8],
        ) -> SyncResult<()> {
            Ok(())
        }
        async fn fetch_sound_asset(
            &self,
            _hash: &str,
            _extension: &str,
        ) -> SyncResult<Option<Vec<u8>>> {
            Ok(None)
        }
    }

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    fn build_service(db: SharedConn) -> OnboardingService {
        // Tests that don't exercise stale-resume use stub
        // pending + sounds dirs that don't exist — the read /
        // walk paths return NotFound and quietly no-op. Tests
        // that DO need real dirs use `build_service_with_pending`.
        build_service_with_pending(db, PathBuf::from("/non/existent/pending"))
    }

    fn build_service_with_pending(db: SharedConn, pending_dir: PathBuf) -> OnboardingService {
        build_service_with(
            db,
            pending_dir,
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
        )
    }

    fn build_service_with(
        db: SharedConn,
        pending_dir: PathBuf,
        secrets: Arc<sync_engine::test_support::FakeSecrets>,
    ) -> OnboardingService {
        let device_id = DeviceId::from_string("dev-this".into());
        let adapter = Arc::new(LocalAdapter::new(db.clone()));
        let store: Arc<dyn sync_engine::SyncStore> = Arc::new(
            crate::event_log::DesktopSyncStore::new(db.clone(), Arc::clone(&adapter)),
        );
        let applier = Arc::new(EventLogApplier::new(
            Arc::clone(&store),
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
            Arc::clone(&adapter),
            device_id.clone(),
        ));
        let snapshot_builder = Arc::new(SnapshotBuilder::new(
            Arc::clone(&store),
            secrets,
            "1.0.0-test",
        ));
        OnboardingService::new(
            db,
            device_id,
            applier,
            snapshot_builder,
            pending_dir,
            // Same stub-path treatment for the sounds dir — the
            // sound-asset sync's `read_dir` returns NotFound on
            // a missing dir and quietly returns empty.
            PathBuf::from("/non/existent/sounds"),
            "1.0.0-test",
        )
    }

    #[tokio::test]
    async fn preview_returns_empty_when_remote_has_no_meta() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let preview = svc.preview(&adapter).await.unwrap();
        assert_eq!(preview, SyncPreview::Empty);
    }

    #[tokio::test]
    async fn preview_returns_existing_with_device_list() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();

        let mut meta = MetaJson::fresh("1.0.0");
        meta.upsert_device(
            &DeviceId::from_string("dev-a".into()),
            DeviceRecord {
                name: Some("Desktop".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);

        let preview = svc.preview(&adapter).await.unwrap();
        let SyncPreview::Existing { devices, .. } = preview else {
            panic!("expected Existing");
        };
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "dev-a");
        assert_eq!(devices[0].name.as_deref(), Some("Desktop"));
        // Different device — `is_this_device` flag must be false.
        assert!(!devices[0].is_this_device);
    }

    #[tokio::test]
    async fn accept_remote_registers_this_device_into_meta() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();

        // Remote starts with one prior device, no logs.
        let mut meta = MetaJson::fresh("1.0.0");
        meta.upsert_device(
            &DeviceId::from_string("dev-other".into()),
            DeviceRecord {
                name: Some("Other Device".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);

        let report = svc.accept_remote(&adapter, Some("Laptop")).await.unwrap();
        assert_eq!(report.device_count, 2);
        assert!(!report.remote_was_empty);

        // meta.json now lists us + the prior device.
        let updated = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(updated
            .device(&DeviceId::from_string("dev-this".into()))
            .is_some());
        assert!(updated
            .device(&DeviceId::from_string("dev-other".into()))
            .is_some());

        // Device name pref was persisted.
        let name = UserPrefsRepo::new(&db.shared())
            .get(PREF_DEVICE_NAME)
            .unwrap();
        assert_eq!(name.as_deref(), Some("Laptop"));
    }

    #[tokio::test]
    async fn accept_remote_errors_when_no_meta_present() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let err = svc.accept_remote(&adapter, None).await.unwrap_err();
        // Should be a "not found" classification — frontend can
        // pattern-match the SyncError::NotFound variant to render the
        // "use adopt_local instead" hint.
        assert!(
            matches!(err, sync_core::SyncError::NotFound(_)),
            "expected NotFound, got: {err:?}",
        );
    }

    #[tokio::test]
    async fn adopt_local_overwrites_remote_meta() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();

        // Remote starts with two prior devices.
        let mut meta = MetaJson::fresh("1.0.0");
        meta.upsert_device(
            &DeviceId::from_string("dev-a".into()),
            DeviceRecord {
                name: Some("A".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        meta.upsert_device(
            &DeviceId::from_string("dev-b".into()),
            DeviceRecord {
                name: Some("B".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);

        let report = svc.adopt_local(&adapter, Some("This"), None).await.unwrap();
        // adopt_local overwrites, so the device count drops to just us.
        assert_eq!(report.device_count, 1);
        assert!(!report.remote_was_empty);

        let updated = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(updated.devices.len(), 1);
        assert!(updated
            .device(&DeviceId::from_string("dev-this".into()))
            .is_some());
        assert!(updated
            .device(&DeviceId::from_string("dev-a".into()))
            .is_none());
    }

    #[tokio::test]
    async fn starting_over_deletes_the_old_dataset_and_puts_this_devices_state_in_its_place() {
        // Toni, 2026-10-30: "Neu beginnen" overwrites the dataset. Its old log
        // files would otherwise come in through the catch-up, here or on a
        // device joining later, and its snapshot through the join.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        UserPrefsRepo::new(&db.shared())
            .set("sync.intervalMinutes", "15")
            .unwrap();
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let old = envelope("ev-old", "dev-old", at, set_interval("45"));
        let adapter = FakeAdapter::with_logs(vec![file_of("dev-old", at, &[&old])]);
        adapter.install_meta(MetaJson::fresh("1.0.0"));
        adapter.install_snapshot(snapshot_at(at, None));

        svc.adopt_local(&adapter, None, None).await.unwrap();

        assert!(adapter.logs.lock().unwrap().is_empty());
        let snapshot = adapter.fetch_snapshot().await.unwrap().unwrap();
        let body: sync_engine::AperioSnapshotBody = serde_json::from_value(snapshot.body).unwrap();
        assert_eq!(
            body.settings
                .get("sync.intervalMinutes")
                .map(String::as_str),
            Some("15"),
            "this device's state",
        );
        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(
            meta.snapshot_timestamp,
            snapshot.metadata.snapshot_timestamp
        );
        assert!(after_self(&adapter).await.exact, "nothing to catch up over");
    }

    #[tokio::test]
    async fn starting_over_without_a_passphrase_puts_no_secret_in_the_snapshot() {
        // Review round 3 of #130: an E2E mark kept from an earlier dataset
        // made the snapshot carry the account secrets, pushed in plaintext.
        use sync_engine::{SecretSlot, SecretStore};
        let (_tmp, db) = fresh_db();
        db.shared()
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
                 VALUES ('acc-1', 'caldav', 'Fastmail', '{}', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')",
                [],
            )
            .unwrap();
        UserPrefsRepo::new(&db.shared())
            .set(crate::credential_sync::PREF_E2E_ENABLED, "true")
            .unwrap();
        let secrets = Arc::new(sync_engine::test_support::FakeSecrets::default());
        secrets
            .store("acc-1", SecretSlot::Password, "hunter2")
            .unwrap();
        let svc = build_service_with(db.shared(), PathBuf::from("/non/existent/pending"), secrets);
        let adapter = FakeAdapter::new();

        svc.adopt_local(&adapter, None, None).await.unwrap();

        let snapshot = adapter.fetch_snapshot().await.unwrap().unwrap();
        assert!(!snapshot.body.to_string().contains("hunter2"));
    }

    #[tokio::test]
    async fn the_heartbeat_reports_once_the_record_is_exact() {
        // The compactors delete no log file while an active device has not
        // caught up (Toni, 2026-10-30); each device says so in meta.json, and
        // a record that only changed there is pushed.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let now = Utc::now();
        svc.heartbeat_meta(&adapter, now, None).await.unwrap();
        assert!(!after_self(&adapter).await.exact);
        let cached = adapter.fetch_meta().await.unwrap().unwrap();

        UserPrefsRepo::new(&db.shared())
            .set(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &now.to_rfc3339())
            .unwrap();
        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        assert!(after_self(&adapter).await.exact);
    }

    #[tokio::test]
    async fn a_joined_device_reports_its_record_exact() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::with_logs(Vec::new());
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.accept_remote(&adapter, None).await.unwrap();

        assert!(after_self(&adapter).await.exact);
    }

    #[tokio::test]
    async fn heartbeat_creates_meta_when_remote_lost_it() {
        // Onboarding ran once; then the remote's meta.json vanished
        // (user deleted the file, NAS volume rebuilt, …). The next
        // heartbeat should re-seed without crashing.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let now = Utc::now();
        svc.heartbeat_meta(&adapter, now, None).await.unwrap();
        let updated = adapter.fetch_meta().await.unwrap().unwrap();
        let rec = updated
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry");
        assert_eq!(rec.app_version, "1.0.0-test");
    }

    #[tokio::test]
    async fn heartbeat_skips_the_push_when_our_record_is_current() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let now = Utc::now();
        // Round-cached meta: our record already carries exactly the horizon
        // we are about to stamp, and a wall-clock stamp from within the
        // refresh window — both halves have to be current for the skip.
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: now,
                last_seen: Some(now - chrono::Duration::hours(1)),
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        // The REMOTE meanwhile gained a marker device (a concurrent peer's
        // heartbeat). If we pushed our cached copy anyway, last-write-wins
        // would clobber the marker — the skip must leave the remote alone.
        let mut remote = cached.clone();
        remote.upsert_device(
            &DeviceId::from_string("dev-marker".into()),
            DeviceRecord {
                name: Some("Marker".into()),
                last_seen_log: now,
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(remote);

        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(
            after
                .device(&DeviceId::from_string("dev-marker".into()))
                .is_some(),
            "record was current -> no push -> the peer's marker survived"
        );
    }

    #[tokio::test]
    async fn heartbeat_push_bases_on_a_fresh_fetch_not_the_round_copy() {
        // A peer wrote meta DURING our round (here: a marker device — in
        // production a compactor raising gc_horizon / flagging devices
        // stale). The push must base on a FRESH fetch, not the round-start
        // copy, or the whole-file PUT would silently revert the peer's
        // write.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let old = Utc::now() - chrono::Duration::minutes(10);
        let now = Utc::now();
        // Round-start copy: our record at the OLD horizon, no marker.
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: old,
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        // Remote meanwhile: the peer's write landed.
        let mut remote = cached.clone();
        remote.upsert_device(
            &DeviceId::from_string("dev-marker".into()),
            DeviceRecord {
                name: Some("Marker".into()),
                last_seen_log: now,
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(remote);

        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(
            after
                .device(&DeviceId::from_string("dev-marker".into()))
                .is_some(),
            "push based on the fresh copy - the peer's write survived"
        );
        let rec = after
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry");
        assert_eq!(rec.last_seen_log, now, "our update landed too");
    }

    #[tokio::test]
    async fn a_heartbeat_keeps_the_stale_flag_a_peer_set() {
        // Review round 7 of #130: a push between rounds names its files through
        // the heartbeat; clearing the flag there, it kept a device that never
        // took the snapshot from ever resuming.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - chrono::Duration::days(30),
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: true,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);
        let now = Utc::now();

        svc.heartbeat_meta(&adapter, now, None).await.unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        let rec = after
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry");
        assert_eq!(rec.last_seen_log, now, "the record was written");
        assert!(rec.stale, "only a resume clears the flag");
    }

    #[tokio::test]
    async fn heartbeat_pushes_when_the_horizon_moved() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let old = Utc::now() - chrono::Duration::minutes(10);
        let now = Utc::now();
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: old,
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(cached.clone());

        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        let rec = after
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry");
        assert_eq!(rec.last_seen_log, now, "moved horizon was pushed");
    }

    /// The stamp is what the device list reads, and it has to keep moving on a
    /// dataset where nothing else does — otherwise a machine that syncs every
    /// quarter of an hour reads as abandoned, which is the exact judgement the
    /// list exists to support.
    #[tokio::test]
    async fn heartbeat_republishes_a_stamp_that_has_gone_stale() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let now = Utc::now();
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                // Nothing has happened on this dataset for two days, so the
                // horizon is unchanged and every OTHER term of the skip test
                // still matches. Only the wall clock has moved.
                last_seen_log: now,
                last_seen: Some(now - chrono::Duration::days(2)),
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(cached.clone());

        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        let seen = after
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry")
            .last_seen
            .expect("the stamp was refreshed");
        assert!(
            now - seen < chrono::Duration::minutes(1),
            "stamp is {seen}, expected roughly {now}",
        );
    }

    /// How the stamp appears on a dataset that predates it: the record reads
    /// as not-current exactly once, and the push that follows fills it in.
    #[tokio::test]
    async fn heartbeat_fills_in_a_stamp_that_was_never_written() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let now = Utc::now();
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: now,
                last_seen: None,
                app_version: "1.0.0-test".into(),
                stale: false,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(cached.clone());

        svc.heartbeat_meta(&adapter, now, Some(&cached))
            .await
            .unwrap();

        assert!(
            after_self(&adapter).await.last_seen.is_some(),
            "an unstamped record must not read as current, or it never gains one",
        );
    }

    /// A registry entry, as terse as the tests below need it.
    fn record(name: Option<&str>, last_seen: Option<DateTime<Utc>>) -> DeviceRecord {
        DeviceRecord {
            name: name.map(str::to_string),
            last_seen_log: Utc::now(),
            last_seen,
            app_version: "1.0.0-test".into(),
            stale: false,
            exact: false,
            unreadable: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// A registry with this device and two leftovers, one of which never
    /// reported a wall clock at all.
    fn registry_with_leftovers() -> MetaJson {
        let now = Utc::now();
        let mut meta = MetaJson::fresh("1.0.0-test");
        meta.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            record(Some("Desktop"), Some(now)),
        );
        meta.upsert_device(
            &DeviceId::from_string("dev-old".into()),
            record(Some("Test-Laptop"), Some(now - chrono::Duration::days(90))),
        );
        meta.upsert_device(
            &DeviceId::from_string("dev-ancient".into()),
            record(None, None),
        );
        meta
    }

    /// The order the list is read in: this device, then by how recently each
    /// one was here, with "never reported" at the bottom.
    #[tokio::test]
    async fn the_device_list_puts_this_device_first_and_the_silent_ones_last() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        adapter.install_meta(registry_with_leftovers());

        let devices = svc.list_devices(&adapter).await.unwrap();

        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["dev-this", "dev-old", "dev-ancient"]);
        assert!(devices[0].is_this_device);
        assert!(!devices[1].is_this_device);
        assert!(
            devices[2].last_seen.is_none(),
            "a record from before the stamp existed must read as unknown, not as 1970",
        );
    }

    /// The whole point: an entry that is not a device any more goes, and
    /// nothing else does.
    #[tokio::test]
    async fn forgetting_a_device_takes_its_record_and_leaves_the_rest() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        adapter.install_meta(registry_with_leftovers());

        svc.forget_device(&adapter, "dev-old").await.unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(after
            .device(&DeviceId::from_string("dev-old".into()))
            .is_none());
        assert!(after
            .device(&DeviceId::from_string("dev-this".into()))
            .is_some());
        assert!(after
            .device(&DeviceId::from_string("dev-ancient".into()))
            .is_some());
    }

    /// Two devices pruning the same leftover in the same minute. The second
    /// one asked for it to be gone and it is gone.
    #[tokio::test]
    async fn forgetting_a_device_that_is_already_gone_is_not_an_error() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        adapter.install_meta(registry_with_leftovers());

        svc.forget_device(&adapter, "dev-old").await.unwrap();
        svc.forget_device(&adapter, "dev-old")
            .await
            .expect("the job was already done");
    }

    /// Refused rather than performed-and-undone: the next heartbeat would put
    /// the record straight back, so honouring it would read as a broken
    /// button rather than as a decision the app declined to make.
    #[tokio::test]
    async fn a_device_cannot_forget_itself() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        adapter.install_meta(registry_with_leftovers());

        let err = svc
            .forget_device(&adapter, "dev-this")
            .await
            .expect_err("this device must refuse to remove itself");
        assert!(err.to_string().contains("disconnect"), "{err}");

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(after
            .device(&DeviceId::from_string("dev-this".into()))
            .is_some());
    }

    /// This device's own record after whatever the test just did.
    async fn after_self(adapter: &FakeAdapter) -> DeviceRecord {
        adapter
            .fetch_meta()
            .await
            .unwrap()
            .unwrap()
            .device(&DeviceId::from_string("dev-this".into()))
            .expect("self entry")
            .clone()
    }

    #[tokio::test]
    async fn resume_from_stale_tolerates_a_missing_snapshot() {
        // A never-compacted dataset has no snapshot.json. If the §19.10
        // backstop ever latches against one (e.g. a legacy now()-baseline
        // meta read as a real snapshot, or a transient race), resume must NOT
        // error — erroring left the device permanently wedged, since the
        // inline compaction that would mint a snapshot sits behind the bailing
        // round. It falls back to a log-only catch-up and clears the stale
        // flag instead. (FakeAdapter::fetch_snapshot returns None.)
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();

        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = Utc::now() - chrono::Duration::days(1);
        meta.upsert_device(
            svc.device_id(),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - chrono::Duration::days(2),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: true,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);

        let report = svc.resume_from_stale(&adapter).await;
        assert!(
            report.is_ok(),
            "a missing snapshot.json must not error during resume: {report:?}",
        );
        // The stale flag is cleared so subsequent rounds proceed.
        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert!(
            !after.device(svc.device_id()).unwrap().stale,
            "resume must clear the stale flag even without a snapshot",
        );
    }

    #[tokio::test]
    async fn a_resume_locks_older_builds_out() {
        // Review round 7 of #130: a write of this build, the only one of a
        // round that resumes before anything else.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let mut meta = MetaJson::fresh("1.0.0");
        meta.schema_version = 1;
        meta.min_app_version = "0.1.0".into();
        meta.upsert_device(
            svc.device_id(),
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now() - chrono::Duration::days(2),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: true,
                exact: false,
                unreadable: Vec::new(),
                pending: Vec::new(),
            },
        );
        adapter.install_meta(meta);

        svc.resume_from_stale(&adapter).await.unwrap();

        let after = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(after.schema_version, sync_core::SCHEMA_VERSION);
        assert_ne!(after.min_app_version, "0.1.0");
    }

    /// The laptop's case (Toni, 2026-10-27): a peer session file named at
    /// `start`, before the snapshot at `start + 1h`, holding a change the
    /// snapshot folded in (`held`, interval 30) and one the peer appended
    /// after it (`appended`, interval 45). Returns the file, the snapshot's
    /// time and the held event's id.
    fn grown_session(start: DateTime<Utc>) -> (LogFile, DateTime<Utc>, String) {
        let peer = DeviceId::from_string("dev-other".into());
        let change = |minutes: i64, value: &str| sync_core::EventEnvelope {
            id: format!("ev-{minutes}"),
            device_id: peer.clone(),
            timestamp: start + chrono::Duration::minutes(minutes),
            event: sync_core::SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
                key: "sync.intervalMinutes".into(),
                value: serde_json::json!(value),
            }),
        };
        let held = change(10, "30");
        let appended = change(120, "45");
        let bytes = [&held, &appended]
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        let log = LogFile {
            name: LogFileName::new(start, peer),
            bytes,
        };
        (log, start + chrono::Duration::hours(1), held.id)
    }

    /// The snapshot at `at`, holding interval 30, naming `covered` (or
    /// nothing).
    fn snapshot_at(at: DateTime<Utc>, covered: Option<Vec<String>>) -> Snapshot {
        let body = sync_engine::AperioSnapshotBody {
            settings: std::collections::BTreeMap::from([(
                "sync.intervalMinutes".to_string(),
                "30".to_string(),
            )]),
            covered_event_ids: covered,
            ..Default::default()
        };
        Snapshot::new(at, "1.0.0", serde_json::to_value(body).unwrap())
    }

    fn interval(db: &DbHandle) -> Option<String> {
        UserPrefsRepo::new(&db.shared())
            .get("sync.intervalMinutes")
            .unwrap()
    }

    fn exact_since(db: &DbHandle) -> Option<DateTime<Utc>> {
        UserPrefsRepo::new(&db.shared())
            .get(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY)
            .unwrap()
            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
            .map(|ts| ts.with_timezone(&Utc))
    }

    #[tokio::test]
    async fn accept_remote_applies_what_a_naming_snapshot_does_not_hold() {
        // DESIGN-sync-snapshot-coverage.md: a joining device fetches every
        // file, a session file named before the snapshot included, and the
        // change the snapshot names is not applied again.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let start: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let (log, snapshot_ts, held) = grown_session(start);
        let adapter = FakeAdapter::with_logs(vec![log]);
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = snapshot_ts;
        adapter.install_meta(meta);
        adapter.install_snapshot(snapshot_at(snapshot_ts, Some(vec![held])));

        let report = svc.accept_remote(&adapter, None).await.unwrap();

        assert_eq!(
            interval(&db).as_deref(),
            Some("45"),
            "the appended change arrives"
        );
        assert_eq!(report.applied, 1, "the held change is not applied again");
        assert!(exact_since(&db).is_some(), "its record is exact");
    }

    #[tokio::test]
    async fn accept_remote_from_a_snapshot_naming_nothing_applies_every_file_too() {
        // A snapshot written before the fix names nothing: the join applies
        // every change in time order on top, and the newest wins.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        // Exact in the dataset it left: says nothing about this one.
        let before: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().unwrap();
        UserPrefsRepo::new(&db.shared())
            .set(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &before.to_rfc3339())
            .unwrap();
        let start: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let (log, snapshot_ts, _held) = grown_session(start);
        let adapter = FakeAdapter::with_logs(vec![log]);
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = snapshot_ts;
        adapter.install_meta(meta);
        adapter.install_snapshot(snapshot_at(snapshot_ts, None));

        svc.accept_remote(&adapter, None).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("45"));
        assert!(
            exact_since(&db).is_some_and(|at| at > before),
            "exact again, here"
        );
    }

    fn envelope(
        id: &str,
        device: &str,
        at: DateTime<Utc>,
        event: sync_core::SyncEvent,
    ) -> sync_core::EventEnvelope {
        sync_core::EventEnvelope {
            id: id.into(),
            device_id: DeviceId::from_string(device.into()),
            timestamp: at,
            event,
        }
    }

    fn file_of(
        device: &str,
        named: DateTime<Utc>,
        envelopes: &[&sync_core::EventEnvelope],
    ) -> LogFile {
        LogFile {
            name: LogFileName::new(named, DeviceId::from_string(device.into())),
            bytes: envelopes
                .iter()
                .map(|e| serde_json::to_string(e).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes(),
        }
    }

    fn set_interval(value: &str) -> sync_core::SyncEvent {
        sync_core::SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
            key: "sync.intervalMinutes".into(),
            value: serde_json::json!(value),
        })
    }

    fn label_count(db: &DbHandle, id: &str) -> i64 {
        let shared = db.shared();
        let conn = shared.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM color_labels WHERE id = ?",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// The laptop before the catch-up (Toni, 2026-10-27): it took a snapshot
    /// that named nothing, holding label `lbl-1` and interval 30, and skipped
    /// the peer's session file by name. On the remote now: that file, which
    /// also set 45 and deleted the label, and a snapshot at `+4h` that holds
    /// all of it, naming its events or not (`covered`). Returns the service,
    /// the adapter and the snapshot's time.
    async fn laptop_before_catch_up(
        db: &DbHandle,
        pending: PathBuf,
        covered: bool,
    ) -> (OnboardingService, FakeAdapter, DateTime<Utc>) {
        let svc = build_service_with_pending(db.shared(), pending);
        let start: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let minutes = |m: i64| start + chrono::Duration::minutes(m);
        let label = envelope(
            "ev-label",
            "dev-other",
            minutes(5),
            sync_core::SyncEvent::ColorLabelCreated(sync_core::EventPayload {
                id: "lbl-1".into(),
                fields: serde_json::json!({ "id": "lbl-1", "name": "Rot", "hex": "#e53935" }),
            }),
        );
        let thirty = envelope("ev-30", "dev-other", minutes(10), set_interval("30"));
        let forty_five = envelope("ev-45", "dev-other", minutes(120), set_interval("45"));
        let deleted = envelope(
            "ev-del",
            "dev-other",
            minutes(180),
            sync_core::SyncEvent::ColorLabelDeleted(sync_core::IdPayload { id: "lbl-1".into() }),
        );
        svc.applier
            .apply_envelopes(vec![label.clone(), thirty.clone()])
            .unwrap();
        assert_eq!(label_count(db, "lbl-1"), 1);

        let snapshot_ts = minutes(240);
        let adapter = FakeAdapter::with_logs(vec![file_of(
            "dev-other",
            start,
            &[&label, &thirty, &forty_five, &deleted],
        )]);
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = snapshot_ts;
        adapter.install_meta(meta);
        let body = sync_engine::AperioSnapshotBody {
            settings: std::collections::BTreeMap::from([(
                "sync.intervalMinutes".to_string(),
                "45".to_string(),
            )]),
            covered_event_ids: covered.then(|| {
                vec![
                    "ev-label".into(),
                    "ev-30".into(),
                    "ev-45".into(),
                    "ev-del".into(),
                ]
            }),
            ..Default::default()
        };
        adapter.install_snapshot(Snapshot::new(
            snapshot_ts,
            "1.0.0",
            serde_json::to_value(body).unwrap(),
        ));
        (svc, adapter, snapshot_ts)
    }

    fn no_pending() -> PathBuf {
        PathBuf::from("/non/existent/pending")
    }

    fn unresolved_conflicts(db: &DbHandle) -> usize {
        crate::conflicts::ConflictsRepo::new(&db.shared())
            .unresolved_count()
            .unwrap()
    }

    #[tokio::test]
    async fn the_catch_up_heals_a_device_that_joined_by_name() {
        // DESIGN-sync-snapshot-coverage.md (Toni, 2026-10-30): the change it
        // skipped arrives, the label the peer deleted goes, nothing asks —
        // and its record is exact from here on.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, _) = laptop_before_catch_up(&db, no_pending(), false).await;

        let report = svc.catch_up(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("45"));
        assert_eq!(label_count(&db, "lbl-1"), 0, "the deleted label goes");
        assert_eq!(unresolved_conflicts(&db), 0);
        assert!(exact_since(&db).is_some());
        // For the round's report: the platform re-reads this setting.
        assert!(report
            .detail
            .settings_keys
            .contains(&"sync.intervalMinutes".to_string()));
    }

    #[tokio::test]
    async fn a_catch_up_meeting_an_unreadable_file_is_not_exact_and_leaves_it_unrecorded() {
        // Review round 2 of #130: a torn read of a peer's live file still
        // marked the device exact, and recorded the file, so a round applied
        // it later outside the catch-up's rules. The next catch-up takes it.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let torn = LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-other".into()),
            ),
            bytes: b"{\"id\":".to_vec(),
        };
        let torn_name = torn.name.to_filename();
        adapter.logs.lock().unwrap().push(torn);

        svc.catch_up(&adapter).await.unwrap();

        assert_eq!(
            interval(&db).as_deref(),
            Some("45"),
            "the rest still arrives"
        );
        assert!(exact_since(&db).is_none());
        let lengths = UserPrefsRepo::new(&db.shared())
            .get(sync_engine::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
            .unwrap()
            .unwrap_or_default();
        assert!(!lengths.contains(&torn_name));
    }

    #[tokio::test]
    async fn a_joining_device_keeps_its_own_change_not_pushed_yet() {
        // Review round 3 of #130: a setting made in the first-launch wizard,
        // pending, before the join. The snapshot holds the peer's value with
        // no clock; the peers take this device's change once it is pushed,
        // so it has to win here too, or the devices part.
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let svc = build_service_with_pending(db.shared(), pending.path().to_path_buf());
        let at = Utc::now() - chrono::Duration::minutes(5);
        let own = envelope("ev-own", "dev-this", at, set_interval("70"));
        let own_file = file_of("dev-this", at, &[&own]);
        std::fs::write(
            pending.path().join(own_file.name.to_filename()),
            &own_file.bytes,
        )
        .unwrap();
        UserPrefsRepo::new(&db.shared())
            .set("sync.intervalMinutes", "70")
            .unwrap();
        stamp_clock(&db, "setting", "sync.intervalMinutes", at);
        let adapter = FakeAdapter::with_logs(Vec::new());
        let snapshot_ts = at - chrono::Duration::hours(1);
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = snapshot_ts;
        adapter.install_meta(meta);
        adapter.install_snapshot(snapshot_at(snapshot_ts, None));

        svc.accept_remote(&adapter, None).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("70"));
    }

    #[tokio::test]
    async fn a_join_that_cannot_fetch_leaves_the_device_as_it_was() {
        // Review round 3 of #130: the join cleared the row clocks and the
        // exact mark before it fetched anything, so a failed join damaged
        // the dataset this device was still syncing with.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        UserPrefsRepo::new(&db.shared())
            .set(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY, &at.to_rfc3339())
            .unwrap();
        stamp_clock(&db, "setting", "sync.intervalMinutes", at);
        let adapter = FakeAdapter::new();
        adapter.install_meta(MetaJson::fresh("1.0.0"));
        *adapter.fail_fetch.lock().unwrap() = true;

        assert!(svc.accept_remote(&adapter, None).await.is_err());

        assert!(exact_since(&db).is_some());
        let clocks: i64 = db
            .shared()
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_row_clock", [], |row| row.get(0))
            .unwrap();
        assert_eq!(clocks, 1);
    }

    #[tokio::test]
    async fn a_file_unreadable_twice_at_the_same_size_no_longer_keeps_the_device_inexact() {
        // Review round 3 of #130: one file no version of this build can read
        // kept every device from being exact, so the GC waited forever and
        // the catch-up ran every hour.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let broken = LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-other".into()),
            ),
            bytes: b"not json".to_vec(),
        };
        let broken_name = broken.name.to_filename();
        adapter.logs.lock().unwrap().push(broken);

        svc.catch_up(&adapter).await.unwrap();
        assert!(exact_since(&db).is_none(), "maybe a torn read: once more");
        svc.catch_up(&adapter).await.unwrap();

        assert!(exact_since(&db).is_some());
        let lengths = UserPrefsRepo::new(&db.shared())
            .get(sync_engine::SYNC_APPLIED_LOG_LENGTHS_PREF_KEY)
            .unwrap()
            .unwrap_or_default();
        assert!(
            lengths.contains(&broken_name),
            "not fetched again unless it grows"
        );
    }

    #[tokio::test]
    async fn an_incomplete_stale_resume_leaves_the_device_not_exact() {
        // Review round 3 of #130: an earlier mark stayed, the heartbeat
        // reported exact, and the GC stopped waiting for this device.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), true).await;
        UserPrefsRepo::new(&db.shared())
            .set(
                SYNC_COVERAGE_EXACT_SINCE_PREF_KEY,
                &snapshot_ts.to_rfc3339(),
            )
            .unwrap();
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-other".into()),
            ),
            bytes: b"{\"id\":".to_vec(),
        });

        svc.resume_from_stale(&adapter).await.unwrap();

        assert!(exact_since(&db).is_none());
    }

    #[tokio::test]
    async fn a_file_another_parser_could_not_read_is_a_first_sighting() {
        // Review rounds 4 and 5 of #130: what an earlier parser recorded as
        // unreadable says nothing about this one, which may read it.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let broken = LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-other".into()),
            ),
            bytes: b"not json".to_vec(),
        };
        let record = sync_engine::UnreadableLogs {
            parser: 0,
            files: HashMap::from([(broken.name.to_filename(), broken.bytes.len() as u64)]),
        };
        UserPrefsRepo::new(&db.shared())
            .set(
                SYNC_UNREADABLE_LOGS_PREF_KEY,
                &serde_json::to_string(&record).unwrap(),
            )
            .unwrap();
        adapter.logs.lock().unwrap().push(broken);

        svc.catch_up(&adapter).await.unwrap();

        assert!(exact_since(&db).is_none(), "once more, under this build");
    }

    #[tokio::test]
    async fn a_join_registers_itself_in_the_meta_a_compaction_wrote_meanwhile() {
        // Review round 4 of #130: the join pushed back the meta.json it read
        // before its long pass, undoing a lock-out and a raised GC horizon.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        adapter.install_meta(MetaJson::fresh("0.2.0"));
        let mut compacted = MetaJson::fresh("0.2.0");
        compacted.min_app_version = "0.2.0".into();
        compacted.gc_horizon = Some("2026-10-05T16:07:24Z".parse().unwrap());
        *adapter.meta_while_fetching.lock().unwrap() = Some(compacted.clone());

        svc.accept_remote(&adapter, None).await.unwrap();

        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(meta.gc_horizon, compacted.gc_horizon);
        assert_eq!(meta.min_app_version, "0.2.0");
        assert!(meta.device(svc.device_id()).is_some());
    }

    #[tokio::test]
    async fn a_line_this_build_does_not_know_keeps_the_device_exact_and_is_named() {
        // Review round 5 of #130: a newer build's live session, holding a
        // line this one cannot read, grew between catch-ups and kept the
        // device from ever being exact. It is no torn read; the device is
        // exact, and names the file in meta.json, so no compactor deletes it.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let newer = LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-other".into()),
            ),
            bytes: b"{\"from\":\"a newer build\"}\n".to_vec(),
        };
        let filename = newer.name.to_filename();
        adapter.logs.lock().unwrap().push(newer);

        svc.catch_up(&adapter).await.unwrap();
        assert!(exact_since(&db).is_some());
        svc.heartbeat_meta(&adapter, snapshot_ts, None)
            .await
            .unwrap();

        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(
            meta.device(svc.device_id())
                .map(|record| record.unreadable.clone()),
            Some(vec![filename]),
        );

        // Read at last: the next heartbeat names it no more, though nothing
        // else about the record changed.
        UserPrefsRepo::new(&db.shared())
            .delete(SYNC_UNREADABLE_LOGS_PREF_KEY)
            .unwrap();
        let round_meta = adapter.fetch_meta().await.unwrap();
        svc.heartbeat_meta(&adapter, snapshot_ts, round_meta.as_ref())
            .await
            .unwrap();
        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(
            meta.device(svc.device_id())
                .map(|record| record.unreadable.clone()),
            Some(Vec::new()),
        );
    }

    #[tokio::test]
    async fn the_catch_up_holds_an_own_change_its_peers_took_before_a_gc() {
        // Review round 5 of #130: an old session, pushed and deleted by a
        // peer's GC, still pending here (a 0.1.0 leftover), counted as not
        // on the remote and was weighed: on a row of unknown time its
        // months-old value won here, and the peers kept theirs.
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let svc = build_service_with_pending(db.shared(), pending.path().to_path_buf());
        let at: DateTime<Utc> = "2026-09-01T08:00:00Z".parse().unwrap();
        let own = envelope("ev-own-old", "dev-this", at, set_interval("30"));
        let own_file = file_of("dev-this", at, &[&own]);
        std::fs::write(
            pending.path().join(own_file.name.to_filename()),
            &own_file.bytes,
        )
        .unwrap();
        UserPrefsRepo::new(&db.shared())
            .set("sync.intervalMinutes", "90")
            .unwrap();
        let adapter = FakeAdapter::with_logs(Vec::new());
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.catch_up(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("90"));
    }

    #[tokio::test]
    async fn an_own_file_torn_again_at_the_same_size_leaves_the_device_exact() {
        // Review round 7 of #130: recorded no more, an own file torn for good
        // kept its device from ever being exact.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-this".into()),
            ),
            bytes: b"{\"id\":\"ev".to_vec(),
        });

        svc.catch_up(&adapter).await.unwrap();
        assert!(exact_since(&db).is_none(), "maybe a torn read: once more");
        svc.catch_up(&adapter).await.unwrap();

        assert!(exact_since(&db).is_some());
        assert!(svc.unreadable_names().is_empty(), "and not named");
    }

    #[tokio::test]
    async fn an_own_file_that_does_not_read_is_not_named() {
        // Review round 6 of #130: this device holds its own events whatever
        // their remote copy looks like; named, the file held the GC back
        // for good, as no ordinary round reads an own file.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        adapter.logs.lock().unwrap().push(LogFile {
            name: LogFileName::new(
                snapshot_ts + chrono::Duration::minutes(10),
                DeviceId::from_string("dev-this".into()),
            ),
            bytes: b"{\"from\":\"a newer build\"}\n".to_vec(),
        });

        svc.catch_up(&adapter).await.unwrap();

        assert!(svc.unreadable_names().is_empty());
    }

    #[tokio::test]
    async fn a_join_weighs_an_own_change_the_snapshot_does_not_hold() {
        // Review round 6 of #130: a join clears every row clock, and a row the
        // snapshot brings no clock for is of unknown time. This device's own
        // change, made after the snapshot, was held there as applied, and the
        // snapshot's older value stayed on this device alone.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let later = at + chrono::Duration::hours(2);
        let own = envelope("ev-own", "dev-this", later, set_interval("70"));
        let body = sync_engine::AperioSnapshotBody {
            settings: std::collections::BTreeMap::from([(
                "sync.intervalMinutes".to_string(),
                "45".to_string(),
            )]),
            ..Default::default()
        };
        let adapter = FakeAdapter::with_logs(vec![file_of("dev-this", later, &[&own])]);
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = at;
        adapter.install_meta(meta);
        adapter.install_snapshot(Snapshot::new(
            at,
            "1.0.0",
            serde_json::to_value(body).unwrap(),
        ));

        svc.accept_remote(&adapter, None).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("70"));
    }

    #[tokio::test]
    async fn the_first_write_of_this_build_locks_older_builds_out() {
        // Review round 6 of #130: until the first compaction older builds
        // went on deleting by name, ignoring what devices name pending.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let adapter = FakeAdapter::new();
        let mut meta = MetaJson::fresh("0.1.0");
        meta.schema_version = 1;
        meta.min_app_version = "0.1.0".into();
        adapter.install_meta(meta);

        svc.heartbeat_meta(&adapter, Utc::now(), None)
            .await
            .unwrap();

        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(meta.schema_version, sync_core::SCHEMA_VERSION);
        assert_eq!(meta.min_app_version, svc.app_version);
    }

    #[tokio::test]
    async fn the_heartbeat_names_every_pending_file_that_holds_anything() {
        // An empty one never goes up (review round 7 of #130).
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let svc = build_service_with_pending(db.shared(), pending.path().to_path_buf());
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let own = file_of(
            "dev-this",
            at,
            &[&envelope("ev-own", "dev-this", at, set_interval("15"))],
        );
        std::fs::write(pending.path().join(own.name.to_filename()), &own.bytes).unwrap();
        let stub = LogFileName::new(
            at + chrono::Duration::hours(1),
            DeviceId::from_string("dev-this".into()),
        );
        std::fs::write(pending.path().join(stub.to_filename()), b"").unwrap();
        let adapter = FakeAdapter::new();
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.heartbeat_meta(&adapter, at, None).await.unwrap();

        let meta = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(
            meta.device(svc.device_id())
                .map(|record| record.pending.clone()),
            Some(vec![own.name.to_filename()]),
        );
    }

    #[tokio::test]
    async fn the_catch_up_brings_back_this_devices_own_changes() {
        // Its own change, pushed and no longer pending: not skipped as own
        // here, and newer than the row, so it wins.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let own = envelope(
            "ev-own",
            "dev-this",
            snapshot_ts + chrono::Duration::minutes(30),
            set_interval("70"),
        );
        adapter.logs.lock().unwrap().push(file_of(
            "dev-this",
            snapshot_ts + chrono::Duration::minutes(25),
            &[&own],
        ));

        let report = svc.catch_up(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("70"));
        assert_eq!(
            report.fetched_logs, 1,
            "the peer's file; its own is not counted"
        );
    }

    #[tokio::test]
    async fn the_catch_up_takes_an_own_change_once_when_pending_and_pushed() {
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let svc = build_service_with_pending(db.shared(), pending.path().to_path_buf());
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let own = envelope("ev-own", "dev-this", at, set_interval("70"));
        let own_file = file_of("dev-this", at, &[&own]);
        std::fs::write(
            pending.path().join(own_file.name.to_filename()),
            &own_file.bytes,
        )
        .unwrap();
        let adapter = FakeAdapter::with_logs(vec![own_file]);
        adapter.install_meta(MetaJson::fresh("1.0.0"));
        // The writer stamped the row when the change was made; a snapshot
        // apply then put an older value back.
        stamp_clock(&db, "setting", "sync.intervalMinutes", at);
        UserPrefsRepo::new(&db.shared())
            .set("sync.intervalMinutes", "30")
            .unwrap();

        let report = svc.catch_up(&adapter).await.unwrap();

        assert_eq!(report.applied, 1);
        assert_eq!(interval(&db).as_deref(), Some("70"));
    }

    /// Stamp a row's clock, whole, as this device's writer does.
    fn stamp_clock(db: &DbHandle, kind: &str, id: &str, at: DateTime<Utc>) {
        let shared = db.shared();
        let conn = shared.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_row_clock (kind, id, changed_us, written_us) VALUES (?, ?, ?, ?)",
            rusqlite::params![kind, id, at.timestamp_micros(), at.timestamp_micros()],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn a_stale_resume_does_not_let_an_older_change_win_over_the_snapshot() {
        // Review round 2 of #130: the snapshot wrote its rows but not their
        // times, so this device's own older change, still pending, won over
        // the snapshot's newer value — and every peer skipped it as older.
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let svc = build_service_with_pending(db.shared(), pending.path().to_path_buf());
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let own = envelope("ev-own", "dev-this", at, set_interval("15"));
        let own_file = file_of("dev-this", at, &[&own]);
        std::fs::write(
            pending.path().join(own_file.name.to_filename()),
            &own_file.bytes,
        )
        .unwrap();
        stamp_clock(&db, "setting", "sync.intervalMinutes", at);
        UserPrefsRepo::new(&db.shared())
            .set("sync.intervalMinutes", "15")
            .unwrap();
        // A peer set 45 two hours later; its file is gone, the snapshot
        // holds it.
        let later = at + chrono::Duration::hours(2);
        let body = sync_engine::AperioSnapshotBody {
            settings: std::collections::BTreeMap::from([(
                "sync.intervalMinutes".to_string(),
                "45".to_string(),
            )]),
            row_clocks: vec![sync_engine::SnapshotRowClock {
                kind: "setting".into(),
                id: "sync.intervalMinutes".into(),
                changed: later,
                written: Some(later),
                deleted: false,
            }],
            ..Default::default()
        };
        let adapter = FakeAdapter::with_logs(Vec::new());
        let mut meta = MetaJson::fresh("1.0.0");
        meta.snapshot_timestamp = later;
        adapter.install_meta(meta);
        adapter.install_snapshot(Snapshot::new(
            later,
            "1.0.0",
            serde_json::to_value(body).unwrap(),
        ));

        svc.resume_from_stale(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("45"));
    }

    #[tokio::test]
    async fn joining_forgets_the_row_clocks_of_the_dataset_it_left() {
        // A clock from the dataset this device left would keep this one's
        // changes out as older.
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());
        let far: DateTime<Utc> = "2030-01-01T00:00:00Z".parse().unwrap();
        stamp_clock(&db, "setting", "sync.intervalMinutes", far);
        let at: DateTime<Utc> = "2026-10-19T07:58:29Z".parse().unwrap();
        let change = envelope("ev-45", "dev-other", at, set_interval("45"));
        let adapter = FakeAdapter::with_logs(vec![file_of("dev-other", at, &[&change])]);
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.accept_remote(&adapter, None).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("45"));
    }

    #[tokio::test]
    async fn the_catch_up_never_moves_the_cursor_back() {
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), false).await;
        let ahead = snapshot_ts + chrono::Duration::hours(10);
        UserPrefsRepo::new(&db.shared())
            .set(SYNC_CURSOR_PREF_KEY, &ahead.to_rfc3339())
            .unwrap();

        svc.catch_up(&adapter).await.unwrap();

        assert_eq!(svc.read_cursor(), ahead);
    }

    #[tokio::test]
    async fn the_catch_up_takes_an_own_change_not_uploaded_yet() {
        let (_tmp, db) = fresh_db();
        let pending = TempDir::new().unwrap();
        let (svc, adapter, snapshot_ts) =
            laptop_before_catch_up(&db, pending.path().to_path_buf(), false).await;
        let own = envelope(
            "ev-pending",
            "dev-this",
            snapshot_ts + chrono::Duration::minutes(40),
            set_interval("80"),
        );
        let own_file = file_of(
            "dev-this",
            snapshot_ts + chrono::Duration::minutes(35),
            &[&own],
        );
        std::fs::write(
            pending.path().join(own_file.name.to_filename()),
            &own_file.bytes,
        )
        .unwrap();

        svc.catch_up(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("80"));
    }

    /// A stale resume over the laptop, the snapshot naming its events or
    /// not: it heals like the catch-up.
    async fn resume_heals(covered: bool) {
        let (_tmp, db) = fresh_db();
        let (svc, adapter, _) = laptop_before_catch_up(&db, no_pending(), covered).await;

        svc.resume_from_stale(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("45"));
        assert_eq!(label_count(&db, "lbl-1"), 0, "the deleted label goes");
        assert!(exact_since(&db).is_some());
    }

    #[tokio::test]
    async fn a_stale_resume_heals_like_the_catch_up() {
        resume_heals(true).await;
    }

    #[tokio::test]
    async fn a_stale_resume_heals_from_a_snapshot_naming_nothing_too() {
        resume_heals(false).await;
    }

    #[tokio::test]
    async fn a_stale_resume_keeps_a_newer_change_the_snapshot_would_revert() {
        // The snapshot can come from a device that was behind this one: a
        // change this device applied after it, still in a file, wins over
        // the snapshot's older value again — quietly, it was the newer one.
        let (_tmp, db) = fresh_db();
        let (svc, adapter, snapshot_ts) = laptop_before_catch_up(&db, no_pending(), true).await;
        let sixty = envelope(
            "ev-60",
            "dev-other",
            snapshot_ts + chrono::Duration::minutes(30),
            set_interval("60"),
        );
        svc.applier.apply_envelopes(vec![sixty.clone()]).unwrap();
        adapter.logs.lock().unwrap().push(file_of(
            "dev-other",
            snapshot_ts + chrono::Duration::minutes(20),
            &[&sixty],
        ));

        svc.resume_from_stale(&adapter).await.unwrap();

        assert_eq!(interval(&db).as_deref(), Some("60"));
        assert_eq!(unresolved_conflicts(&db), 0);
    }

    #[tokio::test]
    async fn accept_remote_advances_local_cursor_past_newest_log() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());

        // Pre-seed: one log from another device with a known
        // timestamp. The test asserts on cursor advance, not on row
        // counts; the delete of a missing row changes nothing.
        let ts: DateTime<Utc> = "2026-05-01T12:34:56Z".parse().unwrap();
        let deleted = envelope(
            "ev-test-001",
            "dev-other",
            ts,
            sync_core::SyncEvent::EventDeleted(sync_core::IdPayload {
                id: "nonexistent".into(),
            }),
        );
        let adapter = FakeAdapter::with_logs(vec![file_of("dev-other", ts, &[&deleted])]);
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.accept_remote(&adapter, None).await.unwrap();

        let cursor = UserPrefsRepo::new(&db.shared())
            .get(SYNC_CURSOR_PREF_KEY)
            .unwrap()
            .expect("cursor should be persisted");
        let parsed: DateTime<Utc> = cursor.parse().unwrap();
        assert_eq!(parsed, ts);
        // No snapshot: every file from the start, nothing GC'd — what this
        // device holds is exactly what it applied (DESIGN-sync-snapshot-coverage.md).
        assert!(UserPrefsRepo::new(&db.shared())
            .get(SYNC_COVERAGE_EXACT_SINCE_PREF_KEY)
            .unwrap()
            .is_some());
    }
}
